//! One parented confirmation at a time. Operations remain owned by their actions.
mod message;
#[cfg(test)]
mod tests;
mod update;
mod view;

use crate::context_menu::Target;
use iced::{advanced::widget::Id, window};
pub(crate) use message::Message;

#[derive(Default)]
pub(crate) struct Dialogs {
	pub(super) pending: Option<Confirmation>,
	pub(crate) deleting: Option<window::Id>,
}

impl Dialogs {
	pub(crate) fn blocked(&self) -> bool {
		self.pending.is_some()
	}

	pub(crate) fn owns(&self, id: window::Id) -> bool {
		self.pending.as_ref().is_some_and(|p| p.id == id)
	}
}

pub(super) struct Confirmation {
	pub id: window::Id,
	pub action: ConfirmationAction,
	pub phase: Phase,
	pub prepared: bool,
	pub previous_focus: Option<Id>,
	pub confirm_focused: bool,
}

pub(super) enum ConfirmationAction {
	Delete(Target),
	Drive(crate::filesystem::volumes::Operation),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
	Opening,
	Open,
	Closing { confirm: bool },
}

pub(super) fn cancel_id(id: window::Id) -> Id {
	Id::from(format!("dialog-{id:?}-cancel"))
}

pub(super) fn confirm_id(id: window::Id) -> Id {
	Id::from(format!("dialog-{id:?}-confirm"))
}
