//! Read-only discovery and listing. Metadata is descriptive, not authorization
//! for a future restore/delete: those operations must revalidate their targets.

use std::{
	collections::HashSet,
	fs, io,
	io::Read,
	os::unix::{
		ffi::OsStringExt,
		fs::{MetadataExt, OpenOptionsExt},
	},
	path::{Component, Path, PathBuf},
};

use super::{Mountpoints, TrashLocation, base_trash_dir, valid_shared_trash, validate_private_dir};

#[derive(Debug, Default)]
pub(crate) struct TrashListing {
	pub entries: Vec<TrashEntry>,
	/// Discovery/read failures that did not prevent listing other locations.
	pub issues: Vec<TrashIssue>,
}

#[derive(Debug)]
pub(crate) struct TrashEntry {
	pub stored_path: PathBuf,
	/// Expected metadata path, even when the metadata is missing or unreadable.
	pub info_path: PathBuf,
	/// Does not follow symlinks. None if the entry changed or could not be read.
	pub file_type: Option<fs::FileType>,
	/// Never inferred from the storage filename. None for missing/invalid Path.
	pub original_path: Option<PathBuf>,
	/// Validated local wall-clock time, YYYY-MM-DDThh:mm:ss, with no timezone.
	pub deletion_date: Option<String>,
	/// Nonempty means damaged metadata or an entry that could not be inspected.
	pub issues: Vec<TrashIssue>,
}

#[derive(Debug)]
pub(crate) struct TrashIssue {
	pub path: PathBuf,
	pub error: io::Error,
}

/// List the current user's home trash and both trash locations on every known
/// mount. Directories are single entries; their contents are not traversed.
/// Missing locations are normal. Damaged entries remain visible with issues.
/// Never creates trash directories. Unmounted volumes and undiscovered Btrfs
/// subvolumes are not scanned. The result is a snapshot, not an atomic view.
pub(crate) fn list_trash() -> io::Result<TrashListing> {
	let mounts = Mountpoints::from_proc_mounts()?;
	Ok(list_from_mounts(&mounts, &base_trash_dir()?))
}

fn list_from_mounts(mounts: &Mountpoints, home: &Path) -> TrashListing {
	let mut listing = TrashListing::default();
	let mut seen = HashSet::new();
	list_location(
		&TrashLocation {
			directory: home.to_owned(),
			topdir: None,
		},
		&mut seen,
		&mut listing,
	);
	let uid = unsafe { libc::getuid() };
	for mount in &mounts.points {
		let shared = mount.path.join(".Trash");
		match fs::symlink_metadata(&shared) {
			Ok(_) if valid_shared_trash(&shared) => {
				list_location(
					&TrashLocation {
						directory: shared.join(uid.to_string()),
						topdir: Some(mount.path.clone()),
					},
					&mut seen,
					&mut listing,
				);
			}
			Ok(_) => listing.issues.push(issue(
				&shared,
				invalid("Shared .Trash is a symlink, not a directory, or lacks the sticky bit"),
			)),
			Err(error) if absent(&error) => {}
			Err(error) => listing.issues.push(issue(&shared, error)),
		}
		list_location(
			&TrashLocation {
				directory: mount.path.join(format!(".Trash-{uid}")),
				topdir: Some(mount.path.clone()),
			},
			&mut seen,
			&mut listing,
		);
	}
	listing
		.entries
		.sort_by(|a, b| a.stored_path.cmp(&b.stored_path));
	listing
}

fn absent(error: &io::Error) -> bool {
	matches!(
		error.kind(),
		io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
	)
}

fn invalid(message: &str) -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, message)
}

fn issue(path: &Path, error: io::Error) -> TrashIssue {
	TrashIssue {
		path: path.to_owned(),
		error,
	}
}

fn list_location(
	location: &TrashLocation, seen: &mut HashSet<(u64, u64)>, listing: &mut TrashListing,
) {
	let result = (|| {
		validate_private_dir(&location.directory)?;
		let metadata = fs::symlink_metadata(&location.directory)?;
		// A bind mount or repeated mount-table entry can expose the same trash.
		if !seen.insert((metadata.dev(), metadata.ino())) {
			return Ok(());
		}
		let directory = fs::canonicalize(&location.directory)?;
		let location = TrashLocation {
			directory,
			topdir: location.topdir.clone(),
		};
		let files = location.directory.join("files");
		validate_private_dir(&files).map_err(|error| {
			io::Error::new(error.kind(), format!("{}: {error}", files.display()))
		})?;
		let entries = fs::read_dir(&files)?;
		let info = location.directory.join("info");
		let info_error = validate_private_dir(&info).err();
		if let Some(error) = &info_error {
			listing.issues.push(issue(
				&info,
				io::Error::new(error.kind(), error.to_string()),
			));
		}
		for entry in entries {
			let entry = match entry {
				Ok(entry) => entry,
				Err(error) => {
					listing.issues.push(issue(&files, error));
					continue;
				}
			};
			let stored_path = entry.path();
			let mut name = entry.file_name();
			name.push(".trashinfo");
			let info_path = info.join(name);
			let mut item = TrashEntry {
				stored_path,
				info_path,
				file_type: None,
				original_path: None,
				deletion_date: None,
				issues: Vec::new(),
			};
			match entry.file_type() {
				Ok(kind) => item.file_type = Some(kind),
				Err(error) => item.issues.push(issue(&item.stored_path, error)),
			}
			if let Some(error) = &info_error {
				item.issues.push(issue(
					&item.info_path,
					io::Error::new(error.kind(), error.to_string()),
				));
			} else {
				match read_info(&item.info_path) {
					Ok(bytes) => {
						let parsed = parse_info(&bytes, &location);
						item.original_path = parsed.original_path;
						item.deletion_date = parsed.deletion_date;
						item.issues.extend(
							parsed
								.errors
								.into_iter()
								.map(|error| issue(&item.info_path, error)),
						);
					}
					Err(error) => item.issues.push(issue(&item.info_path, error)),
				}
			}
			listing.entries.push(item);
		}
		Ok(())
	})();
	if let Err(error) = result {
		// A missing root is normal, but missing files/ inside an existing root
		// is corruption and must be reported rather than silently hidden.
		if !absent(&error) || fs::symlink_metadata(&location.directory).is_ok() {
			listing.issues.push(issue(&location.directory, error));
		}
	}
}

fn read_info(path: &Path) -> io::Result<Vec<u8>> {
	// Do not follow metadata symlinks or block opening FIFOs/device nodes.
	let file = fs::OpenOptions::new()
		.read(true)
		.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
		.open(path)?;
	if !file.metadata()?.is_file() {
		return Err(invalid("Trash metadata is not a regular file"));
	}
	// Metadata is tiny; bound reads of corrupt or deliberately oversized files.
	const LIMIT: u64 = 64 * 1024;
	let mut bytes = Vec::new();
	file.take(LIMIT + 1).read_to_end(&mut bytes)?;
	if bytes.len() as u64 > LIMIT {
		return Err(invalid("Trash metadata exceeds 64 KiB"));
	}
	Ok(bytes)
}

#[derive(Default)]
struct ParsedInfo {
	original_path: Option<PathBuf>,
	deletion_date: Option<String>,
	errors: Vec<io::Error>,
}

fn parse_info(bytes: &[u8], location: &TrashLocation) -> ParsedInfo {
	let mut parsed = ParsedInfo::default();
	let mut lines = bytes
		.split(|byte| *byte == b'\n')
		.map(|line| line.strip_suffix(b"\r").unwrap_or(line));
	if lines.next() != Some(b"[Trash Info]".as_slice()) {
		parsed
			.errors
			.push(invalid("Missing or invalid [Trash Info] header"));
		return parsed;
	}
	let (mut path, mut date) = (None, None);
	for line in lines {
		if let Some(value) = line.strip_prefix(b"Path=") {
			path.get_or_insert(value);
		} else if let Some(value) = line.strip_prefix(b"DeletionDate=") {
			date.get_or_insert(value);
		}
	}
	match path {
		Some(path) => match original_path(path, location) {
			Ok(path) => parsed.original_path = Some(path),
			Err(error) => parsed.errors.push(error),
		},
		None => parsed.errors.push(invalid("Missing Path")),
	}
	match date {
		Some(date) if valid_date(date) => {
			parsed.deletion_date = Some(String::from_utf8_lossy(date).into_owned())
		}
		Some(_) => parsed.errors.push(invalid("Invalid DeletionDate")),
		None => parsed.errors.push(invalid("Missing DeletionDate")),
	}
	parsed
}

fn original_path(encoded: &[u8], location: &TrashLocation) -> io::Result<PathBuf> {
	let mut decoded = Vec::with_capacity(encoded.len());
	let mut index = 0;
	while index < encoded.len() {
		let byte = if encoded[index] == b'%' {
			let digits = encoded
				.get(index + 1..index + 3)
				.ok_or_else(|| invalid("Truncated percent escape in Path"))?;
			let hex = |byte: u8| char::from(byte).to_digit(16).map(|digit| digit as u8);
			let high = hex(digits[0]).ok_or_else(|| invalid("Invalid percent escape in Path"))?;
			let low = hex(digits[1]).ok_or_else(|| invalid("Invalid percent escape in Path"))?;
			index += 3;
			high * 16 + low
		} else {
			let byte = encoded[index];
			index += 1;
			byte
		};
		if byte == 0 {
			return Err(invalid("NUL in Path"));
		}
		decoded.push(byte);
	}
	let path = PathBuf::from(std::ffi::OsString::from_vec(decoded));
	if path.file_name().is_none()
		|| path
			.components()
			.any(|component| component == Component::ParentDir)
	{
		return Err(invalid(
			"Path is empty, a root, or contains parent traversal",
		));
	}
	if path.is_absolute() {
		if location.topdir.is_some() {
			return Err(invalid("Absolute Path in filesystem-local trash"));
		}
		return Ok(path);
	}
	let base = location
		.topdir
		.as_deref()
		.or_else(|| location.directory.parent())
		.ok_or_else(|| invalid("Trash has no original-path base directory"))?;
	// Do not canonicalize: original parents may no longer exist. A future
	// restore operation must separately validate symlinks and permissions.
	Ok(base.join(path))
}

fn valid_date(date: &[u8]) -> bool {
	if date.len() != 19 {
		return false;
	}
	for (index, &byte) in date.iter().enumerate() {
		let expected = match index {
			4 | 7 => b'-',
			10 => b'T',
			13 | 16 => b':',
			_ => {
				if !byte.is_ascii_digit() {
					return false;
				}
				continue;
			}
		};
		if byte != expected {
			return false;
		}
	}
	let number = |start: usize, end: usize| {
		date[start..end]
			.iter()
			.fold(0_u32, |number, byte| number * 10 + u32::from(byte - b'0'))
	};
	let year = number(0, 4);
	let month = number(5, 7);
	let day = number(8, 10);
	let days = match month {
		4 | 6 | 9 | 11 => 30,
		2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
		2 => 28,
		1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
		_ => return false,
	};
	year != 0
		&& (1..=days).contains(&day)
		&& number(11, 13) < 24
		&& number(14, 16) < 60
		&& number(17, 19) <= 60
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::test_support::TestDir;
	use crate::trash::{Mountpoint, move_to_trash, prepare_private_dir, prepare_trash_dir};
	use std::os::unix::{
		ffi::OsStrExt,
		fs::{PermissionsExt, symlink},
	};

	fn mounts(paths: &[&Path]) -> Mountpoints {
		Mountpoints {
			points: paths
				.iter()
				.map(|path| Mountpoint {
					path: path.to_path_buf(),
				})
				.collect(),
		}
	}

	fn location(directory: PathBuf, topdir: Option<PathBuf>) -> TrashLocation {
		fs::create_dir_all(directory.parent().unwrap()).unwrap();
		prepare_trash_dir(&directory).unwrap();
		TrashLocation { directory, topdir }
	}

	fn item(location: &TrashLocation, name: &str, info: Option<&[u8]>) -> PathBuf {
		let path = location.directory.join("files").join(name);
		fs::write(&path, "preserved").unwrap();
		if let Some(bytes) = info {
			fs::write(
				location
					.directory
					.join("info")
					.join(format!("{name}.trashinfo")),
				bytes,
			)
			.unwrap();
		}
		path
	}

	const INFO: &[u8] = b"[Trash Info]\nPath=documents/item\nDeletionDate=2026-11-08T12:34:56\n";

	#[test]
	fn listing_roundtrip_preserves_original_bytes_types_and_metadata() {
		let dir = TestDir::new();
		let home = location(dir.path().join("data/Trash"), None);
		let original = dir
			.path()
			.join(std::ffi::OsString::from_vec(b"original %\xff\n".to_vec()));
		fs::write(&original, "data").unwrap();
		let stored = move_to_trash(&original, &home).unwrap();
		fs::create_dir(home.directory.join("files/folder")).unwrap();
		fs::write(home.directory.join("files/folder/child"), "child").unwrap();
		symlink("missing", home.directory.join("files/dangling")).unwrap();
		let result = list_from_mounts(&mounts(&[dir.path()]), &home.directory);
		assert!(result.issues.is_empty());
		assert_eq!(result.entries.len(), 3);
		let entry = result
			.entries
			.iter()
			.find(|entry| entry.stored_path == stored)
			.unwrap();
		assert_eq!(entry.original_path.as_ref(), Some(&original));
		assert_eq!(
			entry.original_path.as_ref().unwrap().as_os_str().as_bytes(),
			original.as_os_str().as_bytes()
		);
		assert!(entry.deletion_date.is_some());
		assert!(entry.file_type.unwrap().is_file());
		assert!(entry.issues.is_empty());
		let folder = result
			.entries
			.iter()
			.find(|entry| entry.stored_path.ends_with("folder"))
			.unwrap();
		assert!(folder.file_type.unwrap().is_dir());
		assert!(folder.original_path.is_none());
		assert_eq!(folder.issues[0].error.kind(), io::ErrorKind::NotFound);
		let link = result
			.entries
			.iter()
			.find(|entry| entry.stored_path.ends_with("dangling"))
			.unwrap();
		assert!(link.file_type.unwrap().is_symlink());
		assert!(fs::symlink_metadata(&link.stored_path).is_ok());
	}

	#[test]
	fn missing_and_corrupt_metadata_keep_data_visible_and_partial_fields() {
		let dir = TestDir::new();
		let home = location(dir.path().join("Trash"), None);
		item(&home, "missing", None);
		item(
			&home,
			"bad-header",
			Some(b"not trash\nPath=/valid\nDeletionDate=2026-11-08T12:34:56\n"),
		);
		item(
			&home,
			"bad-path",
			Some(b"[Trash Info]\nPath=%GG\nDeletionDate=2026-11-08T12:34:56\n"),
		);
		item(
			&home,
			"bad-date",
			Some(b"[Trash Info]\nPath=/real/original\nDeletionDate=2026-02-30T12:34:56\n"),
		);
		item(&home, "empty", Some(b""));
		// Info alone is not a trashed item (and can be an in-progress move).
		fs::write(home.directory.join("info/orphan.trashinfo"), INFO).unwrap();
		let result = list_from_mounts(&mounts(&[dir.path()]), &home.directory);
		assert_eq!(result.entries.len(), 5);
		for entry in &result.entries {
			assert!(!entry.issues.is_empty());
			assert_eq!(fs::read_to_string(&entry.stored_path).unwrap(), "preserved");
			if entry.stored_path.ends_with("bad-date") {
				assert_eq!(
					entry.original_path.as_deref(),
					Some(Path::new("/real/original"))
				);
				assert!(entry.deletion_date.is_none());
			} else {
				assert!(entry.original_path.is_none());
			}
			if entry.stored_path.ends_with("bad-path") {
				assert!(entry.deletion_date.is_some());
			}
		}
	}

	#[test]
	fn discovers_both_current_uid_roots_and_deduplicates_mount_aliases() {
		let dir = TestDir::new();
		let topdir = dir.path().join("USB Stick");
		fs::create_dir(&topdir).unwrap();
		let uid = unsafe { libc::getuid() };
		let shared = topdir.join(".Trash");
		fs::create_dir(&shared).unwrap();
		fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
		let user = location(shared.join(uid.to_string()), Some(topdir.clone()));
		let private = location(topdir.join(format!(".Trash-{uid}")), Some(topdir.clone()));
		item(&user, "shared", Some(INFO));
		item(&private, "private", Some(INFO));
		let other_uid = uid.wrapping_add(1);
		let other = location(
			topdir.join(format!(".Trash-{other_uid}")),
			Some(topdir.clone()),
		);
		item(&other, "not-ours", Some(INFO));
		let other = location(shared.join(other_uid.to_string()), Some(topdir.clone()));
		item(&other, "not-ours", Some(INFO));
		let alias = dir.path().join("alias");
		symlink(&topdir, &alias).unwrap();
		let home = dir.path().join("absent/data/Trash");
		let result = list_from_mounts(&mounts(&[&topdir, &topdir, &alias]), &home);
		assert!(result.issues.is_empty(), "{:?}", result.issues);
		assert_eq!(result.entries.len(), 2);
		for entry in result.entries {
			assert_eq!(entry.original_path, Some(topdir.join("documents/item")));
			assert!(entry.issues.is_empty());
		}
		assert!(!dir.path().join("absent").exists());
	}

	#[test]
	fn parses_first_keys_unknown_lines_crlf_and_relative_bases() {
		let dir = TestDir::new();
		let home = location(dir.path().join("data/Trash"), None);
		let parsed = parse_info(b"[Trash Info]\r\nUnknown=\xff\r\nPath=dir/a%20b%ff\r\nPath=/wrong\r\nDeletionDate=2024-02-29T23:59:59\r\nDeletionDate=bad\r\n", &home);
		assert!(parsed.errors.is_empty());
		assert_eq!(
			parsed.original_path.unwrap(),
			dir.path()
				.join("data/dir")
				.join(std::ffi::OsString::from_vec(b"a b\xff".to_vec()))
		);
		let parsed = parse_info(b"[Trash Info]\nPath=%GG\nPath=/valid\nDeletionDate=bad\nDeletionDate=2024-02-29T23:59:59\n", &home);
		assert!(parsed.original_path.is_none());
		assert!(parsed.deletion_date.is_none());
		assert_eq!(parsed.errors.len(), 2);
		let local = TrashLocation {
			directory: dir.path().join(".Trash/1000"),
			topdir: Some(dir.path().to_owned()),
		};
		assert_eq!(
			parse_info(INFO, &local).original_path.unwrap(),
			dir.path().join("documents/item")
		);
	}

	#[test]
	fn rejects_unsafe_original_paths_and_invalid_calendar_dates() {
		let dir = TestDir::new();
		let home = TrashLocation {
			directory: dir.path().join("Trash"),
			topdir: None,
		};
		for encoded in [
			b"".as_slice(),
			b"/",
			b".",
			b"../escape",
			b"dir/%2e%2e/escape",
			b"/dir/../escape",
			b"null%00byte",
			b"bad%",
			b"bad%0",
			b"bad%xy",
		] {
			assert!(original_path(encoded, &home).is_err(), "{encoded:?}");
		}
		let local = TrashLocation {
			directory: dir.path().join(".Trash-1000"),
			topdir: Some(dir.path().to_owned()),
		};
		assert!(original_path(b"/outside", &local).is_err());
		for date in [
			"2026-02-29T12:00:00",
			"1900-02-29T00:00:00",
			"2026-00-01T00:00:00",
			"2026-01-00T00:00:00",
			"2026-04-31T00:00:00",
			"2026-01-01T24:00:00",
			"2026-01-01T00:60:00",
			"2026-01-01T00:00:61",
			"2026-01-01 00:00:00",
			"2026-01-01T00:00:00Z",
		] {
			assert!(!valid_date(date.as_bytes()), "{date}");
		}
		assert!(valid_date(b"2000-02-29T00:00:00"));
	}

	#[test]
	fn missing_info_directory_keeps_entries_and_is_not_created() {
		let dir = TestDir::new();
		let home = dir.path().join("Trash");
		prepare_private_dir(&home).unwrap();
		prepare_private_dir(&home.join("files")).unwrap();
		fs::write(home.join("files/entry"), "data").unwrap();
		let result = list_from_mounts(&mounts(&[dir.path()]), &home);
		assert_eq!(result.entries.len(), 1);
		assert!(result.entries[0].original_path.is_none());
		assert_eq!(
			result.entries[0].issues[0].error.kind(),
			io::ErrorKind::NotFound
		);
		assert_eq!(result.issues[0].path, home.join("info"));
		assert!(!home.join("info").exists());
	}

	#[test]
	fn refuses_symlink_metadata_special_files_and_oversized_info() {
		let dir = TestDir::new();
		let home = location(dir.path().join("Trash"), None);
		for name in ["link", "fifo", "directory", "oversized"] {
			item(&home, name, None);
		}
		let outside = dir.path().join("outside-info");
		fs::write(&outside, INFO).unwrap();
		symlink(&outside, home.directory.join("info/link.trashinfo")).unwrap();
		let fifo = std::ffi::CString::new(
			home.directory
				.join("info/fifo.trashinfo")
				.as_os_str()
				.as_bytes(),
		)
		.unwrap();
		assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
		fs::create_dir(home.directory.join("info/directory.trashinfo")).unwrap();
		fs::write(
			home.directory.join("info/oversized.trashinfo"),
			vec![b'x'; 65537],
		)
		.unwrap();
		let result = list_from_mounts(&mounts(&[dir.path()]), &home.directory);
		assert_eq!(result.entries.len(), 4);
		assert!(
			result
				.entries
				.iter()
				.all(|entry| entry.original_path.is_none() && !entry.issues.is_empty())
		);
		assert_eq!(fs::read(&outside).unwrap(), INFO);
	}

	#[test]
	fn location_errors_do_not_hide_other_trash_and_symlink_dirs_are_not_followed() {
		let dir = TestDir::new();
		let good = location(dir.path().join("Trash"), None);
		item(&good, "good", Some(INFO));
		let topdir = dir.path().join("volume");
		fs::create_dir(&topdir).unwrap();
		symlink(&good.directory, topdir.join(".Trash")).unwrap();
		let bad = location(
			topdir.join(format!(".Trash-{}", unsafe { libc::getuid() })),
			Some(topdir.clone()),
		);
		// Replace the empty files directory with a symlink, without touching
		// the referenced storage. It must not duplicate the home entry.
		fs::remove_dir(bad.directory.join("files")).unwrap();
		symlink(good.directory.join("files"), bad.directory.join("files")).unwrap();
		let result = list_from_mounts(&mounts(&[&topdir]), &good.directory);
		assert_eq!(result.entries.len(), 1);
		assert_eq!(result.issues.len(), 2);
		assert!(
			result
				.issues
				.iter()
				.any(|problem| problem.path == topdir.join(".Trash"))
		);
		assert!(good.directory.join("files/good").exists());
	}
}
