//! Mutation authority is captured separately from descriptive trash metadata.
use super::{TrashEntry, listing, validate_private_dir};
use crate::context_menu::{Context, Target};
use serde::{Deserialize, Serialize};
use std::{
	fs, io,
	os::unix::fs::MetadataExt,
	path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Identity {
	pub device: u64,
	pub inode: u64,
	mode: u32,
	uid: u32,
	length: u64,
	modified: (i64, i64),
	changed: (i64, i64),
}

impl Identity {
	pub(super) fn new(metadata: &fs::Metadata) -> Self {
		Self {
			device: metadata.dev(),
			inode: metadata.ino(),
			mode: metadata.mode(),
			uid: metadata.uid(),
			length: metadata.len(),
			modified: (metadata.mtime(), metadata.mtime_nsec()),
			changed: (metadata.ctime(), metadata.ctime_nsec()),
		}
	}

	pub(crate) fn capture(path: &Path) -> io::Result<Self> {
		fs::symlink_metadata(path).map(|metadata| Self::new(&metadata))
	}

	pub(crate) fn matches(&self, path: &Path) -> bool {
		Self::capture(path).is_ok_and(|actual| actual == *self)
	}

	pub(crate) fn same_container(&self, path: &Path) -> bool {
		fs::symlink_metadata(path).is_ok_and(|metadata| {
			metadata.is_dir()
				&& (
					metadata.dev(),
					metadata.ino(),
					metadata.mode(),
					metadata.uid(),
				) == (self.device, self.inode, self.mode, self.uid)
		})
	}
}

#[derive(Debug, Clone)]
pub(crate) struct Captured {
	pub payload: Target,
	pub payload_identity: Identity,
	pub info_path: PathBuf,
	pub info: Option<Target>,
	pub info_identity: Option<Identity>,
	pub original: Option<PathBuf>,
	pub containers: Vec<(PathBuf, Identity)>,
	pub info_bytes: Option<Vec<u8>>,
}

fn invalid(message: impl Into<String>) -> io::Error {
	io::Error::other(message.into())
}

impl Captured {
	pub(crate) fn capture(entry: &TrashEntry, restoring: bool) -> io::Result<Self> {
		let identity = entry
			.identity
			.as_ref()
			.ok_or_else(|| invalid("Trash item could not be inspected"))?;
		if !identity.matches(&entry.stored_path) {
			return Err(invalid(
				"Trash item changed since the listing was loaded; refresh first",
			));
		}
		for (path, expected) in &entry.containers {
			validate_private_dir(path)?;
			if !expected.same_container(path) {
				return Err(invalid("Trash storage changed; refresh first"));
			}
		}

		let files = entry
			.stored_path
			.parent()
			.ok_or_else(|| invalid("Invalid trash payload path"))?;
		let root = files
			.parent()
			.ok_or_else(|| invalid("Invalid trash location"))?;
		if files.file_name().is_none_or(|name| name != "files") || entry.containers.len() < 2 {
			return Err(invalid("Invalid trash storage layout"));
		}
		let mut info_name = entry
			.stored_path
			.file_name()
			.ok_or_else(|| invalid("Missing trash item name"))?
			.to_os_string();
		info_name.push(".trashinfo");
		if entry.info_path != root.join("info").join(info_name) {
			return Err(invalid("Invalid paired trash metadata path"));
		}

		let payload = Target::capture(
			&entry.stored_path,
			if entry.file_type.is_some_and(|kind| kind.is_dir()) {
				Context::Folder
			} else {
				Context::File
			},
		)
		.map_err(invalid)?;
		let info = match &entry.info_identity {
			Some(expected) => {
				if !expected.matches(&entry.info_path) {
					return Err(invalid("Trash metadata changed; refresh first"));
				}
				Some(Target::capture(&entry.info_path, Context::File).map_err(invalid)?)
			}
			None => {
				if fs::symlink_metadata(&entry.info_path).is_ok() {
					return Err(invalid(
						"Trash metadata appeared since listing; refresh first",
					));
				}
				None
			}
		};

		let info_bytes = listing::read_info(&entry.info_path).ok();
		if info_bytes != entry.info_bytes {
			return Err(invalid("Trash metadata contents changed; refresh first"));
		}
		if restoring && (entry.original_path.is_none() || info_bytes.is_none()) {
			return Err(invalid(
				"This item has missing or damaged trash metadata and cannot be restored",
			));
		}

		let captured = Self {
			payload,
			payload_identity: identity.clone(),
			info_path: entry.info_path.clone(),
			info,
			info_identity: entry.info_identity.clone(),
			original: entry.original_path.clone(),
			containers: entry.containers.clone(),
			info_bytes,
		};
		captured.validate()?;
		Ok(captured)
	}

	pub(crate) fn validate(&self) -> io::Result<()> {
		if !self.payload.available() || !self.payload_identity.matches(&self.payload.path) {
			return Err(invalid("Trash payload identity changed"));
		}
		for (path, expected) in &self.containers {
			validate_private_dir(path)?;
			if !expected.same_container(path) {
				return Err(invalid("Trash storage identity changed"));
			}
		}

		if let Some(expected) = &self.info_identity {
			if !self.info.as_ref().is_some_and(Target::available)
				|| !expected.matches(&self.info_path)
			{
				return Err(invalid("Trash metadata identity or contents changed"));
			}
			if self.info_bytes.is_some()
				&& listing::read_info(&self.info_path).ok() != self.info_bytes
			{
				return Err(invalid("Trash metadata contents changed"));
			}
		} else if fs::symlink_metadata(&self.info_path).is_ok() {
			return Err(invalid("The trash metadata slot is now occupied"));
		}
		Ok(())
	}
}
