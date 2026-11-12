#![allow(clippy::unwrap_used)]
use super::*;
use crate::{Args, browser, context_menu::Context, test_support::TestDir};
use std::{fs, os::unix::fs::PermissionsExt};

fn fixture() -> (TestDir, FileManager) {
	let root = TestDir::new();
	let trash = root.path().join("Trash");
	for path in [&trash, &trash.join("files"), &trash.join("info")] {
		fs::create_dir(path).unwrap();
		fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
	}
	fs::write(trash.join("files/item"), b"trash payload").unwrap();
	fs::write(
		trash.join("info/item.trashinfo"),
		format!(
			"[Trash Info]\nPath={}/original\nDeletionDate=2026-11-12T12:00:00\n",
			root.path().display()
		),
	)
	.unwrap();
	let (mut app, _) = FileManager::new(Args {
		path: root.path().into(),
	});
	app.transfers.loaded = true;
	app.trash_state.listing = Some(Arc::new(Listed {
		contents: crate::trash::fixture_listing(&trash),
		pending: Vec::new(),
		warnings: Vec::new(),
	}));
	(root, app)
}

#[test]
fn trash_history_and_files_are_mutually_exclusive_without_losing_directory() {
	let (_root, mut app) = fixture();
	let path = app.path.clone();
	let _ = app.update_trash(Message::Show);
	assert_eq!(app.page, Page::Trash);
	assert_eq!(app.path, path);
	let _ = app.update_transfers(transfers::Message::ShowHistory(true));
	assert_eq!(app.page, Page::History);
	let _ = app.update_trash(Message::Show);
	let _ = app.update_browser(browser::Message::GoTo(path.clone()));
	assert_eq!(app.page, Page::Files);
	assert_eq!(app.path, path);
}

#[test]
fn stale_listing_and_operation_completions_cannot_replace_new_state() {
	let (_root, mut app) = fixture();
	app.trash_state.generation = 4;
	app.trash_state.operation = 7;
	app.trash_state.busy = true;
	let original = app.trash_state.listing.clone().unwrap();
	let _ = app.update_trash(Message::Loaded(3, Err("stale listing error".into())));
	let _ = app.update_trash(Message::Finished(6, Err("stale operation error".into())));
	let _ = app.update_trash(Message::Captured(
		6,
		Intent::Restore,
		Err("stale capture".into()),
	));
	assert!(app.trash_state.busy);
	assert!(app.trash_state.error.is_none());
	assert!(Arc::ptr_eq(
		&original,
		app.trash_state.listing.as_ref().unwrap()
	));
}

#[test]
fn selections_must_belong_to_the_loaded_trash_listing() {
	let (_root, mut app) = fixture();
	let path = app.trash_state.listing.as_ref().unwrap().contents.entries[0]
		.stored_path
		.clone();
	let _ = app.update_trash(Message::Select("/unrelated/path".into(), true));
	assert!(app.trash_state.selected.is_empty());
	let _ = app.update_trash(Message::Select(path.clone(), true));
	assert!(app.trash_state.selected.contains(&path));
	let _ = app.update_trash(Message::SelectAll(false));
	assert!(app.trash_state.selected.is_empty());
}

#[test]
fn permanent_delete_captures_selection_then_requests_confirmation_without_starting_work() {
	let (_root, mut app) = fixture();
	app.page = Page::Trash;
	let listing = app.trash_state.listing.clone().unwrap();
	let path = listing.contents.entries[0].stored_path.clone();
	let _ = app.update_trash(Message::Select(path.clone(), true));
	let _ = app.update_trash(Message::Delete);
	assert!(app.trash_state.busy);
	assert!(!app.dialogs.blocked());
	let operation = app.trash_state.operation;
	let captured = Captured::capture(&listing.contents.entries[0], false).unwrap();
	let _ = app.update_trash(Message::Captured(
		operation,
		Intent::Delete { empty: false },
		Ok(vec![captured]),
	));
	assert!(app.dialogs.blocked());
	assert!(!app.trash_state.busy);
	assert!(!app.transfers.busy);
	assert_eq!(fs::read(path).unwrap(), b"trash payload");
}

#[test]
fn immediate_trashing_runs_off_thread_and_shares_mutation_exclusion() {
	let (root, mut app) = fixture();
	let path = root.path().join("ordinary-file");
	fs::write(&path, b"keep until task runs").unwrap();
	let target = Target::capture(&path, Context::File).unwrap();
	let _ = app.update_trash(Message::Trash(target));
	assert!(app.trash_state.busy);
	assert!(!app.dialogs.blocked());
	assert_eq!(fs::read(&path).unwrap(), b"keep until task runs");
	let generation = app.transfers.generation;
	let _ = app.update_transfers(transfers::Message::Start(transfers::Request {
		sources: vec![path],
		destination: root.path().into(),
		operation: transfers::Operation::Copy,
		durability: transfers::Durability::Buffered,
		acknowledge_volatile: false,
	}));
	assert!(!app.transfers.busy);
	assert_eq!(app.transfers.generation, generation);
}

#[test]
fn restore_capture_dispatches_the_transactional_engine_not_a_plain_file_move() {
	let (_root, mut app) = fixture();
	app.page = Page::Trash;
	let listing = app.trash_state.listing.clone().unwrap();
	let path = listing.contents.entries[0].stored_path.clone();
	let _ = app.update_trash(Message::Select(path.clone(), true));
	let _ = app.update_trash(Message::Restore);
	let operation = app.trash_state.operation;
	let captured = Captured::capture(&listing.contents.entries[0], true).unwrap();
	let _ = app.update_trash(Message::Captured(
		operation,
		Intent::Restore,
		Ok(vec![captured]),
	));
	assert!(!app.trash_state.busy);
	assert!(app.transfers.busy);
	assert_eq!(fs::read(path).unwrap(), b"trash payload");
}

#[test]
fn trash_keyboard_never_pastes_into_the_hidden_directory() {
	let (root, mut app) = fixture();
	let path = root.path().join("prepared");
	fs::write(&path, b"prepared").unwrap();
	let target = Target::capture(&path, Context::File).unwrap();
	let _ = app.update_transfers(transfers::Message::Prepare(
		target,
		transfers::Operation::Copy,
		None,
	));
	app.page = Page::Trash;
	let _ = app.update_browser(browser::Message::Keyboard(keyboard::Event::KeyPressed {
		key: keyboard::Key::Character("v".into()),
		modified_key: keyboard::Key::Character("v".into()),
		physical_key: keyboard::key::Physical::Code(keyboard::key::Code::KeyV),
		location: keyboard::Location::Standard,
		modifiers: keyboard::Modifiers::CTRL,
		text: None,
		repeat: false,
	}));
	assert!(!app.transfers.busy);
	assert!(app.transfers.pending.is_some());
}
