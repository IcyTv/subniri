#![allow(clippy::unwrap_used)]
use super::*;
use crate::test_support::TestDir;
use std::{
	fs,
	path::Path,
	sync::atomic::{AtomicBool, Ordering},
};

fn request(source: &Path, destination: &Path, operation: Operation) -> Request {
	Request {
		sources: vec![source.into()],
		destination: destination.into(),
		operation,
		durability: Durability::Buffered,
		acknowledge_volatile: true,
	}
}

#[test]
fn copy_and_persistent_inverse_undo_need_no_backup_data() {
	let root = TestDir::new();
	let source = root.path().join("notes");
	let destination = root.path().join("destination");
	fs::write(&source, "important").unwrap();
	fs::create_dir(&destination).unwrap();
	let state = root.path().join("state");
	let store = Store::open(state.clone()).unwrap();

	let result = execute(
		&store,
		request(&source, &destination, Operation::Copy),
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|_| {},
	)
	.unwrap();
	assert_eq!(result.status, Status::Complete, "{:?}", result.problems);
	assert_eq!(fs::read(destination.join("notes")).unwrap(), b"important");
	let journal = store.load(&result.id).unwrap();
	assert!(
		journal
			.changes
			.iter()
			.all(|change| change.replaced.is_none())
	);
	drop(store);

	let store = Store::open(state).unwrap();
	let result = undo(&store, &result.id, |_| {}).unwrap();
	assert_eq!(result.status, Status::RolledBack);
	assert!(!destination.join("notes").exists());
	assert_eq!(fs::read(&source).unwrap(), b"important");
}

#[test]
fn consented_central_recovery_restores_replaced_data_across_filesystems() {
	let root = TestDir::new();
	let Some(other) = separate_filesystem(root.path()) else {
		return;
	};
	let source = root.path().join("file");
	let destination = other.path().join("destination");
	fs::write(&source, "new contents").unwrap();
	fs::create_dir(&destination).unwrap();
	fs::write(destination.join("file"), "old contents").unwrap();
	let store =
		Store::open_with_recovery(root.path().join("state"), root.path().join("recovery")).unwrap();

	let result = execute(
		&store,
		request(&source, &destination, Operation::Copy),
		&AtomicBool::new(false),
		|_| Choice::ReplaceCentral,
		|_| {},
	)
	.unwrap();
	assert_eq!(result.status, Status::Complete, "{:?}", result.problems);
	let record = store.load(&result.id).unwrap();
	assert!(record.changes[0].central.as_ref().unwrap().complete());
	assert!(!record.changes[0].backup.path().exists());
	assert_eq!(fs::read(destination.join("file")).unwrap(), b"new contents");

	let undone = undo(&store, &result.id, |_| {}).unwrap();
	assert_eq!(undone.status, Status::RolledBack, "{:?}", undone.problems);
	assert_eq!(fs::read(destination.join("file")).unwrap(), b"old contents");
	assert_eq!(fs::read(&source).unwrap(), b"new contents");

	let retried = undo(&store, &result.id, |_| {}).unwrap();
	assert_eq!(retried.status, Status::RolledBack);
}

#[test]
fn cancellation_with_central_recovery_rolls_back_the_published_replacement() {
	let root = TestDir::new();
	let source = root.path().join("file");
	let destination = root.path().join("destination");
	fs::write(&source, "new").unwrap();
	fs::create_dir(&destination).unwrap();
	fs::write(destination.join("file"), "old").unwrap();
	let store =
		Store::open_with_recovery(root.path().join("state"), root.path().join("recovery")).unwrap();
	let cancelled = AtomicBool::new(false);

	let result = execute(
		&store,
		request(&source, &destination, Operation::Move),
		&cancelled,
		|_| Choice::ReplaceCentral,
		|event| {
			if matches!(
				event,
				Event::Progress {
					phase: Phase::Committing,
					..
				}
			) {
				cancelled.store(true, Ordering::Release);
			}
		},
	)
	.unwrap();

	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert_eq!(fs::read(&source).unwrap(), b"new");
	assert_eq!(fs::read(destination.join("file")).unwrap(), b"old");
}

#[test]
fn failed_replacement_flush_returns_staged_move_before_recognizing_old_destination() {
	let root = TestDir::new();
	let source = root.path().join("file");
	let destination = root.path().join("destination");
	fs::write(&source, "new").unwrap();
	fs::create_dir(&destination).unwrap();
	fs::write(destination.join("file"), "old").unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut input = request(&source, &destination, Operation::Move);
	input.durability = Durability::Durable;

	let result = worker::execute_with_flush(
		&store,
		input,
		&AtomicBool::new(false),
		|_| Choice::Replace,
		|_| {},
		|_, _| Err(std::io::Error::other("injected flush failure")),
	)
	.unwrap();

	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert_eq!(fs::read(&source).unwrap(), b"new");
	assert_eq!(fs::read(destination.join("file")).unwrap(), b"old");
}

#[test]
fn oversized_recovery_requires_explicit_job_scoped_durable_consent() {
	let root = TestDir::new();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut journal = super::model::Journal {
		version: 1,
		id: super::journal::new_id().unwrap(),
		created: super::journal::now().unwrap(),
		status: Status::Running,
		durability: Durability::Buffered,
		approved_recovery_bytes: 0,
		changes: Vec::new(),
		problems: Vec::new(),
	};
	store.save(&journal).unwrap();
	let bytes = 6 * 1024 * 1024 * 1024;
	let source = root.path().join("source");
	let destination = root.path().join("destination");

	for rejected in [
		Choice::Cancel,
		Choice::Replace,
		Choice::ReplaceCentral,
		Choice::Skip,
	] {
		let result = worker::reserve_recovery(
			&store,
			&mut journal,
			&source,
			&destination,
			bytes,
			&mut |conflict| {
				assert_eq!(
					conflict.reason,
					super::model::ConflictReason::RecoveryBudget { bytes }
				);
				rejected
			},
		);
		assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
		assert_eq!(store.load(&journal.id).unwrap().approved_recovery_bytes, 0);
	}

	worker::reserve_recovery(
		&store,
		&mut journal,
		&source,
		&destination,
		bytes,
		&mut |_| Choice::ApproveRecovery,
	)
	.unwrap();
	assert_eq!(
		store.load(&journal.id).unwrap().approved_recovery_bytes,
		bytes
	);
	assert!(!source.exists());
	assert!(!destination.exists());
}

#[test]
fn volatile_move_consent_remains_available_without_an_advanced_dialog() {
	let root = TestDir::new();
	let Some(other) = separate_filesystem(root.path()) else {
		return;
	};
	let source = root.path().join("source");
	fs::write(&source, "keep until consent").unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut input = request(&source, other.path(), Operation::Move);
	input.acknowledge_volatile = false;
	let mut asked = false;

	let cancelled = execute(
		&store,
		input.clone(),
		&AtomicBool::new(false),
		|conflict| {
			assert_eq!(conflict.reason, super::model::ConflictReason::VolatileMove);
			asked = true;
			Choice::Cancel
		},
		|_| {},
	);
	assert_eq!(
		cancelled.unwrap_err().kind(),
		std::io::ErrorKind::Interrupted
	);
	assert!(asked);
	assert_eq!(fs::read(&source).unwrap(), b"keep until consent");

	let copied = execute(
		&store,
		input,
		&AtomicBool::new(false),
		|conflict| {
			assert_eq!(conflict.reason, super::model::ConflictReason::VolatileMove);
			Choice::ApproveRecovery
		},
		|_| {},
	)
	.unwrap();
	assert_eq!(copied.status, Status::Complete);
	assert!(!source.exists());
	assert_eq!(
		undo(&store, &copied.id, |_| {}).unwrap().status,
		Status::RolledBack
	);
	assert_eq!(fs::read(&source).unwrap(), b"keep until consent");
}

#[test]
fn replacement_retains_old_contents_and_undo_restores_them() {
	let root = TestDir::new();
	let source = root.path().join("notes");
	let destination = root.path().join("destination");
	fs::write(&source, "new").unwrap();
	fs::create_dir(&destination).unwrap();
	fs::write(destination.join("notes"), "old").unwrap();
	let store = Store::open(root.path().join("state")).unwrap();

	let result = execute(
		&store,
		request(&source, &destination, Operation::Copy),
		&AtomicBool::new(false),
		|_| Choice::Replace,
		|_| {},
	)
	.unwrap();
	assert_eq!(result.status, Status::Complete);
	assert_eq!(fs::read(destination.join("notes")).unwrap(), b"new");
	assert_eq!(
		undo(&store, &result.id, |_| {}).unwrap().status,
		Status::RolledBack
	);
	assert_eq!(fs::read(destination.join("notes")).unwrap(), b"old");
}

#[test]
fn move_undo_returns_the_surviving_data() {
	let root = TestDir::new();
	let source = root.path().join("folder");
	let destination = root.path().join("destination");
	fs::create_dir(&source).unwrap();
	fs::write(source.join("file"), "contents").unwrap();
	fs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();

	let result = execute(
		&store,
		request(&source, &destination, Operation::Move),
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|_| {},
	)
	.unwrap();
	assert_eq!(result.status, Status::Complete);
	assert!(!source.exists());
	assert_eq!(
		undo(&store, &result.id, |_| {}).unwrap().status,
		Status::RolledBack
	);
	assert_eq!(fs::read(source.join("file")).unwrap(), b"contents");
	assert!(!destination.join("folder").exists());
}

#[test]
fn changed_destination_blocks_undo_without_destroying_either_version() {
	let root = TestDir::new();
	let source = root.path().join("file");
	let destination = root.path().join("destination");
	fs::write(&source, "original").unwrap();
	fs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let result = execute(
		&store,
		request(&source, &destination, Operation::Copy),
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|_| {},
	)
	.unwrap();

	fs::write(destination.join("file"), "edited").unwrap();
	let result = undo(&store, &result.id, |_| {}).unwrap();
	assert_eq!(result.status, Status::NeedsRecovery);
	assert_eq!(fs::read(destination.join("file")).unwrap(), b"edited");
	assert_eq!(fs::read(source).unwrap(), b"original");
}

#[test]
fn cancellation_rolls_back_already_committed_items() {
	let root = TestDir::new();
	let destination = root.path().join("destination");
	fs::create_dir(&destination).unwrap();
	let first = root.path().join("first");
	let second = root.path().join("second");
	fs::write(&first, "first").unwrap();
	fs::write(&second, "second").unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let cancelled = AtomicBool::new(false);
	let mut input = request(&first, &destination, Operation::Move);
	input.sources.push(second.clone());
	let mut commits = 0;

	let result = execute(
		&store,
		input,
		&cancelled,
		|_| Choice::Cancel,
		|event| {
			if matches!(
				event,
				Event::Progress {
					phase: Phase::Committing,
					..
				}
			) {
				commits += 1;
				if commits == 2 {
					cancelled.store(true, Ordering::Release);
				}
			}
		},
	)
	.unwrap();
	assert_eq!(result.status, Status::RolledBack);
	assert_eq!(fs::read(first).unwrap(), b"first");
	assert_eq!(fs::read(second).unwrap(), b"second");
	assert!(!destination.join("first").exists());
	assert!(!destination.join("second").exists());
}

#[test]
fn failed_required_flush_preserves_move_source() {
	let root = TestDir::new();
	let source = root.path().join("file");
	let destination = root.path().join("destination");
	fs::write(&source, "important").unwrap();
	fs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut input = request(&source, &destination, Operation::Move);
	input.durability = Durability::Durable;

	let result = worker::execute_with_flush(
		&store,
		input,
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|_| {},
		|_, _| Err(std::io::Error::other("injected flush failure")),
	)
	.unwrap();
	assert_eq!(result.status, Status::RolledBack);
	assert_eq!(fs::read(source).unwrap(), b"important");
	assert!(!destination.join("file").exists());
}

#[test]
fn folder_merge_undo_preserves_unrelated_destination_entries() {
	let root = TestDir::new();
	let source = root.path().join("folder");
	let destination = root.path().join("destination");
	fs::create_dir(&source).unwrap();
	fs::write(source.join("new"), "new").unwrap();
	fs::create_dir_all(destination.join("folder")).unwrap();
	fs::write(destination.join("folder/old"), "old").unwrap();
	let store = Store::open(root.path().join("state")).unwrap();

	let result = execute(
		&store,
		request(&source, &destination, Operation::Move),
		&AtomicBool::new(false),
		|_| Choice::Merge,
		|_| {},
	)
	.unwrap();
	assert_eq!(result.status, Status::Complete);
	assert!(!source.exists());
	assert_eq!(
		undo(&store, &result.id, |_| {}).unwrap().status,
		Status::RolledBack
	);
	assert_eq!(fs::read(source.join("new")).unwrap(), b"new");
	assert_eq!(fs::read(destination.join("folder/old")).unwrap(), b"old");
	assert!(!destination.join("folder/new").exists());
}

#[test]
fn symlinks_are_copied_without_following_them() {
	let root = TestDir::new();
	let source = root.path().join("link");
	let destination = root.path().join("destination");
	std::os::unix::fs::symlink("missing-target", &source).unwrap();
	fs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();

	let result = execute(
		&store,
		request(&source, &destination, Operation::Copy),
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|_| {},
	)
	.unwrap();
	assert_eq!(result.status, Status::Complete);
	assert_eq!(
		fs::read_link(destination.join("link")).unwrap(),
		Path::new("missing-target")
	);
}

#[test]
fn undo_reconciles_a_replacement_restored_before_the_journal_was_saved() {
	let root = TestDir::new();
	let source = root.path().join("file");
	let destination = root.path().join("destination");
	fs::write(&source, "new").unwrap();
	fs::create_dir(&destination).unwrap();
	fs::write(destination.join("file"), "old").unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let result = execute(
		&store,
		request(&source, &destination, Operation::Copy),
		&AtomicBool::new(false),
		|_| Choice::Replace,
		|_| {},
	)
	.unwrap();
	let journal = store.load(&result.id).unwrap();
	let change = journal.changes.first().unwrap();

	// Simulate death after the inverse filesystem mutations, before their result
	// can be saved. The durable forward record must be enough to reconcile it.
	super::fs::remove_recorded(
		&change.destination.path(),
		change.published.as_ref().unwrap(),
	)
	.unwrap();
	super::fs::rename_new(&change.backup.path(), &change.destination.path()).unwrap();
	let result = undo(&store, &result.id, |_| {}).unwrap();
	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert_eq!(fs::read(destination.join("file")).unwrap(), b"old");
}

#[test]
fn keep_both_and_skip_never_replace_existing_data() {
	let root = TestDir::new();
	let source = root.path().join("file");
	let destination = root.path().join("destination");
	fs::write(&source, "new").unwrap();
	fs::create_dir(&destination).unwrap();
	fs::write(destination.join("file"), "old").unwrap();
	let store = Store::open(root.path().join("state")).unwrap();

	for choice in [Choice::Skip, Choice::KeepBoth] {
		let result = execute(
			&store,
			request(&source, &destination, Operation::Copy),
			&AtomicBool::new(false),
			|_| choice,
			|_| {},
		)
		.unwrap();
		assert_eq!(result.status, Status::Complete, "{:?}", result.problems);
		assert_eq!(fs::read(destination.join("file")).unwrap(), b"old");
	}
	assert_eq!(fs::read(destination.join("file (copy 1)")).unwrap(), b"new");
}

#[test]
fn non_utf8_paths_survive_journal_and_undo() {
	use std::os::unix::ffi::OsStringExt;
	let root = TestDir::new();
	let source = root
		.path()
		.join(std::ffi::OsString::from_vec(vec![b'f', 0xff]));
	let destination = root.path().join("destination");
	fs::write(&source, "contents").unwrap();
	fs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();

	let result = execute(
		&store,
		request(&source, &destination, Operation::Copy),
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|_| {},
	)
	.unwrap();
	assert_eq!(result.status, Status::Complete);
	assert_eq!(
		undo(&store, &result.id, |_| {}).unwrap().status,
		Status::RolledBack
	);
	assert_eq!(fs::read(&source).unwrap(), b"contents");
}

fn separate_filesystem(root: &Path) -> Option<TestDir> {
	use std::os::unix::fs::MetadataExt;
	let shared = Path::new("/dev/shm");
	if fs::metadata(shared).ok()?.dev() == fs::metadata(root).ok()?.dev() {
		return None;
	}
	eprintln!(
		"cross-filesystem fixture: source device {}, destination device {}",
		fs::metadata(shared).ok()?.dev(),
		fs::metadata(root).ok()?.dev()
	);
	Some(TestDir::new_in(shared))
}

#[test]
fn cross_filesystem_move_and_inverse_keep_contents() {
	let root = TestDir::new();
	let Some(other) = separate_filesystem(root.path()) else {
		eprintln!("No separate /dev/shm filesystem available");
		return;
	};
	let source = other.path().join("folder");
	let destination = root.path().join("destination");
	fs::create_dir(&source).unwrap();
	fs::write(source.join("file"), "contents").unwrap();
	fs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut input = request(&source, &destination, Operation::Move);
	input.durability = Durability::Durable;

	let result = execute(
		&store,
		input,
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|_| {},
	)
	.unwrap();
	assert_eq!(result.status, Status::Complete, "{:?}", result.problems);
	assert!(!source.exists());
	assert!(!store.load(&result.id).unwrap().changes[0].renamed);
	let result = undo(&store, &result.id, |_| {}).unwrap();
	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert_eq!(fs::read(source.join("file")).unwrap(), b"contents");
	assert!(!destination.join("folder").exists());
}

#[test]
fn cross_filesystem_flush_error_never_deletes_originals() {
	let root = TestDir::new();
	let Some(other) = separate_filesystem(root.path()) else {
		return;
	};
	let source = other.path().join("file");
	let destination = root.path().join("destination");
	fs::write(&source, "contents").unwrap();
	fs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut input = request(&source, &destination, Operation::Move);
	input.durability = Durability::Durable;

	let result = worker::execute_with_flush(
		&store,
		input,
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|_| {},
		|_, _| Err(std::io::Error::other("writeback failed")),
	)
	.unwrap();
	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert_eq!(fs::read(source).unwrap(), b"contents");
	assert!(!destination.join("file").exists());
}

#[test]
fn interrupted_cross_filesystem_source_deletion_rolls_back_from_destination() {
	use super::model::{Operation, Step};
	let root = TestDir::new();
	let Some(other) = separate_filesystem(root.path()) else {
		return;
	};
	let source = other.path().join("folder");
	let destination = root.path().join("destination");
	fs::create_dir(&source).unwrap();
	fs::write(source.join("first"), "a").unwrap();
	fs::write(source.join("second"), "b").unwrap();
	fs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let result = execute(
		&store,
		request(&source, &destination, Operation::Copy),
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|_| {},
	)
	.unwrap();
	let mut journal = store.load(&result.id).unwrap();
	let change = journal.changes.first_mut().unwrap();
	change.operation = Operation::Move;
	change.step = Step::RemovingSource;
	let quarantine = super::fs::removal_directory(&source, &change.before).unwrap();
	store.save(&journal).unwrap();
	super::fs::private_directory(&quarantine).unwrap();
	super::fs::rename_new(&source, &quarantine.join("data")).unwrap();
	fs::remove_file(quarantine.join("data/first")).unwrap();

	let result = undo(&store, &result.id, |_| {}).unwrap();
	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert_eq!(fs::read(source.join("first")).unwrap(), b"a");
	assert_eq!(fs::read(source.join("second")).unwrap(), b"b");
	assert!(!quarantine.exists());
}

#[test]
fn inverse_cleanup_resumes_after_restored_source_was_published() {
	use super::model::Step;
	let root = TestDir::new();
	let Some(other) = separate_filesystem(root.path()) else {
		return;
	};
	let source = other.path().join("folder");
	let destination = root.path().join("destination");
	fs::create_dir(&source).unwrap();
	fs::write(source.join("first"), "a").unwrap();
	fs::write(source.join("second"), "b").unwrap();
	fs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let result = execute(
		&store,
		request(&source, &destination, Operation::Move),
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|_| {},
	)
	.unwrap();
	assert_eq!(result.status, Status::Complete);
	fs::create_dir(&source).unwrap();
	fs::copy(destination.join("folder/first"), source.join("first")).unwrap();
	fs::copy(destination.join("folder/second"), source.join("second")).unwrap();
	let mut journal = store.load(&result.id).unwrap();
	let change = journal.changes.first_mut().unwrap();
	change.undo_snapshot = Some(super::fs::snapshot(&source).unwrap());
	change.step = Step::Undoing;
	let quarantine = super::fs::removal_directory(
		&destination.join("folder"),
		change.published.as_ref().unwrap(),
	)
	.unwrap();
	store.save(&journal).unwrap();
	super::fs::private_directory(&quarantine).unwrap();
	super::fs::rename_new(&destination.join("folder"), &quarantine.join("data")).unwrap();
	fs::remove_file(quarantine.join("data/first")).unwrap();

	let result = undo(&store, &result.id, |_| {}).unwrap();
	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert_eq!(fs::read(source.join("first")).unwrap(), b"a");
	assert_eq!(fs::read(source.join("second")).unwrap(), b"b");
	assert!(!quarantine.exists());
}
