#![allow(clippy::unwrap_used)]
use super::*;
use crate::{
	test_support::TestDir,
	transfers::{self, Durability, Status},
	trash,
};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

struct Fixture {
	_root: TestDir,
	trash: PathBuf,
	destination: PathBuf,
	source: PathBuf,
	info: PathBuf,
	store: Store,
}

impl Fixture {
	fn new() -> Self {
		let root = TestDir::new();
		let trash = root.path().join("Trash");
		let destination = root.path().join("original/name");
		stdfs::create_dir_all(destination.parent().unwrap()).unwrap();
		for path in [&trash, &trash.join("files"), &trash.join("info")] {
			stdfs::create_dir(path).unwrap();
			stdfs::set_permissions(path, stdfs::Permissions::from_mode(0o700)).unwrap();
		}
		let source = trash.join("files/collision-name.1");
		let info = trash.join("info/collision-name.1.trashinfo");
		stdfs::write(&source, b"trashed payload").unwrap();
		stdfs::write(
			&info,
			format!(
				"[Trash Info]\nPath={}\nDeletionDate=2026-11-12T12:00:00\n",
				destination.display()
			),
		)
		.unwrap();
		let store =
			Store::open_with_recovery(root.path().join("state"), root.path().join("recovery"))
				.unwrap();
		Self {
			_root: root,
			trash,
			destination,
			source,
			info,
			store,
		}
	}

	fn captured(&self) -> Captured {
		let listing = trash::fixture_listing(&self.trash);
		assert!(listing.issues.is_empty(), "{:?}", listing.issues);
		Captured::capture(listing.entries.first().unwrap(), true).unwrap()
	}

	fn restore(&self, decision: impl FnMut(Conflict) -> Choice) -> transfers::Outcome {
		transfers::restore_trash(
			&self.store,
			vec![self.captured()],
			Durability::Buffered,
			&AtomicBool::new(false),
			decision,
			|_| {},
		)
		.unwrap()
	}
}

#[test]
fn restore_uses_original_name_and_undo_returns_payload_and_metadata() {
	let fixture = Fixture::new();
	let metadata = stdfs::read(&fixture.info).unwrap();
	let result = fixture.restore(|_| Choice::Cancel);

	assert_eq!(result.status, Status::Complete, "{:?}", result.problems);
	assert_eq!(
		stdfs::read(&fixture.destination).unwrap(),
		b"trashed payload"
	);
	assert!(!fixture.source.exists());
	assert!(!fixture.info.exists());
	assert_eq!(fixture.store.load(&result.id).unwrap().version, 2);

	let result = transfers::undo(&fixture.store, &result.id, |_| {}).unwrap();
	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert!(!fixture.destination.exists());
	assert_eq!(stdfs::read(&fixture.source).unwrap(), b"trashed payload");
	assert_eq!(stdfs::read(&fixture.info).unwrap(), metadata);
}

#[test]
fn restore_replace_retains_existing_destination_and_undo_recovers_both() {
	let fixture = Fixture::new();
	stdfs::write(&fixture.destination, b"existing destination").unwrap();
	let result = fixture.restore(|conflict| {
		assert_eq!(conflict.reason, ConflictReason::RestoreEntry);
		assert!(!conflict.directories);
		Choice::Replace
	});
	assert_eq!(result.status, Status::Complete, "{:?}", result.problems);
	assert_eq!(
		stdfs::read(&fixture.destination).unwrap(),
		b"trashed payload"
	);

	let result = transfers::undo(&fixture.store, &result.id, |_| {}).unwrap();
	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert_eq!(
		stdfs::read(&fixture.destination).unwrap(),
		b"existing destination"
	);
	assert_eq!(stdfs::read(&fixture.source).unwrap(), b"trashed payload");
	assert!(fixture.info.exists());
}

#[test]
fn skip_and_keep_both_leave_existing_destination_intact() {
	for decision in [Choice::Skip, Choice::KeepBoth] {
		let fixture = Fixture::new();
		stdfs::write(&fixture.destination, b"existing").unwrap();
		let result = fixture.restore(|_| decision);
		assert_eq!(result.status, Status::Complete, "{:?}", result.problems);
		assert_eq!(stdfs::read(&fixture.destination).unwrap(), b"existing");
		if decision == Choice::Skip {
			assert!(fixture.source.exists());
			assert!(fixture.info.exists());
			assert!(fixture.store.load(&result.id).unwrap().changes.is_empty());
		} else {
			assert_eq!(
				stdfs::read(fixture.destination.with_file_name("name (copy 1)")).unwrap(),
				b"trashed payload"
			);
			assert!(!fixture.info.exists());
		}
	}
}

#[test]
fn recreating_missing_parents_requires_approval_and_undo_cleans_only_empty_owned_parents() {
	for approved in [false, true] {
		let fixture = Fixture::new();
		stdfs::remove_dir(fixture.destination.parent().unwrap()).unwrap();
		let result = fixture.restore(|conflict| {
			assert_eq!(conflict.reason, ConflictReason::MissingParents);
			if approved {
				Choice::ApproveRecovery
			} else {
				Choice::Cancel
			}
		});
		assert_eq!(
			result.status,
			if approved {
				Status::Complete
			} else {
				Status::RolledBack
			},
			"{:?}",
			result.problems
		);
		if approved {
			let undone = transfers::undo(&fixture.store, &result.id, |_| {}).unwrap();
			assert_eq!(undone.status, Status::RolledBack, "{:?}", undone.problems);
			assert!(!fixture.destination.parent().unwrap().exists());
		} else {
			assert!(!fixture.destination.parent().unwrap().exists());
		}
		assert!(fixture.source.exists());
		assert!(fixture.info.exists());
	}
}

#[test]
fn reused_trash_payload_or_metadata_slot_blocks_undo_without_overwriting_any_data() {
	for metadata in [false, true] {
		let fixture = Fixture::new();
		let result = fixture.restore(|_| Choice::Cancel);
		let occupied = if metadata {
			&fixture.info
		} else {
			&fixture.source
		};
		stdfs::write(occupied, b"another application's item").unwrap();

		let result = transfers::undo(&fixture.store, &result.id, |_| {}).unwrap();
		assert_eq!(
			result.status,
			Status::NeedsRecovery,
			"{:?}",
			result.problems
		);
		assert_eq!(
			stdfs::read(&fixture.destination).unwrap(),
			b"trashed payload"
		);
		assert_eq!(
			stdfs::read(occupied).unwrap(),
			b"another application's item"
		);
	}
}

#[test]
fn interrupted_metadata_removal_and_interrupted_inverse_are_reconciled() {
	for interrupted_inverse in [false, true] {
		let fixture = Fixture::new();
		let result = fixture.restore(|_| Choice::Cancel);
		let mut journal = fixture.store.load(&result.id).unwrap();
		if interrupted_inverse {
			journal.changes[0].undo_snapshot = journal.changes[0].published.clone();
			fs::rename_new(&fixture.destination, &fixture.source).unwrap();
			journal.status = Status::RollingBack;
		} else {
			let metadata = &journal.restoration.as_ref().unwrap().items[0];
			fs::rename_new(&metadata.backup.path(), &fixture.info).unwrap();
			journal.status = Status::Running;
		}
		fixture.store.save(&journal).unwrap();

		let result = transfers::undo(&fixture.store, &result.id, |_| {}).unwrap();
		assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
		assert!(!fixture.destination.exists());
		assert_eq!(stdfs::read(&fixture.source).unwrap(), b"trashed payload");
		assert!(fixture.info.exists());
	}
}

#[test]
fn expiry_removes_only_owned_metadata_recovery_not_reused_trash_slots() {
	let fixture = Fixture::new();
	let result = fixture.restore(|_| Choice::Cancel);
	stdfs::write(&fixture.source, b"new trash payload").unwrap();
	stdfs::write(&fixture.info, b"new trash metadata").unwrap();
	let issues = super::super::retention::expire(
		&fixture.store,
		super::super::journal::now().unwrap() + 8 * 24 * 60 * 60,
		5 * 1024 * 1024 * 1024,
	)
	.unwrap();

	assert!(issues.is_empty(), "{issues:?}");
	assert!(fixture.store.load(&result.id).is_err());
	assert_eq!(stdfs::read(&fixture.source).unwrap(), b"new trash payload");
	assert_eq!(stdfs::read(&fixture.info).unwrap(), b"new trash metadata");
	assert_eq!(
		stdfs::read(&fixture.destination).unwrap(),
		b"trashed payload"
	);
}

#[test]
fn captured_metadata_and_payload_replacements_are_rejected() {
	for metadata in [false, true] {
		let fixture = Fixture::new();
		let captured = fixture.captured();
		let path = if metadata {
			&fixture.info
		} else {
			&fixture.source
		};
		stdfs::rename(path, path.with_extension("kept")).unwrap();
		stdfs::write(path, b"unrelated replacement").unwrap();
		assert!(captured.validate().is_err());
		assert_eq!(stdfs::read(path).unwrap(), b"unrelated replacement");
	}
}

#[test]
fn restore_rejects_symlinked_original_ancestors_without_touching_either_location() {
	use std::os::unix::fs::symlink;
	let fixture = Fixture::new();
	let parent = fixture.destination.parent().unwrap();
	stdfs::remove_dir(parent).unwrap();
	let other = fixture._root.path().join("elsewhere");
	stdfs::create_dir(&other).unwrap();
	symlink(&other, parent).unwrap();

	let result = fixture.restore(|_| Choice::Cancel);
	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert!(
		result
			.problems
			.iter()
			.any(|problem| problem.contains("symlink"))
	);
	assert_eq!(stdfs::read(&fixture.source).unwrap(), b"trashed payload");
	assert!(fixture.info.exists());
	assert!(stdfs::read_dir(&other).unwrap().next().is_none());
}

#[test]
fn undo_preserves_new_content_in_recreated_original_parent() {
	let fixture = Fixture::new();
	let parent = fixture.destination.parent().unwrap();
	stdfs::remove_dir(parent).unwrap();
	let result = fixture.restore(|_| Choice::ApproveRecovery);
	assert_eq!(result.status, Status::Complete);
	stdfs::write(parent.join("new-file"), b"preserve new data").unwrap();

	let result = transfers::undo(&fixture.store, &result.id, |_| {}).unwrap();
	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert_eq!(
		stdfs::read(parent.join("new-file")).unwrap(),
		b"preserve new data"
	);
	assert!(fixture.source.exists());
	assert!(fixture.info.exists());
}

#[test]
fn invalid_deletion_date_does_not_prevent_restore_when_original_path_is_known() {
	let fixture = Fixture::new();
	let metadata = format!(
		"[Trash Info]\nPath={}\nDeletionDate=invalid\n",
		fixture.destination.display()
	);
	stdfs::write(&fixture.info, &metadata).unwrap();
	let listing = trash::fixture_listing(&fixture.trash);
	assert!(!listing.entries[0].issues.is_empty());
	assert!(listing.entries[0].original_path.is_some());

	let result = fixture.restore(|_| Choice::Cancel);
	assert_eq!(result.status, Status::Complete, "{:?}", result.problems);
	let undone = transfers::undo(&fixture.store, &result.id, |_| {}).unwrap();
	assert_eq!(undone.status, Status::RolledBack);
	assert_eq!(stdfs::read_to_string(&fixture.info).unwrap(), metadata);
}

#[test]
fn restore_and_undo_can_cross_filesystems_without_losing_metadata_pairing() {
	use std::os::unix::fs::MetadataExt;
	if !Path::new("/dev/shm").is_dir() {
		return;
	}
	let other = TestDir::new_in(Path::new("/dev/shm"));
	let mut fixture = Fixture::new();
	if stdfs::metadata(&fixture.source).unwrap().dev()
		== stdfs::metadata(other.path()).unwrap().dev()
	{
		return;
	}
	fixture.destination = other.path().join("missing/nested/restored-name");
	stdfs::write(
		&fixture.info,
		format!(
			"[Trash Info]\nPath={}\nDeletionDate=2026-11-12T12:00:00\n",
			fixture.destination.display()
		),
	)
	.unwrap();

	let result = fixture.restore(|conflict| {
		assert!(matches!(
			conflict.reason,
			ConflictReason::VolatileMove | ConflictReason::MissingParents
		));
		Choice::ApproveRecovery
	});
	assert_eq!(result.status, Status::Complete, "{:?}", result.problems);
	assert!(!fixture.store.load(&result.id).unwrap().changes[0].renamed);
	assert_eq!(
		stdfs::read(&fixture.destination).unwrap(),
		b"trashed payload"
	);
	assert!(!fixture.info.exists());

	let undone = transfers::undo(&fixture.store, &result.id, |_| {}).unwrap();
	assert_eq!(undone.status, Status::RolledBack, "{:?}", undone.problems);
	assert!(!fixture.destination.exists());
	assert!(!other.path().join("missing").exists());
	assert_eq!(stdfs::read(&fixture.source).unwrap(), b"trashed payload");
	assert!(fixture.info.exists());

	let retry = transfers::undo(&fixture.store, &result.id, |_| {}).unwrap();
	assert_eq!(retry.status, Status::RolledBack);
	let issues = super::super::retention::expire(
		&fixture.store,
		super::super::journal::now().unwrap() + 8 * 24 * 60 * 60,
		5 * 1024 * 1024 * 1024,
	)
	.unwrap();
	assert!(issues.is_empty(), "{issues:?}");
	assert!(fixture.store.load(&result.id).is_err());
}
