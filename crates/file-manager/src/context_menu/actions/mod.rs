//! Each action module owns its metadata, availability, and effects.
use super::{Context, Target};
use crate::{FileManager, Message, tags::TagId};
use iced::Task;

mod advanced_copy;
mod advanced_move;
mod copy;
mod cut;
mod delete;
mod duplicate;
mod new_folder;
mod open;
mod open_window;
mod open_with;
mod paste;
mod properties;
mod refresh;
mod rename;
mod sort;
mod tags;
mod trash;
mod view;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
	Open,
	OpenWith,
	OpenWindow,
	Cut,
	Copy,
	AdvancedCopy,
	AdvancedMove,
	AdvancedCopyAuto,
	AdvancedCopyDurable,
	AdvancedCopyBuffered,
	AdvancedMoveAuto,
	AdvancedMoveDurable,
	AdvancedMoveBuffered,
	Delete,
	Duplicate,
	Rename,
	Trash,
	Tags,
	Properties,
	NewFolder,
	Paste,
	Sort,
	View,
	Refresh,
}

pub(super) struct Definition {
	pub id: &'static str,
	pub label: &'static str,
	pub group: &'static str,
	pub aliases: &'static str,
	pub multiple: bool,
	pub scope: Scope,
	pub execute: fn(&mut FileManager, Target, Action) -> Task<Message>,
	pub check: Option<fn(&FileManager, &Target, Action) -> Option<&'static str>>,
}

pub(super) enum Scope {
	Item,
	Folder,
	Background,
	Any,
}

impl Command {
	pub const ALL: [Self; 24] = [
		Self::Open,
		Self::OpenWith,
		Self::OpenWindow,
		Self::Cut,
		Self::Copy,
		Self::AdvancedCopy,
		Self::AdvancedMove,
		Self::AdvancedCopyAuto,
		Self::AdvancedCopyDurable,
		Self::AdvancedCopyBuffered,
		Self::AdvancedMoveAuto,
		Self::AdvancedMoveDurable,
		Self::AdvancedMoveBuffered,
		Self::Delete,
		Self::Duplicate,
		Self::Rename,
		Self::Trash,
		Self::Tags,
		Self::Properties,
		Self::NewFolder,
		Self::Paste,
		Self::Sort,
		Self::View,
		Self::Refresh,
	];

	fn definition(self) -> &'static Definition {
		match self {
			Self::Open => &open::DEFINITION,
			Self::OpenWith => &open_with::DEFINITION,
			Self::OpenWindow => &open_window::DEFINITION,
			Self::Cut => &cut::DEFINITION,
			Self::Copy => &copy::DEFINITION,
			Self::AdvancedCopy => &advanced_copy::DEFINITION,
			Self::AdvancedMove => &advanced_move::DEFINITION,
			Self::AdvancedCopyAuto => &advanced_copy::AUTO,
			Self::AdvancedCopyDurable => &advanced_copy::DURABLE,
			Self::AdvancedCopyBuffered => &advanced_copy::BUFFERED,
			Self::AdvancedMoveAuto => &advanced_move::AUTO,
			Self::AdvancedMoveDurable => &advanced_move::DURABLE,
			Self::AdvancedMoveBuffered => &advanced_move::BUFFERED,
			Self::Delete => &delete::DEFINITION,
			Self::Duplicate => &duplicate::DEFINITION,
			Self::Rename => &rename::DEFINITION,
			Self::Trash => &trash::DEFINITION,
			Self::Tags => &tags::DEFINITION,
			Self::Properties => &properties::DEFINITION,
			Self::NewFolder => &new_folder::DEFINITION,
			Self::Paste => &paste::DEFINITION,
			Self::Sort => &sort::DEFINITION,
			Self::View => &view::DEFINITION,
			Self::Refresh => &refresh::DEFINITION,
		}
	}

	pub fn id(self) -> &'static str {
		self.definition().id
	}

	pub fn label(self) -> &'static str {
		self.definition().label
	}

	pub fn group(self) -> &'static str {
		self.definition().group
	}

	pub fn aliases(self) -> &'static str {
		self.definition().aliases
	}

	pub fn parse(id: &str) -> Option<Self> {
		Self::ALL.into_iter().find(|c| c.id() == id)
	}

	pub fn submenu(self) -> bool {
		matches!(
			self,
			Self::OpenWith
				| Self::Tags | Self::Sort
				| Self::View | Self::AdvancedCopy
				| Self::AdvancedMove
		)
	}

	pub fn menu_parent(self) -> Option<Self> {
		match self {
			Self::AdvancedCopyAuto | Self::AdvancedCopyDurable | Self::AdvancedCopyBuffered => {
				Some(Self::AdvancedCopy)
			}
			Self::AdvancedMoveAuto | Self::AdvancedMoveDurable | Self::AdvancedMoveBuffered => {
				Some(Self::AdvancedMove)
			}
			_ => None,
		}
	}

	pub fn choices(self, app: &FileManager) -> Vec<(Action, String)> {
		match self {
			Self::OpenWith => open_with::choices(),
			Self::Tags => tags::choices(app),
			Self::Sort => sort::choices(),
			Self::View => view::choices(),
			Self::AdvancedCopy => advanced_copy::choices(),
			Self::AdvancedMove => advanced_move::choices(),
			_ => Vec::new(),
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
	Command(Command),
	ChooseApplication,
	Tag(TagId),
	ViewGrid,
	ViewList,
	SortAscending,
	SortDescending,
}

impl Action {
	pub fn key(self) -> String {
		match self {
			Self::Command(c) => c.id().into(),
			Self::ChooseApplication => "choose-application".into(),
			Self::Tag(id) => format!("tag-{id}"),
			Self::ViewGrid => "view-grid".into(),
			Self::ViewList => "view-list".into(),
			Self::SortAscending => "sort-ascending".into(),
			Self::SortDescending => "sort-descending".into(),
		}
	}

	pub fn parent(self) -> Command {
		match self {
			Self::Command(c) => c,
			Self::ChooseApplication => Command::OpenWith,
			Self::Tag(_) => Command::Tags,
			Self::ViewGrid | Self::ViewList => Command::View,
			Self::SortAscending | Self::SortDescending => Command::Sort,
		}
	}

	/// Shared selection checks, followed by the owning action's additional rules.
	pub fn unavailable(
		self, app: &FileManager, target: &Target, available: bool,
	) -> Option<&'static str> {
		let definition = self.parent().definition();
		if !available {
			return Some("Item no longer available");
		}
		if app.transfers.busy
			&& matches!(
				self.parent().menu_parent().unwrap_or(self.parent()),
				Command::Delete
					| Command::Trash
					| Command::Copy | Command::Cut
					| Command::Paste
					| Command::AdvancedCopy
					| Command::AdvancedMove
			) {
			return Some("Wait for the current transfer or rollback to finish");
		}
		if !target.others.is_empty() && !definition.multiple {
			return Some("Not available for multiple items");
		}

		match definition.scope {
			Scope::Item if target.context == Context::Background => {
				return Some("Select a file or folder first");
			}
			Scope::Folder if target.context != Context::Folder => {
				return Some("Select a folder first");
			}
			Scope::Background if target.context != Context::Background => {
				return Some("Right-click empty space");
			}
			_ => {}
		}

		definition.check.and_then(|check| check(app, target, self))
	}

	pub fn execute(self, app: &mut FileManager, target: Target) -> Task<Message> {
		if let Some(reason) = self.unavailable(app, &target, target.available()) {
			app.error = Some(reason.into());
			return Task::none();
		}

		log::debug!(action=%self.key(),target=%target.path.display(),"Context action dispatch");

		(self.parent().definition().execute)(app, target, self)
	}

	pub fn checked(self, app: &FileManager, target: &Target) -> bool {
		match self.parent() {
			Command::Tags => tags::checked(app, target, self),
			Command::View => view::checked(app, self),
			Command::Sort => sort::checked(app, self),
			_ => false,
		}
	}
}

fn stub(_: &mut FileManager, target: Target, action: Action) -> Task<Message> {
	let targets = target.paths();
	log::info!(action=%action.key(),?targets,"Prototype action: here we would execute this command (no file operation performed)");
	Task::none()
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::{Args, test_support::TestDir};

	#[test]
	fn action_metadata_round_trips_and_context_rules_are_shared_with_execution() {
		let dir = TestDir::new();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		let target = Target::capture(dir.path(), Context::Background).unwrap();
		for command in Command::ALL {
			assert_eq!(Command::parse(command.id()), Some(command));
		}
		let action = Action::Command(Command::Copy);
		let reason = action.unavailable(&app, &target, true).unwrap();
		assert_eq!(reason, "Select a file or folder first");
		assert_eq!(action.execute(&mut app, target).units(), 0);
		assert_eq!(app.error.as_deref(), Some(reason));
	}

	#[test]
	fn multi_item_actions_keep_their_individual_availability() {
		let dir = TestDir::new();
		let first = dir.path().join("first");
		let second = dir.path().join("second");
		std::fs::write(&first, "first").unwrap();
		std::fs::write(&second, "second").unwrap();
		let (app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		let mut target = Target::capture(&first, Context::File).unwrap();
		target
			.others
			.push(Target::capture(&second, Context::File).unwrap());
		for command in [
			Command::Cut,
			Command::Copy,
			Command::AdvancedCopy,
			Command::AdvancedMove,
			Command::AdvancedCopyAuto,
			Command::AdvancedCopyDurable,
			Command::AdvancedCopyBuffered,
			Command::AdvancedMoveAuto,
			Command::AdvancedMoveDurable,
			Command::AdvancedMoveBuffered,
			Command::Duplicate,
			Command::Rename,
			Command::Trash,
			Command::Properties,
		] {
			assert_eq!(
				Action::Command(command).unavailable(&app, &target, true),
				None,
				"{command:?}"
			);
		}
		for command in [
			Command::Open,
			Command::OpenWith,
			Command::OpenWindow,
			Command::Tags,
		] {
			assert_eq!(
				Action::Command(command).unavailable(&app, &target, true),
				Some("Not available for multiple items"),
				"{command:?}"
			);
		}
	}

	#[test]
	fn policy_leaf_ids_are_unique_config_valid_favorites() {
		let ids: std::collections::HashSet<_> = Command::ALL.into_iter().map(Command::id).collect();
		assert_eq!(ids.len(), Command::ALL.len());

		for command in Command::ALL
			.into_iter()
			.filter(|c| c.menu_parent().is_some())
		{
			assert_eq!(Command::parse(command.id()), Some(command));
			assert!(config::FILEBERG_ACTION_IDS.contains(&command.id()));
			assert!(!command.submenu());
			assert!(command.menu_parent().unwrap().submenu());
		}
	}

	#[test]
	fn paste_requires_one_destination_and_clipboard_and_checks_busy_first() {
		let dir = TestDir::new();
		let file = dir.path().join("file");
		std::fs::write(&file, "data").unwrap();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		let paste = Action::Command(Command::Paste);

		for context in [Context::Background, Context::Folder] {
			let target = Target::capture(dir.path(), context).unwrap();
			assert_eq!(
				paste.unavailable(&app, &target, true),
				Some("Clipboard has no files to paste")
			);
			assert_eq!(paste.execute(&mut app, target).units(), 0);
			assert_eq!(
				app.error.as_deref(),
				Some("Clipboard has no files to paste")
			);
		}

		let target = Target::capture(&file, Context::File).unwrap();
		assert_eq!(
			paste.unavailable(&app, &target, true),
			Some("Select a folder or right-click empty space")
		);

		let mut target = Target::capture(dir.path(), Context::Folder).unwrap();
		target.others.push(target.clone());
		assert_eq!(
			paste.unavailable(&app, &target, true),
			Some("Not available for multiple items")
		);

		target.others.clear();
		app.transfers.busy = true;
		for command in [
			Command::Copy,
			Command::Cut,
			Command::Paste,
			Command::AdvancedCopy,
			Command::AdvancedMove,
			Command::AdvancedCopyAuto,
			Command::AdvancedCopyDurable,
			Command::AdvancedCopyBuffered,
			Command::AdvancedMoveAuto,
			Command::AdvancedMoveDurable,
			Command::AdvancedMoveBuffered,
		] {
			assert_eq!(
				Action::Command(command).unavailable(&app, &target, true),
				Some("Wait for the current transfer or rollback to finish")
			);
		}
	}
}
