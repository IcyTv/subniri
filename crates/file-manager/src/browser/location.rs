//! Editable locations and worker-produced breadcrumb identities.
use super::Message as BrowserMessage;
use crate::{
	FileManager, Message,
	filesystem::{
		entry::DirEntry,
		locations::{FolderIdentity, expand_path},
	},
};
use iced::Task;
use std::path::{Path, PathBuf};

mod completion;

const COMPLETION_DELAY: std::time::Duration = std::time::Duration::from_millis(120);

#[derive(Debug, Clone)]
pub(crate) struct Listing {
	pub entries: Vec<DirEntry>,
	pub root: Option<FolderIdentity>,
}

#[derive(Default)]
pub(crate) struct Editor {
	pub value: Option<String>,
	pub error: Option<String>,
	pub pending: bool,
	pub suggestions: Vec<FolderIdentity>,
	pub selected_suggestion: Option<usize>,
	generation: u64,
	original: PathBuf,
}

impl Editor {
	pub fn cancel(&mut self) {
		self.generation += 1;
		self.value = None;
		self.error = None;
		self.pending = false;
		self.clear_suggestions();
	}

	pub fn change(&mut self, value: String) {
		if self.value.is_some() {
			self.generation += 1;
			self.value = Some(value);
			self.error = None;
			self.pending = false;
			self.clear_suggestions();
		}
	}

	fn clear_suggestions(&mut self) {
		self.suggestions.clear();
		self.selected_suggestion = None;
	}

	fn selected_folder(&self) -> Option<&FolderIdentity> {
		self.selected_suggestion
			.and_then(|index| self.suggestions.get(index))
	}
}

impl FileManager {
	pub(crate) fn edit_path(&mut self) -> Task<Message> {
		self.typeahead.clear();
		self.location.cancel();
		self.location.original = self.path.clone();
		self.location.value = Some(self.path.to_string_lossy().into_owned());

		Task::batch([
			iced::widget::operation::focus("location"),
			iced::widget::operation::select_all("location"),
			self.schedule_path_suggestions(),
		])
	}

	pub(crate) fn change_path(&mut self, value: String) -> Task<Message> {
		if self.location.value.is_none() {
			return Task::none();
		}

		self.location.change(value);
		self.schedule_path_suggestions()
	}

	fn schedule_path_suggestions(&self) -> Task<Message> {
		let generation = self.location.generation;

		Task::perform(
			async { tokio::time::sleep(COMPLETION_DELAY).await },
			move |()| BrowserMessage::PathSuggestionsDue(generation).into(),
		)
	}

	fn location_available(&self) -> bool {
		self.location.value.is_some()
			&& self.page == super::Page::Files
			&& !self.dialogs.blocked()
			&& !self.transfers.blocked()
	}

	pub(crate) fn path_suggestions_visible(&self) -> bool {
		self.location_available()
			&& !self.location.suggestions.is_empty()
			&& self.menu.is_none()
			&& self.drives.menu.is_none()
	}

	pub(crate) fn request_path_suggestions(&self, generation: u64) -> Task<Message> {
		if !self.location_available()
			|| self.location.pending
			|| generation != self.location.generation
		{
			return Task::none();
		}

		let value = self.location.value.clone().unwrap_or_default();
		let current = self.path.clone();

		Task::perform(
			async move {
				tokio::task::spawn_blocking(move || {
					let roots = config::ConfigFile::load()
						.map(|(_, config)| config.fileberg.roots)
						.unwrap_or_default();
					let locations = crate::filesystem::locations::Locations::new(&roots);

					completion::suggestions(
						&value,
						&current,
						dirs::home_dir().as_deref(),
						&locations,
					)
				})
				.await
				.unwrap_or_default()
			},
			move |suggestions| {
				BrowserMessage::PathSuggestionsLoaded(generation, suggestions).into()
			},
		)
	}

	pub(crate) fn path_suggestions_loaded(
		&mut self, generation: u64, suggestions: Vec<FolderIdentity>,
	) {
		if !self.location_available()
			|| self.location.pending
			|| generation != self.location.generation
		{
			return;
		}

		self.location.suggestions = suggestions;
		self.location.selected_suggestion = None;
	}

	pub(crate) fn move_path_suggestion(&mut self, down: bool) -> Task<Message> {
		if !self.location_available() || self.location.suggestions.is_empty() {
			return Task::none();
		}

		let last = self.location.suggestions.len() - 1;
		let index = match (self.location.selected_suggestion, down) {
			(Some(index), true) => (index + 1).min(last),
			(Some(index), false) => index.saturating_sub(1),
			(None, true) => 0,
			(None, false) => last,
		};
		self.location.selected_suggestion = Some(index);

		iced::widget::operation::scroll_to(
			"path-suggestions",
			iced::widget::scrollable::AbsoluteOffset {
				x: None,
				y: Some(index.saturating_sub(3) as f32 * crate::ui::path_completion::ROW_HEIGHT),
			},
		)
	}

	pub(crate) fn fill_path(&mut self, clicked: Option<PathBuf>) -> Task<Message> {
		if !self.location_available() {
			return Task::none();
		}

		let folder = if let Some(path) = clicked {
			self.location
				.suggestions
				.iter()
				.find(|folder| folder.path == path)
		} else {
			self.location
				.selected_folder()
				.or_else(|| self.location.suggestions.first())
		};
		let Some(path) = folder.and_then(|folder| folder.path.to_str()) else {
			return Task::none();
		};

		let mut value = path.to_owned();
		if !value.ends_with('/') {
			value.push('/');
		}

		let suggestions = self.change_path(value);

		Task::batch([
			suggestions,
			iced::widget::operation::focus("location"),
			iced::widget::operation::move_cursor_to_end("location"),
		])
	}

	pub(crate) fn submit_path(&mut self) -> Task<Message> {
		let Some(value) = self.location.value.as_deref() else {
			return Task::none();
		};
		if self.location.pending {
			return Task::none();
		}

		let path = if let Some(folder) = self.location.selected_folder() {
			Ok(folder.path.clone())
		} else if value == self.location.original.to_string_lossy() {
			Ok(self.location.original.clone())
		} else {
			resolve_path(value, &self.path, dirs::home_dir().as_deref())
		};
		let path = match path {
			Ok(path) => path,
			Err(error) => {
				self.location.error = Some(error);
				return Task::none();
			}
		};

		self.location.generation += 1;
		self.location.pending = true;
		self.location.clear_suggestions();
		let generation = self.location.generation;

		Task::perform(
			async move {
				tokio::task::spawn_blocking(move || {
					std::fs::read_dir(&path)
						.map_err(|error| format!("Cannot open {}: {error}", path.display()))?;

					// Resolve explicit dot components using filesystem semantics: symlink/..
					// need not mean the lexical parent of the symlink itself.
					if path.components().any(|part| {
						matches!(
							part,
							std::path::Component::CurDir | std::path::Component::ParentDir
						)
					}) {
						std::fs::canonicalize(&path).map_err(|error| error.to_string())
					} else {
						Ok(path)
					}
				})
				.await
				.map_err(|error| error.to_string())
				.and_then(|result| result)
			},
			move |result| BrowserMessage::PathValidated(generation, result).into(),
		)
	}

	pub(crate) fn path_validated(
		&mut self, generation: u64, result: Result<PathBuf, String>,
	) -> Task<Message> {
		if self.location.value.is_none() || generation != self.location.generation {
			return Task::none();
		}
		if self.page != super::Page::Files || self.dialogs.blocked() || self.transfers.blocked() {
			self.location.cancel();
			return Task::none();
		}

		self.location.pending = false;
		match result {
			Ok(path) => {
				self.location.cancel();
				self.goto(path)
			}
			Err(error) => {
				self.location.error = Some(error);
				iced::widget::operation::focus("location")
			}
		}
	}
}

fn resolve_path(value: &str, current: &Path, home: Option<&Path>) -> Result<PathBuf, String> {
	if value.is_empty() || value.contains('\0') {
		return Err("Enter a folder path.".into());
	}

	if value == "~" || value.starts_with("~/") {
		return expand_path(value, home).ok_or_else(|| "Home directory is unavailable.".into());
	}

	let path = Path::new(value);
	Ok(if path.is_absolute() {
		path.to_path_buf()
	} else {
		current.join(path)
	})
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::{app::Args, test_support::TestDir};
	use futures::StreamExt;

	fn folder(path: PathBuf) -> FolderIdentity {
		FolderIdentity {
			label: path.file_name().unwrap().to_string_lossy().into_owned(),
			path,
			icon: crate::ui::icons::ui_icon!("folder"),
			symbolic: true,
		}
	}

	#[tokio::test]
	async fn completion_scans_keep_selection_empty_and_reject_obsolete_requests() {
		let dir = TestDir::new();
		std::fs::create_dir(dir.path().join("Child")).unwrap();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});

		let _ = app.edit_path();
		let old = app.location.generation;
		let _ = app.change_path("cH".into());
		let current = app.location.generation;

		assert!(iced_runtime::task::into_stream(app.request_path_suggestions(old)).is_none());

		let task = app.request_path_suggestions(current);
		deliver(&mut app, task).await;

		assert!(
			app.location
				.suggestions
				.iter()
				.any(|folder| folder.path == dir.path().join("Child"))
		);
		assert!(app.location.selected_suggestion.is_none());

		let expected = app
			.location
			.suggestions
			.iter()
			.map(|folder| folder.path.clone())
			.collect::<Vec<_>>();

		app.path_suggestions_loaded(old, Vec::new());

		assert_eq!(
			app.location
				.suggestions
				.iter()
				.map(|folder| folder.path.clone())
				.collect::<Vec<_>>(),
			expected
		);

		app.location.cancel();
		app.path_suggestions_loaded(current, vec![folder(dir.path().join("Child"))]);

		assert!(app.location.suggestions.is_empty());
	}

	#[test]
	fn tab_and_click_fill_actual_paths_without_navigating_or_changing_history() {
		let dir = TestDir::new();
		let child = dir.path().join("Child Folder");
		let second = dir.path().join("Second");
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});

		let _ = app.edit_path();
		let _ = app.change_path("cH".into());
		let generation = app.location.generation;
		app.path_suggestions_loaded(
			generation,
			vec![folder(child.clone()), folder(second.clone())],
		);

		let _ = app.fill_path(None);

		assert_eq!(
			app.location.value.as_deref(),
			Some(format!("{}/", child.display()).as_str())
		);
		assert_eq!(app.path, dir.path());
		assert!(!app.history.can_undo());
		assert!(app.location.suggestions.is_empty());

		let generation = app.location.generation;
		app.path_suggestions_loaded(generation, vec![folder(child), folder(second.clone())]);
		let _ = app.fill_path(Some(second.clone()));

		assert_eq!(
			app.location.value.as_deref(),
			Some(format!("{}/", second.display()).as_str())
		);
		assert_eq!(app.path, dir.path());

		let retained = app.location.value.clone();
		let _ = app.fill_path(Some(dir.path().join("obsolete")));

		assert_eq!(app.location.value, retained);
	}

	#[tokio::test]
	async fn enter_uses_only_explicit_selection_and_typing_resets_it() {
		let dir = TestDir::new();
		let first = dir.path().join("first");
		let second = dir.path().join("second");
		std::fs::create_dir(&first).unwrap();
		std::fs::create_dir(&second).unwrap();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});

		let _ = app.edit_path();
		let _ = app.change_path("first".into());
		app.path_suggestions_loaded(app.location.generation, vec![folder(second.clone())]);

		let task = app.submit_path();
		deliver(&mut app, task).await;

		assert_eq!(app.path, first);

		let _ = app.edit_path();
		let _ = app.change_path("not a path".into());
		app.path_suggestions_loaded(
			app.location.generation,
			vec![folder(first.clone()), folder(second.clone())],
		);
		let _ = app.move_path_suggestion(false);

		assert_eq!(app.location.selected_suggestion, Some(1));

		let _ = app.move_path_suggestion(false);

		assert_eq!(app.location.selected_suggestion, Some(0));

		let _ = app.move_path_suggestion(true);
		let task = app.submit_path();
		deliver(&mut app, task).await;

		assert_eq!(app.path, second);

		let _ = app.edit_path();
		app.path_suggestions_loaded(app.location.generation, vec![folder(first)]);
		let _ = app.move_path_suggestion(true);
		let _ = app.change_path("new text".into());

		assert!(app.location.selected_suggestion.is_none());
		assert!(app.location.suggestions.is_empty());
	}

	#[test]
	fn page_switches_prevent_scan_results_and_fill_actions() {
		let dir = TestDir::new();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});

		let _ = app.edit_path();
		let generation = app.location.generation;
		let value = app.location.value.clone();
		app.page = super::super::Page::History;

		app.path_suggestions_loaded(generation, vec![folder(dir.path().join("Child"))]);
		let _ = app.fill_path(None);

		assert!(app.location.suggestions.is_empty());
		assert_eq!(app.location.value, value);
		assert!(
			iced_runtime::task::into_stream(app.request_path_suggestions(generation)).is_none()
		);
	}

	async fn deliver(app: &mut FileManager, task: Task<Message>) {
		if let Some(mut stream) = iced_runtime::task::into_stream(task) {
			while let Some(action) = stream.next().await {
				if let iced_runtime::Action::Output(message) = action {
					let _ = app.update(message);
				}
			}
		}
	}

	#[tokio::test]
	async fn path_submission_changes_history_only_on_success_and_cancellation_rejects_late_results()
	{
		let dir = TestDir::new();
		let child = dir.path().join("child");
		std::fs::create_dir(&child).unwrap();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});

		let _ = app.edit_path();
		app.location.change("missing".into());
		let task = app.submit_path();
		deliver(&mut app, task).await;

		assert_eq!(app.path, dir.path());
		assert!(app.location.error.is_some());
		assert!(!app.history.can_undo());

		app.location.change("child".into());
		let task = app.submit_path();
		deliver(&mut app, task).await;

		assert_eq!(app.path, child);
		assert!(app.location.value.is_none());
		assert!(app.history.can_undo());

		let _ = app.edit_path();
		app.location.change("..".into());
		let task = app.submit_path();
		app.location.cancel();
		deliver(&mut app, task).await;

		assert_eq!(app.path, child);
	}

	#[tokio::test]
	async fn listing_installs_root_identity_and_rejects_stale_results() {
		let dir = TestDir::new();
		std::fs::create_dir(dir.path().join(".git")).unwrap();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});

		let task = app.load_directory();
		deliver(&mut app, task).await;

		assert_eq!(app.breadcrumb_root.as_ref().unwrap().path, dir.path());

		let id = app.request_id;
		let _ = app.update_browser(BrowserMessage::ListingLoaded(
			id - 1,
			Ok(Listing {
				entries: Vec::new(),
				root: None,
			}),
		));

		assert!(app.breadcrumb_root.is_some());

		let _ = app.goto(dir.path().join(".git"));

		assert!(app.breadcrumb_root.is_none());
	}

	#[test]
	fn leaving_the_file_page_cancels_pending_path_navigation() {
		let dir = TestDir::new();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});

		let _ = app.edit_path();
		let generation = app.location.generation;
		app.page = super::super::Page::History;

		let _ = app.path_validated(generation, Ok(PathBuf::from("/elsewhere")));

		assert_eq!(app.path, dir.path());
		assert!(app.location.value.is_none());
		assert_eq!(app.page, super::super::Page::History);
	}

	#[test]
	fn resolves_absolute_home_and_relative_paths_without_shell_expansion() {
		let current = Path::new("/work/project");
		let home = Some(Path::new("/home/test"));

		for (input, expected) in [
			("/etc", "/etc"),
			("~/docs", "/home/test/docs"),
			("~", "/home/test"),
			("../other", "/work/project/../other"),
			("with spaces", "/work/project/with spaces"),
			("$HOME", "/work/project/$HOME"),
		] {
			assert_eq!(
				resolve_path(input, current, home),
				Ok(PathBuf::from(expected))
			);
		}

		assert!(resolve_path("", current, home).is_err());
		assert!(resolve_path("~/docs", current, None).is_err());
		assert!(resolve_path("bad\0path", current, home).is_err());
	}

	#[test]
	fn editing_and_cancellation_invalidate_pending_results() {
		let mut editor = Editor {
			value: Some("/work".into()),
			pending: true,
			..Default::default()
		};
		let original_generation = editor.generation;

		editor.change("/other".into());

		assert!(editor.generation > original_generation);
		assert!(!editor.pending);

		editor.cancel();

		assert!(editor.value.is_none());
		assert!(editor.error.is_none());
	}
}
