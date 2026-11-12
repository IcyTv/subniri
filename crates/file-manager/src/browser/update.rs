use super::{Message, typeahead};
use crate::FileManager;
use iced::{
	Task,
	keyboard::{self, Key, key::Named},
};

impl FileManager {
	pub(crate) fn update_browser(&mut self, message: Message) -> Task<crate::Message> {
		if (self.editing.editor.is_some() || self.editing.pending.is_some())
			&& matches!(
				&message,
				Message::Clicked(_)
					| Message::OpenEntry(_)
					| Message::OpenSelected
					| Message::GoTo(_)
					| Message::GoUp | Message::GoBack
					| Message::GoForward
					| Message::Search(_)
					| Message::View(_)
					| Message::Sort | Message::Refresh
					| Message::NewFolder
					| Message::NewFile
					| Message::Rename
					| Message::ClearSelection
			) {
			return self.finish_edit_before(message.into());
		}

		if self.page == super::Page::Trash {
			if let Message::Keyboard(event) = message {
				return self.trash_keyboard(event);
			}
			if matches!(message, Message::Refresh) {
				return self.update_trash(crate::trash::Message::Refresh);
			}
		}
		match message {
			Message::EditPath => return self.edit_path(),
			Message::PathChanged(value) => return self.change_path(value),
			Message::PathSuggestionsDue(generation) => {
				return self.request_path_suggestions(generation);
			}
			Message::PathSuggestionsLoaded(generation, suggestions) => {
				self.path_suggestions_loaded(generation, suggestions)
			}
			Message::MovePathSuggestion(down) => return self.move_path_suggestion(down),
			Message::FillPath(path) => return self.fill_path(path),
			Message::SubmitPath => return self.submit_path(),
			Message::CancelPath => {
				self.location.cancel();
				return self.reveal_selection();
			}
			Message::PathValidated(id, result) => return self.path_validated(id, result),
			Message::ListingLoaded(id, result) => {
				if id != self.request_id {
					return Task::none();
				}

				let result = result.map(|listing| {
					self.breadcrumb_root = listing.root;
					listing.entries
				});

				return self.update_browser(Message::Loaded(id, result));
			}
			Message::TypeAhead(edit) => {
				if self.menu.is_some() || self.editing.editor.is_some() {
					return Task::none();
				}
				if edit == typeahead::Edit::Clear {
					self.typeahead.clear();
				} else if !self.loading {
					self.typeahead.apply(edit);
					if let Some(path) = self.typeahead_match().map(|e| e.path.clone()) {
						self.select_only(path);
						self.history.remember_selection(self.selected.clone());
						return self.reveal_selection();
					}
				}
			}
			Message::Clicked(path) => {
				self.typeahead.clear();
				self.apply_selection(path.clone(), self.modifiers, true);
				if let Some(entry) = self.entries.iter().find(|e| e.path == path) {
					return iced::widget::operation::focus(entry.id.clone());
				}
			}
			Message::OpenEntry(path) => {
				if self.modifiers.control() || self.modifiers.shift() {
					return Task::none();
				}
				self.select_only(path);
				return self.open_selected();
			}
			Message::Modifiers(modifiers) => {
				self.modifiers = modifiers;
				if !modifiers.shift() {
					self.selection.end_range();
				}
			}
			Message::SelectionMove(path, modifiers) => {
				self.typeahead.clear();
				self.apply_selection(path, modifiers, false);
				return self.reveal_selection();
			}
			Message::SelectionKey(key, modifiers, focused) => {
				return self.selection_key(key, modifiers, focused);
			}
			Message::OpenSelected => return self.open_selected(),
			Message::GoTo(path) => return self.goto(path),
			Message::GoUp => {
				if let Some(parent) = self.path.parent() {
					return self.goto(parent.to_path_buf());
				}
			}
			Message::GoBack => {
				let selected = self.selection_snapshot();
				if let Some(visit) = self.history.back(selected) {
					return self.navigation_changed(visit);
				}
			}
			Message::GoForward => {
				let selected = self.selection_snapshot();
				if let Some(visit) = self.history.forward(selected) {
					return self.navigation_changed(visit);
				}
			}
			Message::Refresh => return self.load_directory(),
			Message::StorageLoaded(id, usage) => {
				if id == self.request_id {
					self.storage = usage;
				}
			}
			Message::Loaded(id, result) => {
				if id != self.request_id {
					return Task::none();
				}
				self.loading = false;
				match result {
					Ok(entries) => {
						let restoring = self.pending_selection.is_some();
						if let Some(path) = self.pending_selection.take() {
							self.select_only(path);
						}
						self.entries = entries;
						self.reconcile_selection();
						self.history.remember_selection(self.selected.clone());
						if !self.editing.busy
							&& self.editing.editor.is_none()
							&& let Some(message) = self.editing.pending.take()
						{
							return self.update(*message);
						}

						if restoring {
							return self.reveal_selection();
						}
					}
					Err(error) => {
						// Refresh failures retain the useful listing; navigation already cleared it.
						self.pending_selection = None;
						self.editing.pending = None;
						self.error = Some(error);
					}
				}
			}
			Message::Search(query) => {
				self.typeahead.clear();
				self.search = query;
				self.reconcile_selection();
			}
			Message::View(mode) => {
				self.typeahead.clear();
				self.view_mode = mode;
				return self.reveal_selection();
			}
			Message::Sort => self.ascending = !self.ascending,
			Message::NewFolder => return self.start_name_edit(Some(true), None),
			Message::NewFile => return self.start_name_edit(Some(false), None),
			Message::Rename => return self.start_name_edit(None, None),
			Message::EditName(name) => {
				if !self.editing.busy
					&& let Some(editor) = &mut self.editing.editor
				{
					editor.name = name;
					editor.error = None;
				}
			}
			Message::SaveEdit => return self.save_name_edit(),
			Message::CancelEdit => {
				if !self.editing.busy {
					self.editing.editor = None;
					self.editing.pending = None;
					self.reconcile_selection();
				}
			}
			Message::EditFinished(result) => return self.name_edit_finished(result),
			Message::ShowEditedItem => return self.show_edited_item(),
			Message::DismissToast => self.editing.toast = None,
			Message::ToastHover(hovered) => {
				if let Some(toast) = &mut self.editing.toast {
					toast.hovered = hovered;
					toast.last_tick = std::time::Instant::now();
				}
			}
			Message::ToastTick(now) => {
				if let Some(toast) = &mut self.editing.toast {
					if !toast.hovered {
						toast.remaining = toast
							.remaining
							.saturating_sub(now.saturating_duration_since(toast.last_tick));
					}

					toast.last_tick = now;
					if toast.remaining.is_zero() {
						self.editing.toast = None;
					}
				}
			}
			Message::OperationFinished(Err(error)) => self.error = Some(error),
			Message::OperationFinished(Ok(())) => {}
			Message::ClearSelection => {
				self.typeahead.clear();
				self.selected = None;
				self.selection.clear();
				self.pending_selection = None;
			}
			Message::DismissError => self.error = None,
			Message::ToggleInfo => self.show_info = !self.show_info,
			Message::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
				if self.editing.editor.is_some() {
					return match key.as_ref() {
						Key::Named(Named::Escape) => self.update_browser(Message::CancelEdit),
						Key::Named(Named::Enter) => self.save_name_edit(),
						Key::Named(Named::ArrowLeft) if modifiers.alt() => {
							self.update_browser(Message::GoBack)
						}
						Key::Named(Named::ArrowRight) if modifiers.alt() => {
							self.update_browser(Message::GoForward)
						}
						_ => Task::none(),
					};
				}

				if modifiers.control()
					&& !modifiers.alt()
					&& !modifiers.logo()
					&& matches!(key.as_ref(), Key::Character("n" | "N"))
				{
					return self.update_browser(if modifiers.shift() {
						Message::NewFolder
					} else {
						Message::NewFile
					});
				}

				if modifiers.control()
					&& !modifiers.alt()
					&& !modifiers.logo()
					&& key.as_ref() == Key::Character("l")
				{
					return self.edit_path();
				}
				if key.as_ref() == Key::Named(Named::Escape) && self.location.value.is_some() {
					self.location.cancel();
					return self.reveal_selection();
				}

				if modifiers.control()
					&& !modifiers.shift()
					&& !modifiers.alt()
					&& !modifiers.logo()
				{
					match key.as_ref() {
						Key::Character("c") => {
							return self.update_transfers(
								crate::transfers::Message::PrepareSelection(
									crate::transfers::Operation::Copy,
								),
							);
						}
						Key::Character("x") => {
							return self.update_transfers(
								crate::transfers::Message::PrepareSelection(
									crate::transfers::Operation::Move,
								),
							);
						}
						Key::Character("v") if self.page == super::Page::Files => {
							return self.update_transfers(crate::transfers::Message::Paste(
								self.path.clone(),
							));
						}
						_ => {}
					}
				}
				if modifiers.control()
					&& !modifiers.shift()
					&& key.as_ref() == Key::Character("z")
					&& !self.transfers.busy
				{
					let latest = self
						.transfers
						.latest
						.as_ref()
						.filter(|outcome| outcome.status == crate::transfers::Status::Complete)
						.map(|outcome| outcome.id.clone());
					let id = latest.or_else(|| {
						self.transfers
							.history
							.iter()
							.find(|journal| {
								journal.status == crate::transfers::Status::Complete
									&& self
										.transfers
										.latest
										.as_ref()
										.is_none_or(|latest| latest.id != journal.id)
							})
							.map(|journal| journal.id.clone())
					});
					if let Some(id) = id {
						return self.update_transfers(crate::transfers::Message::Undo(id));
					}
				}
				if modifiers.control() && key.as_ref() == Key::Character("k") {
					self.typeahead.clear();
					return iced::widget::operation::focus("search");
				}
				match key.as_ref() {
					Key::Named(Named::Escape) if self.typeahead.is_active() => {
						self.typeahead.clear()
					}
					Key::Named(Named::Escape) => {
						if self.tag_editor.take().is_none() {
							return self.update_browser(Message::ClearSelection);
						}
					}
					Key::Named(Named::Tab) => {
						self.typeahead.clear();
						return if modifiers.shift() {
							iced::widget::operation::focus_previous()
						} else {
							iced::widget::operation::focus_next()
						};
					}
					Key::Named(Named::F5) => return self.load_directory(),
					Key::Named(Named::F2) => return self.update_browser(Message::Rename),
					Key::Named(Named::Enter) => return self.open_selected(),
					Key::Named(Named::ArrowLeft) if modifiers.alt() => {
						return self.update_browser(Message::GoBack);
					}
					Key::Named(Named::ArrowRight) if modifiers.alt() => {
						return self.update_browser(Message::GoForward);
					}
					_ => {}
				}
			}
			Message::Keyboard(_) => {}
		}
		Task::none()
	}
}
