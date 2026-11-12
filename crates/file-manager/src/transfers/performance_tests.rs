#![allow(clippy::unwrap_used)]

use super::*;
use crate::test_support::TestDir;
use std::time::Instant;

#[test]
fn transfer_content_read_budget() {
	let mut measurements = Vec::new();
	for (name, operation, cancel) in [
		("rename", Operation::Move, false),
		("copy", Operation::Copy, false),
		("cancel", Operation::Copy, true),
	] {
		let root = TestDir::new();
		let source = root.path().join("source");
		let destination = root.path().join("destination");
		stdfs::write(&source, vec![7; 8 * 1024 * 1024]).unwrap();
		stdfs::create_dir(&destination).unwrap();
		let store = Store::open(root.path().join("state")).unwrap();
		let cancelled = AtomicBool::new(false);
		fs::INSPECTED_BYTES.with(|bytes| bytes.set(0));
		let started = Instant::now();
		let mut preparation = None;
		let mut rollback = None;
		let outcome = execute(
			&store,
			Request {
				sources: vec![source.clone()],
				destination: destination.clone(),
				operation,
				durability: Durability::Buffered,
				acknowledge_volatile: true,
			},
			&cancelled,
			|_| Choice::Cancel,
			|event| {
				if let Event::Progress { phase, bytes, .. } = event {
					if phase == Phase::Copying {
						preparation.get_or_insert((
							started.elapsed(),
							fs::INSPECTED_BYTES.with(|b| b.get()),
						));
						if cancel && bytes >= 1024 * 1024 {
							cancelled.store(true, Ordering::Release);
						}
					}
					if phase == Phase::RollingBack {
						rollback = Some((Instant::now(), fs::INSPECTED_BYTES.with(|b| b.get())));
					}
				}
			},
		)
		.unwrap();
		let inspected = fs::INSPECTED_BYTES.with(|bytes| bytes.get());
		let preparation_bytes = preparation.map_or(0, |(_, bytes)| bytes);
		let rollback_bytes = rollback.map_or(0, |(_, bytes)| inspected - bytes);
		println!(
			"{name}: elapsed={:?}, preparation={preparation:?}, inspected={inspected}, rollback={:?}, rollback_inspected={rollback_bytes}",
			started.elapsed(),
			rollback.map(|(at, _)| at.elapsed())
		);
		assert_eq!(
			outcome.status,
			if cancel {
				Status::RolledBack
			} else {
				Status::Complete
			},
			"{:?}",
			outcome.problems
		);
		if cancel {
			assert!(source.exists());
			assert!(!destination.join("source").exists());
		}
		measurements.push((name, inspected, preparation_bytes, rollback_bytes));
		if operation == Operation::Move {
			let undone = undo(&store, &outcome.id, |_| {}).unwrap();
			assert_eq!(undone.status, Status::RolledBack, "{:?}", undone.problems);
			assert!(source.exists());
			assert_eq!(fs::INSPECTED_BYTES.with(|bytes| bytes.get()), 0);
		}
	}
	assert_eq!(
		measurements[0].1, 0,
		"same-filesystem rename must not read file contents"
	);
	assert_eq!(
		measurements[1].2, 0,
		"copy preparation must not pre-read file contents"
	);
	assert_eq!(
		measurements[2].3, 0,
		"private partial-copy cleanup must not read file contents"
	);
}

fn copy_request(source: &Path, destination: &Path) -> Request {
	Request {
		sources: vec![source.into()],
		destination: destination.into(),
		operation: Operation::Copy,
		durability: Durability::Buffered,
		acknowledge_volatile: true,
	}
}

#[test]
fn rename_preserves_inode_and_undo_refuses_content_edits_with_restored_mtime() {
	use std::os::unix::fs::MetadataExt;
	let root = TestDir::new();
	let source = root.path().join("source");
	let destination = root.path().join("destination");
	stdfs::write(&source, b"original").unwrap();
	stdfs::create_dir(&destination).unwrap();
	let original = stdfs::metadata(&source).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut request = copy_request(&source, &destination);
	request.operation = Operation::Move;

	let result = execute(
		&store,
		request,
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|_| {},
	)
	.unwrap();
	assert_eq!(result.status, Status::Complete);
	let moved = destination.join("source");
	assert_eq!(stdfs::metadata(&moved).unwrap().ino(), original.ino());
	stdfs::write(&moved, b"modified").unwrap();
	File::open(&moved)
		.unwrap()
		.set_times(FileTimes::new().set_modified(original.modified().unwrap()))
		.unwrap();

	let undone = undo(&store, &result.id, |_| {}).unwrap();
	assert_eq!(undone.status, Status::NeedsRecovery);
	assert!(!source.exists());
	assert_eq!(stdfs::read(&moved).unwrap(), b"modified");
}

#[test]
fn copied_data_is_hashed_in_flight_and_staging_corruption_is_not_published() {
	let root = TestDir::new();
	let source = root.path().join("source");
	let destination = root.path().join("destination");
	stdfs::write(&source, vec![7; 131_072]).unwrap();
	stdfs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut corrupted = false;

	let result = execute(
		&store,
		copy_request(&source, &destination),
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|event| {
			if matches!(
				event,
				Event::Progress {
					phase: Phase::Verifying,
					..
				}
			) {
				let record = store.history().unwrap().pop().unwrap();
				let part = record.changes[0].stage.path();
				stdfs::write(part, vec![8; 131_072]).unwrap();
				corrupted = true;
			}
		},
	)
	.unwrap();

	assert!(corrupted);
	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert!(
		result
			.problems
			.iter()
			.any(|problem| problem.contains("does not match source"))
	);
	assert!(!destination.join("source").exists());
	assert_eq!(stdfs::read(&source).unwrap(), vec![7; 131_072]);
}

#[test]
fn source_changes_during_streaming_prevent_publication() {
	let root = TestDir::new();
	let source = root.path().join("source");
	let destination = root.path().join("destination");
	stdfs::write(&source, vec![7; 131_072]).unwrap();
	stdfs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut changed = false;

	let result = execute(
		&store,
		copy_request(&source, &destination),
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|event| {
			if !changed
				&& matches!(
					event,
					Event::Progress {
						phase: Phase::Copying,
						..
					}
				) {
				stdfs::write(&source, vec![8; 131_072]).unwrap();
				changed = true;
			}
		},
	)
	.unwrap();

	assert!(changed);
	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert!(!destination.join("source").exists());
	assert_eq!(stdfs::read(&source).unwrap(), vec![8; 131_072]);
}

#[test]
fn cancellation_at_verification_discards_private_staging_without_reading_contents() {
	let root = TestDir::new();
	let source = root.path().join("source");
	let destination = root.path().join("destination");
	stdfs::write(&source, vec![7; 131_072]).unwrap();
	stdfs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let cancelled = AtomicBool::new(false);
	fs::INSPECTED_BYTES.with(|bytes| bytes.set(0));

	let result = execute(
		&store,
		copy_request(&source, &destination),
		&cancelled,
		|_| Choice::Cancel,
		|event| {
			if matches!(
				event,
				Event::Progress {
					phase: Phase::Verifying,
					..
				}
			) {
				cancelled.store(true, Ordering::Release);
			}
		},
	)
	.unwrap();

	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert_eq!(fs::INSPECTED_BYTES.with(|bytes| bytes.get()), 0);
	assert!(!destination.join("source").exists());
	assert!(source.exists());
}

#[test]
fn cancellation_before_flushing_skips_sync_and_discards_verified_private_copy_without_rereads() {
	let root = TestDir::new();
	let source = root.path().join("source");
	let destination = root.path().join("destination");
	stdfs::write(&source, vec![7; 131_072]).unwrap();
	stdfs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let cancelled = AtomicBool::new(false);
	let mut request = copy_request(&source, &destination);
	request.durability = Durability::Durable;
	let mut before_rollback = None;
	fs::INSPECTED_BYTES.with(|bytes| bytes.set(0));

	let result = execute_with_flush(
		&store,
		request,
		&cancelled,
		|_| Choice::Cancel,
		|event| {
			if matches!(
				event,
				Event::Progress {
					phase: Phase::Flushing,
					..
				}
			) {
				before_rollback = Some(fs::INSPECTED_BYTES.with(|bytes| bytes.get()));
				cancelled.store(true, Ordering::Release);
			}
		},
		|_, _| panic!("Cancellation must be checked before starting a blocking sync"),
	)
	.unwrap();

	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert_eq!(
		before_rollback,
		Some(fs::INSPECTED_BYTES.with(|bytes| bytes.get()))
	);
	assert!(!destination.join("source").exists());
	assert_eq!(stdfs::read(&source).unwrap(), vec![7; 131_072]);
}

#[test]
fn rename_undo_recovers_publication_before_the_published_snapshot_was_saved() {
	let root = TestDir::new();
	let source = root.path().join("source");
	let destination = root.path().join("destination");
	stdfs::write(&source, b"original").unwrap();
	stdfs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut request = copy_request(&source, &destination);
	request.operation = Operation::Move;
	let result = execute(
		&store,
		request,
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|_| {},
	)
	.unwrap();
	let mut journal = store.load(&result.id).unwrap();
	journal.changes[0].published = None;
	journal.changes[0].step = Step::Publishing;
	journal.status = Status::Running;
	store.save(&journal).unwrap();

	let result = undo(&store, &result.id, |_| {}).unwrap();

	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert_eq!(stdfs::read(&source).unwrap(), b"original");
	assert!(!destination.join("source").exists());
}

#[test]
fn rename_undo_recovers_source_restoration_before_its_snapshot_was_saved() {
	let root = TestDir::new();
	let source = root.path().join("source");
	let destination = root.path().join("destination");
	stdfs::write(&source, b"original").unwrap();
	stdfs::create_dir(&destination).unwrap();
	stdfs::write(destination.join("source"), b"replaced").unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut request = copy_request(&source, &destination);
	request.operation = Operation::Move;
	let result = execute(
		&store,
		request,
		&AtomicBool::new(false),
		|_| Choice::Replace,
		|_| {},
	)
	.unwrap();
	let mut journal = store.load(&result.id).unwrap();
	let change = &mut journal.changes[0];
	change.undo_snapshot = change.published.clone();
	fs::rename_new(&destination.join("source"), &source).unwrap();
	fs::rename_new(&change.backup.path(), &destination.join("source")).unwrap();
	change.step = Step::Undoing;
	journal.status = Status::RollingBack;
	store.save(&journal).unwrap();

	let result = undo(&store, &result.id, |_| {}).unwrap();

	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert_eq!(stdfs::read(&source).unwrap(), b"original");
	assert_eq!(
		stdfs::read(destination.join("source")).unwrap(),
		b"replaced"
	);
}

#[test]
fn legacy_hashed_rename_recovery_still_checks_file_contents() {
	let root = TestDir::new();
	let source = root.path().join("source");
	let destination = root.path().join("destination");
	stdfs::write(&source, b"original").unwrap();
	stdfs::create_dir(&destination).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut request = copy_request(&source, &destination);
	request.operation = Operation::Move;
	let result = execute(
		&store,
		request,
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|_| {},
	)
	.unwrap();
	let mut journal = store.load(&result.id).unwrap();
	let change = &mut journal.changes[0];
	let stage = change.stage.path();
	fs::rename_new(&destination.join("source"), &stage).unwrap();
	change.before = fs::snapshot(&stage).unwrap();
	change.staged = None;
	change.published = None;
	change.step = Step::Prepared;
	journal.status = Status::Running;
	store.save(&journal).unwrap();
	let modified = stdfs::metadata(&stage).unwrap().modified().unwrap();
	stdfs::write(&stage, b"modified").unwrap();
	File::open(&stage)
		.unwrap()
		.set_times(FileTimes::new().set_modified(modified))
		.unwrap();

	let result = undo(&store, &result.id, |_| {}).unwrap();

	assert_eq!(result.status, Status::NeedsRecovery);
	assert!(!source.exists());
	assert_eq!(stdfs::read(&stage).unwrap(), b"modified");
}
