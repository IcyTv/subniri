use super::{Message, typeahead};
use crate::{FileManager, filesystem::entry};
use iced::{
	Task,
	keyboard::{self, Key, key::Named},
};

impl FileManager {
	pub(crate) fn update_browser(&mut self, message: Message) -> Task<crate::Message> {
		match message {
			Message::TypeAhead(edit) => {
				if self.menu.is_some() {
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
						if restoring {
							return self.reveal_selection();
						}
					}
					Err(error) => {
						// Refresh failures retain the useful listing; navigation already cleared it.
						self.pending_selection = None;
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
			Message::NewFolder => {
				self.typeahead.clear();
				self.tag_editor = None;
				self.folder_name = Some(String::new());
				return iced::widget::operation::focus("folder-name");
			}
			Message::CancelFolder => self.folder_name = None,
			Message::FolderName(name) => self.folder_name = Some(name),
			Message::CreateFolder => {
				if self.creating_folder {
					return Task::none();
				}
				if let Some(name) = self.folder_name.clone() {
					self.creating_folder = true;
					let parent = self.path.clone();
					return Task::perform(
						async move {
							let worker_parent = parent.clone();
							let result = tokio::task::spawn_blocking(move || {
								entry::create_folder(&worker_parent, &name)
							})
							.await
							.map_err(|e| e.to_string())
							.and_then(|r| r);
							(parent, result)
						},
						|(parent, result)| Message::FolderCreated(parent, result).into(),
					);
				}
			}
			Message::FolderCreated(parent, result) => {
				self.creating_folder = false;
				match result {
					Ok(path) => {
						if self.closing.is_some() && !self.tag_busy && !self.favorites.running() {
							return self.finish_pending_close();
						}
						self.folder_name = None;
						if parent == self.path {
							self.select_only(path);
							self.search.clear();
							self.active_tag = None;
							return self.load_directory();
						}
					}
					Err(error) => {
						self.closing = None;
						self.error = Some(error);
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
				if modifiers.control() && key.as_ref() == Key::Character("k") {
					self.typeahead.clear();
					return iced::widget::operation::focus("search");
				}
				match key.as_ref() {
					Key::Named(Named::Escape) if self.typeahead.is_active() => {
						self.typeahead.clear()
					}
					Key::Named(Named::Escape) => {
						if self.folder_name.take().is_none() && self.tag_editor.take().is_none() {
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
