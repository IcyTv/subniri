//! Bounded replacement recovery. Cleanup never owns published destination data.
use super::{
	Store, fs,
	model::{Change, Entry, Journal, Kind, Snapshot, Status},
};
use std::{
	collections::HashSet,
	fs as stdfs, io,
	os::unix::fs::MetadataExt,
	path::{Component, Path, PathBuf},
};

const MAX_AGE: u64 = 7 * 24 * 60 * 60;
const MAX_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Expire old history and oldest completed replacement backups over budget.
/// Deferred cleanup is returned to the caller, never silently forgotten.
pub(crate) fn expire(store: &Store, now: u64, budget: u64) -> io::Result<Vec<String>> {
	let mut records = store.history()?;
	records.sort_by_key(|record| (record.created, record.id.clone()));
	let mut problems = Vec::new();
	let mut removed = HashSet::new();
	for record in &mut records {
		if record.status == Status::Expiring
			|| (eligible(record) && now.saturating_sub(record.created) >= MAX_AGE)
		{
			match cleanup(store, record) {
				Ok(()) => {
					removed.insert(record.id.clone());
				}
				Err(error) => problems.push(format!("{}: retention deferred: {error}", record.id)),
			}
		}
	}

	let mut total = 0u64;
	let mut sizes = Vec::new();
	for record in &records {
		if removed.contains(&record.id) {
			continue;
		}
		let (bytes, warnings) = retained_bytes(record);
		let bytes = bytes.saturating_sub(record.approved_recovery_bytes);
		total = total.saturating_add(bytes);
		problems.extend(warnings);
		sizes.push((record.id.clone(), bytes));
	}
	for record in &mut records {
		if total <= budget.min(MAX_BYTES) {
			break;
		}
		if removed.contains(&record.id) || !eligible(record) {
			continue;
		}
		let bytes = sizes
			.iter()
			.find(|(id, _)| id == &record.id)
			.map_or(0, |(_, bytes)| *bytes);
		// Ordinary inverse-operation history consumes no replacement budget.
		if bytes == 0 {
			continue;
		}
		match cleanup(store, record) {
			Ok(()) => {
				total = total.saturating_sub(bytes);
			}
			Err(error) => problems.push(format!("{}: retention deferred: {error}", record.id)),
		}
	}
	if total > budget.min(MAX_BYTES) {
		problems.push(format!(
			"Recovery storage retains {total} bytes; budget is {} bytes",
			budget.min(MAX_BYTES)
		));
	}

	Ok(problems)
}

/// A larger recovery budget requires approval; rejection never starts replacement.
pub(crate) fn reserve(store: &Store, additional: u64) -> io::Result<()> {
	// An impossible reservation must not evict otherwise valid recovery history.
	let budget = if additional > MAX_BYTES {
		MAX_BYTES
	} else {
		MAX_BYTES - additional
	};
	let problems = expire(store, super::journal::now()?, budget)?;
	let total = store.history()?.iter().fold(0u64, |total, record| {
		total.saturating_add(
			retained_bytes(record)
				.0
				.saturating_sub(record.approved_recovery_bytes),
		)
	});
	if additional > MAX_BYTES || total.saturating_add(additional) > MAX_BYTES {
		return Err(io::Error::new(
			io::ErrorKind::QuotaExceeded,
			format!(
				"Replacement recovery exceeds the 5 GiB limit; a larger recovery budget requires approval. Replacement was not started. {}",
				problems.join("; ")
			),
		));
	}

	Ok(())
}

fn eligible(record: &Journal) -> bool {
	record.status == Status::RolledBack
		|| (record.status == Status::Complete && record.problems.is_empty())
}

fn file_bytes(snapshot: &Snapshot) -> u64 {
	snapshot
		.0
		.iter()
		.filter(|entry| entry.kind == Kind::File)
		.fold(0u64, |bytes, entry| bytes.saturating_add(entry.length))
}

fn retained_bytes(record: &Journal) -> (u64, Vec<String>) {
	let mut total = 0u64;
	let mut warnings = Vec::new();
	for change in &record.changes {
		if let Some(central) = &change.central {
			total = total.saturating_add(central.retained_bytes());
			continue;
		}
		let Some(expected) = &change.replaced else {
			continue;
		};
		let result = (|| {
			let root = workspace(change)?;
			if !validate_workspace(change, &root, record.status == Status::Expiring)? {
				return Ok(0);
			}
			let backup = change.backup.path();
			if fs::removal_pending(&backup, expected)? {
				fs::validate_private(&fs::removal_directory(&backup, expected)?)?;
				return Ok(file_bytes(expected));
			}
			if !fs::exists(&backup)? {
				if record.status == Status::Complete {
					return Err(fs::invalid(
						"Completed replacement backup is unexpectedly missing",
					));
				}
				return Ok(0);
			}
			let actual = fs::snapshot(&backup)?;
			if !matches_remaining(&actual, expected, record.status == Status::Expiring) {
				return Err(fs::invalid("Backup identity or contents changed"));
			}
			Ok(file_bytes(&actual))
		})();
		match result {
			Ok(bytes) => total = total.saturating_add(bytes),
			Err(error) => {
				// Missing/offline media is not evidence that recovery bytes vanished.
				total = total.saturating_add(file_bytes(expected));
				warnings.push(format!(
					"{}: recovery accounting deferred: {error}",
					record.id
				));
			}
		}
	}

	(total, warnings)
}

fn workspace(change: &Change) -> io::Result<PathBuf> {
	if change.empty_directory {
		return Err(fs::invalid("Empty-source change has no private workspace"));
	}
	let stage = change.stage.path();
	let root = stage
		.parent()
		.ok_or_else(|| fs::invalid("Missing workspace parent"))?
		.to_path_buf();
	if stage.file_name().is_none_or(|name| name != "part")
		|| change.backup.path() != root.join("replaced")
		|| (change.undo_workspace.is_none() && change.undo_stage.path() != root.join("undo"))
		|| root == change.source.path()
		|| root == change.destination.path()
		|| root
			.file_name()
			.is_none_or(|name| !name.as_encoded_bytes().starts_with(b".fileberg-transfer-"))
		|| change.workspace.kind != Kind::Directory
	{
		return Err(fs::invalid("Invalid private workspace paths"));
	}

	Ok(root)
}

/// Check every path component without following symlink ancestors. The parent
/// must still be on the recorded device even when cleanup already removed root.
fn validate_workspace(change: &Change, root: &Path, allow_missing: bool) -> io::Result<bool> {
	validate_workspace_identity(&change.workspace, root, allow_missing)
}

fn validate_workspace_identity(
	expected: &Entry, root: &Path, allow_missing: bool,
) -> io::Result<bool> {
	if !root.is_absolute() {
		return Err(fs::invalid("Workspace path is not absolute"));
	}
	let parent = root
		.parent()
		.ok_or_else(|| fs::invalid("Missing workspace parent"))?;
	let mut path = PathBuf::new();
	for component in parent.components() {
		if !matches!(component, Component::RootDir | Component::Normal(_)) {
			return Err(fs::invalid("Unsafe workspace ancestor"));
		}
		path.push(component);
		let metadata = stdfs::symlink_metadata(&path)?;
		if !metadata.is_dir() {
			return Err(fs::invalid("Workspace ancestor is not a directory"));
		}
	}
	if stdfs::symlink_metadata(parent)?.dev() != expected.device {
		return Err(fs::invalid("Workspace storage is offline or changed"));
	}
	match stdfs::symlink_metadata(root) {
		Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => return Ok(false),
		Err(error) => return Err(error),
		Ok(metadata) => {
			fs::validate_private(root)?;
			if expected.kind != Kind::Directory
				|| (metadata.dev(), metadata.ino()) != (expected.device, expected.inode)
				|| metadata.mode() & 0o7777 != expected.mode & 0o7777
			{
				return Err(fs::invalid("Private workspace identity changed"));
			}
		}
	}

	Ok(true)
}

fn matches_remaining(actual: &Snapshot, expected: &Snapshot, partial: bool) -> bool {
	if expected.0.is_empty()
		|| expected.0.iter().any(|entry| {
			entry
				.relative
				.path()
				.components()
				.any(|component| !matches!(component, Component::Normal(_)))
		}) {
		return false;
	}
	if !partial {
		return fs::same_contents(actual, expected, true);
	}
	let remaining = Snapshot(
		expected
			.0
			.iter()
			.filter(|entry| {
				actual
					.0
					.iter()
					.any(|current| current.relative == entry.relative)
			})
			.cloned()
			.collect(),
	);
	fs::same_contents(actual, &remaining, true)
}

fn cleanup(store: &Store, record: &mut Journal) -> io::Result<()> {
	let retry = record.status == Status::Expiring;
	if !retry && !eligible(record) {
		return Err(fs::invalid("Cannot expire unfinished or failed recovery"));
	}
	// Preflight every workspace before making undo unavailable.
	for change in &record.changes {
		preflight(change, retry, record.status)?;
	}

	if !retry {
		record.status = Status::Expiring;
		store.save(record)?;
	}
	for change in &record.changes {
		if change.empty_directory {
			continue;
		}
		let root = workspace(change)?;
		// Central recovery has independent storage identity and cleanup progress.
		// A previously removed local workspace must not orphan its backup.
		if let Some(central) = &change.central {
			central.expire()?;
		}
		if !validate_workspace(change, &root, true)? {
			continue;
		}
		preflight(change, true, Status::Expiring)?;
		if let Some(expected) = &change.replaced {
			let backup = change.backup.path();
			if fs::exists(&backup)? || fs::removal_pending(&backup, expected)? {
				// The original manifest also determines the persistent quarantine key.
				fs::remove_recorded(&backup, expected)?;
				fs::sync_parent(&backup)?;
			}
		}
		validate_workspace(change, &root, false)?;
		// Never recurse: unexpected children must preserve workspace and record.
		stdfs::remove_dir(&root)?;
		fs::sync_parent(&root)?;
	}

	store.forget(record)
}

fn preflight(change: &Change, retry: bool, status: Status) -> io::Result<()> {
	if let Some(identity) = &change.undo_workspace {
		let stage = change.undo_stage.path();
		let inverse = stage
			.parent()
			.ok_or_else(|| fs::invalid("Missing inverse workspace"))?;
		if stage.file_name().is_none_or(|name| name != "part")
			|| inverse
				.file_name()
				.is_none_or(|name| !name.as_encoded_bytes().starts_with(b".fileberg-undo-"))
		{
			return Err(fs::invalid("Invalid inverse workspace path"));
		}
		if validate_workspace_identity(identity, inverse, true)? {
			if stdfs::read_dir(inverse)?.next().is_some() {
				return Err(fs::invalid(
					"Inverse workspace still contains recovery data",
				));
			}
			return Err(fs::invalid("Empty inverse workspace awaits owned cleanup"));
		}
	}
	if change.source_quarantine.is_some() || change.quarantine_workspace.is_some() {
		return Err(fs::invalid(
			"Quarantine cleanup requires its owning recovery operation",
		));
	}
	if change.empty_directory {
		return Ok(());
	}
	let root = workspace(change)?;
	if !validate_workspace(change, &root, retry)? {
		return Ok(());
	}
	let backup = change.backup.path();
	let removal = change
		.replaced
		.as_ref()
		.map(|expected| fs::removal_directory(&backup, expected))
		.transpose()?;
	let pending = change
		.replaced
		.as_ref()
		.map(|expected| fs::removal_pending(&backup, expected))
		.transpose()?
		.unwrap_or(false);
	if status == Status::Complete
		&& change.central.is_none()
		&& change.replaced.is_some()
		&& !fs::exists(&backup)?
		&& !pending
	{
		return Err(fs::invalid(
			"Completed replacement backup is unexpectedly missing",
		));
	}
	for child in stdfs::read_dir(&root)? {
		let path = child?.path();
		let owned_removal = removal.as_ref().is_some_and(|removal| *removal == path);
		if (path != backup && !owned_removal) || change.replaced.is_none() {
			return Err(fs::invalid(
				"Unrecorded, staged, or undo data remains in workspace",
			));
		}
		if status == Status::RolledBack {
			return Err(fs::invalid(
				"Rolled-back replacement still has recovery data",
			));
		}
		if owned_removal {
			fs::validate_private(&path)?;
			continue;
		}
		let actual = fs::snapshot(&path)?;
		let expected = change
			.replaced
			.as_ref()
			.ok_or_else(|| fs::invalid("Unrecorded backup"))?;
		if !matches_remaining(&actual, expected, retry) {
			return Err(fs::invalid(
				"Backup identity, contents, or children changed",
			));
		}
	}

	Ok(())
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::super::model::{Durability, Operation, Step, StoredPath};
	use super::*;
	use crate::test_support::TestDir;
	use std::os::unix::fs::{PermissionsExt, symlink};

	fn fixture(root: &TestDir, number: u64, replaced: bool) -> Journal {
		let id = format!("{number:032x}");
		let work = root.path().join(format!(".fileberg-transfer-{id}-0"));
		fs::private_directory(&work).unwrap();
		let workspace = fs::snapshot(&work).unwrap().0.remove(0);
		let destination = root.path().join(format!("live-{number}"));
		stdfs::write(&destination, b"live destination").unwrap();
		let published = fs::snapshot(&destination).unwrap();
		let backup = work.join("replaced");
		let old = if replaced {
			stdfs::write(&backup, b"old").unwrap();
			Some(fs::snapshot(&backup).unwrap())
		} else {
			None
		};
		Journal {
			version: 1,
			id,
			created: number,
			status: Status::Complete,
			durability: Durability::Buffered,
			approved_recovery_bytes: 0,
			problems: Vec::new(),
			changes: vec![Change {
				source: StoredPath::new(&root.path().join(format!("source-{number}"))),
				destination: StoredPath::new(&destination),
				operation: Operation::Copy,
				before: published.clone(),
				workspace,
				stage: StoredPath::new(&work.join("part")),
				staged: Some(published.clone()),
				backup: StoredPath::new(&backup),
				replaced: old,
				published: Some(published),
				source_removed: false,
				source_quarantine: None,
				quarantine_workspace: None,
				renamed: false,
				empty_directory: false,
				step: Step::Committed,
				undo_stage: StoredPath::new(&work.join("undo")),
				undo_snapshot: None,
				undo_workspace: None,
				undo_published: None,
				replacement_restored: false,
				central: None,
			}],
		}
	}

	#[test]
	fn approved_job_storage_survives_budget_cleanup_but_still_expires_by_age() {
		let root = TestDir::new();
		let store = Store::open(root.path().join("state")).unwrap();
		let mut record = fixture(&root, 1, true);
		record.approved_recovery_bytes = 3;
		store.save(&record).unwrap();

		assert!(expire(&store, 2, 0).unwrap().is_empty());
		assert_eq!(store.history().unwrap().len(), 1);
		assert!(record.changes[0].backup.path().exists());

		assert!(expire(&store, MAX_AGE + 2, 0).unwrap().is_empty());
		assert!(store.history().unwrap().is_empty());
	}

	#[test]
	fn age_expiry_only_deletes_backup_and_empty_private_workspace() {
		let root = TestDir::new();
		let store = Store::open(root.path().join("state")).unwrap();
		let record = fixture(&root, 1, true);
		store.save(&record).unwrap();
		assert!(expire(&store, MAX_AGE, MAX_BYTES).unwrap().is_empty());
		assert_eq!(store.history().unwrap().len(), 1);
		assert!(expire(&store, MAX_AGE + 1, MAX_BYTES).unwrap().is_empty());
		assert!(store.history().unwrap().is_empty());
		assert_eq!(
			stdfs::read(record.changes[0].destination.path()).unwrap(),
			b"live destination"
		);
		assert!(!record.changes[0].stage.path().parent().unwrap().exists());
	}

	#[test]
	fn budget_evicts_oldest_replacement_not_ordinary_history() {
		let root = TestDir::new();
		let store = Store::open(root.path().join("state")).unwrap();
		let ordinary = fixture(&root, 1, false);
		let oldest = fixture(&root, 2, true);
		let newest = fixture(&root, 3, true);
		for record in [&ordinary, &oldest, &newest] {
			store.save(record).unwrap();
		}
		assert!(expire(&store, 4, 3).unwrap().is_empty());
		assert!(store.load(&oldest.id).is_err());
		assert!(store.load(&ordinary.id).is_ok());
		assert!(store.load(&newest.id).is_ok());
		assert!(expire(&store, MAX_AGE + 4, MAX_BYTES).unwrap().is_empty());
		assert!(store.history().unwrap().is_empty());
	}

	#[test]
	fn unfinished_history_and_rollback_with_surviving_data_never_expire() {
		let root = TestDir::new();
		let store = Store::open(root.path().join("state")).unwrap();
		for (index, status) in [
			Status::Running,
			Status::RollingBack,
			Status::NeedsRecovery,
			Status::RolledBack,
		]
		.into_iter()
		.enumerate()
		{
			let mut record = fixture(&root, index as u64 + 1, true);
			record.status = status;
			if status == Status::RolledBack {
				record.problems.push("failed transfer".into());
			}
			store.save(&record).unwrap();
		}
		assert!(!expire(&store, MAX_AGE + 10, 0).unwrap().is_empty());
		assert_eq!(store.history().unwrap().len(), 4);
		assert!(
			store
				.history()
				.unwrap()
				.iter()
				.all(|record| record.changes[0].backup.path().exists())
		);
	}

	#[test]
	fn unrecorded_children_and_changed_backups_preserve_records() {
		let root = TestDir::new();
		let store = Store::open(root.path().join("state")).unwrap();
		let extra = fixture(&root, 1, true);
		let changed = fixture(&root, 2, true);
		stdfs::write(extra.changes[0].stage.path(), b"unrecorded").unwrap();
		stdfs::write(changed.changes[0].backup.path(), b"edited").unwrap();
		store.save(&extra).unwrap();
		store.save(&changed).unwrap();
		assert!(!expire(&store, MAX_AGE + 3, 0).unwrap().is_empty());
		assert_eq!(store.load(&extra.id).unwrap().status, Status::Complete);
		assert_eq!(store.load(&changed.id).unwrap().status, Status::Complete);
		assert_eq!(
			stdfs::read(changed.changes[0].backup.path()).unwrap(),
			b"edited"
		);
	}

	#[test]
	fn restart_retries_partial_quarantined_backup_cleanup() {
		let root = TestDir::new();
		let state = root.path().join("state");
		let store = Store::open(state.clone()).unwrap();
		let mut record = fixture(&root, 1, false);
		let backup = record.changes[0].backup.path();
		stdfs::create_dir(&backup).unwrap();
		stdfs::write(backup.join("first"), b"first").unwrap();
		stdfs::write(backup.join("last"), b"last").unwrap();
		record.changes[0].replaced = Some(fs::snapshot(&backup).unwrap());
		record.status = Status::Expiring;
		store.save(&record).unwrap();
		let expected = record.changes[0].replaced.as_ref().unwrap();
		let removal = fs::removal_directory(&backup, expected).unwrap();
		fs::private_directory(&removal).unwrap();
		stdfs::rename(&backup, removal.join("data")).unwrap();
		stdfs::remove_file(removal.join("data/first")).unwrap();
		assert_eq!(retained_bytes(&record).0, 9);
		drop(store);

		let store = Store::open(state).unwrap();
		assert!(expire(&store, 2, MAX_BYTES).unwrap().is_empty());
		assert!(store.history().unwrap().is_empty());
		assert!(!removal.exists());
		assert_eq!(
			stdfs::read(record.changes[0].destination.path()).unwrap(),
			b"live destination"
		);
	}

	#[test]
	fn missing_or_replaced_workspace_defers_and_counts_conservatively() {
		let root = TestDir::new();
		let store = Store::open(root.path().join("state")).unwrap();
		let record = fixture(&root, 1, true);
		let work = workspace(&record.changes[0]).unwrap();
		let offline = root.path().join("offline");
		stdfs::rename(&work, &offline).unwrap();
		store.save(&record).unwrap();
		assert_eq!(retained_bytes(&record).0, 3);
		assert!(!expire(&store, MAX_AGE + 1, 0).unwrap().is_empty());
		symlink(&offline, &work).unwrap();
		assert!(!expire(&store, MAX_AGE + 1, 0).unwrap().is_empty());
		assert_eq!(stdfs::read(offline.join("replaced")).unwrap(), b"old");
		assert_eq!(store.load(&record.id).unwrap().status, Status::Complete);
	}

	#[test]
	fn changed_permissions_and_quarantine_defer_cleanup() {
		let root = TestDir::new();
		let store = Store::open(root.path().join("state")).unwrap();
		let permissions = fixture(&root, 1, true);
		let mut quarantine = fixture(&root, 2, true);
		stdfs::set_permissions(
			workspace(&permissions.changes[0]).unwrap(),
			stdfs::Permissions::from_mode(0o755),
		)
		.unwrap();
		quarantine.changes[0].source_quarantine =
			Some(StoredPath::new(&root.path().join("quarantine")));
		store.save(&permissions).unwrap();
		store.save(&quarantine).unwrap();
		assert!(!expire(&store, MAX_AGE + 3, 0).unwrap().is_empty());
		assert_eq!(store.history().unwrap().len(), 2);
	}

	#[test]
	fn unrecorded_backup_child_defers_without_removing_recorded_children() {
		let root = TestDir::new();
		let store = Store::open(root.path().join("state")).unwrap();
		let mut record = fixture(&root, 1, false);
		let backup = record.changes[0].backup.path();
		stdfs::create_dir(&backup).unwrap();
		stdfs::write(backup.join("recorded"), b"recorded").unwrap();
		record.changes[0].replaced = Some(fs::snapshot(&backup).unwrap());
		store.save(&record).unwrap();
		stdfs::write(backup.join("unexpected"), b"preserve").unwrap();

		assert!(!expire(&store, MAX_AGE + 1, 0).unwrap().is_empty());
		assert_eq!(stdfs::read(backup.join("recorded")).unwrap(), b"recorded");
		assert_eq!(stdfs::read(backup.join("unexpected")).unwrap(), b"preserve");
		assert_eq!(store.load(&record.id).unwrap().status, Status::Complete);
	}

	#[test]
	fn recorded_symlink_backup_never_removes_target_tree() {
		let root = TestDir::new();
		let store = Store::open(root.path().join("state")).unwrap();
		let mut record = fixture(&root, 1, false);
		let target = root.path().join("target");
		stdfs::create_dir(&target).unwrap();
		stdfs::write(target.join("live"), b"preserve").unwrap();
		let backup = record.changes[0].backup.path();
		symlink(&target, &backup).unwrap();
		record.changes[0].replaced = Some(fs::snapshot(&backup).unwrap());
		store.save(&record).unwrap();

		assert_eq!(retained_bytes(&record).0, 0);
		assert!(expire(&store, MAX_AGE + 1, MAX_BYTES).unwrap().is_empty());
		assert_eq!(stdfs::read(target.join("live")).unwrap(), b"preserve");
		assert!(store.history().unwrap().is_empty());
	}

	#[test]
	fn completed_rollback_with_original_error_expires_when_no_data_remains() {
		let root = TestDir::new();
		let store = Store::open(root.path().join("state")).unwrap();
		let mut record = fixture(&root, 1, true);
		record.status = Status::RolledBack;
		record.problems.push("original transfer failed".into());
		stdfs::remove_file(record.changes[0].backup.path()).unwrap();
		record.changes[0].replacement_restored = true;
		store.save(&record).unwrap();

		assert!(expire(&store, MAX_AGE + 1, MAX_BYTES).unwrap().is_empty());
		assert!(store.history().unwrap().is_empty());
		assert_eq!(
			stdfs::read(record.changes[0].destination.path()).unwrap(),
			b"live destination"
		);
	}

	#[test]
	fn separate_inverse_workspace_defers_until_owned_cleanup_finishes() {
		let root = TestDir::new();
		let store = Store::open(root.path().join("state")).unwrap();
		let mut record = fixture(&root, 1, false);
		record.status = Status::RolledBack;
		let inverse = root.path().join(".fileberg-undo-test");
		fs::private_directory(&inverse).unwrap();
		record.changes[0].undo_workspace = Some(fs::snapshot(&inverse).unwrap().0.remove(0));
		record.changes[0].undo_stage = StoredPath::new(&inverse.join("part"));
		stdfs::write(inverse.join("part"), b"recovery").unwrap();
		store.save(&record).unwrap();

		assert!(!expire(&store, MAX_AGE + 1, MAX_BYTES).unwrap().is_empty());
		assert_eq!(stdfs::read(inverse.join("part")).unwrap(), b"recovery");
		stdfs::remove_file(inverse.join("part")).unwrap();
		assert!(!expire(&store, MAX_AGE + 1, MAX_BYTES).unwrap().is_empty());
		stdfs::remove_dir(&inverse).unwrap();
		assert!(expire(&store, MAX_AGE + 1, MAX_BYTES).unwrap().is_empty());
		assert!(store.history().unwrap().is_empty());
	}

	#[test]
	fn oversized_reservation_requires_larger_budget_approval() {
		let root = TestDir::new();
		let store = Store::open(root.path().join("state")).unwrap();
		assert!(
			reserve(&store, MAX_BYTES + 1)
				.unwrap_err()
				.to_string()
				.contains("larger recovery budget requires approval. Replacement was not started.")
		);
		assert!(reserve(&store, MAX_BYTES).is_ok());
	}
}
