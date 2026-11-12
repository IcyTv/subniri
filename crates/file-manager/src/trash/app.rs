use super::{Captured, TrashListing};
use crate::{
	FileManager,
	browser::Page,
	context_menu::Target,
	transfers::{self, Store},
};
use iced::{Task, keyboard};
use std::{
	collections::BTreeSet,
	io,
	path::PathBuf,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
};

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;

#[derive(Default)]
pub(crate) struct State {
	pub listing: Option<Arc<Listed>>,
	pub selected: BTreeSet<PathBuf>,
	pub loading: bool,
	pub busy: bool,
	pub status: Option<&'static str>,
	pub error: Option<String>,
	generation: u64,
	operation: u64,
	cancelled: Arc<AtomicBool>,
}

impl State {
	pub fn cancel_waiting(&self) {
		self.cancelled.store(true, Ordering::Release);
	}
}

#[derive(Debug)]
pub(crate) struct Listed {
	pub contents: TrashListing,
	pub pending: Vec<transfers::PendingDeletion>,
	pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Intent {
	Restore,
	Delete { empty: bool },
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
	Show,
	Refresh,
	Loaded(u64, Result<Arc<Listed>, String>),
	Select(PathBuf, bool),
	SelectAll(bool),
	Restore,
	Delete,
	Empty,
	Captured(u64, Intent, Result<Vec<Captured>, String>),
	Trash(Target),
	Finished(u64, Result<Vec<String>, String>),
	Retry(String),
	Dismiss,
}

impl From<Message> for crate::Message {
	fn from(message: Message) -> Self {
		Self::Trash(message)
	}
}

impl FileManager {
	pub(crate) fn trash_mutation_blocked(&self) -> bool {
		self.trash_state.busy
			|| self.transfers.busy
			|| self.transfers.blocked()
			|| self.dialogs.blocked()
			|| self.dialogs.deleting.is_some()
			|| self.drives.busy.is_some()
			|| self.closing.is_some()
	}

	pub(crate) fn update_trash(&mut self, message: Message) -> Task<crate::Message> {
		match message {
			Message::Show => {
				self.page = Page::Trash;
				self.typeahead.clear();
				let close = self.menu_lifecycle(neo_widgets::popup::Event::CloseAll);
				return close.chain(self.update_trash(Message::Refresh));
			}
			Message::Refresh => {
				if self.trash_state.busy || self.transfers.busy {
					return Task::none();
				}
				self.trash_state.generation += 1;
				self.trash_state.loading = true;
				let generation = self.trash_state.generation;
				return Task::perform(
					async {
						tokio::task::spawn_blocking(|| {
							let contents =
								super::list_trash().map_err(|error| error.to_string())?;
							let mut warnings = Vec::new();
							let pending =
								transfers::read_pending_trash_deletions().unwrap_or_else(|error| {
									warnings.push(format!(
										"Could not read interrupted deletion records: {error}"
									));
									Vec::new()
								});
							Ok(Arc::new(Listed {
								contents,
								pending,
								warnings,
							}))
						})
						.await
						.map_err(|error| error.to_string())
						.and_then(|result| result)
					},
					move |result| Message::Loaded(generation, result).into(),
				);
			}
			Message::Loaded(generation, result) if generation == self.trash_state.generation => {
				self.trash_state.loading = false;
				match result {
					Ok(listing) => {
						self.trash_state.selected.retain(|path| {
							listing
								.contents
								.entries
								.iter()
								.any(|entry| entry.stored_path == *path)
						});
						self.trash_state.listing = Some(listing);
					}
					Err(error) => self.trash_state.error = Some(error),
				}
			}
			Message::Select(path, selected) if !self.trash_state.busy => {
				if self.trash_state.listing.as_ref().is_some_and(|listing| {
					listing
						.contents
						.entries
						.iter()
						.any(|entry| entry.stored_path == path)
				}) {
					if selected {
						self.trash_state.selected.insert(path);
					} else {
						self.trash_state.selected.remove(&path);
					}
				}
			}
			Message::SelectAll(selected) if !self.trash_state.busy => {
				self.trash_state.selected = if selected {
					self.trash_state
						.listing
						.as_ref()
						.map_or_else(BTreeSet::new, |listing| {
							listing
								.contents
								.entries
								.iter()
								.map(|entry| entry.stored_path.clone())
								.collect()
						})
				} else {
					BTreeSet::new()
				};
			}
			Message::Restore => return self.capture_trash_selection(Intent::Restore),
			Message::Delete => {
				return self.capture_trash_selection(Intent::Delete { empty: false });
			}
			Message::Empty => return self.capture_trash_selection(Intent::Delete { empty: true }),
			Message::Captured(operation, intent, result)
				if operation == self.trash_state.operation =>
			{
				self.trash_state.busy = false;
				self.trash_state.status = None;
				if self.closing.is_some() {
					return self.finish_pending_close();
				}
				match result {
					Ok(items) => match intent {
						Intent::Restore => {
							return self.update_transfers(transfers::Message::Restore(items));
						}
						Intent::Delete { empty } => {
							return self.request_trash_delete_confirmation(items, empty);
						}
					},
					Err(error) => self.trash_state.error = Some(error),
				}
			}
			Message::Trash(target) => {
				if self.trash_mutation_blocked() {
					return Task::none();
				}
				return self.start_trash_work("Moving to Trash…", move |_, cancelled| {
					if !target.available() {
						return Err(io::Error::other("Selected items changed before trashing"));
					}
					let mut errors = Vec::new();
					for item in std::iter::once(&target).chain(&target.others) {
						if cancelled.load(Ordering::Acquire) {
							errors.push(
								"Trashing stopped; remaining items were left untouched".into(),
							);
							break;
						}
						if !item.available() {
							errors.push(format!("{} changed; not trashed", item.path.display()));
							continue;
						}
						if let Err(error) = super::trash(&item.path) {
							errors.push(format!("{}: {error}", item.path.display()));
						}
					}
					Ok(errors)
				});
			}
			Message::Retry(id) => {
				if self.trash_mutation_blocked()
					|| !self.trash_state.listing.as_ref().is_some_and(|listing| {
						listing.pending.iter().any(|pending| pending.id == id)
					}) {
					return Task::none();
				}
				return self.start_trash_work("Retrying confirmed deletion…", move |store, _| {
					transfers::retry_trash_deletion(store, &id).map(|_| Vec::new())
				});
			}
			Message::Finished(operation, result) if operation == self.trash_state.operation => {
				self.trash_state.busy = false;
				self.trash_state.status = None;
				let problem = match result {
					Ok(errors) if !errors.is_empty() => Some(errors.join("\n")),
					Err(error) => Some(error),
					_ => None,
				};
				if let Some(error) = problem {
					self.trash_state.error = Some(error.clone());
					if self.page != Page::Trash {
						self.error = Some(error);
					}
				}
				if self.closing.is_some() {
					return self.finish_pending_close();
				}
				return Task::batch([self.load_directory(), self.update_trash(Message::Refresh)]);
			}
			Message::Dismiss => self.trash_state.error = None,
			_ => {}
		}
		Task::none()
	}

	fn capture_trash_selection(&mut self, intent: Intent) -> Task<crate::Message> {
		if self.page != Page::Trash || self.trash_mutation_blocked() || self.trash_state.loading {
			return Task::none();
		}
		let Some(listing) = self.trash_state.listing.clone() else {
			return Task::none();
		};
		let selected = self.trash_state.selected.clone();
		let all = matches!(intent, Intent::Delete { empty: true });
		if listing.contents.entries.is_empty() || (!all && selected.is_empty()) {
			return Task::none();
		}
		self.trash_state.operation += 1;
		self.trash_state.generation += 1;
		self.trash_state.busy = true;
		self.trash_state.error = None;
		self.trash_state.status = Some("Preparing selection…");
		let operation = self.trash_state.operation;
		Task::perform(
			async move {
				tokio::task::spawn_blocking(move || {
					listing
						.contents
						.entries
						.iter()
						.filter(|entry| all || selected.contains(&entry.stored_path))
						.map(|entry| {
							Captured::capture(entry, matches!(intent, Intent::Restore))
								.map_err(|error| error.to_string())
						})
						.collect::<Result<Vec<_>, _>>()
				})
				.await
				.map_err(|error| error.to_string())
				.and_then(|result| result)
			},
			move |result| Message::Captured(operation, intent, result).into(),
		)
	}

	fn start_trash_work(
		&mut self, status: &'static str,
		work: impl FnOnce(&Store, &AtomicBool) -> io::Result<Vec<String>> + Send + 'static,
	) -> Task<crate::Message> {
		self.trash_state.operation += 1;
		self.trash_state.generation += 1;
		self.trash_state.loading = false;
		self.trash_state.busy = true;
		self.trash_state.status = Some(status);
		self.trash_state.error = None;
		self.trash_state.cancelled = Arc::new(AtomicBool::new(false));
		let cancelled = self.trash_state.cancelled.clone();
		let operation = self.trash_state.operation;
		Task::perform(
			async move {
				tokio::task::spawn_blocking(move || {
					let store = Store::open_wait(Store::default_path()?, &cancelled)?;
					work(&store, &cancelled)
				})
				.await
				.map_err(|error| error.to_string())
				.and_then(|result| result.map_err(|error| error.to_string()))
			},
			move |result| Message::Finished(operation, result).into(),
		)
	}

	pub(crate) fn perform_trash_delete(&mut self, items: Vec<Captured>) -> Task<crate::Message> {
		if self.trash_mutation_blocked() {
			return Task::none();
		}
		self.start_trash_work("Deleting permanently…", move |store, _| {
			transfers::delete_trash(store, items)
		})
	}

	pub(crate) fn trash_keyboard(&mut self, event: keyboard::Event) -> Task<crate::Message> {
		let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
			return Task::none();
		};
		use keyboard::{Key, key::Named};
		match key.as_ref() {
			Key::Character("a") if modifiers.control() => {
				self.update_trash(Message::SelectAll(true))
			}
			Key::Named(Named::Escape) => self.update_trash(Message::SelectAll(false)),
			Key::Named(Named::Delete) => self.update_trash(Message::Delete),
			Key::Named(Named::F5) => self.update_trash(Message::Refresh),
			Key::Named(Named::Tab)
				if !modifiers.alt() && !modifiers.control() && !modifiers.logo() =>
			{
				if modifiers.shift() {
					iced::widget::operation::focus_previous()
				} else {
					iced::widget::operation::focus_next()
				}
			}
			Key::Named(Named::ArrowLeft) if modifiers.alt() => {
				self.update_browser(crate::browser::Message::GoBack)
			}
			Key::Named(Named::ArrowRight) if modifiers.alt() => {
				self.update_browser(crate::browser::Message::GoForward)
			}
			Key::Named(Named::ArrowUp) if modifiers.alt() => {
				self.update_browser(crate::browser::Message::GoUp)
			}
			Key::Character("z") if modifiers.control() => self
				.transfers
				.latest
				.as_ref()
				.filter(|outcome| outcome.status == transfers::Status::Complete)
				.map(|outcome| outcome.id.clone())
				.map_or_else(Task::none, |id| {
					self.update_transfers(transfers::Message::Undo(id))
				}),
			_ => Task::none(),
		}
	}
}
