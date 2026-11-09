//! Sidebar storage destinations and their serialized management lifecycle.
#[cfg(test)]
mod tests;
mod update;
mod view;

use crate::filesystem::volumes::{Action, Identity, Snapshot};
use iced::{Point, window};
use std::path::PathBuf;

#[derive(Default)]
pub(crate) struct Drives {
	pub snapshot: Option<Snapshot>,
	pub show_system: bool,
	pub busy: Option<u64>,
	pub menu: Option<Menu>,
	pub generation: u64,
	pub refreshing: Option<u64>,
	pub problem: Option<String>,
}

pub(crate) struct Menu {
	pub id: window::Id,
	pub target: Option<Identity>,
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
	NoAction,
	Refresh,
	Loaded(u64, Result<Snapshot, String>),
	MenuAt(Option<Identity>, Point),
	MenuOpen(Option<Identity>, Point),
	ToggleSystem(window::Id),
	MenuAction(window::Id, Action),
	Request(Identity, Action),
	Finished(u64, Result<Option<PathBuf>, String>),
}

impl From<Message> for crate::Message {
	fn from(message: Message) -> Self {
		Self::Drives(message)
	}
}
