use std::path::Path;

use iced::widget::{Id, svg};
use neo_widgets::phosphor_icon;
use xdg_mime::SharedMimeInfo;

#[derive(Debug, Clone)]
pub struct DirEntry {
	pub name: String,
	pub id: Id,
	pub icon: svg::Handle,
	pub is_dir: bool,
	pub is_hidden: bool,
}

impl DirEntry {
	pub fn from_entry(db: &SharedMimeInfo, entry: ignore::DirEntry) -> Self {
		let metadata = entry.metadata().ok();
		let is_dir = metadata.map(|m| m.is_dir()).unwrap_or(false);
		let is_hidden = entry.file_name().to_string_lossy().starts_with('.');

		let icon = if is_dir {
			if is_hidden {
				phosphor_icon!("folder-dashed")
			} else {
				phosphor_icon!("folder")
			}
		} else {
			file_icon(entry.path(), db)
		};

		Self {
			name: entry.file_name().to_string_lossy().to_string(),
			id: Id::unique(),
			icon,
			is_dir,
			is_hidden,
		}
	}
}

fn file_icon(path: &Path, db: &SharedMimeInfo) -> svg::Handle {
	let ext = path
		.extension()
		.and_then(|e| e.to_str())
		.map(|e| e.to_ascii_lowercase());

	if let Some(ref ext) = ext {
		match ext.as_str() {
			"pdf" => return phosphor_icon!("file-pdf"),
			"zip" | "7z" | "rar" | "tar" | "gz" | "xz" => return phosphor_icon!("file-archive"),

			"rs" => return phosphor_icon!("file-rs"),
			"c" | "h" => return phosphor_icon!("file-c"),
			"cpp" | "cc" | "cxx" | "hpp" | "hh" => return phosphor_icon!("file-cpp"),
			"cs" => return phosphor_icon!("file-c-sharp"),
			"js" | "mjs" | "cjs" => return phosphor_icon!("file-js"),
			"jsx" => return phosphor_icon!("file-jsx"),
			"ts" | "mts" | "cts" => return phosphor_icon!("file-ts"),
			"tsx" => return phosphor_icon!("file-tsx"),
			"py" => return phosphor_icon!("file-py"),
			"css" | "scss" | "sass" | "less" => return phosphor_icon!("file-css"),
			"html" | "htm" => return phosphor_icon!("file-html"),
			"vue" => return phosphor_icon!("file-vue"),
			"sql" => return phosphor_icon!("file-sql"),
			"md" | "markdown" => return phosphor_icon!("file-md"),
			"ini" | "toml" | "conf" | "cfg" => return phosphor_icon!("file-ini"),

			"txt" | "log" => return phosphor_icon!("file-txt"),
			"csv" => return phosphor_icon!("file-csv"),

			"png" => return phosphor_icon!("file-png"),
			"jpg" | "jpeg" => return phosphor_icon!("file-jpg"),
			"svg" => return phosphor_icon!("file-svg"),

			"doc" | "docx" | "odt" | "rtf" => return phosphor_icon!("file-doc"),
			"xls" | "xlsx" | "ods" => return phosphor_icon!("file-xls"),
			"ppt" | "pptx" | "odp" => return phosphor_icon!("file-ppt"),

			_ => (),
		}
	}

	let mut builder = db.guess_mime_type();
	let mime = builder.path(path).guess();
	let mime = mime.mime_type();

	match mime.type_().as_str() {
		"image" => phosphor_icon!("file-image"),
		"video" => phosphor_icon!("file-video"),
		"audio" => phosphor_icon!("file-audio"),
		"text" => phosphor_icon!("file-text"),
		_ => match (mime.type_().as_str(), mime.subtype().as_str()) {
			("application", "pdf") => phosphor_icon!("file-pdf"),
			("application", "zip")
			| ("application", "gzip")
			| ("application", "x-tar")
			| ("application", "x-7z-compressed")
			| ("application", "x-rar") => phosphor_icon!("file-archive"),
			_ => phosphor_icon!("file"),
		},
	}
}
