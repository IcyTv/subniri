use std::{
	collections::HashMap,
	fs::{File, OpenOptions},
	io::{Read, Seek, SeekFrom, Write},
	os::unix::{fs::OpenOptionsExt, io::AsRawFd},
	path::{Path, PathBuf},
	sync::atomic::{AtomicBool, Ordering},
};

use iced::widget::{image, markdown};

const TEXT_BYTES: usize = 256 * 1024;
const IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_IMAGES: usize = 16;

#[derive(Debug, Clone)]
pub(crate) struct Loaded {
	pub(super) body: Body,
	pub(super) pages: u32,
	pub(super) size: u64,
	pub(super) note: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) enum Body {
	Text(String),
	Markdown {
		items: Vec<markdown::Item>,
		images: HashMap<String, image::Handle>,
	},
	Image(image::Handle),
	Metadata(String),
}

pub(super) fn open_regular(path: &Path) -> Result<File, String> {
	// O_NONBLOCK prevents a raced replacement with a FIFO from blocking a worker.
	let file = OpenOptions::new()
		.read(true)
		.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
		.open(path)
		.map_err(|e| format!("Cannot read preview: {e}"))?;
	if !file.metadata().map_err(|e| e.to_string())?.is_file() {
		return Err("Content previews require a regular file".into());
	}

	Ok(file)
}

pub(super) fn read(path: &Path, page: u32, cancel: &AtomicBool) -> Result<Loaded, String> {
	let metadata =
		std::fs::symlink_metadata(path).map_err(|e| format!("Cannot read metadata: {e}"))?;
	let fallback = |reason: &str| Loaded {
		body: Body::Metadata(reason.into()),
		pages: 1,
		size: metadata.len(),
		note: None,
	};
	if metadata.is_dir() {
		return Ok(fallback("Folder · open it to browse its contents"));
	}
	if metadata.file_type().is_symlink() {
		return Ok(fallback("Symbolic link · open it to follow its target"));
	}
	if !metadata.is_file() {
		return Ok(fallback("No content preview for this file type"));
	}

	let mut file = open_regular(path)?;
	let size = file.metadata().map_err(|e| e.to_string())?.len();
	let extension = path
		.extension()
		.unwrap_or_default()
		.to_string_lossy()
		.to_ascii_lowercase();

	let mut loaded = Loaded {
		body: Body::Metadata("No content preview for this file type".into()),
		pages: 1,
		size,
		note: None,
	};
	check_cancel(cancel)?;

	loaded.body = match extension.as_str() {
		"pdf" => {
			let (handle, pages) = super::convert::pdf(&file, page, cancel)?;
			loaded.pages = pages;
			Body::Image(handle)
		}
		"mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "mpg" | "mpeg" | "ogv" => {
			loaded.note = Some("Video still · open the file to play".into());
			Body::Image(super::convert::video(&file, cancel)?)
		}
		"svg" => Body::Image(svg(&mut file, cancel)?),
		"png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "ico" | "avif"
		| "qoi" | "pnm" | "ppm" | "pgm" => {
			loaded.note = (extension == "gif").then(|| "First frame".into());
			Body::Image(raster(file)?)
		}
		// Office documents and archives are intentionally not treated as text.
		"doc" | "docx" | "odt" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp" | "rtf"
		| "zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "rar" => loaded.body,
		_ => {
			let (source, truncated) = text(&mut file)?;
			loaded.note = truncated.then(|| {
				"Preview truncated to 256 KiB, 2,000 lines, and 2,000 characters per line".into()
			});

			if matches!(extension.as_str(), "md" | "markdown" | "mdown" | "mkd") {
				let parsed = markdown::Content::parse(&source);
				let mut images = HashMap::new();
				let root = path
					.parent()
					.unwrap_or(Path::new("."))
					.canonicalize()
					.map_err(|e| e.to_string())?;

				for url in parsed.images().iter().take(MAX_IMAGES) {
					check_cancel(cancel)?;
					if let Ok(handle) = local_image(&root, url, cancel) {
						images.insert(url.clone(), handle);
					}
				}

				Body::Markdown {
					items: parsed.items().to_vec(),
					images,
				}
			} else {
				Body::Text(source)
			}
		}
	};
	check_cancel(cancel)?;

	Ok(loaded)
}

pub(super) fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
	if cancel.load(Ordering::Relaxed) {
		Err("Preview cancelled".into())
	} else {
		Ok(())
	}
}

fn text(file: &mut File) -> Result<(String, bool), String> {
	let mut bytes = Vec::new();
	file.take((TEXT_BYTES + 1) as u64)
		.read_to_end(&mut bytes)
		.map_err(|e| e.to_string())?;
	let mut truncated = bytes.len() > TEXT_BYTES;
	bytes.truncate(TEXT_BYTES);

	// A bounded read may end inside the final UTF-8 code point.
	let source = match std::str::from_utf8(&bytes) {
		Ok(source) => source,
		Err(error) if truncated && error.error_len().is_none() => {
			std::str::from_utf8(&bytes[..error.valid_up_to()]).map_err(|e| e.to_string())?
		}
		Err(_) => return Err("No text preview: file is binary or not UTF-8".into()),
	};
	if source
		.chars()
		.any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
	{
		return Err("No text preview: file contains binary data".into());
	}

	let mut output = String::new();
	for (index, line) in source.lines().enumerate() {
		if index >= 2000 {
			truncated = true;
			break;
		}

		let mut chars = line.chars();
		output.extend(chars.by_ref().take(2000));
		truncated |= chars.next().is_some();
		output.push('\n');
	}

	Ok((output, truncated))
}

pub(super) fn raster(mut file: File) -> Result<image::Handle, String> {
	if file.metadata().map_err(|e| e.to_string())?.len() > IMAGE_BYTES {
		return Err("Image exceeds the 64 MiB preview limit".into());
	}

	file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
	let mut reader = ::image::ImageReader::new(std::io::BufReader::new(file))
		.with_guessed_format()
		.map_err(|e| e.to_string())?;
	let mut limits = ::image::Limits::default();
	limits.max_image_width = Some(8192);
	limits.max_image_height = Some(8192);
	limits.max_alloc = Some(64 * 1024 * 1024);
	reader.limits(limits);

	let decoded = reader
		.decode()
		.map_err(|e| format!("Cannot decode image: {e}"))?;
	let image = decoded.thumbnail(1600, 1600).into_rgba8();

	Ok(image::Handle::from_rgba(
		image.width(),
		image.height(),
		image.into_raw(),
	))
}

fn svg(file: &mut File, cancel: &AtomicBool) -> Result<image::Handle, String> {
	let mut bytes = Vec::new();
	file.take(1024 * 1024 + 1)
		.read_to_end(&mut bytes)
		.map_err(|e| e.to_string())?;
	if bytes.len() > 1024 * 1024 {
		return Err("SVG exceeds the 1 MiB preview limit".into());
	}

	validate_svg(&bytes)?;
	check_cancel(cancel)?;

	// Render exactly the validated bytes, not a pathname another process can replace.
	let mut snapshot = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
	snapshot.write_all(&bytes).map_err(|e| e.to_string())?;
	snapshot.rewind().map_err(|e| e.to_string())?;

	super::convert::svg(snapshot.as_file(), cancel)
}

fn validate_svg(bytes: &[u8]) -> Result<(), String> {
	let source = std::str::from_utf8(bytes).map_err(|_| "SVG is not UTF-8")?;
	let document = roxmltree::Document::parse_with_options(
		source,
		roxmltree::ParsingOptions {
			nodes_limit: 10000,
			..Default::default()
		},
	)
	.map_err(|e| format!("Cannot parse SVG: {e}"))?;

	for node in document.descendants() {
		if node
			.attributes()
			.any(|attribute| attribute.name() == "href" && !attribute.value().starts_with('#'))
			|| node.pi().is_some_and(|pi| pi.target == "xml-stylesheet")
		{
			return Err("SVG previews do not support linked resources or embedded images".into());
		}
	}

	Ok(())
}

fn local_image(root: &Path, uri: &str, cancel: &AtomicBool) -> Result<image::Handle, String> {
	let base = url::Url::from_directory_path(root).map_err(|_| "Invalid document directory")?;
	let url = base.join(uri).map_err(|e| e.to_string())?;
	if url.scheme() != "file" || url.query().is_some() {
		return Err("Only local images are supported".into());
	}

	let path = url.to_file_path().map_err(|_| "Invalid local image path")?;
	let canonical = path.canonicalize().map_err(|e| e.to_string())?;
	if !canonical.starts_with(root) {
		return Err("Image lies outside the document directory".into());
	}

	let mut file = open_regular(&canonical)?;
	// Check the opened object as well, preventing a symlink-parent race from escaping root.
	let actual = std::fs::read_link(PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())))
		.map_err(|e| e.to_string())?;
	if !actual.starts_with(root) {
		return Err("Image lies outside the document directory".into());
	}

	if canonical
		.extension()
		.is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
	{
		svg(&mut file, cancel)
	} else {
		raster(file)
	}
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::preview::tests::test_dir;

	#[test]
	fn text_is_bounded_utf8_and_rejects_binary_or_special_files() {
		let dir = test_dir();
		let path = dir.path().join("text.txt");
		std::fs::write(&path, "é".repeat(TEXT_BYTES)).unwrap();

		let loaded = read(&path, 1, &AtomicBool::new(false)).unwrap();

		assert!(matches!(loaded.body, Body::Text(ref text) if text.len() < TEXT_BYTES));
		assert!(loaded.note.is_some());

		std::fs::write(&path, b"binary\0data").unwrap();

		assert!(read(&path, 1, &AtomicBool::new(false)).is_err());

		let fifo = dir.path().join("fifo");
		nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRUSR).unwrap();

		assert!(open_regular(&fifo).is_err());
		assert!(matches!(
			read(&fifo, 1, &AtomicBool::new(false)).unwrap().body,
			Body::Metadata(_)
		));
	}

	#[test]
	fn images_decode_at_preview_resolution_and_corruption_is_reported() {
		let dir = test_dir();
		let path = dir.path().join("picture.png");
		::image::RgbaImage::from_pixel(2000, 20, ::image::Rgba([255, 0, 0, 255]))
			.save(&path)
			.unwrap();

		let loaded = read(&path, 1, &AtomicBool::new(false)).unwrap();

		assert!(matches!(
			loaded.body,
			Body::Image(image::Handle::Rgba {
				width: 1600,
				height: 16,
				..
			})
		));

		std::fs::write(&path, "not an image").unwrap();

		assert!(read(&path, 1, &AtomicBool::new(false)).is_err());
	}

	#[test]
	fn markdown_loads_only_confined_local_images() {
		let dir = test_dir();
		let root = dir.path().canonicalize().unwrap();
		let image = root.join("a b.png");
		::image::RgbaImage::from_pixel(2, 2, ::image::Rgba([0, 255, 0, 255]))
			.save(&image)
			.unwrap();

		let outside = test_dir();
		let secret = outside.path().join("outside.png");
		::image::RgbaImage::new(2, 2).save(&secret).unwrap();
		std::os::unix::fs::symlink(&secret, root.join("escape.png")).unwrap();

		let cancel = AtomicBool::new(false);

		assert!(local_image(&root, "a%20b.png", &cancel).is_ok());
		assert!(local_image(&root, "https://example.com/a.png", &cancel).is_err());
		assert!(local_image(&root, "escape.png", &cancel).is_err());
		assert!(
			local_image(
				&root,
				&url::Url::from_file_path(&secret).unwrap().to_string(),
				&cancel
			)
			.is_err()
		);

		let document = root.join("readme.md");
		std::fs::write(
			&document,
			"# Title\n\n![local](a%20b.png)\n\n![blocked](escape.png)",
		)
		.unwrap();

		let loaded = read(&document, 1, &AtomicBool::new(false)).unwrap();

		assert!(matches!(loaded.body, Body::Markdown { ref images, .. } if images.len() == 1));
	}

	#[test]
	#[ignore = "requires Nix SVG preview tool and unprivileged user namespaces"]
	fn svg_renders_text_in_sandbox() {
		let dir = test_dir();
		let path = dir.path().join("drawing.svg");
		std::fs::write(&path, r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50"><text x="0" y="30">Preview</text></svg>"#).unwrap();

		let loaded = read(&path, 1, &AtomicBool::new(false)).unwrap();

		assert!(matches!(
			loaded.body,
			Body::Image(image::Handle::Rgba {
				width: 1600,
				height: 400,
				..
			})
		));
	}

	#[test]
	fn svg_validation_blocks_external_embedded_and_stylesheet_resources() {
		for href in [
			"file:///etc/passwd",
			"https://example.com/image.png",
			"data:image/png;base64,AAAA",
		] {
			let svg =
				format!(r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="{href}"/></svg>"#);

			assert!(validate_svg(svg.as_bytes()).is_err());
		}

		assert!(
			validate_svg(
				br##"<svg xmlns="http://www.w3.org/2000/svg"><use href="#local"/></svg>"##
			)
			.is_ok()
		);
		assert!(validate_svg(br#"<?xml-stylesheet href="file:///etc/passwd"?><svg/>"#).is_err());
		assert!(validate_svg(br#"<!DOCTYPE svg [<!ENTITY a "a">]><svg/>"#).is_err());
	}

	#[test]
	#[ignore = "requires Nix SVG preview tool and unprivileged user namespaces"]
	fn nested_svg_layers_cannot_exhaust_host_memory() {
		let dir = test_dir();
		let path = dir.path().join("layers.svg");
		let svg = format!(
			r#"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="1600">{}<rect width="1600" height="1600"/>{}</svg>"#,
			"<g opacity=\"0.5\">".repeat(150),
			"</g>".repeat(150)
		);
		std::fs::write(&path, svg).unwrap();

		let result = read(&path, 1, &AtomicBool::new(false));

		assert!(
			result.is_err(),
			"renderer should hit the isolated memory limit"
		);
	}

	#[test]
	fn folders_symlinks_office_and_archives_use_metadata_and_cancelled_reads_stop() {
		let dir = test_dir();
		let path = dir.path().join("document.docx");
		std::fs::write(&path, "not text preview").unwrap();
		std::os::unix::fs::symlink(&path, dir.path().join("link")).unwrap();

		for path in [dir.path().to_owned(), path.clone(), dir.path().join("link")] {
			assert!(matches!(
				read(&path, 1, &AtomicBool::new(false)).unwrap().body,
				Body::Metadata(_)
			));
		}

		assert!(read(&path, 1, &AtomicBool::new(true)).is_err());
	}
}
