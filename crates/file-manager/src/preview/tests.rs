#![allow(clippy::unwrap_used)]

use super::*;
use crate::{Args, browser, filesystem::entry, test_support::TestDir};

pub(super) fn test_dir() -> TestDir {
	let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
		.join("../../.opencode/fileberg-previews/tests");
	std::fs::create_dir_all(&root).unwrap();

	TestDir::new_in(&root)
}

fn app(dir: &TestDir) -> FileManager {
	let (mut app, _) = FileManager::new(Args {
		path: dir.path().into(),
	});
	let _ = app.update(browser::Message::Loaded(
		app.request_id,
		entry::read_directory(dir.path()),
	));

	app
}

#[test]
fn follows_membership_not_keyboard_cursor_and_cancels_on_clear() {
	let dir = test_dir();
	let first = dir.path().join("a.txt");
	let second = dir.path().join("b.txt");
	std::fs::write(&first, "first").unwrap();
	std::fs::write(&second, "second").unwrap();
	let mut app = app(&dir);

	let _ = app.update(browser::Message::Clicked(first.clone()));

	assert_eq!(app.preview.target.as_ref().unwrap().path, first);

	let cancel = app.preview.cancel.clone();
	app.selected = Some(second);
	let _ = app.reconcile_preview();

	assert_eq!(app.preview.target.as_ref().unwrap().path, first);
	assert!(!cancel.load(Ordering::Relaxed));

	app.selection.paths.clear();
	let _ = app.reconcile_preview();

	assert!(app.preview.target.is_none());
	assert!(cancel.load(Ordering::Relaxed));
}

#[test]
fn multiple_selection_navigation_narrow_windows_and_shutdown_hide_preview() {
	let dir = test_dir();
	let first = dir.path().join("a.txt");
	let second = dir.path().join("b.txt");
	std::fs::write(&first, "first").unwrap();
	std::fs::write(&second, "second").unwrap();
	let mut app = app(&dir);

	let _ = app.update(browser::Message::Clicked(first.clone()));
	let _ = app.update_preview(Message::Reveal(app.preview.generation));

	assert!(app.preview.visible(1100.0));

	app.selection.paths.insert(second.clone());
	let _ = app.reconcile_preview();

	assert!(!app.preview.visible(1100.0));

	app.selection.paths.remove(&second);
	app.host_size.width = 800.0;
	let _ = app.reconcile_preview();

	assert!(app.preview.target.is_none());

	app.host_size.width = 1100.0;
	let _ = app.reconcile_preview();

	assert_eq!(app.preview.target.as_ref().unwrap().path, first);

	app.page = browser::Page::History;
	let _ = app.reconcile_preview();

	assert!(app.preview.target.is_none());

	app.page = browser::Page::Files;
	let _ = app.reconcile_preview();
	app.closing = Some(app.host);
	let _ = app.reconcile_preview();

	assert!(app.preview.target.is_none());
}

#[test]
fn stale_results_cannot_replace_new_selection_or_survive_refresh() {
	let dir = test_dir();
	let path = dir.path().join("a.txt");
	std::fs::write(&path, "old").unwrap();
	let mut app = app(&dir);
	let _ = app.update(browser::Message::Clicked(path));
	let generation = app.preview.generation;
	let result = Box::new(load::Loaded {
		body: load::Body::Text("old".into()),
		pages: 3,
		size: 3,
		note: None,
	});

	app.request_id += 1;
	let _ = app.reconcile_preview();
	let _ = app.update_preview(Message::Loaded(generation, Ok(result.clone())));

	assert!(app.preview.content.is_none());
	assert!(app.preview.loading);

	let _ = app.update_preview(Message::Loaded(app.preview.generation, Ok(result)));

	assert_eq!(app.preview.pages, 3);
	assert!(!app.preview.loading);

	let current = app.preview.generation;
	let _ = app.update_preview(Message::Page(2));

	assert!(app.preview.generation > current);
	assert_eq!(app.preview.page, 2);
	assert!(app.preview.content.is_none());

	let current = app.preview.generation;
	let _ = app.update_preview(Message::Page(4));

	assert_eq!(app.preview.generation, current);
}

#[test]
fn pane_width_is_bounded_and_resize_rejects_nonfinite_values() {
	let mut state = State::default();
	state.width = 5000.0;

	assert_eq!(state.pane_width(1100.0), 550.0);

	state.width = -100.0;

	assert_eq!(state.pane_width(1100.0), MIN_PANE_WIDTH);

	let dir = test_dir();
	let mut app = app(&dir);
	let original = app.preview.width;

	let _ = app.update_preview(Message::Resize(f32::NAN));

	assert_eq!(app.preview.width, original);
}

#[test]
fn initial_pane_does_not_reflow_a_held_click_or_double_click_target() {
	use browser::drag::{Hit, Message as DragMessage};

	let dir = test_dir();
	let folder = dir.path().join("folder");
	std::fs::create_dir(&folder).unwrap();
	let mut app = app(&dir);
	app.transfers.loaded = true;

	let _ = app.update(DragMessage::Pressed(folder.clone(), iced::Point::ORIGIN));
	let generation = app.preview.generation;
	let _ = app.update_preview(Message::Reveal(generation));

	assert!(!app.preview.visible(1100.0));

	let _ = app.update(DragMessage::Released(iced::Point::ORIGIN, Hit::default()));
	let _ = app.update_preview(Message::Reveal(generation));

	assert!(!app.preview.visible(1100.0));

	let _ = app.update(DragMessage::Pressed(folder.clone(), iced::Point::ORIGIN));
	let _ = app.update(DragMessage::Released(iced::Point::ORIGIN, Hit::default()));

	assert_eq!(app.path, folder);

	let _ = app.update_preview(Message::Reveal(generation));

	assert!(app.preview.target.is_none());
}

#[test]
fn dismiss_keeps_selection_cancels_work_and_rejects_late_results() {
	let dir = test_dir();
	let path = dir.path().join("notes.txt");
	std::fs::write(&path, "notes").unwrap();
	let mut app = app(&dir);
	let _ = app.update(browser::Message::Clicked(path.clone()));
	let generation = app.preview.generation;
	let cancel = app.preview.cancel.clone();
	let _ = app.update_preview(Message::Reveal(generation));

	assert!(app.preview.visible(1100.0));

	let _ = app.update(Message::Dismiss);
	let _ = app.update(Message::Reveal(generation));
	let _ = app.update(Message::Loaded(generation, Err("obsolete".into())));

	assert_eq!(app.selected.as_ref(), Some(&path));
	assert!(app.selection.paths.contains(&path));
	assert!(cancel.load(Ordering::Relaxed));
	assert!(!app.preview.visible(1100.0));
	assert!(!app.preview.loading);
	assert!(app.preview.content.is_none());
	assert!(app.preview.error.is_none());
}

#[test]
fn dismissal_survives_refresh_and_resizing_but_resets_on_selection_change() {
	let dir = test_dir();
	let first = dir.path().join("a.txt");
	let second = dir.path().join("b.txt");
	std::fs::write(&first, "first").unwrap();
	std::fs::write(&second, "second").unwrap();
	let mut app = app(&dir);
	let _ = app.update(browser::Message::Clicked(first.clone()));
	let _ = app.update(Message::Dismiss);

	app.request_id += 1;
	let _ = app.reconcile_preview();
	app.host_size.width = 800.0;
	let _ = app.reconcile_preview();
	app.host_size.width = 1100.0;
	let _ = app.reconcile_preview();
	let _ = app.update(Message::Reveal(app.preview.generation));

	assert_eq!(app.preview.dismissed.as_ref(), Some(&first));
	assert!(!app.preview.visible(1100.0));
	assert!(!app.preview.loading);

	let _ = app.update(browser::Message::Clicked(second));
	let _ = app.update(Message::Reveal(app.preview.generation));

	assert!(app.preview.dismissed.is_none());
	assert!(app.preview.visible(1100.0));
	assert!(app.preview.loading);

	let _ = app.update(Message::Dismiss);
	app.selection.paths.clear();
	let _ = app.reconcile_preview();
	let _ = app.update(browser::Message::Clicked(first));
	let _ = app.update(Message::Reveal(app.preview.generation));

	assert!(app.preview.dismissed.is_none());
	assert!(app.preview.visible(1100.0));
}
