#![allow(clippy::unwrap_used)]

use super::{Dialog, DialogContent, Message, Notice};
use crate::{
	Args, FileManager, browser,
	context_menu::{Context, Target},
	test_support::TestDir,
	transfers::{Choice, Conflict, Durability, Event, Operation, Outcome, Phase, Status},
};
use iced::{keyboard, window};
use std::{
	fs,
	sync::{atomic::Ordering, mpsc},
	time::{Duration, Instant},
};

fn fixture() -> (TestDir, FileManager) {
	let dir = TestDir::new();
	// Dropping tasks keeps startup reads, D-Bus work, and config writes unpolled.
	let (mut app, _) = FileManager::new(Args {
		path: dir.path().into(),
	});
	app.tag_busy = false;
	app.loading = false;
	let _ = app.update(Message::Loaded(
		app.transfers.config_generation,
		Ok(Durability::Auto),
	));
	(dir, app)
}

fn conflict_dialog(app: &mut FileManager) -> (window::Id, mpsc::Receiver<super::Decision>) {
	let id = window::Id::unique();
	let (sender, receiver) = mpsc::channel();
	app.transfers.busy = true;
	app.transfers.generation = 7;
	app.transfers.phase = Some(Phase::Copying);
	app.transfers.response = Some(sender);
	app.transfers.dialog = Some(Dialog {
		id,
		open: true,
		content: DialogContent::Conflict(Conflict {
			source: app.path.join("source"),
			destination: app.path.join("destination"),
			directories: false,
			reason: Default::default(),
		}),
	});
	(id, receiver)
}

#[test]
fn stale_loaded_cannot_finish_newer_reload() {
	let (_dir, mut app) = fixture();
	let stale_generation = app.transfers.config_generation;
	app.transfers.loaded = false;
	let _ = app.update(Message::Load);
	let generation = app.transfers.config_generation;
	assert!(generation > stale_generation);

	let _ = app.update(Message::Loaded(stale_generation, Ok(Durability::Buffered)));
	let _ = app.update(Message::Loaded(stale_generation, Err("stale load".into())));

	assert_eq!(app.transfers.policy, Durability::Auto);
	assert!(!app.transfers.loaded);
	assert!(app.error.is_none());

	let _ = app.update(Message::Loaded(generation, Ok(Durability::Durable)));

	assert_eq!(app.transfers.policy, Durability::Durable);
	assert!(app.transfers.loaded);
	assert!(app.error.is_none());
}

#[test]
fn stale_loaded_cannot_overwrite_newer_global_policy() {
	let (_dir, mut app) = fixture();
	let stale_generation = app.transfers.config_generation;
	let _ = app.update(Message::Load);
	let generation = app.transfers.config_generation;
	let _ = app.update(Message::Loaded(generation, Ok(Durability::Durable)));
	let _ = app.update(Message::Loaded(stale_generation, Ok(Durability::Buffered)));
	let _ = app.update(Message::Loaded(stale_generation, Err("stale load".into())));

	assert_eq!(app.transfers.policy, Durability::Durable);
	assert!(app.transfers.loaded);
	assert!(app.error.is_none());
}

fn source(dir: &TestDir, name: &str) -> Target {
	let path = dir.path().join(name);
	fs::write(&path, b"prepared source").unwrap();
	Target::capture(&path, Context::File).unwrap()
}

fn outcome(status: Status) -> Outcome {
	Outcome {
		id: "simulated-job".into(),
		status,
		problems: Vec::new(),
	}
}

#[test]
fn preparing_copy_or_move_preserves_files_and_does_not_start_job() {
	for operation in [Operation::Copy, Operation::Move] {
		let (dir, mut app) = fixture();
		let target = source(&dir, "source");
		let path = target.path.clone();
		let generation = app.transfers.generation;

		let _ = app.update(Message::Prepare(target, operation, None));

		assert_eq!(fs::read(&path).unwrap(), b"prepared source");
		assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
		assert!(!app.transfers.busy);
		assert_eq!(app.transfers.generation, generation);
		assert!(app.transfers.phase.is_none());
		assert!(app.transfers.started.is_none());
		assert!(app.transfers.running_pending.is_none());
		assert!(app.transfers.can_paste());
		assert_eq!(app.transfers.prepared_operation(&path), Some(operation));
		assert_eq!(
			app.transfers.prepared_operation(&dir.path().join("other")),
			None
		);
	}
}

#[test]
fn paste_request_uses_current_global_policy_unless_explicitly_overridden() {
	let (dir, mut app) = fixture();
	let target = source(&dir, "source");
	let destination = dir.path().join("destination");
	let _ = app.update(Message::Prepare(target.clone(), Operation::Copy, None));

	let ordinary = app.transfers.pending.as_ref().unwrap();
	assert_eq!(
		ordinary
			.request(destination.clone(), Durability::Durable)
			.durability,
		Durability::Durable
	);
	assert_eq!(
		ordinary
			.request(destination.clone(), Durability::Buffered)
			.durability,
		Durability::Buffered
	);

	let _ = app.update(Message::Prepare(
		target.clone(),
		Operation::Move,
		Some(Durability::Auto),
	));
	let advanced = app.transfers.pending.as_ref().unwrap();
	let request = advanced.request(destination.clone(), Durability::Durable);
	assert_eq!(request.durability, Durability::Auto);
	assert_eq!(request.operation, Operation::Move);
	assert_eq!(request.sources, target.paths());
	assert_eq!(request.destination, destination);
}

#[test]
fn new_selection_capture_disables_pasting_the_previous_prepared_files() {
	let (dir, mut app) = fixture();
	let old = source(&dir, "old");
	let new = source(&dir, "new");
	let _ = app.update(Message::Prepare(old, Operation::Move, None));
	app.selection.paths.insert(new.path.clone());
	app.entries = crate::filesystem::entry::read_directory(dir.path()).unwrap();

	let _ = app.update(Message::PrepareSelection(Operation::Copy));
	assert!(!app.transfers.can_paste());
	let _ = app.update(Message::Paste(dir.path().join("destination")));
	assert!(!app.transfers.busy);

	let generation = app.transfers.pending_generation;
	let _ = app.update(Message::CapturedPrepare(
		generation,
		Operation::Copy,
		Ok(new.clone()),
	));
	assert!(app.transfers.can_paste());
	assert_eq!(
		app.transfers.pending.as_ref().unwrap().target.path,
		new.path
	);
}

#[test]
fn launch_refuses_a_destination_replaced_while_waiting_for_the_store() {
	let (dir, _) = fixture();
	let destination = dir.path().join("destination");
	fs::create_dir(&destination).unwrap();
	let clicked = Target::capture(&destination, Context::Folder).unwrap();
	let resolved = Target::capture(&destination, Context::Folder).unwrap();
	let source = source(&dir, "source");
	let moved = dir.path().join("old-destination");
	fs::rename(&destination, &moved).unwrap();
	fs::create_dir(&destination).unwrap();

	let result = super::validate_launch_targets(Some(&source), Some(&clicked), Some(&resolved));

	assert!(result.unwrap_err().contains("destination changed"));
	assert_eq!(fs::read(&source.path).unwrap(), b"prepared source");
	assert!(fs::read_dir(&destination).unwrap().next().is_none());
	assert!(fs::read_dir(&moved).unwrap().next().is_none());
}

#[test]
fn navigation_preserves_prepared_sources_and_operation() {
	let (dir, mut app) = fixture();
	let mut target = source(&dir, "first");
	target.others.push(source(&dir, "second"));
	let paths = target.paths();
	let destination = dir.path().join("destination");
	fs::create_dir(&destination).unwrap();
	let _ = app.update(Message::Prepare(target, Operation::Move, None));
	let pending_generation = app.transfers.pending.as_ref().unwrap().generation;

	let _ = app.update(browser::Message::GoTo(destination.clone()));

	assert_eq!(app.path, destination);
	let pending = app.transfers.pending.as_ref().unwrap();
	assert_eq!(pending.target.paths(), paths);
	assert_eq!(pending.generation, pending_generation);
	for path in &paths {
		assert_eq!(
			app.transfers.prepared_operation(path),
			Some(Operation::Move)
		);
		assert_eq!(fs::read(path).unwrap(), b"prepared source");
	}
	assert!(app.transfers.can_paste());
	assert!(!app.transfers.busy);
}

#[test]
fn prepared_policy_override_survives_global_reload_and_paste() {
	for policy in [None, Some(Durability::Buffered)] {
		let (dir, mut app) = fixture();
		let target = source(&dir, "source");
		let path = target.path.clone();
		let _ = app.update(Message::Prepare(target, Operation::Copy, policy));
		let _ = app.update(Message::Load);
		let _ = app.update(Message::Loaded(
			app.transfers.config_generation,
			Ok(Durability::Durable),
		));

		assert_eq!(app.transfers.policy, Durability::Durable);
		assert_eq!(app.transfers.pending.as_ref().unwrap().policy, policy);
		assert!(app.transfers.can_paste());

		// Construct the worker task without polling it or opening its real store.
		let task = app.update(Message::Paste(dir.path().into()));

		assert!(task.units() > 0);
		assert!(app.transfers.busy);
		assert!(!app.transfers.can_paste());
		assert_eq!(app.transfers.phase, Some(Phase::Preparing));
		assert_eq!(app.transfers.pending.as_ref().unwrap().policy, policy);
		assert_eq!(
			app.transfers.running_pending,
			Some(app.transfers.pending.as_ref().unwrap().generation)
		);
		assert_eq!(fs::read(path).unwrap(), b"prepared source");
	}
}

#[test]
fn paste_without_pending_or_while_busy_is_ignored() {
	let (dir, mut app) = fixture();
	let generation = app.transfers.generation;
	let task = app.update(Message::Paste(dir.path().into()));

	assert_eq!(task.units(), 0);
	assert!(!app.transfers.busy);
	assert!(!app.transfers.can_paste());
	assert_eq!(app.transfers.generation, generation);

	let target = source(&dir, "source");
	let _ = app.update(Message::Prepare(target, Operation::Move, None));
	app.transfers.loaded = false;
	let task = app.update(Message::Paste(dir.path().into()));
	assert_eq!(task.units(), 0);
	assert!(!app.transfers.can_paste());
	assert!(!app.transfers.busy);

	app.transfers.loaded = true;
	app.transfers.busy = true;
	app.transfers.phase = Some(Phase::Flushing);
	let pending_generation = app.transfers.pending.as_ref().unwrap().generation;
	let task = app.update(Message::Paste(dir.path().into()));

	assert_eq!(task.units(), 0);
	assert_eq!(app.transfers.generation, generation);
	assert_eq!(app.transfers.phase, Some(Phase::Flushing));
	assert_eq!(
		app.transfers.pending.as_ref().unwrap().generation,
		pending_generation
	);
	assert!(app.transfers.running_pending.is_none());
}

#[test]
fn completion_clears_only_successful_matching_prepared_move() {
	for operation in [Operation::Copy, Operation::Move] {
		for status in [Status::Complete, Status::RolledBack, Status::NeedsRecovery] {
			let (dir, mut app) = fixture();
			let target = source(&dir, "source");
			let _ = app.update(Message::Prepare(target, operation, None));
			let _ = app.update(Message::Paste(dir.path().into()));

			let _ = app.update(Message::Notice(
				app.transfers.generation,
				Notice::Done(Ok(outcome(status))),
			));

			assert!(!app.transfers.busy);
			assert!(app.transfers.running_pending.is_none());
			assert_eq!(
				app.transfers.pending.is_none(),
				operation == Operation::Move && status == Status::Complete
			);
			assert_eq!(app.transfers.latest.as_ref().unwrap().status, status);
		}
	}
}

#[test]
fn failed_or_panicked_move_keeps_prepared_sources() {
	for panic in [false, true] {
		let (dir, mut app) = fixture();
		let target = source(&dir, "source");
		let path = target.path.clone();
		let _ = app.update(Message::Prepare(target, Operation::Move, None));
		let _ = app.update(Message::Paste(dir.path().into()));
		let generation = app.transfers.generation;

		let _ = app.update(if panic {
			Message::WorkerExited(generation, Err("simulated panic".into()))
		} else {
			Message::Notice(generation, Notice::Done(Err("simulated failure".into())))
		});

		assert!(!app.transfers.busy);
		assert!(app.transfers.can_paste());
		assert_eq!(
			app.transfers.prepared_operation(&path),
			Some(Operation::Move)
		);
		assert!(app.transfers.running_pending.is_none());
	}
}

#[test]
fn successful_old_job_cannot_clear_newer_prepared_move() {
	let (dir, mut app) = fixture();
	let first = source(&dir, "first");
	let second = source(&dir, "second");
	let second_path = second.path.clone();
	let _ = app.update(Message::Prepare(first, Operation::Move, None));
	let old_pending = app.transfers.pending.as_ref().unwrap().generation;
	let _ = app.update(Message::Prepare(second, Operation::Move, None));
	app.transfers.busy = true;
	app.transfers.generation = 10;
	app.transfers.running_pending = Some(old_pending);

	let _ = app.update(Message::Notice(
		9,
		Notice::Done(Ok(outcome(Status::Complete))),
	));
	assert!(app.transfers.busy);
	assert_eq!(app.transfers.running_pending, Some(old_pending));
	assert_eq!(
		app.transfers.prepared_operation(&second_path),
		Some(Operation::Move)
	);

	let _ = app.update(Message::Notice(
		10,
		Notice::Done(Ok(outcome(Status::Complete))),
	));

	assert!(!app.transfers.busy);
	assert_eq!(
		app.transfers.prepared_operation(&second_path),
		Some(Operation::Move)
	);
}

#[test]
fn cancel_prepared_invalidates_late_selection_capture() {
	let (dir, mut app) = fixture();
	let target = source(&dir, "source");
	app.selection.only(target.path.clone());
	let _ = app.update(Message::Prepare(target.clone(), Operation::Move, None));
	let _ = app.update(Message::PrepareSelection(Operation::Copy));
	let captured_generation = app.transfers.pending_generation;

	let _ = app.update(Message::CancelPrepared);
	assert!(app.transfers.pending.is_none());
	assert!(!app.transfers.can_paste());
	assert!(app.transfers.pending_generation > captured_generation);

	let _ = app.update(Message::CapturedPrepare(
		captured_generation,
		Operation::Copy,
		Ok(target),
	));
	let _ = app.update(Message::CapturedPrepare(
		captured_generation,
		Operation::Copy,
		Err("late capture".into()),
	));

	assert!(app.transfers.pending.is_none());
	assert!(app.error.is_none());
}

#[test]
fn stale_selection_capture_cannot_replace_newer_prepared_target() {
	let (dir, mut app) = fixture();
	let first = source(&dir, "first");
	let second = source(&dir, "second");
	let second_path = second.path.clone();
	app.selection.only(first.path.clone());
	let _ = app.update(Message::PrepareSelection(Operation::Copy));
	let captured_generation = app.transfers.pending_generation;
	let _ = app.update(Message::Prepare(
		second,
		Operation::Move,
		Some(Durability::Durable),
	));

	let _ = app.update(Message::CapturedPrepare(
		captured_generation,
		Operation::Copy,
		Ok(first),
	));
	let _ = app.update(Message::CapturedPrepare(
		captured_generation,
		Operation::Copy,
		Err("stale capture".into()),
	));

	assert_eq!(
		app.transfers.prepared_operation(&second_path),
		Some(Operation::Move)
	);
	assert_eq!(
		app.transfers.pending.as_ref().unwrap().policy,
		Some(Durability::Durable)
	);
	assert!(app.error.is_none());
}

#[test]
fn current_selection_capture_prepares_without_starting_worker() {
	let (dir, mut app) = fixture();
	let target = source(&dir, "source");
	let path = target.path.clone();
	app.selection.only(path.clone());
	let task = app.update(Message::PrepareSelection(Operation::Copy));
	assert!(task.units() > 0);
	let generation = app.transfers.pending_generation;

	let _ = app.update(Message::CapturedPrepare(
		generation,
		Operation::Copy,
		Ok(target),
	));

	assert_eq!(
		app.transfers.prepared_operation(&path),
		Some(Operation::Copy)
	);
	assert_eq!(app.transfers.pending.as_ref().unwrap().policy, None);
	assert!(app.transfers.can_paste());
	assert!(!app.transfers.busy);
	assert_eq!(fs::read(path).unwrap(), b"prepared source");
}

#[test]
fn show_history_switches_view_and_folder_navigation_exits_history() {
	let (dir, mut app) = fixture();
	let folder = dir.path().join("folder");
	fs::create_dir(&folder).unwrap();

	let _ = app.update(Message::ShowHistory(true));
	assert!(app.transfers.show_history);
	let _ = app.update(Message::ShowHistory(false));
	assert!(!app.transfers.show_history);
	let _ = app.update(Message::ShowHistory(true));
	let _ = app.update(browser::Message::GoTo(folder.clone()));

	assert_eq!(app.path, folder);
	assert!(!app.transfers.show_history);
}

#[test]
fn eta_requires_copying_known_total_nonzero_progress_and_elapsed_time() {
	let (_dir, mut app) = fixture();
	app.transfers.bytes = 200;
	app.transfers.total_bytes = Some(4000);
	let now = Instant::now();
	for tick in 0..=8 {
		app.transfers
			.copy_rate
			.observe(now + Duration::from_millis(tick * 250), tick * 25);
	}

	for phase in [
		Phase::Preparing,
		Phase::Verifying,
		Phase::Flushing,
		Phase::RollingBack,
	] {
		app.transfers.phase = Some(phase);
		assert_eq!(app.transfers.eta(), None);
	}
	app.transfers.phase = Some(Phase::Copying);
	assert_eq!(app.transfers.eta(), Some(Duration::from_secs(38)));

	app.transfers.total_bytes = None;
	assert_eq!(app.transfers.eta(), None);
	app.transfers.total_bytes = Some(4000);
	app.transfers.bytes = 0;
	assert_eq!(app.transfers.eta(), None);
	app.transfers.bytes = 4000;
	assert_eq!(app.transfers.eta(), None);
}

#[test]
fn eta_excludes_long_preparation_and_resets_after_a_non_copy_phase() {
	let (_dir, mut app) = fixture();
	app.transfers.started = Some(Instant::now() - Duration::from_secs(60));
	let generation = app.transfers.generation;
	let _ = app.update(Message::Notice(
		generation,
		Notice::Event(Event::Progress {
			phase: Phase::Copying,
			bytes: 64 * 1024,
			total: Some(1024 * 1024),
		}),
	));
	assert_eq!(app.transfers.eta(), None);

	let _ = app.update(Message::Notice(
		generation,
		Notice::Event(Event::Progress {
			phase: Phase::Preparing,
			bytes: 64 * 1024,
			total: Some(1024 * 1024),
		}),
	));
	assert_eq!(app.transfers.eta(), None);
}

#[test]
fn progress_reveal_is_delayed_and_ignores_finished_or_stale_jobs() {
	let (dir, mut app) = fixture();
	let target = source(&dir, "source");
	let _ = app.update(Message::Prepare(target, Operation::Move, None));
	let _ = app.update(Message::Paste(dir.path().into()));
	let generation = app.transfers.generation;
	assert!(!app.transfers.progress_visible);
	let _ = app.update(Message::ShowProgress(generation - 1));
	assert!(!app.transfers.progress_visible);
	let _ = app.update(Message::ShowProgress(generation));
	assert!(app.transfers.progress_visible);
	app.transfers.progress_visible = false;
	app.transfers.busy = false;
	let _ = app.update(Message::ShowProgress(generation));
	assert!(!app.transfers.progress_visible);
}

#[test]
fn stale_job_notices_cannot_clear_or_change_newer_busy_state() {
	let (_dir, mut app) = fixture();
	let (id, _answers) = conflict_dialog(&mut app);
	app.transfers.bytes = 42;
	app.transfers.total_bytes = Some(100);
	let stale_generation = app.transfers.generation - 1;

	for notice in [
		Notice::Event(Event::Progress {
			phase: Phase::RollingBack,
			bytes: 999,
			total: Some(1000),
		}),
		Notice::Event(Event::Warning("stale warning".into())),
		Notice::Done(Err("stale failure".into())),
	] {
		let _ = app.update(Message::Notice(stale_generation, notice));
	}
	let _ = app.update(Message::WorkerExited(
		stale_generation,
		Err("stale panic".into()),
	));

	assert!(app.transfers.busy);
	assert_eq!(app.transfers.phase, Some(Phase::Copying));
	assert_eq!(app.transfers.bytes, 42);
	assert_eq!(app.transfers.total_bytes, Some(100));
	assert!(app.transfers.response.is_some());
	assert!(app.transfers.owns(id));
	assert!(app.error.is_none());
}

#[test]
fn conflict_blocks_parent_navigation_but_allows_worker_progress() {
	let (_dir, mut app) = fixture();
	let (id, _answers) = conflict_dialog(&mut app);
	let original_path = app.path.clone();
	let request_id = app.request_id;

	let navigation = app.update(browser::Message::GoTo(app.path.join("other")));
	let _ = app.update(browser::Message::GoUp);
	let _ = app.update(browser::Message::Search("blocked".into()));
	let _ = app.update(Message::Notice(
		app.transfers.generation,
		Notice::Event(Event::Progress {
			phase: Phase::Flushing,
			bytes: 8192,
			total: Some(8192),
		}),
	));

	assert_eq!(navigation.units(), 0);
	assert_eq!(app.path, original_path);
	assert_eq!(app.request_id, request_id);
	assert!(app.search.is_empty());
	assert_eq!(app.transfers.phase, Some(Phase::Flushing));
	assert_eq!(app.transfers.bytes, 8192);
	assert_eq!(app.transfers.total_bytes, Some(8192));
	assert!(app.transfers.busy);
	assert!(app.transfers.owns(id));
}

#[test]
fn escape_and_window_close_cancel_conflict_worker_and_dismiss_modal() {
	for dismissal in 0..3 {
		let (_dir, mut app) = fixture();
		let (id, answers) = conflict_dialog(&mut app);
		let message = match dismissal {
			0 => crate::Message::Input(
				id,
				iced::Event::Keyboard(crate::browser::typeahead::tests::key(
					keyboard::Key::Named(keyboard::key::Named::Escape),
					None,
					keyboard::Modifiers::empty(),
				)),
				iced::event::Status::Ignored,
			),
			1 => crate::Message::WindowEvent(id, window::Event::CloseRequested),
			_ => crate::Message::WindowEvent(id, window::Event::Closed),
		};

		let _ = app.update(message);

		assert!(app.transfers.cancelled.load(Ordering::Acquire));
		assert!(app.transfers.dialog.is_none());
		assert!(!app.transfers.blocked());
		assert!(
			app.transfers.busy,
			"worker completion still owns busy state"
		);
		let decision = answers.try_recv().unwrap();
		assert_eq!(decision.choice, Choice::Cancel);
		assert!(!decision.remaining);
	}
}

#[test]
fn host_close_waits_for_busy_transfer_before_emitting_exit() {
	for event in [window::Event::CloseRequested, window::Event::Closed] {
		let (_dir, mut app) = fixture();
		app.transfers.busy = true;
		let host = app.host;

		let closing = app.update(crate::Message::WindowEvent(host, event));

		assert_eq!(
			closing.units(),
			0,
			"busy worker must prevent immediate exit"
		);
		assert_eq!(app.closing, Some(host));
		assert!(app.transfers.busy);
		assert!(app.transfers.cancelled.load(Ordering::Acquire));

		let completed = app.update(Message::Notice(
			app.transfers.generation,
			Notice::Done(Err("cancelled".into())),
		));

		assert!(!app.transfers.busy);
		assert!(app.closing.is_none());
		assert!(
			completed.units() > 0,
			"completion must release pending exit"
		);
	}
}

#[test]
fn worker_panic_reports_retained_journal_and_releases_busy_state() {
	let (_dir, mut app) = fixture();
	let (_id, _answers) = conflict_dialog(&mut app);

	let _ = app.update(Message::WorkerExited(
		app.transfers.generation,
		Err("worker panicked".into()),
	));

	assert!(!app.transfers.busy);
	assert!(app.transfers.response.is_none());
	assert!(app.transfers.dialog.is_none());
	assert_eq!(
		app.error.as_deref(),
		Some("Transfer worker stopped; recovery journal retained: worker panicked")
	);
}
