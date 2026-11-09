#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use super::*;
use crate::{
	Args, FileManager, browser,
	context_menu::{Action, Command, Context, Target},
	test_support::TestDir,
};
use futures::StreamExt;
use iced::{Task, window};

fn fixture() -> (TestDir, FileManager, Target) {
	let dir = TestDir::new();
	let path = dir.path().join("delete-me");
	std::fs::write(&path, "original").unwrap();
	let target = Target::capture(&path, Context::File).unwrap();
	let (mut app, _) = FileManager::new(Args {
		path: dir.path().into(),
	});
	app.entries = crate::filesystem::entry::read_directory(dir.path()).unwrap();
	app.loading = false;
	(dir, app, target)
}

fn request(app: &mut FileManager, target: Target) -> window::Id {
	let _ = Action::Command(Command::Delete).execute(app, target);
	app.dialogs.pending.as_ref().unwrap().id
}
fn opened(app: &mut FileManager, id: window::Id) {
	let _ = app.update_dialog(Message::Prepared(id, None));
	let _ = app.dialog_window_event(
		id,
		window::Event::Opened {
			position: None,
			size: iced::Size::new(480.0, 230.0),
			scale_factor: 1.0,
		},
	);
}
async fn outputs(app: &mut FileManager, task: Task<crate::Message>) {
	if let Some(mut stream) = iced_runtime::task::into_stream(task) {
		while let Some(action) = stream.next().await {
			if let iced_runtime::Action::Output(message) = action {
				let _ = app.update(message);
			}
		}
	}
}

#[test]
fn request_never_deletes_and_only_one_dialog_can_own_a_request() {
	let (_dir, mut app, target) = fixture();
	let id = request(&mut app, target.clone());
	assert!(target.available());
	assert!(app.dialogs.blocked());
	assert_eq!(
		app.update_dialog(Message::Respond(id, true)).units(),
		0,
		"response before Opened"
	);
	let _ = app.request_delete_confirmation(target.clone());
	assert_eq!(app.dialogs.pending.as_ref().unwrap().id, id);
	assert!(target.available());
}

#[test]
fn cancel_close_and_failed_creation_never_execute() {
	for ready in [false, true] {
		let (_dir, mut app, target) = fixture();
		let id = request(&mut app, target.clone());
		if ready {
			opened(&mut app, id);
			let _ = app.dialog_window_event(id, window::Event::CloseRequested);
			assert_eq!(
				app.dialogs.pending.as_ref().unwrap().phase,
				Phase::Closing { confirm: false }
			);
		}
		let _ = app.dialog_window_event(id, window::Event::Closed);
		assert!(!app.dialogs.blocked());
		assert!(target.available());
		assert_eq!(app.error.is_some(), !ready);
	}
}

#[test]
fn stale_responses_and_close_notifications_do_not_affect_new_dialog() {
	let (_dir, mut app, target) = fixture();
	let first = request(&mut app, target.clone());
	opened(&mut app, first);
	let _ = app.update_dialog(Message::Respond(first, false));
	let _ = app.dialog_window_event(first, window::Event::Closed);
	let second = request(&mut app, target.clone());
	opened(&mut app, second);
	let _ = app.update_dialog(Message::Respond(first, true));
	let _ = app.dialog_window_event(first, window::Event::Closed);
	assert_eq!(app.dialogs.pending.as_ref().unwrap().phase, Phase::Open);
	assert_eq!(app.dialogs.pending.as_ref().unwrap().id, second);
	assert!(target.available());
}

#[test]
fn parent_interactions_are_blocked_but_background_results_and_lifecycle_continue() {
	let (_dir, mut app, target) = fixture();
	let original_path = app.path.clone();
	let id = request(&mut app, target.clone());
	app.search = "retained".into();
	app.show_info = true;
	let _ = app.update(browser::Message::GoUp);
	let _ = app.update(browser::Message::ClearSelection);
	let _ = app.update(browser::Message::Search("ignored".into()));
	let _ = app.update(browser::Message::ToggleInfo);
	let _ = app.update(crate::Message::Input(
		app.host,
		iced::Event::Keyboard(crate::browser::typeahead::tests::key(
			iced::keyboard::Key::Character("x".into()),
			Some("x"),
			iced::keyboard::Modifiers::empty(),
		)),
		iced::event::Status::Ignored,
	));
	let _ = app.update(crate::context_menu::Message::MenuDispatch(
		target.clone(),
		Action::Command(Command::Delete),
	));
	let _ = app.update(browser::Message::OperationFinished(Err(
		"background error".into()
	)));
	assert_eq!(app.error.as_deref(), Some("background error"));
	assert_eq!(app.path, original_path);
	assert_eq!(app.search, "retained");
	assert!(app.show_info);
	assert_eq!(app.dialogs.pending.as_ref().unwrap().id, id);
	assert!(target.available());
}

#[tokio::test]
async fn confirm_uses_captured_target_exactly_once_and_preserves_other_files() {
	let (dir, mut app, target) = fixture();
	let other = dir.path().join("other");
	std::fs::write(&other, "keep").unwrap();
	let id = request(&mut app, target.clone());
	opened(&mut app, id);
	let _ = app.update_dialog(Message::Respond(id, true));
	assert_eq!(app.update_dialog(Message::Respond(id, true)).units(), 0);
	let task = app.dialog_window_event(id, window::Event::Closed);
	assert!(!app.dialogs.blocked());
	assert_eq!(app.dialogs.deleting, Some(id));
	outputs(&mut app, task).await;
	assert!(!target.path.exists());
	assert_eq!(std::fs::read_to_string(&other).unwrap(), "keep");
	assert!(app.dialogs.deleting.is_none());
	assert_eq!(
		app.dialog_window_event(id, window::Event::Closed).units(),
		0
	);
}

#[test]
fn replaced_member_invalidates_entire_confirmation_batch() {
	let (dir, mut app, mut target) = fixture();
	let other = dir.path().join("other");
	std::fs::write(&other, "old").unwrap();
	target
		.others
		.push(Target::capture(&other, Context::File).unwrap());
	let id = request(&mut app, target.clone());
	opened(&mut app, id);
	std::fs::rename(&other, dir.path().join("old-other")).unwrap();
	std::fs::write(&other, "replacement").unwrap();
	let _ = app.update_dialog(Message::Respond(id, true));
	assert_eq!(
		app.dialog_window_event(id, window::Event::Closed).units(),
		0
	);
	assert!(target.path.exists());
	assert_eq!(std::fs::read_to_string(&other).unwrap(), "replacement");
	assert_eq!(app.error.as_deref(), Some("Item no longer available"));
}

#[test]
fn parent_shutdown_discards_confirmed_but_not_executed_request() {
	let (_dir, mut app, target) = fixture();
	let id = request(&mut app, target.clone());
	opened(&mut app, id);
	let _ = app.update_dialog(Message::Respond(id, true));
	let _ = app.update(crate::Message::CloseRequested(app.host));
	let _ = app.dialog_window_event(id, window::Event::Closed);
	assert!(target.available());
	assert!(!app.dialogs.blocked());
}

#[test]
fn stale_completion_cannot_clear_another_deletion_or_shutdown_guard() {
	let (_dir, mut app, _) = fixture();
	let active = window::Id::unique();
	app.dialogs.deleting = Some(active);
	let _ = app.update_dialog(Message::Deleted(window::Id::unique(), Ok(())));
	assert_eq!(app.dialogs.deleting, Some(active));
	let _ = app.update(crate::Message::CloseRequested(app.host));
	assert_eq!(app.closing, Some(app.host));
	assert_eq!(app.finish_pending_close().units(), 0);
}

#[test]
fn escape_cancels_and_tab_changes_focus_without_default_confirmation() {
	let (_dir, mut app, target) = fixture();
	let id = request(&mut app, target.clone());
	opened(&mut app, id);
	assert!(!app.dialogs.pending.as_ref().unwrap().confirm_focused);
	let key = |named| {
		crate::browser::typeahead::tests::key(
			iced::keyboard::Key::Named(named),
			None,
			iced::keyboard::Modifiers::empty(),
		)
	};
	let _ = app.update_dialog(Message::Key(id, key(iced::keyboard::key::Named::Tab)));
	assert!(app.dialogs.pending.as_ref().unwrap().confirm_focused);
	let _ = app.update_dialog(Message::Key(id, key(iced::keyboard::key::Named::Escape)));
	assert_eq!(
		app.dialogs.pending.as_ref().unwrap().phase,
		Phase::Closing { confirm: false }
	);
	let _ = app.dialog_window_event(id, window::Event::Closed);
	assert!(target.available());
}
