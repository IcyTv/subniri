//! Trash metadata participates in the same persistent transaction as its payload.
use super::{
	fs,
	journal::Store,
	model::{Choice, Conflict, ConflictReason, Entry, Journal, Kind, Snapshot, StoredPath},
};
use crate::trash::{Captured, Identity};
use serde::{Deserialize, Serialize};
use std::{
	fs as stdfs, io,
	path::{Component, Path},
	sync::atomic::{AtomicBool, Ordering},
};

#[cfg(test)]
#[path = "restore_tests.rs"]
mod tests;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Restoration {
	pub items: Vec<Record>,
	pub parents: Vec<CreatedParent>,
	#[serde(default)]
	pub retired_workspaces: Vec<StoredPath>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Record {
	pub source: StoredPath,
	pub destination: StoredPath,
	info: StoredPath,
	info_before: Snapshot,
	workspace: StoredPath,
	workspace_identity: Entry,
	backup: StoredPath,
	containers: Vec<(StoredPath, Identity)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CreatedParent {
	path: StoredPath,
	identity: Option<Identity>,
}

impl Record {
	pub fn prepare(item: &Captured, id: &str, index: usize) -> io::Result<Self> {
		item.validate()?;
		let destination = item
			.original
			.as_ref()
			.ok_or_else(|| fs::invalid("Original trash location is unavailable"))?;
		if !destination.is_absolute()
			|| destination.file_name().is_none()
			|| destination
				.components()
				.any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
		{
			return Err(fs::invalid("Unsafe original trash location"));
		}
		if item
			.containers
			.first()
			.is_some_and(|(root, _)| destination.starts_with(root))
		{
			return Err(fs::invalid("Cannot restore an entry into trash storage"));
		}
		let parent = item
			.info_path
			.parent()
			.ok_or_else(|| fs::invalid("Missing trash metadata parent"))?;
		let workspace = parent.join(format!(".fileberg-restore-{id}-{index}"));
		let info_before = fs::snapshot(&item.info_path)?;
		if !info_before
			.0
			.first()
			.is_some_and(|entry| entry.kind == Kind::File)
		{
			return Err(fs::invalid("Trash metadata is not a regular file"));
		}
		item.validate()?;
		fs::private_directory(&workspace)?;
		fs::sync_parent(&workspace)?;
		let workspace_identity = fs::metadata_snapshot(&workspace)?
			.0
			.into_iter()
			.next()
			.ok_or_else(|| fs::invalid("Missing metadata workspace identity"))?;
		Ok(Self {
			source: StoredPath::new(&item.payload.path),
			destination: StoredPath::new(destination),
			info: StoredPath::new(&item.info_path),
			info_before,
			workspace: StoredPath::new(&workspace),
			workspace_identity,
			backup: StoredPath::new(&workspace.join("info")),
			containers: item
				.containers
				.iter()
				.map(|(path, identity)| (StoredPath::new(path), identity.clone()))
				.collect(),
		})
	}

	fn validate(&self) -> io::Result<()> {
		let source = self.source.path();
		let files = source
			.parent()
			.ok_or_else(|| fs::invalid("Invalid trash source"))?;
		let root = files
			.parent()
			.ok_or_else(|| fs::invalid("Invalid trash root"))?;
		let info = self.info.path();
		let workspace = self.workspace.path();
		let mut info_name = source
			.file_name()
			.ok_or_else(|| fs::invalid("Invalid trash name"))?
			.to_os_string();
		info_name.push(".trashinfo");
		let expected_containers = [root.to_path_buf(), files.to_path_buf(), root.join("info")];
		if !source.is_absolute()
			|| !workspace.is_absolute()
			|| files.file_name().is_none_or(|name| name != "files")
			|| info != root.join("info").join(info_name)
			|| workspace.parent() != info.parent()
			|| workspace
				.file_name()
				.is_none_or(|name| !name.as_encoded_bytes().starts_with(b".fileberg-restore-"))
			|| self.backup.path() != workspace.join("info")
			|| !self
				.containers
				.iter()
				.map(|(path, _)| path.path())
				.eq(expected_containers)
		{
			return Err(fs::invalid("Invalid trash restoration layout"));
		}
		for (path, identity) in &self.containers {
			fs::validate_private(&path.path())?;
			if !identity.same_container(&path.path()) {
				return Err(fs::invalid("Trash storage identity changed or is offline"));
			}
		}
		fs::validate_private(&workspace)?;
		let identity = Identity::capture(&workspace)?;
		if (identity.device, identity.inode)
			!= (
				self.workspace_identity.device,
				self.workspace_identity.inode,
			) {
			return Err(fs::invalid("Trash metadata recovery workspace changed"));
		}
		Ok(())
	}

	fn matches_info(&self, path: &Path) -> io::Result<bool> {
		Ok(fs::exists(path)? && fs::same_contents(&fs::snapshot(path)?, &self.info_before, true))
	}

	pub fn commit(&self) -> io::Result<()> {
		self.validate()?;
		let info = self.info.path();
		fs::unchanged(&info, &self.info_before)?;
		let backup = self.backup.path();
		fs::rename_new(&info, &backup)?;
		fs::sync_parent(&info)?;
		fs::sync_parent(&backup)
	}

	pub fn preflight_inverse(&self) -> io::Result<()> {
		self.validate()?;
		let info = self.info.path();
		if fs::exists(&info)? {
			if !self.matches_info(&info)? {
				return Err(fs::invalid(
					"Trash metadata slot was reused; preserving restored data",
				));
			}
		} else if !self.matches_info(&self.backup.path())? {
			return Err(fs::invalid(
				"Original trash metadata is unavailable; recovery retained",
			));
		}
		Ok(())
	}

	pub fn finish_inverse(&self) -> io::Result<()> {
		self.preflight_inverse()?;
		let info = self.info.path();
		if self.matches_info(&info)? {
			return Ok(());
		}
		let backup = self.backup.path();
		fs::rename_new(&backup, &info)?;
		fs::sync_parent(&backup)?;
		fs::sync_parent(&info)
	}

	fn preflight_expiry(&self, allow_missing: bool) -> io::Result<()> {
		let workspace = self.workspace.path();
		if allow_missing && !fs::exists(&workspace)? {
			return Ok(());
		}
		self.validate()?;
		let backup = self.backup.path();
		let removal = fs::removal_directory(&backup, &self.info_before)?;
		for entry in stdfs::read_dir(&workspace)? {
			let path = entry?.path();
			if path == backup {
				if !self.matches_info(&path)? {
					return Err(fs::invalid(
						"Retained trash metadata changed; preserving it",
					));
				}
			} else if path == removal {
				fs::validate_private(&path)?;
			} else {
				return Err(fs::invalid(
					"Unexpected metadata recovery data; preserving workspace",
				));
			}
		}
		Ok(())
	}

	fn expire(&self) -> io::Result<()> {
		self.preflight_expiry(true)?;
		let workspace = self.workspace.path();
		if !fs::exists(&workspace)? {
			return Ok(());
		}
		let backup = self.backup.path();
		if fs::exists(&backup)? || fs::removal_pending(&backup, &self.info_before)? {
			fs::remove_recorded(&backup, &self.info_before)?;
		}
		stdfs::remove_dir(&workspace)?;
		fs::sync_parent(&workspace)
	}
}

pub(super) fn ensure_parent(
	store: &Store, journal: &mut Journal, destination: &Path, source: &Path,
	cancelled: &AtomicBool, conflict: &mut impl FnMut(Conflict) -> Choice,
) -> io::Result<()> {
	let parent = destination
		.parent()
		.ok_or_else(|| fs::invalid("Missing original parent"))?;
	let mut path = std::path::PathBuf::new();
	let mut missing = Vec::new();
	for component in parent.components() {
		if !matches!(component, Component::RootDir | Component::Normal(_)) {
			return Err(fs::invalid("Unsafe restore ancestor"));
		}
		path.push(component);
		match stdfs::symlink_metadata(&path) {
			Ok(metadata) if metadata.is_dir() => {}
			Ok(_) => {
				return Err(fs::invalid(format!(
					"Restore ancestor is a symlink or not a directory: {}",
					path.display()
				)));
			}
			Err(error) if error.kind() == io::ErrorKind::NotFound => missing.push(path.clone()),
			Err(error) => return Err(error),
		}
	}

	if missing.is_empty() {
		return Ok(());
	}
	if conflict(Conflict {
		source: source.into(),
		destination: parent.into(),
		directories: false,
		reason: ConflictReason::MissingParents,
	}) != Choice::ApproveRecovery
	{
		return Err(io::ErrorKind::Interrupted.into());
	}

	for path in missing {
		if cancelled.load(Ordering::Acquire) {
			return Err(io::ErrorKind::Interrupted.into());
		}
		let state = journal
			.restoration
			.as_mut()
			.ok_or_else(|| fs::invalid("Missing restore journal"))?;
		state.parents.push(CreatedParent {
			path: StoredPath::new(&path),
			identity: None,
		});
		store.save(journal)?;

		// Recheck every existing ancestor before creation; never follow a replaced
		// symlink when recreating an original directory tree.
		let parent = path.parent().ok_or_else(|| fs::invalid("Missing parent"))?;
		let directory = fs::open_directory(parent)?;
		use std::{
			ffi::CString,
			os::{fd::AsRawFd, unix::ffi::OsStrExt},
		};
		let name = CString::new(
			path.file_name()
				.ok_or_else(|| fs::invalid("Missing directory name"))?
				.as_bytes(),
		)?;
		if unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o755) } != 0 {
			return Err(io::Error::last_os_error());
		}
		let pinned = std::path::PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
			.join(
				path.file_name()
					.ok_or_else(|| fs::invalid("Missing directory name"))?,
			);
		let identity = Identity::capture(&pinned)?;
		if !identity.matches(&path) {
			return Err(fs::invalid("Restore ancestor changed during creation"));
		}

		journal
			.restoration
			.as_mut()
			.and_then(|state| state.parents.last_mut())
			.ok_or_else(|| fs::invalid("Missing parent record"))?
			.identity = Some(identity);
		store.save(journal)?;
		fs::sync_parent(&path)?;
	}
	Ok(())
}

pub(super) fn cleanup_parents(store: &Store, journal: &mut Journal) -> io::Result<()> {
	super::retention::retire_restore_workspaces(store, journal)?;
	let Some(state) = &journal.restoration else {
		return Ok(());
	};
	for parent in state.parents.iter().rev() {
		let path = parent.path.path();
		if !fs::exists(&path)? {
			continue;
		}
		let Some(expected) = &parent.identity else {
			return Err(fs::invalid(
				"Created directory identity was not saved; preserving it",
			));
		};
		if expected.same_container(&path) && stdfs::read_dir(&path)?.next().is_none() {
			stdfs::remove_dir(&path)?;
			fs::sync_parent(&path)?;
		}
	}
	Ok(())
}

pub(super) fn preflight_expiry(journal: &Journal) -> io::Result<()> {
	if let Some(state) = &journal.restoration {
		for item in &state.items {
			item.preflight_expiry(journal.status == super::Status::Expiring)?;
		}
	}
	Ok(())
}

pub(super) fn expire(journal: &Journal) -> io::Result<()> {
	if let Some(state) = &journal.restoration {
		for item in &state.items {
			item.expire()?;
		}
	}
	Ok(())
}
