#![allow(clippy::unwrap_used)]
use super::*;
use crate::{test_support::TestDir, trash};
use std::os::unix::fs::{PermissionsExt, symlink};

struct Fixture {
	_root: TestDir,
	trash: PathBuf,
	store: Store,
}

impl Fixture {
	fn new() -> Self {
		let root = TestDir::new();
		let trash = root.path().join("Trash");
		for path in [&trash, &trash.join("files"), &trash.join("info")] {
			stdfs::create_dir(path).unwrap();
			stdfs::set_permissions(path, stdfs::Permissions::from_mode(0o700)).unwrap();
		}
		let store =
			Store::open_with_recovery(root.path().join("state"), root.path().join("recovery"))
				.unwrap();
		Self {
			_root: root,
			trash,
			store,
		}
	}

	fn add(&self, name: &str) -> PathBuf {
		let path = self.trash.join("files").join(name);
		stdfs::write(&path, b"confirmed item").unwrap();
		stdfs::write(
			self.trash.join("info").join(format!("{name}.trashinfo")),
			format!("[Trash Info]\nPath=/original/{name}\nDeletionDate=2026-11-12T12:00:00\n"),
		)
		.unwrap();
		path
	}

	fn capture(&self) -> Vec<Captured> {
		trash::fixture_listing(&self.trash)
			.entries
			.iter()
			.map(|entry| Captured::capture(entry, false).unwrap())
			.collect()
	}
}

#[test]
fn confirmed_batch_removes_paired_metadata_but_leaves_new_arrivals() {
	let fixture = Fixture::new();
	let selected = fixture.add("selected");
	let captured = fixture.capture();
	let new = fixture.add("new-arrival");

	assert!(delete_trash(&fixture.store, captured).unwrap().is_empty());
	assert!(!selected.exists());
	assert!(!fixture.trash.join("info/selected.trashinfo").exists());
	assert!(new.exists());
	assert!(fixture.trash.join("info/new-arrival.trashinfo").exists());
	assert!(pending_trash_deletions(&fixture.store).unwrap().is_empty());
}

#[test]
fn stale_batch_member_prevents_all_deletion() {
	let fixture = Fixture::new();
	let first = fixture.add("first");
	let second = fixture.add("second");
	let captured = fixture.capture();
	stdfs::rename(&second, fixture._root.path().join("kept")).unwrap();
	stdfs::write(&second, b"new item").unwrap();

	assert!(delete_trash(&fixture.store, captured).is_err());
	assert!(first.exists());
	assert_eq!(stdfs::read(second).unwrap(), b"new item");
}

#[test]
fn deleting_trashed_symlink_does_not_follow_its_target() {
	let fixture = Fixture::new();
	let path = fixture.add("link");
	stdfs::remove_file(&path).unwrap();
	let outside = fixture._root.path().join("preserve");
	stdfs::write(&outside, b"untouched").unwrap();
	symlink(&outside, &path).unwrap();

	assert!(
		delete_trash(&fixture.store, fixture.capture())
			.unwrap()
			.is_empty()
	);
	assert!(stdfs::symlink_metadata(path).is_err());
	assert_eq!(stdfs::read(outside).unwrap(), b"untouched");
}

#[test]
fn damaged_or_missing_info_does_not_require_guessing_original_paths_for_deletion() {
	for missing in [false, true] {
		let fixture = Fixture::new();
		let path = fixture.add("item");
		let info = fixture.trash.join("info/item.trashinfo");
		if missing {
			stdfs::remove_file(&info).unwrap();
		} else {
			stdfs::write(&info, b"malformed metadata").unwrap();
		}

		assert!(
			delete_trash(&fixture.store, fixture.capture())
				.unwrap()
				.is_empty()
		);
		assert!(!path.exists());
		assert!(!info.exists());
	}
}

#[test]
fn interrupted_quarantine_before_checkpoint_can_be_retried_from_its_record() {
	let fixture = Fixture::new();
	let path = fixture.add("item");
	let mut record = prepare(&fixture.store, &fixture.capture().remove(0)).unwrap();
	let workspace = record.workspace.path();
	fs::private_directory(&workspace).unwrap();
	record.workspace_identity = fs::metadata_snapshot(&workspace)
		.unwrap()
		.0
		.into_iter()
		.next();
	save(&fixture.store, &record).unwrap();
	fs::rename_new(&path, &workspace.join("data")).unwrap();
	assert_eq!(pending_trash_deletions(&fixture.store).unwrap().len(), 1);

	retry_trash_deletion(&fixture.store, &record.id).unwrap();
	assert!(!path.exists());
	assert!(!workspace.exists());
	assert!(!fixture.trash.join("info/item.trashinfo").exists());
	assert!(pending_trash_deletions(&fixture.store).unwrap().is_empty());
}

#[test]
fn changed_workspace_child_blocks_cleanup_and_preserves_retry_record() {
	let fixture = Fixture::new();
	fixture.add("item");
	let mut record = prepare(&fixture.store, &fixture.capture().remove(0)).unwrap();
	let workspace = record.workspace.path();
	fs::private_directory(&workspace).unwrap();
	record.workspace_identity = fs::metadata_snapshot(&workspace)
		.unwrap()
		.0
		.into_iter()
		.next();
	save(&fixture.store, &record).unwrap();
	stdfs::write(workspace.join("unexpected"), b"preserve").unwrap();

	assert!(retry_trash_deletion(&fixture.store, &record.id).is_err());
	assert_eq!(
		stdfs::read(workspace.join("unexpected")).unwrap(),
		b"preserve"
	);
	assert_eq!(pending_trash_deletions(&fixture.store).unwrap().len(), 1);
}
