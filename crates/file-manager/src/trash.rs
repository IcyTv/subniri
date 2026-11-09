use std::{
	ffi::{CString, OsString},
	fs, io,
	io::Write,
	os::unix::{
		ffi::{OsStrExt, OsStringExt},
		fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
	},
	path::{Path, PathBuf},
};

mod listing;
pub(crate) use listing::{TrashEntry, TrashIssue, TrashListing, list_trash};

/// Move an entry into freedesktop trash without copying or overwriting entries.
/// On failure the source is never deliberately erased. No permanent-delete
/// fallback is performed, including on read-only or unsupported filesystems.
pub(crate) fn trash(path: impl AsRef<Path>) -> io::Result<PathBuf> {
	let path = absolute_path(path.as_ref())?;
	let mounts = Mountpoints::from_proc_mounts()?;
	let location = mounts.trash_location(&path, &base_trash_dir()?)?;

	move_to_trash(&path, &location)
}

/// Whether an entry is in plausible trash storage, including the root itself.
/// This safety check includes all numeric UIDs and ignores ownership/mode;
/// it does not imply the trash is usable. Never creates directories or follows
/// the final symlink.
pub(crate) fn is_in_trash(path: impl AsRef<Path>) -> io::Result<bool> {
	let path = absolute_path(path.as_ref())?;
	let mounts = Mountpoints::from_proc_mounts()?;

	mounts.is_in_trash(&path, &base_trash_dir()?)
}

struct TrashLocation {
	directory: PathBuf,
	// None for home trash; otherwise Path must be relative to this root.
	topdir: Option<PathBuf>,
}

/// https://specifications.freedesktop.org/trash/latest/
fn base_trash_dir() -> io::Result<PathBuf> {
	dirs::data_dir()
		.map(|path| path.join("Trash"))
		.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Could not determine XDG_DATA_HOME"))
}

struct Mountpoints {
	points: Vec<Mountpoint>,
}

impl Mountpoints {
	fn is_in_trash(&self, path: &Path, home_trash: &Path) -> io::Result<bool> {
		let path = absolute_path(path)?;

		if existing_trash_contains(&path, home_trash)? {
			return Ok(true);
		}

		let topdir = self.topdir_for_path(&path)?;
		let Some(name) = path
			.strip_prefix(&topdir)
			.ok()
			.and_then(|relative| relative.components().next())
		else {
			return Ok(false);
		};

		let name = name.as_os_str();
		let bytes = name.as_bytes();
		let private = bytes
			.strip_prefix(b".Trash-")
			.is_some_and(|uid| !uid.is_empty() && uid.iter().all(u8::is_ascii_digit));

		// Protect the entire shared .Trash and every numeric private trash,
		// even if their owner or mode makes them invalid as a destination.
		// Only reserve these names immediately under a filesystem topdir.
		if bytes == b".Trash" || private {
			return existing_trash_contains(&path, &topdir.join(name));
		}

		Ok(false)
	}

	fn from_proc_mounts() -> io::Result<Self> {
		let mounts = fs::read("/proc/self/mounts")?;

		let mut points = mounts
			.split(|byte| *byte == b'\n')
			.filter_map(Mountpoint::from_line)
			.collect::<Vec<_>>();

		if points.is_empty() {
			return Err(io::Error::new(
				io::ErrorKind::NotFound,
				"No mountpoints found",
			));
		}

		// Most specific mountpoint first.
		points.sort_unstable_by_key(|m| std::cmp::Reverse(m.path.as_os_str().len()));

		Ok(Self { points })
	}

	fn mount_for_path(&self, path: &Path) -> Option<&Mountpoint> {
		self.points.iter().find(|m| path.starts_with(&m.path))
	}

	/// Finds the effective filesystem root, including nested
	/// Btrfs subvolumes that are not explicitly mounted.
	fn topdir_for_path(&self, path: &Path) -> io::Result<PathBuf> {
		let path = absolute_path(path)?;

		let mount = self
			.mount_for_path(&path)
			.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No matching mountpoint"))?;

		let source_dev = fs::symlink_metadata(&path)?.dev();

		// Start at the source's parent. For ordinary files, this
		// avoids interpreting the file itself as a filesystem root.
		let mut current = if path == mount.path {
			path.clone()
		} else {
			path.parent().unwrap_or(&path).to_path_buf()
		};

		// If the source itself is a subvolume root, its device
		// can differ from its parent.
		if fs::symlink_metadata(&current)?.dev() != source_dev {
			return Ok(path);
		}

		while current != mount.path {
			let Some(parent) = current.parent() else {
				break;
			};

			let parent_dev = fs::metadata(parent)?.dev();

			if parent_dev != source_dev {
				return Ok(current);
			}

			current = parent.to_path_buf();
		}

		Ok(mount.path.clone())
	}

	/// Finds and prepares the trash directory for a file.
	///
	/// Does not move the file or write its .trashinfo metadata.
	fn trash_location(&self, path: &Path, home_trash: &Path) -> io::Result<TrashLocation> {
		let path = absolute_path(path)?;
		let topdir = self.topdir_for_path(&path)?;

		// Check before selecting home trash, too (mounts can share a device).
		if path == topdir {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				"Cannot trash a filesystem root",
			));
		}
		if self.is_in_trash(&path, home_trash)? {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				"Cannot trash existing trash storage",
			));
		}

		// Ensure XDG_DATA_HOME exists before inspecting its device.
		let data_home = home_trash.parent().ok_or_else(|| {
			io::Error::new(io::ErrorKind::InvalidInput, "Home trash has no parent")
		})?;
		fs::create_dir_all(data_home)?;
		let home_trash = fs::canonicalize(data_home)?.join("Trash");
		check_trash_relationship(&path, &home_trash)?;

		let source_dev = fs::symlink_metadata(&path)?.dev();
		let home_dev = fs::metadata(data_home)?.dev();

		if source_dev == home_dev {
			prepare_trash_dir(&home_trash)?;

			return Ok(TrashLocation {
				directory: home_trash,
				topdir: None,
			});
		}

		prepare_topdir_trash(&path, &topdir)
	}
}

fn prepare_topdir_trash(path: &Path, topdir: &Path) -> io::Result<TrashLocation> {
	let uid = unsafe { libc::getuid() };

	// Method 1: $topdir/.Trash/$uid
	let shared = topdir.join(".Trash");
	let private = topdir.join(format!(".Trash-{uid}"));
	check_trash_relationship(path, &shared)?;
	check_trash_relationship(path, &private)?;

	if valid_shared_trash(&shared) {
		let user_trash = shared.join(uid.to_string());

		if prepare_trash_dir(&user_trash).is_ok() {
			return Ok(TrashLocation {
				directory: user_trash,
				topdir: Some(topdir.to_owned()),
			});
		}
	}

	// Method 2: $topdir/.Trash-$uid. Never create the shared .Trash.
	prepare_trash_dir(&private)?;

	Ok(TrashLocation {
		directory: private,
		topdir: Some(topdir.to_owned()),
	})
}

fn check_trash_relationship(source: &Path, trash: &Path) -> io::Result<()> {
	if source.starts_with(trash) || trash.starts_with(source) {
		return Err(io::Error::new(
			io::ErrorKind::InvalidInput,
			"Cannot trash a trash directory, its contents, or its ancestors",
		));
	}

	Ok(())
}

#[derive(Debug)]
struct Mountpoint {
	path: PathBuf,
}

impl Mountpoint {
	fn from_line(line: &[u8]) -> Option<Self> {
		let mut parts = line
			.split(u8::is_ascii_whitespace)
			.filter(|part| !part.is_empty());

		// Skip mount source.
		parts.next()?;

		let escaped_path = parts.next()?;
		let path = decode_mount_path(escaped_path)?;

		if !path.is_absolute() {
			return None;
		}

		Some(Self { path })
	}
}

/// Decode octal escapes from /proc/self/mounts.
fn decode_mount_path(input: impl AsRef<[u8]>) -> Option<PathBuf> {
	let bytes = input.as_ref();
	let mut output = Vec::with_capacity(bytes.len());
	let mut i = 0;

	while i < bytes.len() {
		if bytes[i] == b'\\' {
			if i + 3 >= bytes.len() {
				return None;
			}

			let digits = &bytes[i + 1..i + 4];

			if !digits.iter().all(|b| (b'0'..=b'7').contains(b)) {
				return None;
			}

			let value = u16::from(digits[0] - b'0') * 64
				+ u16::from(digits[1] - b'0') * 8
				+ u16::from(digits[2] - b'0');
			let value = u8::try_from(value).ok()?;
			if value == 0 {
				return None;
			}

			output.push(value);
			i += 4;
		} else {
			output.push(bytes[i]);
			i += 1;
		}
	}

	Some(PathBuf::from(std::ffi::OsString::from_vec(output)))
}

/// Makes a path absolute while preserving the final component
/// (important when trashing a symlink).
fn absolute_path(path: &Path) -> io::Result<PathBuf> {
	if path.as_os_str().is_empty() {
		return Err(io::Error::new(
			io::ErrorKind::InvalidInput,
			"Empty source path",
		));
	}

	let absolute = if path.is_absolute() {
		path.to_path_buf()
	} else {
		std::env::current_dir()?.join(path)
	};

	// Paths ending in /, . or .. may denote a root or directory. Canonicalize
	// those special components rather than accidentally selecting their parent.
	let bytes = path.as_os_str().as_bytes();
	if absolute.file_name().is_none() || bytes.ends_with(b"/.") || bytes.ends_with(b"/") {
		return fs::canonicalize(absolute);
	}

	let parent = absolute
		.parent()
		.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Path has no parent"))?;
	let parent = fs::canonicalize(parent)?;

	match absolute.file_name() {
		Some(name) => Ok(parent.join(name)),
		None => Ok(parent),
	}
}

/// Validates an administrator-provided shared .Trash directory.
fn valid_shared_trash(path: &Path) -> bool {
	let Ok(metadata) = fs::symlink_metadata(path) else {
		return false;
	};

	metadata.file_type().is_dir() && metadata.mode() & 0o1000 != 0
}

fn existing_trash_contains(path: &Path, trash: &Path) -> io::Result<bool> {
	match fs::symlink_metadata(trash) {
		Ok(metadata) if metadata.is_dir() => {}
		Ok(_) => return Ok(false),
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
		Err(error) => return Err(error),
	}

	// Resolve aliases in XDG_DATA_HOME without accepting a symlink as the
	// actual trash root. Ownership and permissions are deliberately ignored:
	// this protects storage rather than deciding whether we may write to it.
	Ok(path.starts_with(absolute_path(trash)?))
}

/// Creates and validates a user-specific trash directory.
fn prepare_trash_dir(path: &Path) -> io::Result<()> {
	prepare_private_dir(path)?;
	for name in ["files", "info"] {
		prepare_private_dir(&path.join(name))?;
	}

	Ok(())
}

fn prepare_private_dir(path: &Path) -> io::Result<()> {
	// Set the mode at creation, not with a later chmod that leaves a window
	// of access for other users or follows a substituted symlink.
	match fs::DirBuilder::new().mode(0o700).create(path) {
		Ok(()) => {}
		Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
		Err(e) => return Err(e),
	}

	validate_private_dir(path)
}

fn validate_private_dir(path: &Path) -> io::Result<()> {
	let uid = unsafe { libc::getuid() };
	let metadata = fs::symlink_metadata(path)?;

	if !metadata.file_type().is_dir() {
		return Err(io::Error::new(
			io::ErrorKind::InvalidData,
			"Trash path is not a real directory",
		));
	}

	if metadata.uid() != uid {
		return Err(io::Error::new(
			io::ErrorKind::PermissionDenied,
			"Trash directory belongs to another user",
		));
	}

	// A private trash must not be accessible to other users.
	if metadata.mode() & 0o077 != 0 {
		return Err(io::Error::new(
			io::ErrorKind::PermissionDenied,
			"Trash directory has unsafe permissions",
		));
	}

	Ok(())
}

fn encode_path(path: &Path) -> String {
	let mut encoded = String::new();

	for &byte in path.as_os_str().as_bytes() {
		if byte.is_ascii_alphanumeric() || b"/-_.!~*'()".contains(&byte) {
			encoded.push(char::from(byte));
		} else {
			use std::fmt::Write;
			write!(encoded, "%{byte:02X}").expect("writing to String cannot fail");
		}
	}

	encoded
}

fn deletion_date() -> io::Result<String> {
	let now = unsafe { libc::time(std::ptr::null_mut()) };
	let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
	if now == -1 || unsafe { libc::localtime_r(&now, local.as_mut_ptr()) }.is_null() {
		return Err(io::Error::last_os_error());
	}

	let local = unsafe { local.assume_init() };

	Ok(format!(
		"{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
		local.tm_year + 1900,
		local.tm_mon + 1,
		local.tm_mday,
		local.tm_hour,
		local.tm_min,
		local.tm_sec
	))
}

fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
	let source = CString::new(source.as_os_str().as_bytes())?;
	let destination = CString::new(destination.as_os_str().as_bytes())?;

	// Unlike std::fs::rename, this also protects orphaned files (with no info
	// entry) and entries placed by a concurrent writer. Never emulate by copy.
	if unsafe {
		libc::renameat2(
			libc::AT_FDCWD,
			source.as_ptr(),
			libc::AT_FDCWD,
			destination.as_ptr(),
			libc::RENAME_NOREPLACE,
		)
	} == -1
	{
		return Err(io::Error::last_os_error());
	}

	Ok(())
}

fn move_to_trash(source: &Path, location: &TrashLocation) -> io::Result<PathBuf> {
	check_trash_relationship(source, &location.directory)?;

	let original = match &location.topdir {
		Some(topdir) => source.strip_prefix(topdir).map_err(|_| {
			io::Error::new(
				io::ErrorKind::InvalidInput,
				"Source is outside trash filesystem",
			)
		})?,
		None => source,
	};
	let contents = format!(
		"[Trash Info]\nPath={}\nDeletionDate={}\n",
		encode_path(original),
		deletion_date()?
	);

	let name = source
		.file_name()
		.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Source has no filename"))?;

	// Leave room for collision suffixes and .trashinfo on typical Linux
	// filesystems. Original names are recovered exclusively from Path above.
	let prefix = &name.as_bytes()[..name.len().min(180)];

	for counter in 0_u64.. {
		let mut name = OsString::from_vec(prefix.to_vec());
		if counter != 0 {
			name.push(format!(".{counter}"));
		}
		let destination = location.directory.join("files").join(&name);
		name.push(".trashinfo");
		let info_path = location.directory.join("info").join(name);

		let mut info = match fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.mode(0o600)
			.open(&info_path)
		{
			Ok(file) => file,
			Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
			Err(error) => return Err(error),
		};

		let result = info
			.write_all(contents.as_bytes())
			.and_then(|()| info.sync_all())
			.and_then(|()| rename_no_replace(source, &destination));

		match result {
			Ok(()) => return Ok(destination),
			Err(error) => {
				// Only remove the metadata we reserved, never any source or
				// destination entry. Surface cleanup failures rather than hide them.
				fs::remove_file(&info_path).map_err(|cleanup| {
					io::Error::new(
						cleanup.kind(),
						format!(
							"{error}; cleaning up {} failed: {cleanup}",
							info_path.display()
						),
					)
				})?;

				if error.kind() != io::ErrorKind::AlreadyExists {
					return Err(error);
				}
			}
		}
	}

	unreachable!("exhausted trash collision counter")
}

#[cfg(test)]
mod test {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::test_support::TestDir;
	use std::os::unix::fs::{PermissionsExt, symlink};

	fn local_trash(root: &Path) -> TrashLocation {
		let directory = root.join("trash");
		prepare_trash_dir(&directory).unwrap();

		TrashLocation {
			directory,
			topdir: None,
		}
	}

	fn metadata_for(destination: &Path) -> String {
		let mut name = destination.file_name().unwrap().to_os_string();
		name.push(".trashinfo");

		fs::read_to_string(
			destination
				.parent()
				.unwrap()
				.parent()
				.unwrap()
				.join("info")
				.join(name),
		)
		.unwrap()
	}

	fn test_mounts(topdir: &Path) -> Mountpoints {
		Mountpoints {
			points: vec![Mountpoint {
				path: topdir.to_owned(),
			}],
		}
	}

	#[test]
	fn detection_includes_home_root_and_contents_without_creating_directories() {
		let dir = TestDir::new();
		let mounts = test_mounts(dir.path());
		let home = dir.path().join("data/Trash");
		let ordinary = dir.path().join("ordinary");
		fs::write(&ordinary, "data").unwrap();

		assert!(!mounts.is_in_trash(&ordinary, &home).unwrap());
		assert!(!home.parent().unwrap().exists());

		fs::create_dir(home.parent().unwrap()).unwrap();
		prepare_private_dir(&home).unwrap();

		// Discovery must not require or create files/info in an existing root.
		assert!(mounts.is_in_trash(&home, &home).unwrap());
		assert_eq!(fs::read_dir(&home).unwrap().count(), 0);

		prepare_trash_dir(&home).unwrap();
		let nested = home.join("files/folder");
		fs::create_dir(&nested).unwrap();
		fs::write(nested.join("item"), "data").unwrap();

		assert!(mounts.is_in_trash(&nested.join("item"), &home).unwrap());
		assert!(mounts.is_in_trash(&home.join("info"), &home).unwrap());

		let sibling = home.with_file_name("Trash-old");
		fs::create_dir(&sibling).unwrap();

		assert!(!mounts.is_in_trash(&sibling, &home).unwrap());
		assert!(!mounts.is_in_trash(home.parent().unwrap(), &home).unwrap());
	}

	#[test]
	fn detection_checks_both_topdir_trashes_even_on_home_device() {
		let dir = TestDir::new();
		let mounts = test_mounts(dir.path());
		let home = dir.path().join("absent/data/Trash");
		let uid = unsafe { libc::getuid() };
		let shared = dir.path().join(".Trash");
		fs::create_dir(&shared).unwrap();
		fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
		let user = shared.join(uid.to_string());
		let private = dir.path().join(format!(".Trash-{uid}"));

		for root in [&user, &private] {
			prepare_trash_dir(root).unwrap();
			let file = root.join("files/item");
			fs::write(&file, "data").unwrap();

			assert!(mounts.is_in_trash(root, &home).unwrap());
			assert!(mounts.is_in_trash(&file, &home).unwrap());
		}

		assert!(mounts.is_in_trash(&shared, &home).unwrap());

		fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();

		assert!(mounts.is_in_trash(&user, &home).unwrap());
		assert!(mounts.is_in_trash(&private, &home).unwrap());
		assert!(!dir.path().join("absent").exists());
	}

	#[test]
	fn detection_preserves_symlink_identity_and_resolves_parent_aliases() {
		let dir = TestDir::new();
		let mounts = test_mounts(dir.path());
		let data = dir.path().join("data");
		fs::create_dir(&data).unwrap();
		let home = data.join("Trash");
		prepare_trash_dir(&home).unwrap();

		let outside = dir.path().join("outside");
		symlink(home.join("files"), &outside).unwrap();

		assert!(!mounts.is_in_trash(&outside, &home).unwrap());

		let inside = home.join("files/dangling");
		symlink("missing", &inside).unwrap();

		assert!(mounts.is_in_trash(&inside, &home).unwrap());

		let alias = dir.path().join("data-alias");
		symlink(&data, &alias).unwrap();

		assert!(mounts.is_in_trash(&inside, &alias.join("Trash")).unwrap());
		assert!(
			mounts
				.is_in_trash(&alias.join("Trash/files/dangling"), &home)
				.unwrap()
		);
	}

	#[test]
	fn detection_protects_unsafe_roots_but_preserves_symlink_identity() {
		let dir = TestDir::new();
		let mounts = test_mounts(dir.path());
		let home = dir.path().join("Trash");
		fs::create_dir(&home).unwrap();
		fs::set_permissions(&home, fs::Permissions::from_mode(0o755)).unwrap();

		assert!(mounts.is_in_trash(&home, &home).unwrap());
		assert_eq!(fs::read_dir(&home).unwrap().count(), 0);

		let uid = unsafe { libc::getuid() };
		let shared = dir.path().join(".Trash");
		symlink(&home, &shared).unwrap();
		let private = dir.path().join(format!(".Trash-{uid}"));
		symlink(&home, &private).unwrap();

		assert!(!mounts.is_in_trash(&private, &home).unwrap());
		assert!(!mounts.is_in_trash(dir.path(), &home).unwrap());
	}

	#[test]
	fn detection_protects_all_uid_trashes_before_destination_preparation() {
		let dir = TestDir::new();
		let mounts = test_mounts(dir.path());
		let home = dir.path().join("absent/data/Trash");

		// Deliberately permissive: safety detection must not require a trash
		// directory to pass destination validation, nor match our UID.
		for name in [
			".Trash",
			".Trash-0",
			".Trash-999",
			".Trash-1000",
			".Trash-00042",
		] {
			let root = dir.path().join(name);
			fs::create_dir(&root).unwrap();
			fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
			let nested = root.join("999/files");
			fs::create_dir_all(&nested).unwrap();
			let file = nested.join("item");
			fs::write(&file, "preserved").unwrap();

			for path in [&root, &nested, &file] {
				assert!(
					mounts.is_in_trash(path, &home).unwrap(),
					"{}",
					path.display()
				);
				assert_eq!(
					mounts.trash_location(path, &home).err().unwrap().kind(),
					io::ErrorKind::InvalidInput
				);
			}

			assert_eq!(fs::read_to_string(&file).unwrap(), "preserved");
		}

		assert!(!dir.path().join("absent").exists());
	}

	#[test]
	fn detection_does_not_reserve_project_or_non_numeric_names() {
		let dir = TestDir::new();
		let mounts = test_mounts(dir.path());
		let home = dir.path().join("absent/Trash");

		for name in [
			".Trash-",
			".Trash-old",
			".Trash-999-old",
			".Trash-+999",
			".Trash-９９９",
			".Trash-old/files",
			"project/.Trash",
			"project/.Trash-999",
		] {
			let path = dir.path().join(name);
			fs::create_dir_all(&path).unwrap();

			assert!(
				!mounts.is_in_trash(&path, &home).unwrap(),
				"{}",
				path.display()
			);
		}

		let file = dir.path().join(".Trash-12345");
		fs::write(&file, "not a folder").unwrap();

		assert!(!mounts.is_in_trash(&file, &home).unwrap());
		assert!(!dir.path().join("absent").exists());
	}

	#[test]
	fn mount_decoding_and_specificity() {
		assert_eq!(
			Mountpoint::from_line(b"device /mnt/raw\xff ext4 rw 0 0")
				.unwrap()
				.path
				.as_os_str()
				.as_bytes(),
			b"/mnt/raw\xff"
		);
		assert_eq!(
			decode_mount_path(r"/mnt/a\040b\134c\377")
				.unwrap()
				.as_os_str()
				.as_bytes(),
			b"/mnt/a b\\c\xff"
		);

		for invalid in [r"/a\", r"/a\09x", r"/a\777", r"/a\000"] {
			assert!(decode_mount_path(invalid).is_none());
		}

		let mounts = Mountpoints {
			points: vec![
				Mountpoint {
					path: "/mnt/data".into(),
				},
				Mountpoint { path: "/".into() },
			],
		};

		assert_eq!(
			mounts
				.mount_for_path(Path::new("/mnt/data/file"))
				.unwrap()
				.path,
			Path::new("/mnt/data")
		);
		assert_eq!(
			mounts
				.mount_for_path(Path::new("/mnt/database/file"))
				.unwrap()
				.path,
			Path::new("/")
		);
	}

	#[test]
	fn private_trash_created_without_shared_directory() {
		let dir = TestDir::new();
		let source = dir.path().join("file");
		fs::write(&source, "data").unwrap();

		let location = prepare_topdir_trash(&source, dir.path()).unwrap();

		assert_eq!(
			location.directory,
			dir.path()
				.join(format!(".Trash-{}", unsafe { libc::getuid() }))
		);
		assert!(!dir.path().join(".Trash").exists());

		for path in [
			&location.directory,
			&location.directory.join("files"),
			&location.directory.join("info"),
		] {
			assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o700);
		}
	}

	#[test]
	fn shared_trash_preferred_and_failure_falls_back() {
		let dir = TestDir::new();
		let shared = dir.path().join(".Trash");
		fs::create_dir(&shared).unwrap();
		fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
		let source = dir.path().join("file");
		let user = shared.join(unsafe { libc::getuid() }.to_string());

		assert_eq!(
			prepare_topdir_trash(&source, dir.path()).unwrap().directory,
			user
		);

		fs::set_permissions(user.join("info"), fs::Permissions::from_mode(0o755)).unwrap();

		assert_ne!(
			prepare_topdir_trash(&source, dir.path()).unwrap().directory,
			user
		);

		fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();

		assert!(!valid_shared_trash(&shared));
	}

	#[test]
	fn unsafe_directories_and_symlinks_rejected() {
		let dir = TestDir::new();
		let target = dir.path().join("target");
		fs::create_dir(&target).unwrap();
		fs::set_permissions(&target, fs::Permissions::from_mode(0o1777)).unwrap();
		let link = dir.path().join("link");
		symlink(&target, &link).unwrap();

		assert!(!valid_shared_trash(&link));
		assert!(prepare_trash_dir(&link).is_err());
		assert!(prepare_trash_dir(&target).is_err());

		let private = dir.path().join("private");
		prepare_private_dir(&private).unwrap();
		symlink(&target, private.join("files")).unwrap();

		assert!(prepare_trash_dir(&private).is_err());
	}

	#[test]
	fn invalid_shared_user_directory_falls_back_without_following_link() {
		let dir = TestDir::new();
		let shared = dir.path().join(".Trash");
		fs::create_dir(&shared).unwrap();
		fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
		let target = dir.path().join("target");
		prepare_private_dir(&target).unwrap();
		symlink(&target, shared.join(unsafe { libc::getuid() }.to_string())).unwrap();

		let location = prepare_topdir_trash(&dir.path().join("file"), dir.path()).unwrap();

		assert_eq!(
			location.directory,
			dir.path()
				.join(format!(".Trash-{}", unsafe { libc::getuid() }))
		);
		assert_eq!(fs::read_dir(target).unwrap().count(), 0);
	}

	#[test]
	fn normalized_paths_preserve_only_final_symlink() {
		let dir = TestDir::new();
		let real = dir.path().join("real");
		fs::create_dir(&real).unwrap();
		let link = dir.path().join("link");
		symlink(&real, &link).unwrap();

		assert_eq!(absolute_path(&link).unwrap(), link);
		assert_eq!(
			absolute_path(&link.join("file")).unwrap(),
			real.join("file")
		);
		assert_eq!(absolute_path(&link.join(".")).unwrap(), real);
		assert_eq!(absolute_path(&real.join("..")).unwrap(), dir.path());
		assert_eq!(absolute_path(Path::new("/")).unwrap(), Path::new("/"));
		assert!(absolute_path(Path::new("")).is_err());
	}

	#[test]
	fn home_selection_and_root_rejection() {
		let dir = TestDir::new();
		let source = dir.path().join("file");
		fs::write(&source, "data").unwrap();
		let mounts = Mountpoints::from_proc_mounts().unwrap();
		let home = dir.path().join("data/Trash");

		assert_eq!(
			mounts.trash_location(&source, &home).unwrap().directory,
			home
		);

		let topdir = mounts.topdir_for_path(&source).unwrap();

		assert!(mounts.trash_location(&topdir, &home).is_err());
		assert!(mounts.trash_location(dir.path(), &home).is_err());
	}

	#[test]
	fn file_metadata_and_name_collisions() {
		let dir = TestDir::new();
		let location = local_trash(dir.path());
		let source = dir.path().join("file %\n.txt");
		fs::write(&source, "first").unwrap();
		fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
		let before = fs::metadata(&source).unwrap();

		let first = move_to_trash(&source, &location).unwrap();

		let after = fs::metadata(&first).unwrap();
		assert!(!source.exists());
		assert_eq!(before.ino(), after.ino());
		assert_eq!(before.mode(), after.mode());
		assert_eq!(before.mtime(), after.mtime());

		let metadata = metadata_for(&first);
		assert!(metadata.starts_with(&format!(
			"[Trash Info]\nPath={}\nDeletionDate=",
			encode_path(&source)
		)));
		assert!(metadata.contains("file%20%25%0A.txt"));

		let date = metadata
			.lines()
			.last()
			.unwrap()
			.strip_prefix("DeletionDate=")
			.unwrap();
		assert_eq!(date.len(), 19);
		assert_eq!(&date[10..11], "T");

		fs::write(&source, "second").unwrap();

		let second = move_to_trash(&source, &location).unwrap();

		assert_ne!(first, second);
		assert_eq!(fs::read_to_string(first).unwrap(), "first");
		assert_eq!(fs::read_to_string(second).unwrap(), "second");
	}

	#[test]
	fn orphaned_destination_never_overwritten() {
		let dir = TestDir::new();
		let location = local_trash(dir.path());
		let source = dir.path().join("file");
		fs::write(&source, "new").unwrap();
		let orphan = location.directory.join("files/file");
		fs::write(&orphan, "old").unwrap();

		let destination = move_to_trash(&source, &location).unwrap();

		assert_ne!(destination, orphan);
		assert_eq!(fs::read_to_string(orphan).unwrap(), "old");
		assert!(!location.directory.join("info/file.trashinfo").exists());
	}

	#[test]
	fn long_original_names_fit_metadata_filename_limit() {
		let dir = TestDir::new();
		let location = local_trash(dir.path());
		let original_name = "x".repeat(255);
		let source = dir.path().join(&original_name);
		fs::write(&source, "data").unwrap();

		let destination = move_to_trash(&source, &location).unwrap();

		assert!(metadata_for(&destination).contains(&original_name));
		assert_eq!(fs::read_to_string(destination).unwrap(), "data");
	}

	#[test]
	fn symlinks_directories_and_non_utf8_paths() {
		let dir = TestDir::new();
		let location = local_trash(dir.path());
		let name = OsString::from_vec(b"link\xff".to_vec());
		let source = dir.path().join(name);
		symlink("missing-target", &source).unwrap();

		assert_eq!(absolute_path(&source).unwrap(), source);

		let destination = move_to_trash(&source, &location).unwrap();

		assert_eq!(
			fs::read_link(&destination).unwrap(),
			Path::new("missing-target")
		);
		assert!(metadata_for(&destination).contains("link%FF"));

		let source = dir.path().join("folder");
		fs::create_dir(&source).unwrap();
		fs::write(source.join("child"), "data").unwrap();

		let destination = move_to_trash(&source, &location).unwrap();

		assert_eq!(
			fs::read_to_string(destination.join("child")).unwrap(),
			"data"
		);
	}

	#[test]
	fn topdir_metadata_is_relative() {
		let dir = TestDir::new();
		let source = dir.path().join("file");
		fs::write(&source, "data").unwrap();
		let location = prepare_topdir_trash(&source, dir.path()).unwrap();

		let destination = move_to_trash(&source, &location).unwrap();

		assert!(metadata_for(&destination).starts_with("[Trash Info]\nPath=file\n"));
	}

	#[test]
	fn failures_preserve_source_and_clean_reserved_info() {
		let dir = TestDir::new();
		let location = local_trash(dir.path());
		let source = dir.path().join("file");
		fs::write(&source, "data").unwrap();

		// Renaming the source into its descendant fails even for root.
		let folder = dir.path().join("folder");
		fs::create_dir(&folder).unwrap();
		let nested = local_trash(&folder);

		assert!(move_to_trash(&folder, &nested).is_err());
		assert!(folder.exists());
		assert!(move_to_trash(&dir.path().join("missing"), &location).is_err());
		assert_eq!(
			fs::read_dir(location.directory.join("info"))
				.unwrap()
				.count(),
			0
		);

		// An invalid files directory fails after reserving info.
		let broken = dir.path().join("broken");
		fs::create_dir(&broken).unwrap();
		prepare_private_dir(&broken.join("info")).unwrap();
		fs::write(broken.join("files"), "not a directory").unwrap();

		assert!(
			move_to_trash(
				&source,
				&TrashLocation {
					directory: broken.clone(),
					topdir: None
				}
			)
			.is_err()
		);
		assert_eq!(fs::read_to_string(&source).unwrap(), "data");
		assert_eq!(fs::read_dir(broken.join("info")).unwrap().count(), 0);
	}

	#[test]
	fn concurrent_reservations_are_unique() {
		let dir = TestDir::new();
		let location = local_trash(dir.path());

		let destinations = std::thread::scope(|scope| {
			let mut threads = Vec::new();

			for index in 0..8 {
				let parent = dir.path().join(index.to_string());
				fs::create_dir(&parent).unwrap();
				let source = parent.join("same-name");
				fs::write(&source, index.to_string()).unwrap();

				let location = &location;
				threads.push(scope.spawn(move || move_to_trash(&source, location).unwrap()));
			}

			threads
				.into_iter()
				.map(|thread| thread.join().unwrap())
				.collect::<std::collections::HashSet<_>>()
		});

		assert_eq!(destinations.len(), 8);
		for destination in destinations {
			assert!(metadata_for(&destination).contains("DeletionDate="));
		}
	}
}
