//! Immutable target identities and popup page state.
use super::{Action, Command};
#[cfg(test)]
use crate::FileManager;
use iced::window;
use std::{
	fs::{File, OpenOptions},
	os::unix::fs::{MetadataExt, OpenOptionsExt},
	path::{Path, PathBuf},
	sync::Arc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
	File,
	Folder,
	Background,
}

/// Hold an O_PATH descriptor so unlink/recreate cannot reuse this item's inode
/// while the menu is alive. Availability is latched false once identity is lost.
#[derive(Debug, Clone)]
pub struct Target {
	pub path: PathBuf,
	pub context: Context,
	/// Other selected items, captured at menu opening with their own identity guards.
	pub others: Vec<Target>,
	file: Arc<File>,
	identity: (u64, u64),
}

impl Target {
	pub fn capture(path: &Path, context: Context) -> Result<Self, String> {
		let flags = if context == Context::Background {
			nix::libc::O_PATH
		} else {
			nix::libc::O_PATH | nix::libc::O_NOFOLLOW
		};
		let file = OpenOptions::new()
			.read(true)
			.custom_flags(flags)
			.open(path)
			.map_err(|e| format!("Cannot inspect {}: {e}", path.display()))?;
		let metadata = file.metadata().map_err(|e| e.to_string())?;
		Ok(Self {
			path: path.into(),
			context,
			others: Vec::new(),
			identity: (metadata.dev(), metadata.ino()),
			file: Arc::new(file),
		})
	}

	pub fn available(&self) -> bool {
		let metadata = if self.context == Context::Background {
			std::fs::metadata(&self.path)
		} else {
			std::fs::symlink_metadata(&self.path)
		};
		self.file.metadata().is_ok()
			&& metadata.is_ok_and(|m| (m.dev(), m.ino()) == self.identity)
			&& self.others.iter().all(Target::available)
	}

	pub fn paths(&self) -> Vec<PathBuf> {
		let mut paths = vec![self.path.clone()];
		paths.extend(self.others.iter().map(|t| t.path.clone()));
		paths
	}
}

pub struct Page {
	pub id: window::Id,
	pub parent_action: Option<Command>,
	pub query: String,
	pub highlighted: Option<Action>,
	pub hovered: Option<Action>,
	pub cursor: Option<iced::Point>,
}

impl Page {
	pub fn new(id: window::Id, parent_action: Option<Command>) -> Self {
		Self {
			id,
			parent_action,
			query: String::new(),
			highlighted: None,
			hovered: None,
			cursor: None,
		}
	}

	pub fn search_id(&self) -> String {
		format!("context-search-{:?}", self.id)
	}

	pub fn row_id(&self, action: Action) -> String {
		format!("context-row-{:?}-{}", self.id, action.key())
	}

	pub fn pin_id(&self, action: Action) -> String {
		format!("context-pin-{:?}-{}", self.id, action.key())
	}

	pub fn scroll_id(&self) -> String {
		format!("context-scroll-{:?}", self.id)
	}
}

pub struct Menu {
	pub target: Target,
	pub available: bool,
	pub pages: Vec<Page>,
	pub drag: Option<Drag>,
}

pub struct Drag {
	pub page: window::Id,
	pub source: Command,
	pub origin: Option<iced::Point>,
	pub dragging: bool,
	pub over: Option<(Command, bool)>,
}

impl Menu {
	pub fn page(&self, id: window::Id) -> Option<&Page> {
		self.pages.iter().find(|p| p.id == id)
	}

	pub fn page_mut(&mut self, id: window::Id) -> Option<&mut Page> {
		self.pages.iter_mut().find(|p| p.id == id)
	}
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
	use super::*;
	use crate::{Args, test_support::TestDir};
	fn app() -> (TestDir, FileManager, window::Id) {
		let dir = TestDir::new();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		let id = window::Id::unique();
		app.menu = Some(Menu {
			target: Target::capture(dir.path(), Context::Background).unwrap(),
			available: true,
			pages: vec![Page::new(id, None)],
			drag: None,
		});
		(dir, app, id)
	}
	#[test]
	fn background_hides_item_commands_but_search_explains_and_aliases_them() {
		let (_dir, mut app, id) = app();
		assert!(
			!app.menu_rows(id)
				.iter()
				.any(|r| r.action == Action::Command(Command::Copy))
		);
		app.menu.as_mut().unwrap().page_mut(id).unwrap().query = "delete".into();
		let rows = app.menu_rows(id);
		assert_eq!(rows[0].action, Action::Command(Command::Trash));
		assert!(rows[0].disabled.is_some());
		app.menu.as_mut().unwrap().page_mut(id).unwrap().query = "vew".into();
		assert!(
			app.menu_rows(id)
				.iter()
				.any(|r| r.action == Action::Command(Command::View))
		);
	}
	#[test]
	fn snapshot_rejects_same_path_replacement() {
		let dir = TestDir::new();
		let path = dir.path().join("item");
		std::fs::write(&path, "old").unwrap();
		let target = Target::capture(&path, Context::File).unwrap();
		assert!(target.available());
		std::fs::rename(&path, dir.path().join("old")).unwrap();
		std::fs::write(&path, "new").unwrap();
		assert!(!target.available());
	}

	#[test]
	fn background_snapshot_follows_and_protects_a_symlinked_directory() {
		let dir = TestDir::new();
		let real = dir.path().join("real");
		let link = dir.path().join("link");
		std::fs::create_dir(&real).unwrap();
		std::os::unix::fs::symlink(&real, &link).unwrap();
		let target = Target::capture(&link, Context::Background).unwrap();
		assert!(target.available());
		std::fs::rename(&real, dir.path().join("old-real")).unwrap();
		std::fs::create_dir(&real).unwrap();
		assert!(!target.available());
	}
	#[test]
	fn all_prototype_commands_are_reachable_in_their_contexts() {
		let (dir, mut app, id) = app();
		let file = dir.path().join("file");
		std::fs::write(&file, "x").unwrap();
		let mut reachable = std::collections::HashSet::new();
		for context in [Context::File, Context::Folder, Context::Background] {
			app.menu.as_mut().unwrap().target = Target::capture(
				if context == Context::File {
					&file
				} else {
					dir.path()
				},
				context,
			)
			.unwrap();
			for row in app.menu_rows(id) {
				if let Action::Command(c) = row.action {
					reachable.insert(c);
				}
			}
		}
		// Tags are deliberately unavailable until their asynchronous store arrives.
		for command in Command::ALL {
			if command != Command::Tags {
				assert!(reachable.contains(&command), "{command:?}");
			}
		}
	}
}
