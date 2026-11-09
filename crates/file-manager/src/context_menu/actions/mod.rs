//! Each action module owns its metadata, availability, and effects.
use super::{Context, Target};
use crate::{FileManager, Message, tags::TagId};
use iced::Task;

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
	pub const ALL: [Self; 16] = [
		Self::Open,
		Self::OpenWith,
		Self::OpenWindow,
		Self::Cut,
		Self::Copy,
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
		matches!(self, Self::OpenWith | Self::Tags | Self::Sort | Self::View)
	}

	pub fn choices(self, app: &FileManager) -> Vec<(Action, String)> {
		match self {
			Self::OpenWith => open_with::choices(),
			Self::Tags => tags::choices(app),
			Self::Sort => sort::choices(),
			Self::View => view::choices(),
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
}
