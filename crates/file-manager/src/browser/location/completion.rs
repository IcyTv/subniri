//! Bounded suggestion data, loaded on a directory worker without decoding previews.
use std::{
	collections::BTreeMap,
	fs,
	path::{Path, PathBuf},
};

use crate::{
	filesystem::locations::{FolderIdentity, Locations},
	ui::icons::ui_icon,
};

pub(super) const LIMIT: usize = 64;

pub(super) fn suggestions(
	value: &str, current: &Path, home: Option<&Path>, locations: &Locations,
) -> Vec<FolderIdentity> {
	if value.contains('\0') {
		return Vec::new();
	}

	let mut matches = BTreeMap::new();
	if let Some((parent, prefix)) = parent_and_prefix(value, current, home) {
		let prefix = prefix.to_lowercase();
		if let Ok(entries) = fs::read_dir(parent) {
			for entry in entries.flatten() {
				let name = entry.file_name();
				let Some(name) = name.to_str() else { continue };

				if (!prefix.starts_with('.') && name.starts_with('.'))
					|| !name.to_lowercase().starts_with(&prefix)
				{
					continue;
				}

				let path = entry.path();
				if path.to_str().is_none() || !path.is_dir() {
					continue;
				}

				let folder = locations.folder(&path).unwrap_or_else(|| FolderIdentity {
					path: path.clone(),
					label: name.into(),
					icon: ui_icon!("folder"),
					symbolic: true,
				});
				matches.insert(path, folder);
			}
		}
	}

	// Friendly names are shortcuts only for a bare component, never part of an OS path.
	if !value.contains('/') && !value.starts_with('.') {
		let prefix = value.to_lowercase();
		for root in locations.roots() {
			if root.path.to_str().is_some()
				&& !root
					.path
					.file_name()
					.is_some_and(|name| name.to_string_lossy().starts_with('.'))
				&& root.label.to_lowercase().starts_with(&prefix)
				&& root.path.is_dir()
			{
				let folder = locations.folder(&root.path).unwrap_or_else(|| root.clone());
				matches.insert(root.path.clone(), folder);
			}
		}
	}

	let mut matches = matches.into_values().collect::<Vec<_>>();
	matches.sort_by_cached_key(|folder| (folder.label.to_lowercase(), folder.path.clone()));
	matches.truncate(LIMIT);

	matches
}

fn parent_and_prefix<'a>(
	value: &'a str, current: &Path, home: Option<&Path>,
) -> Option<(PathBuf, &'a str)> {
	if value.ends_with('/') {
		return Some((super::resolve_path(value, current, home).ok()?, ""));
	}

	if let Some((parent, prefix)) = value.rsplit_once('/') {
		let parent = if parent.is_empty() { "/" } else { parent };
		return Some((super::resolve_path(parent, current, home).ok()?, prefix));
	}

	Some((current.to_path_buf(), value))
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::test_support::TestDir;

	#[test]
	fn matches_folders_and_folder_links_without_hidden_or_file_results() {
		use std::os::unix::fs::symlink;
		let dir = TestDir::new();

		for name in ["Documents", "downloads", ".private"] {
			fs::create_dir(dir.path().join(name)).unwrap();
		}
		fs::write(dir.path().join("document.txt"), "text").unwrap();
		symlink("Documents", dir.path().join("DocLink")).unwrap();
		symlink("missing", dir.path().join("DocBroken")).unwrap();
		let hidden = dir.path().join(".private");
		let roots = config::FilebergRoots(vec![config::FilebergRoot {
			path: hidden.to_string_lossy().into_owned(),
			label: "Private".into(),
			icon: "folder".into(),
		}]);
		let locations = Locations::new(&roots);

		let found = suggestions("dO", dir.path(), None, &locations);

		assert_eq!(
			found
				.iter()
				.filter(|folder| folder.path.starts_with(dir.path()))
				.map(|folder| folder.path.file_name().unwrap().to_str().unwrap())
				.collect::<Vec<_>>(),
			["DocLink", "Documents", "downloads"]
		);
		assert!(
			suggestions("", dir.path(), None, &locations)
				.iter()
				.all(|folder| folder.path != hidden)
		);
		assert_eq!(
			suggestions(".p", dir.path(), None, &locations)[0].path,
			hidden
		);
	}

	#[test]
	fn resolves_relative_absolute_and_home_components_and_friendly_roots() {
		let dir = TestDir::new();
		let folder = dir.path().join("Actual folder");
		fs::create_dir(&folder).unwrap();
		let custom = config::FilebergRoots(vec![config::FilebergRoot {
			path: folder.to_string_lossy().into_owned(),
			label: "Workspace".into(),
			icon: "briefcase".into(),
		}]);
		let locations = Locations::new(&custom);

		for value in ["aC", "~/aC"] {
			assert_eq!(
				suggestions(value, dir.path(), Some(dir.path()), &locations)[0].path,
				folder
			);
		}

		let absolute = format!("{}/aC", dir.path().display());

		assert_eq!(
			suggestions(&absolute, dir.path(), None, &locations)[0].path,
			folder
		);

		let found = suggestions("wOrK", dir.path(), None, &locations);

		assert_eq!(found[0].label, "Workspace");
		assert_eq!(found[0].path, folder);
		assert!(suggestions("missing/Work", dir.path(), None, &locations).is_empty());
		assert!(suggestions("bad\0path", dir.path(), None, &locations).is_empty());
	}

	#[test]
	fn results_are_sorted_bounded_and_deduplicated_by_path() {
		let dir = TestDir::new();
		for index in (0..LIMIT + 5).rev() {
			fs::create_dir(dir.path().join(format!("Folder{index:03}"))).unwrap();
		}
		let custom = config::FilebergRoots(vec![config::FilebergRoot {
			path: dir.path().join("Folder000").to_string_lossy().into_owned(),
			label: "Folder000".into(),
			icon: "star".into(),
		}]);
		let locations = Locations::new(&custom);

		let found = suggestions("Folder", dir.path(), None, &locations);

		assert_eq!(found.len(), LIMIT);
		assert_eq!(found[0].label, "Folder000");
		assert_eq!(found[LIMIT - 1].label, format!("Folder{:03}", LIMIT - 1));
		assert_eq!(
			found
				.iter()
				.filter(|folder| folder.path.ends_with("Folder000"))
				.count(),
			1
		);
	}

	#[test]
	fn non_utf8_paths_are_not_inserted_through_lossy_text() {
		use std::os::unix::ffi::OsStringExt;

		let dir = TestDir::new();
		let name = std::ffi::OsString::from_vec(vec![b'u', 0xff]);
		fs::create_dir(dir.path().join(name)).unwrap();
		let locations = Locations::new(&Default::default());

		assert!(
			suggestions("u", dir.path(), None, &locations)
				.iter()
				.all(|folder| !folder.path.starts_with(dir.path()))
		);
	}

	#[test]
	fn automatic_label_shortcuts_keep_custom_identity_precedence() {
		let dir = TestDir::new();
		let home = dirs::home_dir().unwrap();
		let roots = config::FilebergRoots(vec![config::FilebergRoot {
			path: home.to_string_lossy().into_owned(),
			label: "Personal workspace".into(),
			icon: "star".into(),
		}]);
		let locations = Locations::new(&roots);

		let found = suggestions("Home", dir.path(), None, &locations);
		let home = found.iter().find(|folder| folder.path == home).unwrap();

		assert_eq!(home.label, "Personal workspace");
		assert_eq!(home.icon, ui_icon!("star"));
	}
}
