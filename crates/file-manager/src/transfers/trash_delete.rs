//! Confirmed permanent deletion owns a captured batch, never a live trash root.
//! Interrupted cleanup retains its manifest outside desktop trash for retry.
use super::{
	fs,
	journal::{self, Store},
	model::{Entry, Snapshot, StoredPath},
};
use crate::trash::{Captured, Identity};
use serde::{Deserialize, Serialize};
use std::{
	fs::{self as stdfs, OpenOptions},
	io::{self, Read, Write},
	os::unix::fs::OpenOptionsExt,
	path::{Path, PathBuf},
};

#[cfg(test)]
#[path = "trash_delete_tests.rs"]
mod tests;

#[derive(Debug, Clone)]
pub(crate) struct PendingDeletion {
	pub id: String,
	pub original: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Step {
	Prepared,
	Quarantined,
	PayloadDeleted,
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
	version: u32,
	id: String,
	original: StoredPath,
	info: StoredPath,
	contents: Snapshot,
	metadata: Option<Snapshot>,
	containers: Vec<(StoredPath, Identity)>,
	workspace: StoredPath,
	workspace_identity: Option<Entry>,
	step: Step,
}

fn record_path(store: &Store, id: &str) -> io::Result<PathBuf> {
	path_in(&store.deletions_root(), id)
}

fn path_in(root: &Path, id: &str) -> io::Result<PathBuf> {
	if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
		return Err(fs::invalid("Invalid trash deletion identifier"));
	}
	Ok(root.join(format!("{id}.json")))
}

fn save(store: &Store, record: &Record) -> io::Result<()> {
	let bytes = serde_json::to_vec(record)?;
	let limit = if record.workspace_identity.is_none() {
		48
	} else {
		64
	} * 1024
		* 1024;
	if bytes.len() > limit {
		return Err(fs::invalid(
			"Deletion manifest exceeds the supported safety limit; remaining data was preserved",
		));
	}
	let root = store.deletions_root();
	if !fs::exists(&root)? {
		fs::private_directory(&root)?;
		fs::sync_parent(&root)?;
	}
	fs::validate_private(&root)?;
	let path = record_path(store, &record.id)?;
	let temporary = path.with_extension("next");
	let mut file = OpenOptions::new()
		.write(true)
		.create(true)
		.truncate(true)
		.mode(0o600)
		.custom_flags(libc::O_NOFOLLOW)
		.open(&temporary)?;
	file.write_all(&bytes)?;
	file.sync_all()?;
	stdfs::rename(&temporary, &path)?;
	fs::sync_parent(&path)
}

fn load(store: &Store, id: &str) -> io::Result<Record> {
	load_from(&store.deletions_root(), id)
}

fn load_from(root: &Path, id: &str) -> io::Result<Record> {
	fs::validate_private(root)?;
	let path = path_in(root, id)?;
	let file = OpenOptions::new()
		.read(true)
		.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
		.open(path)?;
	if !file.metadata()?.is_file() {
		return Err(fs::invalid("Trash deletion record is not a regular file"));
	}
	let record: Record = serde_json::from_reader(file.take(64 * 1024 * 1024))?;
	if record.version != 1 || record.id != id {
		return Err(fs::invalid("Unsupported trash deletion record"));
	}
	Ok(record)
}

fn forget(store: &Store, id: &str) -> io::Result<()> {
	let path = record_path(store, id)?;
	stdfs::remove_file(&path)?;
	fs::sync_parent(&path)
}

fn prepare(store: &Store, item: &Captured) -> io::Result<Record> {
	item.validate()?;
	let original = &item.payload.path;
	let root = original
		.parent()
		.and_then(Path::parent)
		.ok_or_else(|| fs::invalid("Missing trash root"))?;
	let id = journal::new_id()?;
	let contents = fs::metadata_snapshot(original)?;
	let metadata = item
		.info
		.as_ref()
		.map(|_| fs::metadata_snapshot(&item.info_path))
		.transpose()?;
	item.validate()?;
	let record = Record {
		version: 1,
		workspace: StoredPath::new(&root.join(format!(".fileberg-delete-{id}"))),
		workspace_identity: None,
		id,
		original: StoredPath::new(original),
		info: StoredPath::new(&item.info_path),
		contents,
		metadata,
		containers: item
			.containers
			.iter()
			.map(|(path, identity)| (StoredPath::new(path), identity.clone()))
			.collect(),
		step: Step::Prepared,
	};
	save(store, &record)?;
	Ok(record)
}

fn validate_layout(record: &Record) -> io::Result<()> {
	let original = record.original.path();
	let files = original
		.parent()
		.ok_or_else(|| fs::invalid("Invalid trash payload path"))?;
	let root = files
		.parent()
		.ok_or_else(|| fs::invalid("Invalid trash root"))?;
	let mut name = original
		.file_name()
		.ok_or_else(|| fs::invalid("Invalid trash name"))?
		.to_os_string();
	name.push(".trashinfo");
	if !original.is_absolute()
		|| files.file_name().is_none_or(|part| part != "files")
		|| record.info.path() != root.join("info").join(name)
		|| record.workspace.path() != root.join(format!(".fileberg-delete-{}", record.id))
		|| record.containers.len() < 2
	{
		return Err(fs::invalid("Invalid permanent deletion layout"));
	}
	for (path, expected) in &record.containers {
		fs::validate_private(&path.path())?;
		if !expected.same_container(&path.path()) {
			return Err(fs::invalid(
				"Trash storage changed or is offline; deletion paused",
			));
		}
	}
	Ok(())
}

fn resume(store: &Store, record: &mut Record) -> io::Result<()> {
	validate_layout(record)?;
	let workspace = record.workspace.path();
	if !fs::exists(&workspace)? && record.step == Step::PayloadDeleted {
		// Cleanup already removed the workspace. Do not revisit a metadata slot
		// which another application may have reused since that commit.
		return forget(store, &record.id);
	}
	if record.workspace_identity.is_none() {
		if !fs::exists(&workspace)? {
			fs::private_directory(&workspace)?;
			fs::sync_parent(&workspace)?;
		}
		fs::validate_private(&workspace)?;
		if stdfs::read_dir(&workspace)?.next().is_some() {
			return Err(fs::invalid(
				"Unidentified deletion workspace contains data; preserving it",
			));
		}
		record.workspace_identity = fs::metadata_snapshot(&workspace)?.0.into_iter().next();
		save(store, record)?;
	}
	fs::validate_private(&workspace)?;
	let expected = record
		.workspace_identity
		.as_ref()
		.ok_or_else(|| fs::invalid("Missing deletion workspace identity"))?;
	let actual = Identity::capture(&workspace)?;
	if (actual.device, actual.inode) != (expected.device, expected.inode) {
		return Err(fs::invalid("Permanent deletion workspace identity changed"));
	}
	let data = workspace.join("data");
	let original = record.original.path();
	if record.step == Step::Prepared {
		if fs::exists(&data)? {
			if fs::exists(&original)? {
				return Err(fs::invalid(
					"Trash slot and quarantine are both occupied; preserving both",
				));
			}
		} else {
			fs::unchanged(&original, &record.contents)?;
			fs::rename_new(&original, &data)?;
			fs::sync_parent(&original)?;
			fs::sync_parent(&data)?;
		}
		record.contents = fs::relocated_snapshot(&data, &record.contents)?;
		record.step = Step::Quarantined;
		save(store, record)?;
	}
	if record.step == Step::Quarantined {
		if fs::exists(&data)? || fs::removal_pending(&data, &record.contents)? {
			fs::remove_recorded(&data, &record.contents)?;
		}
		record.step = Step::PayloadDeleted;
		save(store, record)?;
	}
	if let Some(metadata) = &record.metadata {
		let info = record.info.path();
		if fs::exists(&info)? || fs::removal_pending(&info, metadata)? {
			fs::remove_recorded(&info, metadata)?;
		}
	}
	// Never recurse into an unexpected child of this private workspace.
	stdfs::remove_dir(&workspace)?;
	fs::sync_parent(&workspace)?;
	forget(store, &record.id)
}

pub(crate) fn delete_trash(store: &Store, items: Vec<Captured>) -> io::Result<Vec<String>> {
	// A stale member invalidates the captured batch before its first deletion.
	for item in &items {
		item.validate()?;
	}
	let mut errors = Vec::new();
	for item in &items {
		let result = prepare(store, item).and_then(|mut record| resume(store, &mut record));
		if let Err(error) = result {
			errors.push(format!("{}: {error}", item.payload.path.display()));
		}
	}
	Ok(errors)
}

#[cfg(test)]
fn pending_trash_deletions(store: &Store) -> io::Result<Vec<PendingDeletion>> {
	pending_from(&store.deletions_root())
}

/// Records are atomically replaced; descriptive listing does not need to lock
/// out a transfer or retention pass. Retry still acquires the mutation lock.
pub(crate) fn read_pending_trash_deletions() -> io::Result<Vec<PendingDeletion>> {
	pending_from(&Store::default_path()?.join("trash-deletions"))
}

fn pending_from(root: &Path) -> io::Result<Vec<PendingDeletion>> {
	if !fs::exists(&root)? {
		return Ok(Vec::new());
	}
	fs::validate_private(&root)?;
	let mut pending = Vec::new();
	for entry in stdfs::read_dir(root)? {
		let path = entry?.path();
		if path
			.extension()
			.is_some_and(|extension| extension == "json")
		{
			let id = path
				.file_stem()
				.and_then(|name| name.to_str())
				.ok_or_else(|| fs::invalid("Invalid deletion record name"))?;
			let record = match load_from(root, id) {
				Ok(record) => record,
				Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
				Err(error) => return Err(error),
			};
			pending.push(PendingDeletion {
				id: id.into(),
				original: record.original.path(),
			});
		}
	}
	Ok(pending)
}

pub(crate) fn retry_trash_deletion(store: &Store, id: &str) -> io::Result<()> {
	resume(store, &mut load(store, id)?)
}
