//! Consented, replacement-only recovery outside the destination filesystem.
//!
//! The caller persists every callback before proceeding and must not remove the
//! original until `preserve` returns a complete record. Unknown leftovers are
//! deliberately retained; an inability to clean up is not permission to erase.
use super::{
	fs, journal,
	model::{Entry, Kind, Snapshot, StoredPath},
	worker,
};
use serde::{Deserialize, Serialize};
use std::{
	ffi::CString,
	fs::{self as stdfs, File},
	io,
	os::{
		fd::{AsRawFd, FromRawFd},
		unix::{ffi::OsStrExt, fs::MetadataExt},
	},
	path::{Component, Path, PathBuf},
	sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
	Copying,
	Complete,
	Restoring,
	Publishing,
	Restored,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Backup {
	destination: StoredPath,
	original: Snapshot,
	workspace: StoredPath,
	identity: Entry,
	storage_identity: Entry,
	/// Also records an incomplete copy after a recoverable copy error.
	partial: Option<Snapshot>,
	backup: Option<Snapshot>,
	restore_workspace: Option<StoredPath>,
	restore_identity: Option<Entry>,
	restore_storage_identity: Option<Entry>,
	restore_partial: Option<Snapshot>,
	/// Written durably before rename, so publication needs no result journal.
	restore_snapshot: Option<Snapshot>,
	phase: Phase,
}

impl Backup {
	pub(super) fn complete(&self) -> bool {
		self.backup.is_some() && self.phase != Phase::Copying
	}

	pub(super) fn retained_bytes(&self) -> u64 {
		if self.phase == Phase::Restored && self.cleanup_absent().unwrap_or(false) {
			return 0;
		}
		self.original
			.0
			.iter()
			.filter(|entry| entry.kind == Kind::File)
			.fold(0u64, |bytes, entry| bytes.saturating_add(entry.length))
	}

	fn cleanup_absent(&self) -> io::Result<bool> {
		if !workspace_absent_on_storage(&self.workspace.path(), &self.storage_identity)? {
			return Ok(false);
		}
		match (
			&self.restore_workspace,
			&self.restore_identity,
			&self.restore_storage_identity,
		) {
			(None, None, None) => Ok(true),
			(Some(work), Some(_), Some(storage)) => {
				workspace_absent_on_storage(&work.path(), storage)
			}
			_ => Ok(false),
		}
	}

	pub(super) fn restored(&self, destination: &Path) -> io::Result<bool> {
		self.check_destination(destination)?;
		if !fs::exists(destination)? {
			return Ok(false);
		}
		let actual = fs::snapshot(destination)?;
		Ok(
			(self.complete() && fs::same_contents(&actual, &self.original, true))
				|| self
					.restore_snapshot
					.as_ref()
					.is_some_and(|expected| fs::same_contents(&actual, expected, true)),
		)
	}

	/// Does not inspect or remove the live destination. No newly observed manifest
	/// is used here: deterministic deletion quarantines require the saved manifest.
	pub(super) fn expire(&self) -> io::Result<()> {
		if let (Some(work), Some(identity)) = (&self.restore_workspace, &self.restore_identity) {
			cleanup_workspace(
				&work.path(),
				identity,
				self.restore_storage_identity
					.as_ref()
					.ok_or_else(|| fs::invalid("Missing restore storage identity"))?,
				self.restore_partial
					.as_ref()
					.or(self.restore_snapshot.as_ref()),
			)?;
		} else if self.restore_workspace.is_some() || self.restore_identity.is_some() {
			return Err(fs::invalid("Incomplete central restore workspace record"));
		}
		cleanup_workspace(
			&self.workspace.path(),
			&self.identity,
			&self.storage_identity,
			self.partial.as_ref().or(self.backup.as_ref()),
		)
	}

	fn check_destination(&self, destination: &Path) -> io::Result<()> {
		if self.destination.path() != destination {
			return Err(fs::invalid(
				"Central recovery destination does not match journal",
			));
		}
		Ok(())
	}
}

pub(super) fn preserve(
	root: &Path, destination: &Path, original: &Snapshot, cancelled: &AtomicBool,
	mut save: impl FnMut(&Backup) -> io::Result<()>,
) -> io::Result<Backup> {
	if !root.is_absolute() || !destination.is_absolute() || root.starts_with(destination) {
		return Err(fs::invalid("Unsafe central recovery location"));
	}
	fs::unchanged(destination, original)?;
	check_cancel(cancelled)?;
	ensure_private_root(root)?;
	if crate::filesystem::classification::classify(root)?
		== crate::filesystem::classification::StorageClass::Volatile
	{
		return Err(fs::invalid(
			"Central recovery storage is volatile; choose persistent XDG data storage before replacing files",
		));
	}
	check_capacity(root, original)?;
	let storage_identity = directory_identity(root)?;
	let (work, identity) = new_workspace(root, "central")?;
	let mut record = Backup {
		destination: StoredPath::new(destination),
		original: original.clone(),
		workspace: StoredPath::new(&work),
		identity,
		storage_identity,
		partial: None,
		backup: None,
		restore_workspace: None,
		restore_identity: None,
		restore_storage_identity: None,
		restore_partial: None,
		restore_snapshot: None,
		phase: Phase::Copying,
	};
	// An empty workspace may remain if this save fails. No data has been copied.
	save(&record)?;
	let part = work.join("part");
	let copied = worker::copy_tree(destination, &part, original, true, cancelled, &mut |_| {});
	// Record a partial before attempting any cleanup, including on cancellation.
	if fs::exists(&part)? {
		record.partial = Some(fs::snapshot(&part)?);
		save(&record)?;
	}
	copied?;
	fs::unchanged(destination, original)?;
	let snapshot = record
		.partial
		.as_ref()
		.ok_or_else(|| fs::invalid("Missing central backup copy"))?;
	if !fs::same_contents(snapshot, original, false) {
		return Err(fs::invalid("Central backup verification failed"));
	}
	fs::sync_tree(&part, snapshot)?;
	fs::sync_parent(&part)?;
	check_cancel(cancelled)?;
	record.backup = Some(snapshot.clone());
	record.phase = Phase::Complete;
	save(&record)?;
	Ok(record)
}

pub(super) fn restore(
	backup: &mut Backup, destination: &Path, original: &Snapshot,
	mut save: impl FnMut(&Backup) -> io::Result<()>,
) -> io::Result<()> {
	backup.check_destination(destination)?;
	if *original != backup.original {
		return Err(fs::invalid(
			"Central recovery original does not match journal",
		));
	}
	// A complete backup permits finishing a previously interrupted original
	// deletion. Never start deleting a surviving original here.
	if fs::removal_pending(destination, original)? {
		if !backup.complete() {
			return Err(fs::invalid(
				"Original deletion has no complete central backup",
			));
		}
		validate_workspace(&backup.workspace.path(), &backup.identity)?;
		let expected = backup
			.backup
			.as_ref()
			.ok_or_else(|| fs::invalid("Missing backup"))?;
		let part = backup.workspace.path().join("part");
		if !fs::same_contents(&fs::snapshot(&part)?, expected, true) {
			return Err(fs::invalid(
				"Central backup changed; original quarantine retained",
			));
		}
		fs::remove_recorded(destination, original)?;
	}
	let original_survives =
		fs::exists(destination)? && fs::same_contents(&fs::snapshot(destination)?, original, true);
	if original_survives || backup.restored(destination)? {
		// Publication may have reached the filesystem before its parent fsync or
		// result journal. Make the surviving destination durable before expiry.
		let actual = fs::snapshot(destination)?;
		fs::sync_tree(destination, &actual)?;
		fs::sync_parent(destination)?;
		backup.phase = Phase::Restored;
		save(backup)?;
		capture_partials(backup, &mut save)?;
		return backup.expire();
	}
	if fs::exists(destination)? {
		return Err(fs::invalid(
			"Recovery destination occupied or changed; backup retained",
		));
	}
	if !backup.complete() {
		return Err(fs::invalid(
			"Incomplete central backup; original is unavailable",
		));
	}

	let work = backup.workspace.path();
	validate_workspace(&work, &backup.identity)?;
	let part = work.join("part");
	let expected = backup
		.backup
		.clone()
		.ok_or_else(|| fs::invalid("Missing backup"))?;
	if !fs::same_contents(&fs::snapshot(&part)?, &expected, true) {
		return Err(fs::invalid("Central backup changed; preserving it"));
	}
	if backup.restore_workspace.is_none() {
		let parent = destination
			.parent()
			.ok_or_else(|| fs::invalid("Missing restore parent"))?;
		check_capacity(parent, &expected)?;
		backup.restore_storage_identity = Some(directory_identity(parent)?);
		let (restore_work, identity) = new_workspace(parent, "central-restore")?;
		backup.restore_workspace = Some(StoredPath::new(&restore_work));
		backup.restore_identity = Some(identity);
		backup.phase = Phase::Restoring;
		save(backup)?;
	}
	let restore_work = backup
		.restore_workspace
		.as_ref()
		.ok_or_else(|| fs::invalid("Missing restore workspace"))?
		.path();
	let identity = backup
		.restore_identity
		.as_ref()
		.ok_or_else(|| fs::invalid("Missing restore identity"))?;
	validate_workspace(&restore_work, identity)?;
	let stage = restore_work.join("part");
	// Resume a fully recorded copy without changing its identity. Otherwise record
	// the interrupted copy once, and keep that manifest through deletion retries.
	let ready = if let Some(snapshot) = &backup.restore_snapshot {
		!fs::removal_pending(&stage, snapshot)?
			&& fs::exists(&stage)?
			&& fs::same_contents(&fs::snapshot(&stage)?, snapshot, true)
	} else {
		false
	};
	if !ready {
		capture_partials(backup, &mut save)?;
		if let Some(snapshot) = &backup.restore_partial {
			remove_if_present(&stage, snapshot)?;
		}
		backup.restore_partial = None;
		backup.restore_snapshot = None;
		backup.phase = Phase::Restoring;
		save(backup)?;
		let copied = worker::copy_tree(
			&part,
			&stage,
			&expected,
			true,
			&AtomicBool::new(false),
			&mut |_| {},
		);
		if fs::exists(&stage)? {
			backup.restore_partial = Some(fs::snapshot(&stage)?);
			save(backup)?;
		}
		copied?;
		let snapshot = backup
			.restore_partial
			.as_ref()
			.ok_or_else(|| fs::invalid("Missing restored copy"))?;
		if !fs::same_contents(snapshot, &expected, false) {
			return Err(fs::invalid("Central restore verification failed"));
		}
		fs::sync_tree(&stage, snapshot)?;
		fs::sync_parent(&stage)?;
		backup.restore_snapshot = Some(snapshot.clone());
	}
	backup.phase = Phase::Publishing;
	save(backup)?;
	fs::rename_new(&stage, destination)?;
	fs::sync_parent(&stage)?;
	fs::sync_parent(destination)?;
	if !backup.restored(destination)? {
		return Err(fs::invalid("Restored destination changed; backup retained"));
	}
	backup.phase = Phase::Restored;
	save(backup)?;
	backup.expire()
}

/// Only restoration may adopt an interrupted private copy, because its callback
/// makes the adopted manifest persistent before the first destructive operation.
fn capture_partials(
	backup: &mut Backup, save: &mut impl FnMut(&Backup) -> io::Result<()>,
) -> io::Result<()> {
	let work = backup.workspace.path();
	if backup.partial.is_none() && fs::exists(&work)? {
		validate_workspace(&work, &backup.identity)?;
		let part = work.join("part");
		if fs::exists(&part)? {
			backup.partial = Some(fs::snapshot(&part)?);
			save(backup)?;
		}
	}
	if backup.restore_partial.is_none() {
		if let (Some(work), Some(identity)) = (&backup.restore_workspace, &backup.restore_identity)
		{
			let work = work.path();
			if fs::exists(&work)? {
				validate_workspace(&work, identity)?;
				let part = work.join("part");
				if let Some(snapshot) = &backup.restore_snapshot {
					backup.restore_partial = Some(snapshot.clone());
					save(backup)?;
				} else if fs::exists(&part)? {
					backup.restore_partial = Some(fs::snapshot(&part)?);
					save(backup)?;
				}
			}
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::test_support::TestDir;

	struct Fixture {
		_dir: TestDir,
		root: PathBuf,
		destination: PathBuf,
		original: Snapshot,
	}

	impl Fixture {
		fn new() -> Self {
			let dir = TestDir::new();
			let root = dir.path().join("central");
			fs::private_directory(&root).unwrap();
			let destination = dir.path().join("destination");
			stdfs::create_dir(&destination).unwrap();
			stdfs::write(destination.join("first"), "old data").unwrap();
			stdfs::write(destination.join("second"), "more data").unwrap();
			let original = fs::snapshot(&destination).unwrap();
			Self {
				_dir: dir,
				root,
				destination,
				original,
			}
		}

		fn preserve(&self) -> Backup {
			preserve(
				&self.root,
				&self.destination,
				&self.original,
				&AtomicBool::new(false),
				|_| Ok(()),
			)
			.unwrap()
		}

		fn remove_original(&self) {
			fs::remove_recorded(&self.destination, &self.original).unwrap();
		}
	}

	#[test]
	fn missing_nested_root_is_created_private_without_changing_existing_parents() {
		let mut fixture = Fixture::new();
		let parent = fixture.root.clone();
		let before = stdfs::symlink_metadata(&parent).unwrap();
		fixture.root = parent.join("missing/data/recovery");

		let backup = fixture.preserve();

		for path in [
			parent.join("missing"),
			parent.join("missing/data"),
			fixture.root.clone(),
		] {
			let metadata = stdfs::symlink_metadata(path).unwrap();
			assert_eq!(metadata.mode() & 0o777, 0o700);
			assert_eq!(metadata.uid(), unsafe { libc::getuid() });
		}
		let after = stdfs::symlink_metadata(parent).unwrap();
		assert_eq!(
			(before.mode(), before.uid(), before.ino()),
			(after.mode(), after.uid(), after.ino())
		);
		assert!(backup.complete());
	}

	#[test]
	fn central_root_rejects_symlink_parents_and_does_not_repair_existing_permissions() {
		use std::os::unix::fs::PermissionsExt;
		let mut fixture = Fixture::new();
		let real = fixture.root.clone();
		let alias = real.with_file_name("alias");
		std::os::unix::fs::symlink(&real, &alias).unwrap();
		fixture.root = alias.join("must-not-create");

		assert!(
			preserve(
				&fixture.root,
				&fixture.destination,
				&fixture.original,
				&AtomicBool::new(false),
				|_| Ok(())
			)
			.is_err()
		);
		assert!(!real.join("must-not-create").exists());

		stdfs::set_permissions(&real, stdfs::Permissions::from_mode(0o755)).unwrap();
		fixture.root = real.clone();
		assert!(
			preserve(
				&fixture.root,
				&fixture.destination,
				&fixture.original,
				&AtomicBool::new(false),
				|_| Ok(())
			)
			.is_err()
		);
		assert_eq!(stdfs::symlink_metadata(real).unwrap().mode() & 0o777, 0o755);
		fs::unchanged(&fixture.destination, &fixture.original).unwrap();
	}

	#[test]
	fn restored_accounting_clears_only_after_cleanup_on_verified_storage() {
		let fixture = Fixture::new();
		let mut backup = fixture.preserve();
		backup.phase = Phase::Restored;
		assert_eq!(backup.retained_bytes(), 17);
		backup.expire().unwrap();
		assert_eq!(backup.retained_bytes(), 0);

		fs::rename_new(&fixture.root, &fixture.root.with_file_name("offline")).unwrap();
		fs::private_directory(&fixture.root).unwrap();

		assert_eq!(backup.retained_bytes(), 17);
		assert!(backup.expire().is_err());
	}

	#[test]
	fn durable_backup_restores_tree_without_removing_original_itself() {
		let fixture = Fixture::new();
		let mut backup = fixture.preserve();
		assert!(backup.complete());
		assert_eq!(backup.retained_bytes(), 17);
		fs::unchanged(&fixture.destination, &fixture.original).unwrap();
		fixture.remove_original();

		restore(&mut backup, &fixture.destination, &fixture.original, |_| {
			Ok(())
		})
		.unwrap();

		assert!(backup.restored(&fixture.destination).unwrap());
		assert!(fs::same_contents(
			&fs::snapshot(&fixture.destination).unwrap(),
			&fixture.original,
			false
		));
		assert!(!backup.workspace.path().exists());
		assert_eq!(backup.retained_bytes(), 0);
		// Cleanup remains idempotent after all workspaces have disappeared.
		restore(&mut backup, &fixture.destination, &fixture.original, |_| {
			Ok(())
		})
		.unwrap();
	}

	#[test]
	fn cancellation_after_initial_journal_leaves_original_untouched() {
		let fixture = Fixture::new();
		let cancelled = AtomicBool::new(false);
		let mut saved = None;
		let result = preserve(
			&fixture.root,
			&fixture.destination,
			&fixture.original,
			&cancelled,
			|backup| {
				assert!(!backup.workspace.path().join("part").exists());
				saved = Some(backup.clone());
				cancelled.store(true, Ordering::Release);
				Ok(())
			},
		);

		assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
		fs::unchanged(&fixture.destination, &fixture.original).unwrap();
		let mut backup = saved.unwrap();
		assert!(!backup.complete());
		assert!(!backup.restored(&fixture.destination).unwrap());
		restore(&mut backup, &fixture.destination, &fixture.original, |_| {
			Ok(())
		})
		.unwrap();
	}

	#[test]
	fn occupied_destination_keeps_both_copies() {
		let fixture = Fixture::new();
		let mut backup = fixture.preserve();
		fixture.remove_original();
		stdfs::write(&fixture.destination, "new occupant").unwrap();

		assert!(
			restore(
				&mut backup,
				&fixture.destination,
				&fixture.original,
				|_| Ok(())
			)
			.is_err()
		);
		assert_eq!(stdfs::read(&fixture.destination).unwrap(), b"new occupant");
		assert!(backup.workspace.path().join("part/first").exists());
	}

	#[test]
	fn publication_before_result_journal_is_recognized_by_identity() {
		let fixture = Fixture::new();
		let mut backup = fixture.preserve();
		fixture.remove_original();
		let mut saved = None;
		let result = restore(
			&mut backup,
			&fixture.destination,
			&fixture.original,
			|record| {
				saved = Some(record.clone());
				if record.phase == Phase::Publishing {
					Err(fs::invalid("simulated crash"))
				} else {
					Ok(())
				}
			},
		);
		assert!(result.is_err());
		let mut restarted: Backup =
			serde_json::from_slice(&serde_json::to_vec(&saved.unwrap()).unwrap()).unwrap();
		let stage = restarted
			.restore_workspace
			.as_ref()
			.unwrap()
			.path()
			.join("part");
		fs::rename_new(&stage, &fixture.destination).unwrap();

		restore(
			&mut restarted,
			&fixture.destination,
			&fixture.original,
			|_| Ok(()),
		)
		.unwrap();

		assert!(restarted.restored(&fixture.destination).unwrap());
		assert!(!restarted.workspace.path().exists());
	}

	#[test]
	fn changed_backup_refuses_restore_and_expiry() {
		let fixture = Fixture::new();
		let mut backup = fixture.preserve();
		fixture.remove_original();
		let changed = backup.workspace.path().join("part/first");
		stdfs::write(&changed, "changed").unwrap();

		assert!(
			restore(
				&mut backup,
				&fixture.destination,
				&fixture.original,
				|_| Ok(())
			)
			.is_err()
		);
		assert!(backup.expire().is_err());
		assert_eq!(stdfs::read(&changed).unwrap(), b"changed");
		assert!(!fixture.destination.exists());
	}

	#[test]
	fn cleanup_retries_use_original_manifest_for_quarantine() {
		let fixture = Fixture::new();
		let backup = fixture.preserve();
		let part = backup.workspace.path().join("part");
		let quarantine = fs::removal_directory(&part, backup.partial.as_ref().unwrap()).unwrap();
		fs::private_directory(&quarantine).unwrap();
		fs::rename_new(&part, &quarantine.join("data")).unwrap();
		stdfs::remove_file(quarantine.join("data/first")).unwrap();

		backup.expire().unwrap();
		backup.expire().unwrap();

		fs::unchanged(&fixture.destination, &fixture.original).unwrap();
		assert!(!backup.workspace.path().exists());
	}

	#[test]
	fn unrecorded_partial_requires_journaled_restore_before_cleanup() {
		let fixture = Fixture::new();
		let mut backup = fixture.preserve();
		backup.partial = None;
		backup.backup = None;
		backup.phase = Phase::Copying;
		assert!(backup.expire().is_err());
		let mut adopted = false;

		restore(
			&mut backup,
			&fixture.destination,
			&fixture.original,
			|record| {
				adopted |= record.partial.is_some();
				Ok(())
			},
		)
		.unwrap();

		assert!(adopted);
		fs::unchanged(&fixture.destination, &fixture.original).unwrap();
	}

	#[test]
	fn interrupted_original_deletion_is_settled_before_restore() {
		let fixture = Fixture::new();
		let mut backup = fixture.preserve();
		let quarantine = fs::removal_directory(&fixture.destination, &fixture.original).unwrap();
		fs::private_directory(&quarantine).unwrap();
		fs::rename_new(&fixture.destination, &quarantine.join("data")).unwrap();
		stdfs::remove_file(quarantine.join("data/first")).unwrap();

		restore(&mut backup, &fixture.destination, &fixture.original, |_| {
			Ok(())
		})
		.unwrap();

		assert!(!quarantine.exists());
		assert!(backup.restored(&fixture.destination).unwrap());
	}

	#[test]
	fn interrupted_restore_copy_is_journaled_before_discard_and_retried() {
		let fixture = Fixture::new();
		let mut backup = fixture.preserve();
		fixture.remove_original();
		let parent = fixture.destination.parent().unwrap();
		let (work, identity) = new_workspace(parent, "central-restore").unwrap();
		backup.restore_storage_identity = Some(directory_identity(parent).unwrap());
		backup.restore_workspace = Some(StoredPath::new(&work));
		backup.restore_identity = Some(identity);
		backup.phase = Phase::Restoring;
		stdfs::create_dir(work.join("part")).unwrap();
		stdfs::write(work.join("part/unfinished"), "partial").unwrap();
		let mut saw_manifest = false;

		restore(
			&mut backup,
			&fixture.destination,
			&fixture.original,
			|record| {
				if record.restore_partial.as_ref().is_some_and(|snapshot| {
					snapshot
						.0
						.iter()
						.any(|entry| entry.relative.path() == Path::new("unfinished"))
				}) {
					assert!(work.join("part/unfinished").exists());
					saw_manifest = true;
				}
				Ok(())
			},
		)
		.unwrap();

		assert!(saw_manifest);
		assert!(backup.restored(&fixture.destination).unwrap());
	}

	#[test]
	fn missing_workspace_on_replaced_storage_is_not_expired() {
		let fixture = Fixture::new();
		let backup = fixture.preserve();
		let relocated = fixture.root.with_file_name("relocated");
		fs::rename_new(&fixture.root, &relocated).unwrap();
		fs::private_directory(&fixture.root).unwrap();

		assert!(backup.expire().is_err());
		assert!(
			relocated
				.join(backup.workspace.path().file_name().unwrap())
				.join("part/first")
				.exists()
		);
	}

	#[test]
	fn volatile_central_storage_is_rejected_before_original_data_can_be_removed() {
		let fixture = Fixture::new();
		let shm = Path::new("/dev/shm");
		let Ok(metadata) = stdfs::symlink_metadata(shm) else {
			return;
		};
		if !metadata.is_dir() || metadata.dev() == stdfs::metadata(&fixture.root).unwrap().dev() {
			return;
		}
		let central = TestDir::new_in(shm);
		let root = central.path().join("private");
		fs::private_directory(&root).unwrap();
		let result = preserve(
			&root,
			&fixture.destination,
			&fixture.original,
			&AtomicBool::new(false),
			|_| Ok(()),
		);

		assert!(result.unwrap_err().to_string().contains("volatile"));
		fs::unchanged(&fixture.destination, &fixture.original).unwrap();
	}
}

fn check_cancel(cancelled: &AtomicBool) -> io::Result<()> {
	if cancelled.load(Ordering::Acquire) {
		return Err(io::ErrorKind::Interrupted.into());
	}
	Ok(())
}

/// Walk pinned directory handles from `/`, never following symlinks. Existing
/// directories retain their ownership and permissions; only missing ones are
/// created. Sync both each new directory and its parent before using the root.
fn ensure_private_root(root: &Path) -> io::Result<()> {
	if !root.is_absolute() {
		return Err(fs::invalid("Expected an absolute central recovery root"));
	}
	let mut parent = File::open("/")?;
	for component in root.components() {
		let name = match component {
			Component::RootDir => continue,
			Component::Normal(name) => CString::new(name.as_bytes())?,
			_ => return Err(fs::invalid("Noncanonical central recovery root")),
		};
		let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
		let mut descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
		let mut created = false;
		if descriptor < 0 {
			let error = io::Error::last_os_error();
			if error.kind() != io::ErrorKind::NotFound {
				return Err(error);
			}
			if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
				let error = io::Error::last_os_error();
				if error.kind() != io::ErrorKind::AlreadyExists {
					return Err(error);
				}
			} else {
				created = true;
			}
			descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
			if descriptor < 0 {
				return Err(io::Error::last_os_error());
			}
		}
		let directory = unsafe { File::from_raw_fd(descriptor) };
		if created {
			directory.sync_all()?;
			parent.sync_all()?;
		}
		parent = directory;
	}
	let metadata = parent.metadata()?;
	if metadata.uid() != unsafe { libc::getuid() } || metadata.mode() & 0o077 != 0 {
		return Err(fs::invalid(
			"Unsafe central recovery root ownership or permissions",
		));
	}
	// Detect replacement between walking the parent handles and path-based work.
	fs::validate_private(root)?;
	let current = stdfs::symlink_metadata(root)?;
	if (current.dev(), current.ino()) != (metadata.dev(), metadata.ino()) {
		return Err(fs::invalid("Central recovery root changed during creation"));
	}
	Ok(())
}

fn new_workspace(parent: &Path, label: &str) -> io::Result<(PathBuf, Entry)> {
	let work = parent.join(format!(".fileberg-{label}-{}", journal::new_id()?));
	fs::private_directory(&work)?;
	fs::sync_parent(&work)?;
	let identity = fs::snapshot(&work)?
		.0
		.into_iter()
		.next()
		.ok_or_else(|| fs::invalid("Missing workspace identity"))?;
	Ok((work, identity))
}

fn validate_workspace(work: &Path, identity: &Entry) -> io::Result<()> {
	fs::validate_private(work)?;
	let metadata = stdfs::symlink_metadata(work)?;
	if identity.kind != Kind::Directory
		|| (metadata.dev(), metadata.ino(), metadata.mode())
			!= (identity.device, identity.inode, identity.mode)
	{
		return Err(fs::invalid(
			"Central recovery filesystem/workspace identity changed",
		));
	}
	Ok(())
}

fn remove_if_present(path: &Path, expected: &Snapshot) -> io::Result<()> {
	if fs::exists(path)? || fs::removal_pending(path, expected)? {
		fs::remove_recorded(path, expected)?;
	}
	Ok(())
}

fn cleanup_workspace(
	work: &Path, identity: &Entry, storage_identity: &Entry, partial: Option<&Snapshot>,
) -> io::Result<()> {
	if workspace_absent_on_storage(work, storage_identity)? {
		return Ok(());
	}
	validate_workspace(work, identity)?;
	let part = work.join("part");
	if let Some(snapshot) = partial {
		remove_if_present(&part, snapshot)?;
	} else if fs::exists(&part)? {
		return Err(fs::invalid(
			"Unrecorded central partial retained for journaled recovery",
		));
	}
	// Unknown files and quarantines prevent removing the directory. Never recurse.
	stdfs::remove_dir(work)?;
	fs::sync_parent(work)
}

fn workspace_absent_on_storage(work: &Path, storage_identity: &Entry) -> io::Result<bool> {
	let parent = work
		.parent()
		.ok_or_else(|| fs::invalid("Missing workspace parent"))?;
	let current_storage = directory_identity(parent)?;
	if (current_storage.device, current_storage.inode)
		!= (storage_identity.device, storage_identity.inode)
	{
		return Err(fs::invalid(
			"Central recovery storage is offline or changed",
		));
	}
	Ok(!fs::exists(work)?)
}

fn directory_identity(path: &Path) -> io::Result<Entry> {
	let metadata = stdfs::symlink_metadata(path)?;
	if !metadata.is_dir() {
		return Err(fs::invalid("Recovery storage parent is not a directory"));
	}
	Ok(Entry {
		relative: StoredPath::new(Path::new("")),
		device: metadata.dev(),
		inode: metadata.ino(),
		kind: Kind::Directory,
		mode: metadata.mode(),
		length: metadata.len(),
		modified: (metadata.mtime(), metadata.mtime_nsec()),
		changed: (metadata.ctime(), metadata.ctime_nsec()),
		digest: Vec::new(),
	})
}

fn check_capacity(root: &Path, snapshot: &Snapshot) -> io::Result<()> {
	let path = CString::new(root.as_os_str().as_bytes())?;
	let mut capacity = std::mem::MaybeUninit::<libc::statvfs>::uninit();
	if unsafe { libc::statvfs(path.as_ptr(), capacity.as_mut_ptr()) } != 0 {
		return Err(io::Error::last_os_error());
	}
	let capacity = unsafe { capacity.assume_init() };
	let block = u128::from(capacity.f_frsize)
		.max(u128::from(capacity.f_bsize))
		.max(4096);
	let available = u128::from(capacity.f_bavail).saturating_mul(u128::from(capacity.f_frsize));
	// Account for allocation rounding, directory/symlink metadata, and a margin.
	// This is a preflight only: concurrent allocations and quotas can still fail.
	let required = snapshot.0.iter().fold(1024 * 1024u128, |bytes, entry| {
		let data = if entry.kind == Kind::File {
			u128::from(entry.length).div_ceil(block) * block
		} else {
			0
		};
		bytes.saturating_add(data).saturating_add(block)
	});
	if required > available {
		return Err(io::Error::new(
			io::ErrorKind::StorageFull,
			"Insufficient space for central recovery copy",
		));
	}
	Ok(())
}
