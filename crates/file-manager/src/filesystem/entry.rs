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
	let name = name.trim();
	if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
		return Err("Enter a folder name, without /, . or ...".into());
	}
	let path = parent.join(name);
	fs::create_dir(&path).map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
	Ok(path)
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
