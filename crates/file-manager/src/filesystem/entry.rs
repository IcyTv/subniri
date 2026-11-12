use std::{
	fs,
	path::{Path, PathBuf},
};

use iced::widget::{Id, image::Handle};
use xdg_mime::SharedMimeInfo;

#[derive(Debug, Clone)]
pub struct DirEntry {
	pub name: String,
	pub path: PathBuf,
	pub size: u64,
	pub id: Id,
	pub kind: FileKind,
	pub thumbnail: Option<Handle>,
	pub is_dir: bool,
	pub is_hidden: bool,
}

impl DirEntry {
	pub fn from_path(db: &SharedMimeInfo, path: PathBuf) -> Result<Self, String> {
		let metadata = fs::metadata(&path)
			.or_else(|_| fs::symlink_metadata(&path))
			.map_err(|e| format!("{}: {e}", path.display()))?;
		let is_dir = metadata.is_dir();
		let name = path
			.file_name()
			.unwrap_or_default()
			.to_string_lossy()
			.into_owned();
		let is_hidden = name.starts_with('.');

		let kind = file_kind(&path, db);
		let thumbnail = if !is_dir && kind == FileKind::Image {
			thumbnail(&path)
		} else {
			None
		};

		Ok(Self {
			name,
			path,
			size: metadata.len(),
			id: Id::unique(),
			kind,
			thumbnail,
			is_dir,
			is_hidden,
		})
	}
}

pub fn read_directory(path: &Path) -> Result<Vec<DirEntry>, String> {
	let db = SharedMimeInfo::new();
	let read = fs::read_dir(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
	let mut entries = read
		.map(|result| {
			let entry =
				result.map_err(|e| format!("Cannot read an entry in {}: {e}", path.display()))?;
			DirEntry::from_path(&db, entry.path())
		})
		.collect::<Result<Vec<_>, _>>()?;
	entries.sort_by(|a, b| {
		b.is_dir
			.cmp(&a.is_dir)
			.then_with(|| a.path.file_name().cmp(&b.path.file_name()))
	});
	Ok(entries)
}

pub fn format_size(bytes: u64) -> String {
	if bytes < 1024 {
		format!("{bytes} B")
	} else if bytes < 1024 * 1024 {
		format!("{:.1} KB", bytes as f64 / 1024.0)
	} else if bytes < 1024 * 1024 * 1024 {
		format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
	} else {
		format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
	}
}

pub fn create_folder(parent: &Path, name: &str) -> Result<PathBuf, String> {
	validate_name(name)?;

	let path = parent.join(name);
	fs::create_dir(&path).map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
	Ok(path)
}

pub fn validate_name(name: &str) -> Result<(), String> {
	if name.trim().is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
		return Err("Enter a name other than . or .., without / or NUL.".into());
	}

	Ok(())
}

pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf, String> {
	validate_name(name)?;

	let path = parent.join(name);
	fs::OpenOptions::new()
		.write(true)
		.create_new(true)
		.open(&path)
		.map_err(|e| format!("Cannot create {}: {e}", path.display()))?;

	Ok(path)
}

/// Linux's exclusive rename refuses existing destinations atomically, including
/// dangling symlinks. Never fall back to std::fs::rename, which can overwrite.
pub fn rename(target: &crate::context_menu::Target, name: &str) -> Result<PathBuf, String> {
	use std::{ffi::CString, os::unix::ffi::OsStrExt};

	validate_name(name)?;
	if !target.available() {
		return Err("The item changed or disappeared. Refresh before renaming.".into());
	}

	let parent = target
		.path
		.parent()
		.ok_or("Cannot rename the filesystem root")?;
	let destination = parent.join(name);
	if destination == target.path {
		return Ok(destination);
	}

	let source = CString::new(target.path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
	let dest = CString::new(destination.as_os_str().as_bytes()).map_err(|e| e.to_string())?;

	let result = unsafe {
		libc::renameat2(
			libc::AT_FDCWD,
			source.as_ptr(),
			libc::AT_FDCWD,
			dest.as_ptr(),
			libc::RENAME_NOREPLACE,
		)
	};
	if result != 0 {
		return Err(format!(
			"Cannot rename {}: {}",
			target.path.display(),
			std::io::Error::last_os_error()
		));
	}

	Ok(destination)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
	Pdf,
	Code,
	Archive,
	Text,
	Image,
	Audio,
	Video,
	Document,
}

fn file_kind(path: &Path, db: &SharedMimeInfo) -> FileKind {
	let ext = path
		.extension()
		.and_then(|e| e.to_str())
		.map(|e| e.to_ascii_lowercase());

	if let Some(ref ext) = ext {
		match ext.as_str() {
			"pdf" => return FileKind::Pdf,
			"zip" | "7z" | "rar" | "tar" | "gz" | "xz" => return FileKind::Archive,
			"rs" | "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hh" | "cs" | "js" | "mjs"
			| "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" | "py" | "css" | "scss" | "sass"
			| "less" | "html" | "htm" | "vue" | "sql" | "md" | "markdown" | "ini" | "toml"
			| "conf" | "cfg" | "json" | "yaml" | "yml" | "nix" | "sh" => return FileKind::Code,
			"txt" | "log" | "csv" => return FileKind::Text,
			"png" | "jpg" | "jpeg" | "svg" | "webp" | "gif" | "avif" => return FileKind::Image,
			_ => (),
		}
	}

	let mut builder = db.guess_mime_type();
	let mime = builder.path(path).guess();
	let mime = mime.mime_type();

	match mime.type_().as_str() {
		"image" => FileKind::Image,
		"video" => FileKind::Video,
		"audio" => FileKind::Audio,
		"text" => FileKind::Text,
		_ => match (mime.type_().as_str(), mime.subtype().as_str()) {
			("application", "pdf") => FileKind::Pdf,
			("application", "zip" | "gzip" | "x-tar" | "x-7z-compressed" | "x-rar") => {
				FileKind::Archive
			}
			_ => FileKind::Document,
		},
	}
}

fn thumbnail(path: &Path) -> Option<Handle> {
	// Directory loading already runs on a worker; never decode full-size images
	// in the view. Unsupported/corrupt images retain the document-card fallback.
	let mut reader = image::ImageReader::open(path)
		.ok()?
		.with_guessed_format()
		.ok()?;
	let mut limits = image::Limits::default();
	limits.max_image_width = Some(8192);
	limits.max_image_height = Some(8192);
	limits.max_alloc = Some(64 * 1024 * 1024);
	reader.limits(limits);
	let pixels = reader.decode().ok()?.thumbnail(166, 128).to_rgba8();
	Some(Handle::from_rgba(
		pixels.width(),
		pixels.height(),
		pixels.into_raw(),
	))
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::test_support::TestDir;
	#[test]
	fn reads_real_metadata_and_sorts_folders_first() {
		let dir = TestDir::new();
		fs::write(dir.path().join("a.txt"), "abc").unwrap();
		fs::write(dir.path().join(".hidden"), "x").unwrap();
		fs::create_dir(dir.path().join("z-folder")).unwrap();
		let entries = read_directory(dir.path()).unwrap();
		assert_eq!(entries.first().unwrap().name, "z-folder");
		assert_eq!(entries.iter().find(|e| e.name == "a.txt").unwrap().size, 3);
		assert!(entries.iter().any(|e| e.is_hidden));
		assert!(read_directory(&dir.path().join("missing")).is_err());
	}
	#[test]
	fn folder_creation_rejects_escape_and_existing_names() {
		let dir = TestDir::new();
		for name in ["", " ", ".", "..", "../outside", "/absolute", "x\0y"] {
			assert!(create_folder(dir.path(), name).is_err());
		}
		let created = create_folder(dir.path(), "New folder").unwrap();
		assert!(created.is_dir());
		assert!(create_folder(dir.path(), "New folder").is_err());
	}

	#[test]
	fn empty_file_creation_never_truncates_files_or_follows_symlinks() {
		use std::os::unix::fs::symlink;

		let dir = TestDir::new();
		let existing = dir.path().join("existing");

		fs::write(&existing, "keep this").unwrap();
		symlink(&existing, dir.path().join("link")).unwrap();
		symlink(dir.path().join("missing"), dir.path().join("dangling")).unwrap();

		for name in [
			"",
			" ",
			".",
			"..",
			"../outside",
			"/absolute",
			"x\0y",
			"existing",
			"link",
			"dangling",
		] {
			assert!(create_file(dir.path(), name).is_err(), "accepted {name:?}");
		}

		let created = create_file(dir.path(), "New File").unwrap();

		assert_eq!(fs::metadata(created).unwrap().len(), 0);
		assert_eq!(fs::read_to_string(existing).unwrap(), "keep this");
		assert!(!dir.path().join("missing").exists());
	}

	#[test]
	fn rename_is_exclusive_and_preserves_file_folder_and_symlink_contents() {
		use crate::context_menu::{Context, Target};
		use std::os::unix::fs::{MetadataExt, symlink};

		let dir = TestDir::new();
		let source = dir.path().join("source.txt");

		fs::write(&source, "source data").unwrap();
		fs::write(dir.path().join("taken"), "destination data").unwrap();
		symlink(dir.path().join("missing"), dir.path().join("dangling")).unwrap();
		let target = Target::capture(&source, Context::File).unwrap();
		let inode = fs::metadata(&source).unwrap().ino();

		assert!(rename(&target, "taken").is_err());
		assert!(rename(&target, "dangling").is_err());
		assert!(rename(&target, "../outside").is_err());
		assert_eq!(rename(&target, "source.txt").unwrap(), source);

		let renamed = rename(&target, "renamed.txt").unwrap();

		assert_eq!(fs::metadata(&renamed).unwrap().ino(), inode);
		assert_eq!(fs::read_to_string(&renamed).unwrap(), "source data");
		assert_eq!(
			fs::read_to_string(dir.path().join("taken")).unwrap(),
			"destination data"
		);
		assert!(!source.exists());

		fs::create_dir(dir.path().join("folder")).unwrap();
		fs::write(dir.path().join("folder/child"), "child").unwrap();
		let folder = Target::capture(&dir.path().join("folder"), Context::Folder).unwrap();

		let moved = rename(&folder, "renamed folder").unwrap();

		assert_eq!(fs::read_to_string(moved.join("child")).unwrap(), "child");

		let link = Target::capture(&dir.path().join("dangling"), Context::File).unwrap();

		let moved_link = rename(&link, "renamed link").unwrap();

		assert_eq!(
			fs::read_link(moved_link).unwrap(),
			dir.path().join("missing")
		);
	}

	#[test]
	fn rename_rejects_an_item_replaced_since_editing_started() {
		use crate::context_menu::{Context, Target};

		let dir = TestDir::new();
		let source = dir.path().join("source");

		fs::write(&source, "original").unwrap();
		let target = Target::capture(&source, Context::File).unwrap();
		fs::rename(&source, dir.path().join("original")).unwrap();
		fs::write(&source, "replacement").unwrap();

		assert!(rename(&target, "renamed").is_err());
		assert_eq!(fs::read_to_string(source).unwrap(), "replacement");
		assert!(!dir.path().join("renamed").exists());
	}

	#[test]
	fn retains_non_utf8_path_identity() {
		use std::os::unix::ffi::OsStringExt;
		let dir = TestDir::new();
		let path = dir
			.path()
			.join(std::ffi::OsString::from_vec(vec![b'a', 0xff]));
		fs::write(&path, "x").unwrap();
		assert_eq!(
			read_directory(dir.path()).unwrap().first().unwrap().path,
			path
		);
	}

	#[test]
	fn file_art_classification_and_bounded_thumbnail_decode() {
		let dir = TestDir::new();
		let db = SharedMimeInfo::new();
		assert_eq!(file_kind(Path::new("report.pdf"), &db), FileKind::Pdf);
		assert_eq!(file_kind(Path::new("main.rs"), &db), FileKind::Code);
		assert_eq!(file_kind(Path::new("backup.zip"), &db), FileKind::Archive);
		assert_eq!(file_kind(Path::new("notes.txt"), &db), FileKind::Text);
		let path = dir.path().join("preview.png");
		image::RgbImage::from_pixel(320, 240, image::Rgb([100, 150, 200]))
			.save(&path)
			.unwrap();
		let entry = DirEntry::from_path(&db, path.clone()).unwrap();
		assert_eq!(entry.kind, FileKind::Image);
		assert!(entry.thumbnail.is_some());
		fs::write(&path, "not a png").unwrap();
		assert!(DirEntry::from_path(&db, path).unwrap().thumbnail.is_none());
	}
}
