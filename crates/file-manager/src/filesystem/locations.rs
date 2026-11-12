//! Folder identities are resolved on directory workers, never while rendering.
use std::{
	fs,
	path::{Path, PathBuf},
};

use crate::ui::icons::folder_symbol;
use iced::widget::svg;

#[derive(Debug, Clone)]
pub(crate) struct FolderIdentity {
	pub path: PathBuf,
	pub label: String,
	pub icon: svg::Handle,
	/// Built-in symbols follow the palette; custom artwork retains its own colors.
	pub symbolic: bool,
}

pub(crate) struct Locations {
	roots: Vec<FolderIdentity>,
	custom_start: usize,
}

impl Locations {
	pub fn new(custom: &config::FilebergRoots) -> Self {
		let home = dirs::home_dir();
		let mut roots = Vec::new();

		for (path, label, icon) in [
			(home.clone(), "Home", "house"),
			(dirs::desktop_dir(), "Desktop", "desktop"),
			(dirs::download_dir(), "Downloads", "download-simple"),
			(dirs::document_dir(), "Documents", "file-text"),
			(dirs::picture_dir(), "Pictures", "image"),
			(dirs::audio_dir(), "Music", "music-notes"),
			(dirs::video_dir(), "Videos", "film-strip"),
			(dirs::public_dir(), "Public", "users"),
			(dirs::template_dir(), "Templates", "copy"),
		] {
			if let Some(path) = path
				&& !roots.iter().any(|root: &FolderIdentity| root.path == path)
			{
				roots.push(identity(path, label, icon));
			}
		}

		let custom_start = roots.len();

		// Append custom roots last so exact-path lookup can give them precedence.
		for root in &custom.0 {
			let Some(path) = expand_path(&root.path, home.as_deref()) else {
				continue;
			};

			let (icon, symbolic) = folder_symbol(&root.icon)
				.map(|icon| (icon, true))
				.or_else(|| {
					let path = expand_path(&root.icon, home.as_deref())?;

					// Bound user artwork reads; missing or non-SVG artwork uses the folder fallback.
					use std::os::unix::fs::OpenOptionsExt;
					let file = fs::OpenOptions::new()
						.read(true)
						.custom_flags(libc::O_NONBLOCK)
						.open(path)
						.ok()?;
					if !file.metadata().ok()?.is_file() {
						return None;
					}

					let mut bytes = Vec::new();
					use std::io::Read;
					file.take(1024 * 1024 + 1).read_to_end(&mut bytes).ok()?;
					if bytes.len() > 1024 * 1024
						|| !std::str::from_utf8(&bytes).ok()?.contains("<svg")
					{
						return None;
					}

					Some((svg::Handle::from_memory(bytes), false))
				})
				.unwrap_or_else(|| (crate::ui::icons::ui_icon!("folder"), true));

			roots.push(FolderIdentity {
				path,
				label: root.label.clone(),
				icon,
				symbolic,
			});
		}

		Self {
			roots,
			custom_start,
		}
	}

	pub fn folder(&self, path: &Path) -> Option<FolderIdentity> {
		self.exact_root(path).or_else(|| {
			let name = path.file_name()?.to_str()?.to_ascii_lowercase();
			let icon = match name.as_str() {
				"desktop" => "desktop",
				"downloads" => "download-simple",
				"documents" | "docs" => "file-text",
				"pictures" | "images" | "photos" => "image",
				"music" | "audio" => "music-notes",
				"videos" | "movies" => "film-strip",
				"public" | "shared" => "users",
				"templates" => "copy",
				"projects" | "src" | "source" => "code",
				"design assets" => "pen-nib",
				"work" => "briefcase",
				"personal" => "smiley",
				"archive" | "archives" | "backups" => "archive",
				"very important" => "star",
				".config" | "config" => "gear",
				"packages" | "node_modules" => "package",
				_ => return None,
			};

			Some(identity(
				path.to_path_buf(),
				&path.file_name()?.to_string_lossy(),
				icon,
			))
		})
	}

	pub fn roots(&self) -> &[FolderIdentity] {
		&self.roots
	}

	pub fn breadcrumb_root(&self, path: &Path) -> Option<FolderIdentity> {
		for ancestor in path.ancestors() {
			if let Some(root) = self.exact_root(ancestor) {
				return Some(root);
			}
		}

		None
	}

	fn exact_root(&self, path: &Path) -> Option<FolderIdentity> {
		self.roots[self.custom_start..]
			.iter()
			.rev()
			.find(|root| root.path == path)
			.cloned()
			.or_else(|| git_identity(path))
			.or_else(|| {
				self.roots[..self.custom_start]
					.iter()
					.find(|root| root.path == path)
					.cloned()
			})
	}
}

fn identity(path: PathBuf, label: &str, icon: &str) -> FolderIdentity {
	FolderIdentity {
		path,
		label: label.into(),
		icon: folder_symbol(icon).unwrap_or_else(|| crate::ui::icons::ui_icon!("folder")),
		symbolic: true,
	}
}

fn git_identity(path: &Path) -> Option<FolderIdentity> {
	let metadata = fs::metadata(path.join(".git")).ok()?;
	if !metadata.is_dir() && !metadata.is_file() {
		return None;
	}

	Some(identity(
		path.to_path_buf(),
		&path.file_name()?.to_string_lossy(),
		"git-branch",
	))
}

pub(crate) fn expand_path(value: &str, home: Option<&Path>) -> Option<PathBuf> {
	if value == "~" {
		return home.map(Path::to_path_buf);
	}
	if let Some(tail) = value.strip_prefix("~/") {
		return home.map(|home| home.join(tail));
	}
	Path::new(value).is_absolute().then(|| PathBuf::from(value))
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::test_support::TestDir;

	#[test]
	fn deepest_root_wins_with_custom_then_git_then_automatic_ties() {
		let dir = TestDir::new();
		let project = dir.path().join("project");
		fs::create_dir_all(project.join("src/deep")).unwrap();
		fs::create_dir(project.join(".git")).unwrap();
		let mut locations = Locations {
			roots: vec![
				identity(dir.path().to_path_buf(), "Automatic", "house"),
				identity(project.clone(), "Automatic project", "download-simple"),
			],
			custom_start: 2,
		};

		assert_eq!(
			locations
				.breadcrumb_root(&project.join("src/deep"))
				.unwrap()
				.label,
			"project"
		);

		locations
			.roots
			.push(identity(dir.path().to_path_buf(), "Custom parent", "star"));

		assert_eq!(
			locations
				.breadcrumb_root(&project.join("src"))
				.unwrap()
				.path,
			project
		);

		locations
			.roots
			.push(identity(project.clone(), "Custom project", "briefcase"));

		assert_eq!(
			locations
				.breadcrumb_root(&project.join("src/deep"))
				.unwrap()
				.label,
			"Custom project"
		);
		assert_eq!(locations.folder(&project).unwrap().label, "Custom project");
	}

	#[test]
	fn worktrees_are_roots_but_common_names_and_path_prefixes_are_not() {
		let dir = TestDir::new();
		let worktree = dir.path().join("worktree");
		fs::create_dir_all(worktree.join("src")).unwrap();
		fs::write(
			worktree.join(".git"),
			"gitdir: /some/repository/.git/worktrees/example\n",
		)
		.unwrap();
		let locations = Locations {
			roots: Vec::new(),
			custom_start: 0,
		};

		assert_eq!(
			locations
				.breadcrumb_root(&worktree.join("src"))
				.unwrap()
				.path,
			worktree
		);
		assert!(locations.folder(&dir.path().join("DOWNLOADS")).is_some());
		assert!(
			locations
				.breadcrumb_root(&dir.path().join("downloads"))
				.is_none()
		);
		assert!(
			locations
				.breadcrumb_root(&dir.path().join("worktree-other"))
				.is_none()
		);
		assert!(locations.folder(&dir.path().join("ordinary")).is_none());
	}

	#[test]
	fn custom_svg_and_missing_artwork_resolve_on_the_worker() {
		let dir = TestDir::new();
		let artwork = dir.path().join("custom.svg");
		fs::write(&artwork, "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\"><path d=\"M0 0h10v10z\"/></svg>").unwrap();
		let roots = config::FilebergRoots(vec![config::FilebergRoot {
			path: dir.path().to_string_lossy().into_owned(),
			label: "Custom".into(),
			icon: artwork.to_string_lossy().into_owned(),
		}]);

		let locations = Locations::new(&roots);
		let custom = locations.folder(dir.path()).unwrap();

		assert_ne!(custom.icon, folder_symbol("folder").unwrap());
		assert!(!custom.symbolic);

		fs::write(&artwork, "not svg").unwrap();
		let fallback = Locations::new(&roots).folder(dir.path()).unwrap();

		assert_eq!(fallback.label, "Custom");
		assert_eq!(fallback.icon, folder_symbol("folder").unwrap());
		assert!(fallback.symbolic);
	}
}
