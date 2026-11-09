use std::{
	collections::HashMap,
	path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Visit {
	pub path: PathBuf,
	pub selected: Option<PathBuf>,
}

/// Browser history owns visit snapshots, rather than treating navigation as a
/// path-only edit. Directory memory also applies to explicit/sidebar revisits.
pub struct History {
	visits: Vec<Visit>,
	cursor: usize,
	remembered: HashMap<PathBuf, Option<PathBuf>>,
}

impl History {
	pub fn new(path: PathBuf) -> Self {
		Self {
			visits: vec![Visit {
				path,
				selected: None,
			}],
			cursor: 0,
			remembered: HashMap::new(),
		}
	}
	pub fn can_undo(&self) -> bool {
		self.cursor > 0
	}
	pub fn can_redo(&self) -> bool {
		self.cursor + 1 < self.visits.len()
	}

	pub fn remember_selection(&mut self, selected: Option<PathBuf>) {
		if let Some(current) = self.visits.get_mut(self.cursor) {
			current.selected.clone_from(&selected);
			self.remembered.insert(current.path.clone(), selected);
		}
	}

	pub fn go_to(&mut self, path: PathBuf, selected: Option<PathBuf>) -> Option<Visit> {
		let previous = self.visits.get(self.cursor)?.path.clone();
		if previous == path {
			return None;
		}
		self.remember_selection(selected);
		let selected = child_towards(&path, &previous)
			.or_else(|| self.remembered.get(&path).cloned().flatten());
		self.visits.truncate(self.cursor + 1);
		let visit = Visit { path, selected };
		self.visits.push(visit.clone());
		self.cursor += 1;
		Some(visit)
	}

	pub fn back(&mut self, selected: Option<PathBuf>) -> Option<Visit> {
		if !self.can_undo() {
			return None;
		}
		let previous = self.visits.get(self.cursor)?.path.clone();
		self.remember_selection(selected);
		self.cursor -= 1;
		let mut visit = self.visits.get(self.cursor)?.clone();
		visit.selected = child_towards(&visit.path, &previous).or(visit.selected);
		Some(visit)
	}

	pub fn forward(&mut self, selected: Option<PathBuf>) -> Option<Visit> {
		if !self.can_redo() {
			return None;
		}
		self.remember_selection(selected);
		self.cursor += 1;
		let mut visit = self.visits.get(self.cursor)?.clone();
		if let Some(next) = self.visits.get(self.cursor + 1) {
			visit.selected = child_towards(&visit.path, &next.path).or(visit.selected);
		}
		Some(visit)
	}
}

/// Select only an immediate child visible in the destination's listing, even
/// when a breadcrumb/history transition crosses several directory levels.
fn child_towards(parent: &Path, descendant: &Path) -> Option<PathBuf> {
	let child = descendant.strip_prefix(parent).ok()?.components().next()?;
	match child {
		std::path::Component::Normal(name) => Some(parent.join(name)),
		_ => None,
	}
}

/// Labels are presentation only. Navigation always retains the original OS path.
pub fn breadcrumbs(path: &Path, home: Option<&Path>) -> Vec<(String, PathBuf)> {
	if home == Some(path) {
		return vec![("Home".into(), path.to_path_buf())];
	}
	let mut result = Vec::new();
	let (mut destination, remainder) = if let Some(home) = home.filter(|h| path.starts_with(h)) {
		(home.to_path_buf(), path.strip_prefix(home).unwrap_or(path))
	} else {
		(PathBuf::new(), path)
	};
	for component in remainder.components() {
		destination.push(component);
		if component == std::path::Component::RootDir {
			continue;
		}
		result.push((
			component.as_os_str().to_string_lossy().into_owned(),
			destination.clone(),
		));
	}
	result
}

/// The mount itself is rendered as the disk-name anchor. Descendants keep their
/// absolute paths, even though the mount's host-side ancestors are not displayed.
pub fn mounted_breadcrumbs(
	path: &Path, mount: &Path, home: Option<&Path>,
) -> Vec<(String, PathBuf)> {
	let Ok(remainder) = path.strip_prefix(mount) else {
		return Vec::new();
	};
	let mut result = Vec::new();
	let mut destination = mount.to_path_buf();
	let mut remainder = remainder;

	if let Some(home) =
		home.filter(|home| *home != mount && home.starts_with(mount) && path.starts_with(home))
	{
		result.push(("Home".into(), home.to_path_buf()));
		destination = home.to_path_buf();
		remainder = path.strip_prefix(home).unwrap_or(remainder);
	}

	for component in remainder.components() {
		destination.push(component);
		result.push((
			component.as_os_str().to_string_lossy().into_owned(),
			destination.clone(),
		));
	}

	result
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn mounted_breadcrumbs_hide_host_ancestors_but_keep_absolute_destinations() {
		let mount = Path::new("/run/media/michael/Backup");
		let path = mount.join("Photos/Holiday");

		let parts = mounted_breadcrumbs(&path, mount, Some(Path::new("/home/michael")));

		assert_eq!(
			parts,
			vec![
				("Photos".into(), mount.join("Photos")),
				("Holiday".into(), path),
			]
		);
		assert!(mounted_breadcrumbs(mount, mount, None).is_empty());
		assert!(mounted_breadcrumbs(Path::new("/run/media/michael"), mount, None).is_empty());
	}

	#[test]
	fn mounted_breadcrumbs_preserve_home_navigation_without_hiding_other_users() {
		let root = Path::new("/");
		let home = Path::new("/home/michael");

		let parts = mounted_breadcrumbs(&home.join("Documents"), root, Some(home));

		assert_eq!(
			parts,
			vec![
				("Home".into(), home.to_path_buf()),
				("Documents".into(), home.join("Documents")),
			]
		);
		assert_eq!(
			mounted_breadcrumbs(Path::new("/home/other"), root, Some(home)),
			vec![
				("home".into(), "/home".into()),
				("other".into(), "/home/other".into()),
			]
		);
	}

	#[test]
	fn mounted_breadcrumbs_preserve_non_utf8_navigation_paths() {
		use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
		let mount = Path::new("/media/Backup");
		let path = mount.join(OsStr::from_bytes(b"photos-\xff"));

		let parts = mounted_breadcrumbs(&path, mount, None);

		assert_eq!(
			parts.first().map(|(_, destination)| destination),
			Some(&path)
		);
	}

	#[test]
	fn breadcrumbs_keep_destinations_and_respect_actual_home() {
		let parts = breadcrumbs(
			Path::new("/home/m/Documents/Work"),
			Some(Path::new("/home/m")),
		);
		assert_eq!(
			parts,
			vec![
				("Documents".into(), "/home/m/Documents".into()),
				("Work".into(), "/home/m/Documents/Work".into())
			]
		);
		assert_eq!(
			breadcrumbs(Path::new("/home/other"), Some(Path::new("/home/m")))
				.first()
				.map(|p| p.0.as_str()),
			Some("home")
		);
		assert_eq!(
			breadcrumbs(Path::new("/home/m"), Some(Path::new("/home/m"))),
			vec![("Home".into(), "/home/m".into())]
		);
		assert!(breadcrumbs(Path::new("/"), Some(Path::new("/home/m"))).is_empty());
	}
	#[test]
	fn history_round_trip_and_branching() {
		let mut history = History::new("/a".into());
		assert!(history.back(None).is_none());
		assert!(history.go_to("/a".into(), None).is_none());
		let _ = history.go_to("/b".into(), Some("/a/file".into()));
		assert_eq!(history.back(None).map(|v| v.path), Some("/a".into()));
		assert_eq!(history.forward(None).map(|v| v.path), Some("/b".into()));
		assert!(history.forward(None).is_none());
		let _ = history.back(None);
		let _ = history.go_to("/c".into(), None);
		assert!(!history.can_redo());
	}

	#[test]
	fn up_and_back_mark_the_child_but_restore_the_childs_file_selection() {
		let mut history = History::new("/a".into());
		let _ = history.go_to("/a/b".into(), Some("/a/b".into()));
		let up = history.go_to("/a".into(), Some("/a/b/notes.txt".into()));
		assert_eq!(up.and_then(|v| v.selected), Some("/a/b".into()));
		assert_eq!(
			history.back(Some("/a/b".into())).and_then(|v| v.selected),
			Some("/a/b/notes.txt".into())
		);
		assert_eq!(
			history
				.back(Some("/a/b/notes.txt".into()))
				.and_then(|v| v.selected),
			Some("/a/b".into())
		);
	}

	#[test]
	fn forward_points_to_the_next_redo_step_then_restores_the_final_selection() {
		let mut history = History::new("/a".into());
		let _ = history.go_to("/a/b".into(), Some("/a/b".into()));
		let _ = history.go_to("/a/b/c".into(), Some("/a/b/c".into()));
		let _ = history.back(Some("/a/b/c/file.txt".into()));
		let _ = history.back(Some("/a/b/another.txt".into()));
		// A remembered file in B must not obscure the remaining redo route to C.
		assert_eq!(
			history
				.forward(Some("/a/b".into()))
				.and_then(|v| v.selected),
			Some("/a/b/c".into())
		);
		assert_eq!(
			history
				.forward(Some("/a/b/c".into()))
				.and_then(|v| v.selected),
			Some("/a/b/c/file.txt".into())
		);
	}

	#[test]
	fn sidebar_revisits_remember_selection_and_breadcrumbs_mark_immediate_children() {
		let mut history = History::new("/a".into());
		let _ = history.go_to("/elsewhere".into(), Some("/a/notes.txt".into()));
		assert_eq!(
			history.go_to("/a".into(), None).and_then(|v| v.selected),
			Some("/a/notes.txt".into())
		);
		assert_eq!(
			child_towards(Path::new("/a"), Path::new("/a/b/c")),
			Some("/a/b".into())
		);
		assert_eq!(child_towards(Path::new("/a"), Path::new("/another")), None);
	}
}
