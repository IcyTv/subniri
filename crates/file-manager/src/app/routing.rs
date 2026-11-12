//! Window identity and modality are decided before feature handlers run.
use super::{FileManager, Message};
use crate::{browser, context_menu, dialogs, tags};
use iced::{Task, event::Status, keyboard, window};

impl Message {
	pub(super) fn is_parent_interaction(&self) -> bool {
		match self {
			Self::Drives(message) => !matches!(
				message,
				crate::drives::Message::Refresh
					| crate::drives::Message::Loaded(..)
					| crate::drives::Message::Finished(..)
			),
			Self::Browser(message) => !matches!(
				message,
				browser::Message::Loaded(..)
					| browser::Message::StorageLoaded(..)
					| browser::Message::FolderCreated(..)
					| browser::Message::OperationFinished(..)
			),
			Self::Tags(message) => !matches!(
				message,
				tags::Message::TagsLoaded(_) | tags::Message::TagsSaved(_)
			),
			Self::ContextMenu(message) => !matches!(
				message,
				context_menu::Message::FavoritesLoaded(..)
					| context_menu::Message::FavoritesSaved(..)
					| context_menu::Message::FavoritesReload
					| context_menu::Message::MenuLifecycle(_)
					| context_menu::Message::MenuPoll(_)
					| context_menu::Message::MenuStatus(..)
			),
			_ => false,
		}
	}
}

impl FileManager {
	pub(super) fn route_input(
		&mut self, id: window::Id, event: iced::Event, status: Status,
	) -> Task<Message> {
		if self.transfers.owns(id) {
			return if let iced::Event::Keyboard(event) = event {
				self.update_transfers(crate::transfers::Message::Key(id, event))
			} else {
				Task::none()
			};
		}
		if self.dialogs.owns(id) {
			return if let iced::Event::Keyboard(event) = event {
				self.update_dialog(dialogs::Message::Key(id, event))
			} else {
				Task::none()
			};
		}
		if self.dialogs.blocked() || self.transfers.blocked() {
			return Task::none();
		}
		if self.drives.menu.as_ref().is_some_and(|m| m.id == id) {
			return if let iced::Event::Keyboard(event) = event {
				self.drive_menu_key(event)
			} else {
				Task::none()
			};
		}
		if self.drives.menu.is_some() && id == self.host {
			return Task::none();
		}
		if id != self.host && !self.menu.as_ref().is_some_and(|m| m.page(id).is_some()) {
			return Task::none();
		}
		if id == self.host {
			if let Some(message) = browser::typeahead::dismiss_event(event.clone(), status, id) {
				let _ = self.update(message);
			}
			if let iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) = event {
				return self.update_browser(browser::Message::Modifiers(modifiers));
			}
		}
		match event {
			iced::Event::Keyboard(event) if status == Status::Ignored => {
				self.menu_key(id, event, None)
			}
			iced::Event::Mouse(event) => {
				self.update_context_menu(context_menu::Message::MenuPointer(id, event))
			}
			_ => Task::none(),
		}
	}

	pub(super) fn route_window_event(
		&mut self, id: window::Id, event: window::Event,
	) -> Task<Message> {
		if self.transfers.owns(id) {
			return self.update_transfers(crate::transfers::Message::Window(id, event));
		}
		if self.dialogs.owns(id) {
			return self.dialog_window_event(id, event);
		}
		if id == self.host {
			return match event {
				window::Event::Opened { size, .. } | window::Event::Resized(size) => {
					self.host_size = size;
					Task::none()
				}
				window::Event::CloseRequested => {
					let close = self.menu_lifecycle(neo_widgets::popup::Event::CloseAll);
					close.chain(self.update(Message::CloseRequested(id)))
				}
				window::Event::Closed => self.update(Message::CloseRequested(id)),
				window::Event::Unfocused => {
					self.typeahead.clear();
					self.modifiers = keyboard::Modifiers::empty();
					Task::none()
				}
				_ => Task::none(),
			};
		}
		if self.drives.menu.as_ref().is_some_and(|m| m.id == id) {
			return self.drive_menu_event(id, event);
		}
		if self.menu.as_ref().is_some_and(|m| m.page(id).is_some()) {
			return self.menu_window_event(id, event);
		}
		// A stale asynchronous popup creation must not survive as an orphan grab.
		if matches!(event, window::Event::Opened { .. }) {
			return window::close(id);
		}
		Task::none()
	}
}
