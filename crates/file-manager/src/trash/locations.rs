//! Remember effective filesystem roots that are absent from the mount table.
//! Registration precedes trashing so a successful move remains discoverable.
use super::{
	Identity, Mountpoint, Mountpoints, absolute_path, prepare_private_dir, validate_private_dir,
};
use serde::{Deserialize, Serialize};
use std::{
	fs::{self, OpenOptions},
	io::{self, Read, Write},
	os::unix::{
		ffi::{OsStrExt, OsStringExt},
		fs::OpenOptionsExt,
	},
	path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct Location {
	path: Vec<u8>,
	identity: Identity,
}

fn root() -> io::Result<PathBuf> {
	dirs::state_dir()
		.map(|path| path.join("fileberg/trash-locations"))
		.ok_or_else(|| io::Error::other("Could not determine Fileberg state directory"))
}

pub(super) fn remember(path: &Path) -> io::Result<()> {
	remember_in(&root()?, path)
}

fn remember_in(root: &Path, path: &Path) -> io::Result<()> {
	let path = absolute_path(path)?;
	let location = Location {
		path: path.as_os_str().as_bytes().to_vec(),
		identity: Identity::capture(&path)?,
	};
	fs::create_dir_all(
		root.parent()
			.ok_or_else(|| io::Error::other("Missing location registry parent"))?,
	)?;
	prepare_private_dir(root)?;
	let name = format!(
		"{}-{}.json",
		location.identity.device, location.identity.inode
	);
	let destination = root.join(name);

	let bytes = serde_json::to_vec(&location)?;
	// Each writer owns its temporary name; readers only inspect published JSON.
	let stamp = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map_err(io::Error::other)?
		.as_nanos();
	let temporary = root.join(format!("{}-{stamp}.next", std::process::id()));
	let mut file = OpenOptions::new()
		.write(true)
		.create_new(true)
		.mode(0o600)
		.custom_flags(libc::O_NOFOLLOW)
		.open(&temporary)?;
	file.write_all(&bytes)?;
	file.sync_all()?;
	fs::rename(&temporary, destination)?;
	fs::File::open(root)?.sync_all()
}

pub(super) fn extend(mounts: &mut Mountpoints) -> io::Result<()> {
	extend_from(&root()?, mounts)
}

fn extend_from(root: &Path, mounts: &mut Mountpoints) -> io::Result<()> {
	if !root.try_exists()? {
		return Ok(());
	}
	validate_private_dir(root)?;
	for entry in fs::read_dir(root)? {
		let path = entry?.path();
		if path.extension().is_none_or(|extension| extension != "json") {
			continue;
		}
		let file = OpenOptions::new()
			.read(true)
			.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
			.open(&path)?;
		if !file.metadata()?.is_file() {
			return Err(io::Error::other("Invalid trash location record"));
		}
		let location: Location = serde_json::from_reader(file.take(1024 * 1024))?;
		let path = PathBuf::from(std::ffi::OsString::from_vec(location.path));
		if path.is_absolute()
			&& location.identity.same_container(&path)
			&& !mounts.points.iter().any(|mount| mount.path == path)
		{
			mounts.points.push(Mountpoint {
				path,
				virtual_fs: false,
			});
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::test_support::TestDir;

	#[test]
	fn remembered_unmounted_roots_survive_reload_and_reject_replacements() {
		let directory = TestDir::new();
		let root = directory.path().join("registry");
		let volume = directory.path().join("subvolume");
		fs::create_dir(&volume).unwrap();
		remember_in(&root, &volume).unwrap();
		let source = volume.join("original-file");
		fs::write(&source, b"discoverable payload").unwrap();
		let location = super::super::prepare_topdir_trash(&source, &volume).unwrap();
		let stored = super::super::move_to_trash(&source, &location).unwrap();

		let mut mounts = Mountpoints { points: Vec::new() };
		extend_from(&root, &mut mounts).unwrap();
		extend_from(&root, &mut mounts).unwrap();
		assert_eq!(mounts.points.len(), 1);
		assert_eq!(mounts.points[0].path, volume);
		let listing =
			super::super::listing::list_from_mounts(&mounts, &directory.path().join("home-trash"));
		assert!(listing.issues.is_empty(), "{:?}", listing.issues);
		assert_eq!(listing.entries.len(), 1);
		assert_eq!(listing.entries[0].stored_path, stored);
		assert_eq!(listing.entries[0].original_path.as_ref(), Some(&source));

		fs::rename(&volume, directory.path().join("offline")).unwrap();
		fs::create_dir(&volume).unwrap();
		let mut mounts = Mountpoints { points: Vec::new() };
		extend_from(&root, &mut mounts).unwrap();
		assert!(mounts.points.is_empty());

		let renamed = directory.path().join("offline");
		remember_in(&root, &renamed).unwrap();
		extend_from(&root, &mut mounts).unwrap();
		assert_eq!(mounts.points.len(), 1);
		assert_eq!(mounts.points[0].path, renamed);
	}
}
