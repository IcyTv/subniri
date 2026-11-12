use super::model::{Entry, Kind, Snapshot, StoredPath};
use sha2::{Digest, Sha256};
use std::{
	ffi::CString,
	fs::{self, File, OpenOptions},
	io::{self, Read},
	os::{
		fd::{AsRawFd, FromRawFd},
		unix::{
			ffi::OsStrExt,
			fs::{MetadataExt, OpenOptionsExt},
		},
	},
	path::Path,
};

#[cfg(test)]
thread_local! {
	pub(super) static INSPECTED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub(super) fn invalid(message: impl Into<String>) -> io::Error {
	io::Error::other(message.into())
}

pub(super) fn snapshot(path: &Path) -> io::Result<Snapshot> {
	snapshot_with(path, true, None)
}

/// Identity and change stamps suffice for a rename or private incomplete data.
/// An empty regular-file digest explicitly records that no content was read.
pub(super) fn metadata_snapshot(path: &Path) -> io::Result<Snapshot> {
	snapshot_with(path, false, None)
}

pub(super) fn metadata_snapshot_cancellable(
	path: &Path, cancelled: &std::sync::atomic::AtomicBool,
) -> io::Result<Snapshot> {
	snapshot_with(path, false, Some(cancelled))
}

pub(super) fn snapshot_matching(path: &Path, expected: &Snapshot) -> io::Result<Snapshot> {
	snapshot_with(path, has_content_hashes(expected), None)
}

pub(super) fn has_content_hashes(expected: &Snapshot) -> bool {
	expected
		.0
		.iter()
		.any(|entry| entry.kind == Kind::File && !entry.digest.is_empty())
}

pub(super) fn snapshot_cancellable(
	path: &Path, cancelled: &std::sync::atomic::AtomicBool,
) -> io::Result<Snapshot> {
	snapshot_with(path, true, Some(cancelled))
}

fn snapshot_with(
	path: &Path, hashed: bool, cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> io::Result<Snapshot> {
	let mut entries = Vec::new();
	inspect(path, Path::new(""), &mut entries, hashed, cancelled)?;
	Ok(Snapshot(entries))
}

fn inspect(
	root: &Path, relative: &Path, entries: &mut Vec<Entry>, hashed: bool,
	cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> io::Result<()> {
	let check_cancelled = || {
		if cancelled.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
			Err(io::Error::from(io::ErrorKind::Interrupted))
		} else {
			Ok(())
		}
	};
	check_cancelled()?;
	let path = if relative.as_os_str().is_empty() {
		root.to_path_buf()
	} else {
		root.join(relative)
	};
	let metadata = fs::symlink_metadata(&path)?;
	let kind = if metadata.is_symlink() {
		Kind::Symlink
	} else if metadata.is_file() {
		Kind::File
	} else if metadata.is_dir() {
		Kind::Directory
	} else {
		return Err(invalid(format!(
			"Unsupported filesystem entry: {}",
			path.display()
		)));
	};
	let mut hash = Sha256::new();
	if kind == Kind::File && hashed {
		let mut file = OpenOptions::new()
			.read(true)
			.custom_flags(libc::O_NOFOLLOW)
			.open(&path)?;
		let opened = file.metadata()?;
		if (opened.dev(), opened.ino()) != (metadata.dev(), metadata.ino()) {
			return Err(invalid("File changed while opening"));
		}
		let mut buffer = [0u8; 65536];
		loop {
			check_cancelled()?;
			let read = file.read(&mut buffer)?;
			if read == 0 {
				break;
			}
			hash.update(
				buffer
					.get(..read)
					.ok_or_else(|| invalid("Invalid read length"))?,
			);
			#[cfg(test)]
			INSPECTED_BYTES.with(|bytes| bytes.set(bytes.get() + read as u64));
		}
		let after = file.metadata()?;
		if stamp(&metadata) != stamp(&after) {
			return Err(invalid(format!(
				"File changed during inspection: {}",
				path.display()
			)));
		}
	} else if kind == Kind::Symlink {
		hash.update(fs::read_link(&path)?.as_os_str().as_bytes());
	}
	entries.push(Entry {
		relative: StoredPath::new(relative),
		device: metadata.dev(),
		inode: metadata.ino(),
		kind,
		mode: metadata.mode(),
		length: metadata.len(),
		modified: (metadata.mtime(), metadata.mtime_nsec()),
		changed: (metadata.ctime(), metadata.ctime_nsec()),
		digest: if kind == Kind::File && !hashed {
			Vec::new()
		} else {
			hash.finalize().to_vec()
		},
	});
	if kind == Kind::Directory {
		let mut children = fs::read_dir(&path)?
			.map(|entry| entry.map(|e| e.file_name()))
			.collect::<io::Result<Vec<_>>>()?;
		children.sort();
		for name in children {
			inspect(root, &relative.join(name), entries, hashed, cancelled)?;
		}
		if stamp(&metadata) != stamp(&fs::symlink_metadata(&path)?) {
			return Err(invalid(format!(
				"Directory changed during inspection: {}",
				path.display()
			)));
		}
	}

	Ok(())
}

fn stamp(metadata: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
	(
		metadata.dev(),
		metadata.ino(),
		metadata.len(),
		metadata.mtime(),
		metadata.mtime_nsec(),
		metadata.ctime(),
		metadata.ctime_nsec(),
	)
}

pub(super) fn unchanged(path: &Path, expected: &Snapshot) -> io::Result<()> {
	if snapshot_matching(path, expected)? != *expected {
		return Err(invalid(format!(
			"Entry changed; preserving {}",
			path.display()
		)));
	}

	Ok(())
}

pub(super) fn unchanged_metadata(path: &Path, expected: &Snapshot) -> io::Result<()> {
	let mut metadata = expected.clone();
	for entry in &mut metadata.0 {
		if entry.kind == Kind::File {
			entry.digest.clear();
		}
	}
	unchanged(path, &metadata)
}

/// Preserve verified digests across our own rename, which changes root ctime.
/// Every child retains its identity and change stamp; no file data is reread.
pub(super) fn relocated_snapshot(path: &Path, expected: &Snapshot) -> io::Result<Snapshot> {
	let mut actual = metadata_snapshot(path)?;
	if actual.0.len() != expected.0.len() {
		return Err(invalid("Entry tree changed during rename"));
	}
	for (current, original) in actual.0.iter_mut().zip(&expected.0) {
		current.digest = original.digest.clone();
		let mut comparable = current.clone();
		if comparable.relative.0.is_empty() {
			comparable.changed = original.changed;
		}
		if comparable != *original {
			return Err(invalid("Entry changed during rename"));
		}
	}
	Ok(actual)
}

pub(super) fn entry_path(root: &Path, relative: &StoredPath) -> std::path::PathBuf {
	if relative.0.is_empty() {
		root.to_path_buf()
	} else {
		root.join(relative.path())
	}
}

/// Rename changes ctime, so compare identity, names, content and user metadata,
/// but not ctime. Directory sizes are filesystem representation details.
/// Metadata-only file manifests require ctime too; relocation handles our own
/// rename explicitly instead of treating a changed stamp as unchanged content.
pub(super) fn same_contents(actual: &Snapshot, expected: &Snapshot, identity: bool) -> bool {
	actual.0.len() == expected.0.len()
		&& actual.0.iter().zip(&expected.0).all(|(a, b)| {
			a.relative == b.relative
				&& a.kind == b.kind
				&& (a.mode & 0o777) == (b.mode & 0o777)
				&& a.digest == b.digest
				&& (a.kind != Kind::File
					|| !a.digest.is_empty()
					|| !identity || a.changed == b.changed)
				&& (a.kind == Kind::Directory || (a.length == b.length && a.modified == b.modified))
				&& (!identity || (a.device, a.inode) == (b.device, b.inode))
		})
}

pub(super) fn exists(path: &Path) -> io::Result<bool> {
	match fs::symlink_metadata(path) {
		Ok(_) => Ok(true),
		Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
		Err(error) => Err(error),
	}
}

pub(super) fn rename_new(source: &Path, destination: &Path) -> io::Result<()> {
	let source = CString::new(source.as_os_str().as_bytes())?;
	let destination = CString::new(destination.as_os_str().as_bytes())?;
	// Never substitute an existence check followed by an overwriting rename.
	if unsafe {
		libc::renameat2(
			libc::AT_FDCWD,
			source.as_ptr(),
			libc::AT_FDCWD,
			destination.as_ptr(),
			libc::RENAME_NOREPLACE,
		)
	} != 0
	{
		return Err(io::Error::last_os_error());
	}

	Ok(())
}

/// The quarantine name is deterministic from the persistent record, so a crash
/// halfway through deletion does not make surviving entries undiscoverable.
pub(super) fn removal_directory(
	path: &Path, expected: &Snapshot,
) -> io::Result<std::path::PathBuf> {
	let mut hash = Sha256::new();
	hash.update(path.as_os_str().as_bytes());
	hash.update(serde_json::to_vec(expected)?);
	let key: String = hash
		.finalize()
		.iter()
		.take(16)
		.map(|byte| format!("{byte:02x}"))
		.collect();
	Ok(path
		.parent()
		.ok_or_else(|| invalid("Missing removal parent"))?
		.join(format!(".fileberg-delete-{key}")))
}

pub(super) fn removal_pending(path: &Path, expected: &Snapshot) -> io::Result<bool> {
	exists(&removal_directory(path, expected)?)
}

/// Quarantine before deletion, then operate through pinned directory handles.
/// Each entry is moved into private storage and checked again before unlinking.
/// Unexpected entries remain recoverable rather than being recursively erased.
pub(super) fn remove_recorded(path: &Path, expected: &Snapshot) -> io::Result<()> {
	remove_with_hook(path, expected, || {}).map_err(|error| {
		let location =
			removal_directory(path, expected).map_or_else(|_| path.to_path_buf(), |path| path);
		io::Error::new(
			error.kind(),
			format!(
				"Cleanup incomplete; data preserved at {} or {}: {error}",
				path.display(),
				location.display()
			),
		)
	})
}

fn remove_with_hook(path: &Path, expected: &Snapshot, hook: impl FnOnce()) -> io::Result<()> {
	let quarantine = removal_directory(path, expected)?;
	let parent = open_directory(
		path.parent()
			.ok_or_else(|| invalid("Missing removal parent"))?,
	)?;
	let name = path
		.file_name()
		.ok_or_else(|| invalid("Cannot delete a filesystem root"))?;
	if !exists(&quarantine)? {
		private_directory(&quarantine)?;
		sync_parent(&quarantine)?;
	}
	validate_private(&quarantine)?;
	let scratch = open_directory(&quarantine)?;
	let data = quarantine.join("data");
	let has_scratch = fs::read_dir(&quarantine)?.next().is_some();
	if !has_scratch && exists(path)? {
		if !same_contents(&snapshot_matching(path, expected)?, expected, true) {
			return Err(invalid(format!(
				"Entry changed; preserving {}",
				path.display()
			)));
		}
		// If the name changes concurrently, the new entry is retained in quarantine
		// and identity validation below prevents its destruction.
		rename_at(&parent, name, &scratch, std::ffi::OsStr::new("data"))?;
		parent.sync_all()?;
		scratch.sync_all()?;
	}
	if exists(&data)? {
		let actual = snapshot_matching(&data, expected)?;
		for current in &actual.0 {
			let valid = expected
				.0
				.iter()
				.find(|entry| entry.relative == current.relative)
				.is_some_and(|original| verify_entry(current, original).is_ok());
			if !valid {
				if !has_scratch {
					// A concurrently replaced name is not ours to delete. Restore it
					// without clobbering anything that arrived at the original path.
					let _ = rename_at(&scratch, std::ffi::OsStr::new("data"), &parent, name);
				}
				return Err(invalid(format!(
					"Unverified data preserved at {} or {}",
					path.display(),
					data.display()
				)));
			}
		}
	}
	let root = if expected
		.0
		.first()
		.is_some_and(|entry| entry.kind == Kind::Directory)
		&& exists(&data)?
	{
		Some(open_directory(&data)?)
	} else {
		None
	};
	hook();
	for (index, entry) in expected.0.iter().enumerate().rev() {
		let slot = format!("entry-{index}");
		let slot_path = quarantine.join(&slot);
		if !exists(&slot_path)? {
			let relative = entry.relative.path();
			let location = if entry.relative.0.is_empty() {
				Some((scratch.try_clone()?, std::ffi::OsString::from("data")))
			} else if let Some(root) = &root {
				let parent = open_relative(root, relative.parent().unwrap_or(Path::new("")));
				match parent {
					Ok(parent) => Some((
						parent,
						relative
							.file_name()
							.ok_or_else(|| invalid("Invalid manifest path"))?
							.to_owned(),
					)),
					Err(error) if error.kind() == io::ErrorKind::NotFound => None,
					Err(error) => return Err(error),
				}
			} else {
				None
			};
			if let Some((parent, name)) = location {
				match rename_at(&parent, &name, &scratch, std::ffi::OsStr::new(&slot)) {
					Ok(()) => {}
					Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
					Err(error) => return Err(error),
				}
			} else {
				continue;
			}
		}
		let current = snapshot_shallow(&slot_path, !entry.digest.is_empty())?;
		verify_entry(&current, entry)
			.map_err(|_| invalid(format!("Changed entry retained at {}", slot_path.display())))?;
		let slot = CString::new(slot)?;
		let flags = if entry.kind == Kind::Directory {
			libc::AT_REMOVEDIR
		} else {
			0
		};
		if unsafe { libc::unlinkat(scratch.as_raw_fd(), slot.as_ptr(), flags) } != 0 {
			return Err(io::Error::last_os_error());
		}
	}
	scratch.sync_all()?;
	fs::remove_dir(&quarantine)?;
	sync_parent(&quarantine)
}

fn verify_entry(actual: &Entry, expected: &Entry) -> io::Result<()> {
	if (actual.device, actual.inode, actual.kind, actual.mode)
		!= (
			expected.device,
			expected.inode,
			expected.kind,
			expected.mode,
		) || (actual.kind != Kind::Directory
		&& (actual.digest != expected.digest
			|| actual.modified != expected.modified
			|| actual.length != expected.length))
	{
		return Err(invalid("Entry identity or contents changed"));
	}

	Ok(())
}

fn rename_at(
	source: &File, name: &std::ffi::OsStr, target: &File, new_name: &std::ffi::OsStr,
) -> io::Result<()> {
	let name = CString::new(name.as_bytes())?;
	let new_name = CString::new(new_name.as_bytes())?;
	if unsafe {
		libc::renameat2(
			source.as_raw_fd(),
			name.as_ptr(),
			target.as_raw_fd(),
			new_name.as_ptr(),
			libc::RENAME_NOREPLACE,
		)
	} != 0
	{
		return Err(io::Error::last_os_error());
	}

	Ok(())
}

fn open_relative(root: &File, relative: &Path) -> io::Result<File> {
	let mut directory = root.try_clone()?;
	for component in relative.components() {
		let std::path::Component::Normal(name) = component else {
			return Err(invalid("Unsafe relative manifest path"));
		};
		let name = CString::new(name.as_bytes())?;
		let descriptor = unsafe {
			libc::openat(
				directory.as_raw_fd(),
				name.as_ptr(),
				libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
			)
		};
		if descriptor < 0 {
			return Err(io::Error::last_os_error());
		}
		directory = unsafe { File::from_raw_fd(descriptor) };
	}

	Ok(directory)
}

fn open_directory(path: &Path) -> io::Result<File> {
	if !path.is_absolute() {
		return Err(invalid("Expected an absolute directory"));
	}
	let root = File::open("/")?;
	open_relative(
		&root,
		path.strip_prefix("/")
			.map_err(|error| invalid(error.to_string()))?,
	)
}

fn snapshot_shallow(path: &Path, hashed: bool) -> io::Result<Entry> {
	let metadata = fs::symlink_metadata(path)?;
	if metadata.is_dir() {
		return Ok(Entry {
			relative: StoredPath::new(Path::new("")),
			device: metadata.dev(),
			inode: metadata.ino(),
			kind: Kind::Directory,
			mode: metadata.mode(),
			length: metadata.len(),
			modified: (metadata.mtime(), metadata.mtime_nsec()),
			changed: (metadata.ctime(), metadata.ctime_nsec()),
			digest: Sha256::digest([]).to_vec(),
		});
	}

	snapshot_with(path, hashed, None)?
		.0
		.into_iter()
		.next()
		.ok_or_else(|| invalid("Empty snapshot"))
}

pub(super) fn private_directory(path: &Path) -> io::Result<()> {
	use std::os::unix::fs::DirBuilderExt;
	fs::DirBuilder::new().mode(0o700).create(path)?;
	validate_private(path)
}

pub(super) fn validate_private(path: &Path) -> io::Result<()> {
	let metadata = fs::symlink_metadata(path)?;
	if !metadata.is_dir()
		|| metadata.uid() != unsafe { libc::getuid() }
		|| metadata.mode() & 0o077 != 0
	{
		return Err(invalid(format!(
			"Unsafe private storage: {}",
			path.display()
		)));
	}

	Ok(())
}

pub(super) fn sync_parent(path: &Path) -> io::Result<()> {
	super::durability::sync_directory(
		path.parent()
			.ok_or_else(|| invalid("Missing parent directory"))?,
	)
}

pub(super) fn sync_tree(path: &Path, expected: &Snapshot) -> io::Result<()> {
	for entry in expected.0.iter().rev() {
		let child = entry_path(path, &entry.relative);
		match entry.kind {
			Kind::File => OpenOptions::new()
				.read(true)
				.custom_flags(libc::O_NOFOLLOW)
				.open(child)?
				.sync_all()?,
			Kind::Directory => super::durability::sync_directory(&child)?,
			Kind::Symlink => sync_parent(&child)?,
		}
	}

	Ok(())
}

pub(super) fn lock(file: &File) -> io::Result<()> {
	use std::os::fd::AsRawFd;
	if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
		return Err(io::Error::last_os_error());
	}

	Ok(())
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::test_support::TestDir;

	#[test]
	fn interrupted_tree_deletion_resumes_from_its_recorded_quarantine() {
		let root = TestDir::new();
		let source = root.path().join("source");
		fs::create_dir(&source).unwrap();
		fs::write(source.join("first"), "a").unwrap();
		fs::write(source.join("second"), "b").unwrap();
		let expected = snapshot(&source).unwrap();
		let quarantine = removal_directory(&source, &expected).unwrap();
		private_directory(&quarantine).unwrap();
		rename_new(&source, &quarantine.join("data")).unwrap();
		fs::remove_file(quarantine.join("data/first")).unwrap();

		remove_recorded(&source, &expected).unwrap();

		assert!(!source.exists());
		assert!(!quarantine.exists());
	}

	#[test]
	fn deletion_does_not_follow_a_directory_replaced_by_a_symlink() {
		let root = TestDir::new();
		let source = root.path().join("source");
		let victim = root.path().join("victim");
		fs::create_dir(&source).unwrap();
		fs::create_dir(&victim).unwrap();
		fs::write(source.join("file"), "ours").unwrap();
		fs::write(victim.join("file"), "not ours").unwrap();
		let expected = snapshot(&source).unwrap();
		let quarantine = removal_directory(&source, &expected).unwrap();

		let result = remove_with_hook(&source, &expected, || {
			fs::rename(quarantine.join("data"), quarantine.join("retained")).unwrap();
			std::os::unix::fs::symlink(&victim, quarantine.join("data")).unwrap();
		});

		assert!(result.is_err());
		assert_eq!(fs::read(victim.join("file")).unwrap(), b"not ours");
	}

	#[test]
	fn changed_entries_are_rejected_before_quarantining() {
		let root = TestDir::new();
		let source = root.path().join("file");
		fs::write(&source, "before").unwrap();
		let expected = snapshot(&source).unwrap();
		fs::write(&source, "after").unwrap();

		assert!(remove_recorded(&source, &expected).is_err());
		assert_eq!(fs::read(&source).unwrap(), b"after");
	}
}
