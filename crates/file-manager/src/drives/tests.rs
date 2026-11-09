#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::{
	Args, FileManager,
	filesystem::volumes::{Operation, tests::fixture},
	test_support::TestDir,
};

fn app(paths: &[&str]) -> (TestDir, FileManager, Identity) {
	let dir = TestDir::new();
	let (mut app, _) = FileManager::new(Args {
		path: dir.path().into(),
	});
	let snapshot = fixture(paths);
	let identity = snapshot.volumes[0].identity.clone();
	app.drives.snapshot = Some(snapshot);
	app.drives.refreshing = None;

	(dir, app, identity)
}

#[test]
fn stale_refreshes_and_operation_completions_do_not_overwrite_newer_state() {
	let (_dir, mut app, _) = app(&[]);
	app.drives.refreshing = Some(5);
	app.drives.busy = Some(6);

	let _ = app.update_drives(Message::Loaded(4, Err("stale".into())));
	let _ = app.update_drives(Message::Finished(3, Err("stale".into())));

	assert!(app.drives.snapshot.is_some());
	assert!(app.error.is_none());
	assert_eq!(app.drives.busy, Some(6));
	assert_eq!(app.drives.refreshing, Some(5));
}

#[test]
fn busy_operations_block_duplicates_and_shutdown_waits() {
	let (_dir, mut app, identity) = app(&[]);
	app.drives.busy = Some(1);

	let task = app.update_drives(Message::Request(identity, Action::Mount));
	let refresh = app.update_drives(Message::Refresh);
	let _ = app.update(crate::Message::CloseRequested(app.host));

	assert_eq!(task.units(), 0);
	assert_eq!(refresh.units(), 0);
	assert!(!app.dialogs.blocked());
	assert_eq!(app.closing, Some(app.host));
	assert_eq!(app.finish_pending_close().units(), 0);
}

#[test]
fn mount_click_requests_confirmation_without_starting_an_operation() {
	let (_dir, mut app, identity) = app(&[]);

	let _ = app.update_drives(Message::Request(identity.clone(), Action::Mount));
	let _ = app.update_drives(Message::Request(identity, Action::Mount));

	assert!(app.dialogs.blocked());
	assert!(app.drives.busy.is_none());
}

#[test]
fn management_failure_preserves_navigation_and_releases_busy_state() {
	let (_dir, mut app, _) = app(&[]);
	let path = app.path.clone();
	app.drives.busy = Some(3);

	let _ = app.update_drives(Message::Finished(3, Err("Not authorized".into())));

	assert_eq!(app.path, path);
	assert_eq!(app.error.as_deref(), Some("Not authorized"));
	assert!(app.drives.busy.is_none());
	assert!(app.drives.refreshing.is_some());
}

#[test]
fn system_toggle_only_accepts_current_empty_sidebar_menu_and_is_session_local() {
	let (_dir, mut app, identity) = app(&[]);
	let id = window::Id::unique();
	app.drives.menu = Some(Menu {
		id,
		target: Some(identity),
	});

	let _ = app.update_drives(Message::ToggleSystem(id));
	assert!(!app.drives.show_system);

	app.drives.menu = Some(Menu { id, target: None });
	let _ = app.update_drives(Message::ToggleSystem(window::Id::unique()));
	assert!(!app.drives.show_system);

	let _ = app.update_drives(Message::ToggleSystem(id));
	assert!(app.drives.show_system);
}

#[test]
fn discovery_failure_retains_root_fallback_and_rejects_stale_menu_identity() {
	let (_dir, mut app, mut identity) = app(&[]);
	identity.uuid = "replacement".into();

	let _ = app.update_drives(Message::Request(identity, Action::Mount));
	assert!(!app.dialogs.blocked());
	assert!(app.error.is_some());

	app.drives.refreshing = Some(2);
	let _ = app.update_drives(Message::Loaded(2, Err("No service".into())));

	assert!(app.drives.snapshot.is_none());
	assert!(app.drives.problem.as_ref().unwrap().contains("No service"));
	let _ = app.drive_cards();
}

#[test]
fn cancelling_storage_confirmation_never_starts_io() {
	let (_dir, mut app, _) = app(&[]);
	let snapshot = app.drives.snapshot.as_ref().unwrap();
	let operation = Operation::prepare(snapshot, &snapshot.volumes[0], Action::Mount).unwrap();
	let _ = app.request_drive_confirmation(operation);
	// A failed/unopened dialog must cancel, just like a deletion confirmation.
	let id = app.dialogs.pending.as_ref().unwrap().id;

	let _ = app.dialog_window_event(id, window::Event::Closed);

	assert!(!app.dialogs.blocked());
	assert!(app.drives.busy.is_none());
}

#[test]
fn confirmed_storage_request_starts_once_only_after_dialog_closes() {
	let (_dir, mut app, identity) = app(&[]);
	let _ = app.update_drives(Message::Request(identity, Action::Mount));
	let id = app.dialogs.pending.as_ref().unwrap().id;
	let _ = app.update_dialog(crate::dialogs::Message::Prepared(id, None));
	let _ = app.dialog_window_event(
		id,
		window::Event::Opened {
			position: None,
			size: iced::Size::new(520.0, 340.0),
			scale_factor: 1.0,
		},
	);

	let _ = app.update_dialog(crate::dialogs::Message::Respond(id, true));
	assert!(app.drives.busy.is_none());
	assert_eq!(
		app.update_dialog(crate::dialogs::Message::Respond(id, true))
			.units(),
		0
	);

	// Deliberately do not poll this task: its executor is tested with Fake instead
	// of touching the host's actual system bus or storage.
	let task = app.dialog_window_event(id, window::Event::Closed);

	assert!(app.drives.busy.is_some());
	assert!(!app.dialogs.blocked());
	assert!(task.units() > 0);
	assert_eq!(
		app.dialog_window_event(id, window::Event::Closed).units(),
		0
	);
}

#[test]
fn go_up_can_escape_the_mount_root_and_history_restores_the_disk_context() {
	let (dir, mut app, _) = app(&[]);
	let mount = dir.path().join("Backup");
	let photos = mount.join("Photos");
	std::fs::create_dir_all(&photos).unwrap();
	app.drives.snapshot = Some(fixture(&[mount.to_str().unwrap()]));
	let _ = app.update_browser(crate::browser::Message::GoTo(photos));

	let _ = app.update_browser(crate::browser::Message::GoUp);

	assert_eq!(app.path, mount);
	assert_eq!(
		app.drives
			.snapshot
			.as_ref()
			.unwrap()
			.breadcrumb_root(&app.path)
			.map(|(_, m)| &m.path),
		Some(&mount)
	);

	let _ = app.update_browser(crate::browser::Message::GoUp);

	assert_eq!(app.path, dir.path());
	assert!(
		app.drives
			.snapshot
			.as_ref()
			.unwrap()
			.breadcrumb_root(&app.path)
			.is_none()
	);

	let _ = app.update_browser(crate::browser::Message::GoBack);

	assert_eq!(app.path, mount);
	assert!(
		app.drives
			.snapshot
			.as_ref()
			.unwrap()
			.breadcrumb_root(&app.path)
			.is_some()
	);
}
