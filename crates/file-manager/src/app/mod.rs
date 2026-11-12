use std::path::PathBuf;
mod routing;

use clap::Parser;
use iced::{Subscription, Task, Theme, keyboard, theme};
use iced_exwlshell::{
	actions::IcedXdgWindowSettings,
	reexport::PixelSize,
	settings::{LayerShellSettings, StartMode},
	to_exwlshell_message,
};
use neo_widgets::appearance::{Appearance, Event as AppearanceEvent};

use crate::tags::{TagEditor, TagId, TagStore};
use crate::{browser::navigation::History, filesystem::entry::DirEntry};
use context_menu::Message as ContextMenuMessage;

use crate::browser::{selection, typeahead};
use crate::context_menu::favorites;
#[cfg(test)]
use crate::filesystem::entry;
use crate::filesystem::storage;
use crate::{browser, context_menu, tags};

#[derive(Debug, Clone, Parser)]
pub(crate) struct Args {
	#[clap(default_value_os_t = default_path())]
	pub(crate) path: PathBuf,
}

pub(crate) fn default_path() -> PathBuf {
	dirs::home_dir()
		.or_else(|| std::env::current_dir().ok())
		.unwrap_or_else(|| "/".into())
}

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
	log::init!("file_manager", "fileberg")?;
	let args = Args::parse();
	iced_exwlshell::daemon(
		move || {
			let (app, task) = FileManager::new(args.clone());
			let window = Task::done(Message::NewBaseWindow {
				id: app.host,
				settings: IcedXdgWindowSettings {
					parent: None,
					size: Some(PixelSize::px(1100, 760)),
					client_side_decorations: true,
				},
			});
			(app, window.chain(task))
		},
		|| "fileberg".to_owned(),
		FileManager::update,
		FileManager::view,
	)
	.title(FileManager::title)
	.settings(iced_exwlshell::Settings {
		id: Some("fileberg".into()),
		default_font: iced::Font::new("DM Sans"),
		fonts: vec![
			include_bytes!("../../assets/fonts/DMSans.ttf")
				.as_slice()
				.into(),
			include_bytes!("../../assets/fonts/SpaceMono-Regular.ttf")
				.as_slice()
				.into(),
		],
		..Default::default()
	})
	.theme(|app: &FileManager, _| app.appearance.colors().iced_theme())
	.style(FileManager::style)
	.subscription(FileManager::subscription)
	.layer_settings(LayerShellSettings {
		start_mode: StartMode::Background,
		..Default::default()
	})
	.run()?;
	Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewMode {
	Grid,
	List,
}

#[to_exwlshell_message]
#[derive(Debug, Clone)]
pub(crate) enum Message {
	Appearance(AppearanceEvent),
	Drives(crate::drives::Message),
	Dialog(crate::dialogs::Message),
	Input(iced::window::Id, iced::Event, iced::event::Status),
	Browser(browser::Message),
	ContextMenu(context_menu::Message),
	Tags(tags::Message),
	WindowEvent(iced::window::Id, iced::window::Event),
	CloseRequested(iced::window::Id),
}

pub(crate) struct FileManager {
	pub(crate) appearance: Appearance,
	pub(crate) drives: crate::drives::Drives,
	pub(crate) dialogs: crate::dialogs::Dialogs,
	pub(crate) favorites: favorites::Favorites,
	pub(crate) host: iced::window::Id,
	pub(crate) host_size: iced::Size,
	pub(crate) menu: Option<context_menu::Menu>,
	pub(crate) popups: neo_widgets::popup::Popups<()>,
	pub(crate) path: PathBuf,
	pub(crate) history: History,
	pub(crate) entries: Vec<DirEntry>,
	/// Keyboard cursor, which may be outside selection membership after Ctrl+arrows.
	pub(crate) selected: Option<PathBuf>,
	pub(crate) selection: selection::Selection,
	pub(crate) modifiers: keyboard::Modifiers,
	pub(crate) pending_selection: Option<PathBuf>,
	pub(crate) search: String,
	pub(crate) view_mode: ViewMode,
	pub(crate) ascending: bool,
	pub(crate) loading: bool,
	pub(crate) request_id: u64,
	pub(crate) error: Option<String>,
	pub(crate) folder_name: Option<String>,
	pub(crate) creating_folder: bool,
	pub(crate) tags: Option<TagStore>,
	pub(crate) tag_problem: Option<String>,
	pub(crate) tag_busy: bool,
	pub(crate) active_tag: Option<TagId>,
	pub(crate) tag_editor: Option<TagEditor>,
	pub(crate) show_info: bool,
	pub(crate) closing: Option<iced::window::Id>,
	pub(crate) storage: Option<storage::Usage>,
	pub(crate) typeahead: typeahead::TypeAhead,
}

impl FileManager {
	pub(crate) fn new(args: Args) -> (Self, Task<Message>) {
		let path = std::path::absolute(&args.path).unwrap_or(args.path);
		let mut app = Self {
			appearance: Appearance::default(),
			drives: crate::drives::Drives::default(),
			dialogs: crate::dialogs::Dialogs::default(),
			favorites: favorites::Favorites::default(),
			host: iced::window::Id::unique(),
			host_size: iced::Size::new(1100.0, 760.0),
			menu: None,
			popups: neo_widgets::popup::Popups::default(),
			history: History::new(path.clone()),
			path,
			entries: Vec::new(),
			selected: None,
			selection: selection::Selection::default(),
			modifiers: keyboard::Modifiers::default(),
			pending_selection: None,
			search: String::new(),
			view_mode: ViewMode::Grid,
			ascending: true,
			loading: false,
			request_id: 0,
			error: None,
			folder_name: None,
			creating_folder: false,
			tags: None,
			tag_problem: None,
			tag_busy: true,
			active_tag: None,
			tag_editor: None,
			show_info: false,
			closing: None,
			storage: None,
			typeahead: typeahead::TypeAhead::default(),
		};
		let task = app.load_directory();
		let drives = app.update_drives(crate::drives::Message::Refresh);
		(
			app,
			Task::batch([task, drives, Self::load_tags(), favorites::load(0)]),
		)
	}

	fn subscription(&self) -> Subscription<Message> {
		use futures::StreamExt;
		let mut subscriptions = vec![
			self.appearance.subscription().map(Message::Appearance),
			iced::time::every(std::time::Duration::from_secs(2))
				.map(|_| crate::drives::Message::Refresh.into()),
			iced::event::listen_with(|event, status, id| match event {
				iced::Event::Window(iced::window::Event::RedrawRequested(_)) => None,
				iced::Event::Window(event) => Some(Message::WindowEvent(id, event)),
				event => Some(Message::Input(id, event, status)),
			}),
			neo_widgets::popup::parent_clicks(self.popups.context_children())
				.map(|event| ContextMenuMessage::MenuLifecycle(event).into()),
			Subscription::run(|| {
				config::ConfigFile::watch().map_or_else(
					|error| {
						futures::stream::once(async move {
							ContextMenuMessage::FavoritesLoaded(0, Err(error.to_string())).into()
						})
						.boxed()
					},
					|watch| {
						watch
							.map(|_| ContextMenuMessage::FavoritesReload.into())
							.boxed()
					},
				)
			}),
		];
		if let Some(id) = self
			.menu
			.as_ref()
			.and_then(|m| m.pages.first().map(|p| p.id))
		{
			subscriptions.push(
				iced::time::every(std::time::Duration::from_millis(300))
					.with(id)
					.map(|(id, _)| ContextMenuMessage::MenuPoll(id).into()),
			);
		}
		Subscription::batch(subscriptions)
	}

	pub(crate) fn update(&mut self, message: impl Into<Message>) -> Task<Message> {
		let message = message.into();
		if self.dialogs.blocked() && message.is_parent_interaction() {
			return Task::none();
		}
		match message {
			Message::Appearance(event) => self.appearance.update(event),
			Message::Drives(message) => return self.update_drives(message),
			Message::Dialog(message) => return self.update_dialog(message),
			Message::Input(id, event, status) => return self.route_input(id, event, status),
			Message::Browser(message) => return self.update_browser(message),
			Message::ContextMenu(message) => return self.update_context_menu(message),
			Message::Tags(message) => return self.update_tags(message),
			Message::WindowEvent(id, event) => return self.route_window_event(id, event),
			Message::CloseRequested(id) => {
				if id != self.host {
					return Task::none();
				}
				let close = self.cancel_dialog_for_shutdown();
				self.closing = Some(id);
				return close.chain(self.finish_pending_close());
			}
			_ => {}
		}
		Task::none()
	}

	fn style(&self, _theme: &Theme) -> theme::Style {
		theme::Style {
			// Popups include unpainted shadow gutters. Paint the host in its view,
			// not in the shared surface clear color.
			background_color: iced::Color::TRANSPARENT,
			text_color: self.appearance.colors().text,
		}
	}

	pub(crate) fn finish_pending_close(&mut self) -> Task<Message> {
		if self.dialogs.deleting.is_some()
			|| self.drives.busy.is_some()
			|| self.tag_busy
			|| self.creating_folder
			|| self.favorites.running()
		{
			return Task::none();
		}
		self.closing
			.take()
			.map_or_else(Task::none, |_| iced::exit())
	}
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::browser::Message;
	use crate::test_support::TestDir;
	#[test]
	fn selection_search_and_stale_loads() {
		let dir = TestDir::new();
		std::fs::write(dir.path().join("notes.txt"), "abc").unwrap();
		std::fs::write(dir.path().join("image.png"), "abc").unwrap();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().to_owned(),
		});
		let _ = app.update(Message::Loaded(
			app.request_id,
			entry::read_directory(dir.path()),
		));
		let selected = dir.path().join("notes.txt");
		let _ = app.update(Message::Clicked(selected.clone()));
		assert_eq!(app.selected, Some(selected));
		let _ = app.update(Message::Search("IMAGE".into()));
		assert_eq!(app.visible_entries().len(), 1);
		assert!(app.selected.is_none());
		let _ = app.update(Message::Loaded(app.request_id - 1, Err("stale".into())));
		assert!(app.error.is_none());
		assert_eq!(app.entries.len(), 2);
	}

	#[test]
	fn tag_filter_combines_with_search_and_survives_view_switch() {
		let dir = TestDir::new();
		for name in ["design.pdf", "notes.md"] {
			std::fs::write(dir.path().join(name), "x").unwrap();
		}
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		let store = TagStore::open(dir.path().join("data/tags.json"))
			.unwrap()
			.edited(tags::Edit::Create {
				name: "Work".into(),
				parent: None,
			})
			.unwrap()
			.edited(tags::Edit::Create {
				name: "Design".into(),
				parent: Some(2),
			})
			.unwrap()
			.edited(tags::Edit::Assign {
				path: dir.path().join("design.pdf"),
				id: 3,
				assigned: true,
			})
			.unwrap();
		let _ = app.update(tags::Message::TagsLoaded(Ok(store)));
		let _ = app.update(Message::Loaded(
			app.request_id,
			entry::read_directory(dir.path()),
		));
		let _ = app.update(tags::Message::TagFilter(Some(2)));
		assert_eq!(
			app.visible_entries()
				.iter()
				.map(|e| e.name.as_str())
				.collect::<Vec<_>>(),
			vec!["design.pdf"]
		);
		let _ = app.update(Message::Clicked(dir.path().join("design.pdf")));
		let _ = app.update(Message::View(ViewMode::List));
		assert!(app.selected.is_some());
		let _ = app.update(Message::Search("notes".into()));
		assert!(app.visible_entries().is_empty());
		assert!(app.selected.is_none());
		let _ = app.update(Message::Search(String::new()));
		let _ = app.update(tags::Message::TagFilter(None));
		assert!(app.visible_entries().len() >= 2);
	}

	#[test]
	fn save_errors_keep_snapshot_and_cancel_pending_close() {
		let dir = TestDir::new();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		let _ = app.update(tags::Message::TagsLoaded(Ok(TagStore::open(
			dir.path().join("tags.json"),
		)
		.unwrap())));
		app.tag_busy = true;
		let id = app.host;
		let _ = app.update(crate::Message::CloseRequested(id));
		assert_eq!(app.closing, Some(id));
		let _ = app.update(tags::Message::TagsSaved(Err("disk full".into())));
		assert!(app.closing.is_none());
		assert!(!app.tag_busy);
		assert_eq!(app.tags.as_ref().unwrap().data.tags.len(), 1);
		assert_eq!(app.tag_problem.as_deref(), Some("disk full"));
	}

	#[test]
	fn descending_sort_keeps_folders_first_and_errors_are_visible() {
		let dir = TestDir::new();
		std::fs::create_dir(dir.path().join("Folder")).unwrap();
		std::fs::write(dir.path().join("a.txt"), "a").unwrap();
		std::fs::write(dir.path().join("z.txt"), "z").unwrap();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		let _ = app.update(Message::Loaded(
			app.request_id,
			entry::read_directory(dir.path()),
		));
		let _ = app.update(Message::Sort);
		assert_eq!(
			app.visible_entries()
				.iter()
				.map(|e| e.name.as_str())
				.collect::<Vec<_>>(),
			vec!["Folder", "z.txt", "a.txt"]
		);
		let _ = app.update(Message::Loaded(
			app.request_id,
			Err("permission denied".into()),
		));
		assert_eq!(app.error.as_deref(), Some("permission denied"));
		assert_eq!(
			app.entries.len(),
			3,
			"failed refresh preserves the last listing"
		);
	}

	#[test]
	fn refresh_keeps_cached_entries_but_navigation_never_reuses_the_previous_directory() {
		let dir = TestDir::new();
		let file = dir.path().join("file");
		std::fs::write(&file, "x").unwrap();
		std::fs::create_dir(dir.path().join("child")).unwrap();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		finish_directory_load(&mut app);
		app.selected = Some(file.clone());
		let _ = app.update(Message::Refresh);
		assert!(app.loading);
		assert_eq!(app.entries.len(), 2);
		let _ = app.update(Message::Loaded(app.request_id, Err("unavailable".into())));
		assert_eq!(app.entries.len(), 2);
		assert_eq!(app.selected, Some(file));
		let _ = app.update(Message::GoTo(dir.path().join("child")));
		assert!(app.loading);
		assert!(app.entries.is_empty());
		let _ = app.update(Message::Loaded(app.request_id, Err("unavailable".into())));
		assert!(app.entries.is_empty());
		assert!(app.selected.is_none());
	}

	fn finish_directory_load(app: &mut FileManager) {
		let _ = app.update(Message::Loaded(
			app.request_id,
			entry::read_directory(&app.path),
		));
	}

	#[test]
	fn inspector_expansion_only_changes_on_explicit_requests() {
		let dir = TestDir::new();
		let first = dir.path().join("first");
		let second = dir.path().join("second");
		std::fs::write(&first, "x").unwrap();
		std::fs::write(&second, "y").unwrap();
		std::fs::create_dir(dir.path().join("child")).unwrap();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		finish_directory_load(&mut app);
		assert!(!app.show_info, "details start collapsed");
		for selected in [first.clone(), second.clone()] {
			let _ = app.update(Message::Clicked(selected));
			assert!(!app.show_info, "selection must not expand details");
		}
		let _ = app.update(Message::ToggleInfo);
		assert!(app.show_info);
		let _ = app.update(Message::Clicked(first.clone()));
		let _ = app.update(Message::ClearSelection);
		assert!(
			app.show_info,
			"clearing selection retains the expanded placeholder"
		);
		let _ = app.update(Message::Clicked(second));
		let _ = app.update(Message::View(ViewMode::List));
		assert!(app.show_info);
		let _ = app.update(Message::Refresh);
		finish_directory_load(&mut app);
		assert!(app.show_info);
		let _ = app.update(Message::GoTo(dir.path().join("child")));
		finish_directory_load(&mut app);
		assert!(app.show_info);
		let _ = app.update(Message::ToggleInfo);
		assert!(!app.show_info);
		let _ = app.update(Message::GoTo(dir.path().into()));
		finish_directory_load(&mut app);
		let target = context_menu::Target::capture(&first, context_menu::Context::File).unwrap();
		let _ = app.menu_dispatch(
			target,
			context_menu::Action::Command(context_menu::Command::Properties),
		);
		assert!(
			app.show_info,
			"Properties is also an explicit expansion request"
		);
	}

	#[test]
	fn history_restores_files_and_marks_the_remaining_redo_route_after_loading() {
		let dir = TestDir::new();
		let b = dir.path().join("b");
		let c = b.join("c");
		std::fs::create_dir_all(&c).unwrap();
		let file = c.join("notes.txt");
		let other = b.join("other.txt");
		std::fs::write(&file, "x").unwrap();
		std::fs::write(&other, "x").unwrap();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		finish_directory_load(&mut app);
		let _ = app.update(Message::OpenEntry(b.clone()));
		finish_directory_load(&mut app);
		let _ = app.update(Message::OpenEntry(c.clone()));
		finish_directory_load(&mut app);
		let _ = app.update(Message::Clicked(file.clone()));
		let _ = app.update(Message::GoBack);
		assert_eq!(app.pending_selection, Some(c.clone()));
		finish_directory_load(&mut app);
		assert_eq!(app.selected, Some(c.clone()));
		let _ = app.update(Message::Clicked(other));
		let _ = app.update(Message::GoBack);
		finish_directory_load(&mut app);
		assert_eq!(app.selected, Some(b.clone()));
		let _ = app.update(Message::GoForward);
		finish_directory_load(&mut app);
		assert_eq!(app.selected, Some(c.clone()));
		let _ = app.update(Message::GoForward);
		finish_directory_load(&mut app);
		assert_eq!(app.selected, Some(file.clone()));
		let _ = app.update(Message::GoUp);
		finish_directory_load(&mut app);
		assert_eq!(app.selected, Some(c));
		let _ = app.update(Message::GoBack);
		finish_directory_load(&mut app);
		assert_eq!(app.selected, Some(file));
	}

	#[test]
	fn pending_selection_survives_stale_scans_and_tags_loading_but_missing_files_clear_it() {
		let dir = TestDir::new();
		let child = dir.path().join("child");
		std::fs::create_dir(&child).unwrap();
		let file = child.join("selected.txt");
		std::fs::write(&file, "x").unwrap();
		let (mut app, _) = FileManager::new(Args {
			path: child.clone(),
		});
		finish_directory_load(&mut app);
		let _ = app.update(Message::Clicked(file.clone()));
		let _ = app.update(Message::GoUp);
		let stale_id = app.request_id;
		let _ = app.update(Message::GoBack);
		assert_eq!(app.pending_selection, Some(file.clone()));
		let _ = app.update(Message::Loaded(stale_id, Ok(Vec::new())));
		let _ = app.update(tags::Message::TagsLoaded(Ok(TagStore::open(
			dir.path().join("tags.json"),
		)
		.unwrap())));
		assert_eq!(app.pending_selection, Some(file.clone()));
		// Leaving before the scan finishes must save the pending, not an empty selection.
		let _ = app.update(Message::GoForward);
		let _ = app.update(Message::GoBack);
		finish_directory_load(&mut app);
		assert_eq!(app.selected, Some(file.clone()));
		let _ = app.update(Message::GoUp);
		finish_directory_load(&mut app);
		std::fs::remove_file(file).unwrap();
		let _ = app.update(Message::GoBack);
		finish_directory_load(&mut app);
		assert!(app.selected.is_none());
		assert!(app.pending_selection.is_none());
	}

	#[test]
	fn explicitly_clearing_selection_cancels_pending_navigation_highlight() {
		let dir = TestDir::new();
		let child = dir.path().join("child");
		std::fs::create_dir(&child).unwrap();
		let (mut app, _) = FileManager::new(Args { path: child });
		finish_directory_load(&mut app);
		let _ = app.update(Message::GoUp);
		assert!(app.pending_selection.is_some());
		let _ = app.update(Message::ClearSelection);
		finish_directory_load(&mut app);
		assert!(app.selected.is_none());
	}

	#[test]
	fn typing_refines_selection_without_filtering_and_dismissal_preserves_the_file() {
		let dir = TestDir::new();
		for name in ["foo", "bar", "faz", "foo bar"] {
			std::fs::write(dir.path().join(name), "x").unwrap();
		}
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		finish_directory_load(&mut app);
		let _ = app.update(Message::TypeAhead(typeahead::Edit::Append("f".into())));
		assert_eq!(app.selected, Some(dir.path().join("faz")));
		assert_eq!(app.visible_entries().len(), 4);
		assert!(app.search.is_empty());
		let _ = app.update(Message::TypeAhead(typeahead::Edit::Append("o".into())));
		assert_eq!(app.selected, Some(dir.path().join("foo")));
		let _ = app.update(Message::TypeAhead(typeahead::Edit::Append("o ".into())));
		assert_eq!(app.selected, Some(dir.path().join("foo bar")));
		let _ = app.update(Message::TypeAhead(typeahead::Edit::Append("z".into())));
		assert!(app.typeahead_match().is_none());
		assert_eq!(app.selected, Some(dir.path().join("foo bar")));
		let _ = app.update(Message::TypeAhead(typeahead::Edit::Backspace));
		assert!(app.typeahead_match().is_some());
		let escape = typeahead::tests::key(
			keyboard::Key::Named(keyboard::key::Named::Escape),
			None,
			keyboard::Modifiers::default(),
		);
		let _ = app.update(Message::Keyboard(escape));
		assert!(!app.typeahead.is_active());
		assert_eq!(app.selected, Some(dir.path().join("foo bar")));
		let _ = app.update(Message::TypeAhead(typeahead::Edit::Append("F".into())));
		assert_eq!(app.selected, Some(dir.path().join("faz")));
		let message = typeahead::dismiss_event(
			iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
			iced::event::Status::Captured,
			iced::window::Id::unique(),
		)
		.unwrap();
		let _ = app.update(message);
		assert!(!app.typeahead.is_active());
		assert_eq!(app.selected, Some(dir.path().join("faz")));
	}

	#[test]
	fn prefix_matching_respects_visible_entries_and_clears_when_navigating() {
		let dir = TestDir::new();
		for name in ["faz", "foo"] {
			std::fs::write(dir.path().join(name), "x").unwrap();
		}
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		finish_directory_load(&mut app);
		let _ = app.update(Message::Search("foo".into()));
		let _ = app.update(Message::TypeAhead(typeahead::Edit::Append("f".into())));
		assert_eq!(app.selected, Some(dir.path().join("foo")));
		assert_eq!(app.search, "foo");
		let _ = app.update(Message::GoUp);
		assert!(!app.typeahead.is_active());
	}
}
