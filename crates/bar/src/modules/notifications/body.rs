//! Small bounded markup reader. Unknown tags have no executable behavior.
use super::Message;
use iced::{
	Element, Font, font,
	widget::{column, rich_text, span},
};
use neo_widgets::style::Colors;

pub fn view(
	body: &str, images: &[Option<iced::widget::image::Handle>], colors: Colors,
) -> Element<'static, Message> {
	let input: String = body.chars().take(16_384).collect();
	let mut spans = Vec::new();
	let mut content = column![].spacing(8);
	let mut image_index = 0;
	let mut stack: Vec<(String, Font, bool, Option<String>)> = Vec::new();
	let mut font = Font::DEFAULT;
	let mut underline = false;
	let mut link: Option<String> = None;
	for token in tokens(&input) {
		if let Token::Tag(tag) = token {
			if let Some(close) = tag.strip_prefix('/') {
				if stack.last().is_some_and(|s| s.0 == close) {
					if let Some((_, old_font, old_underline, old_link)) = stack.pop() {
						font = old_font;
						underline = old_underline;
						link = old_link;
					}
				}
			} else {
				let name = tag.split_whitespace().next().unwrap_or("");
				if matches!(name, "b" | "i" | "u" | "a") && stack.len() < 16 {
					stack.push((name.into(), font, underline, link.clone()));
					match name {
						"b" => font.weight = font::Weight::Bold,
						"i" => font.style = font::Style::Italic,
						"u" => underline = true,
						"a" => link = attribute(tag, "href").filter(|uri| safe_uri(uri)),
						_ => {}
					}
				} else if name == "img" {
					// Flush preceding text so each image keeps its document position.
					if !spans.is_empty() {
						content = content.push(
							rich_text(std::mem::take(&mut spans))
								.size(14)
								.on_link_click(Message::Link),
						);
					}
					if let Some(Some(handle)) = images.get(image_index) {
						content = content.push(
							iced::widget::image(handle.clone())
								.width(200)
								.height(100)
								.content_fit(iced::ContentFit::Contain),
						);
					} else {
						let alt: String = attribute(tag, "alt")
							.unwrap_or_default()
							.chars()
							.filter(|c| !c.is_control())
							.take(256)
							.collect();
						let label = if alt.trim().is_empty() {
							"[Image unavailable]".to_owned()
						} else {
							alt
						};
						content = content
							.push(iced::widget::text(label).size(14).color(colors.secondary));
					}
					image_index += 1;
				}
			}
			continue;
		}
		let Token::Text(raw) = token else {
			continue;
		};
		let label = entities(raw);
		spans.push(
			span(label)
				.font(font)
				.underline(underline || link.is_some())
				.link_maybe(link.clone())
				.color(colors.text),
		);
	}
	if !spans.is_empty() {
		content = content.push(rich_text(spans).size(14).on_link_click(Message::Link));
	}
	content.into()
}

#[derive(Clone, Copy)]
enum Token<'a> {
	Tag(&'a str),
	Text(&'a str),
}

/// Both consumers share tag boundaries and the same text/image segment budget.
/// Malformed or oversized tags remain literal text, never image-cache slots.
fn tokens(mut rest: &str) -> Vec<Token<'_>> {
	let mut result = Vec::new();
	let mut segments = 0;
	while !rest.is_empty() && segments < 512 {
		if rest.starts_with('<') {
			if let Some(end) = rest.find('>').filter(|end| *end < 2048) {
				let tag = &rest[1..end];
				result.push(Token::Tag(tag));
				if tag.split_whitespace().next() == Some("img") {
					segments += 1;
				}
				rest = &rest[end + 1..];
				continue;
			}
		}
		let end = rest
			.char_indices()
			.skip(1)
			.find(|(_, c)| *c == '<')
			.map_or(rest.len(), |(i, _)| i);
		result.push(Token::Text(&rest[..end]));
		segments += 1;
		rest = &rest[end..];
	}
	result
}

fn attribute(tag: &str, name: &str) -> Option<String> {
	let (_, tail) = tag.split_once(&format!("{name}="))?;
	let quote = tail.chars().next()?;
	if !matches!(quote, '\'' | '"') {
		return None;
	}
	Some(entities(tail[1..].split_once(quote)?.0))
}

pub fn safe_uri(uri: &str) -> bool {
	!uri.is_empty()
		&& !uri.chars().any(|c| c.is_control() || c.is_whitespace())
		&& uri.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
		&& url::Url::parse(uri).is_ok()
}

pub fn load_images(body: &str) -> Vec<Option<iced::widget::image::Handle>> {
	let input: String = body.chars().take(16_384).collect();
	tokens(&input)
		.into_iter()
		.filter_map(|token| match token {
			Token::Tag(tag) if tag.split_whitespace().next() == Some("img") => {
				Some(attribute(tag, "src"))
			}
			_ => None,
		})
		.take(4)
		.map(|src| src.and_then(|src| local_image(&src)))
		.collect()
}

pub(super) fn local_image(src: &str) -> Option<iced::widget::image::Handle> {
	let bytes = local_bytes(src, 2 * 1024 * 1024)?;
	decode_image(bytes, 200, 100)
}

pub(super) fn local_bytes(src: &str, maximum: u64) -> Option<Vec<u8>> {
	use std::io::Read;
	let path = if src.starts_with('/') {
		std::path::PathBuf::from(src)
	} else {
		url::Url::parse(src).ok()?.to_file_path().ok()?
	};
	let mut options = std::fs::OpenOptions::new();
	use std::os::unix::fs::OpenOptionsExt;
	options.read(true).custom_flags(nix::libc::O_NONBLOCK);
	let file = options.open(path).ok()?;
	if !file.metadata().ok()?.is_file() {
		return None;
	}
	let mut bytes = Vec::new();
	file.take(maximum + 1).read_to_end(&mut bytes).ok()?;
	(bytes.len() as u64 <= maximum).then_some(bytes)
}

pub(super) fn decode_image(
	bytes: Vec<u8>, width: u32, height: u32,
) -> Option<iced::widget::image::Handle> {
	let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
		.with_guessed_format()
		.ok()?;
	let mut limits = image::Limits::default();
	limits.max_image_width = Some(2048);
	limits.max_image_height = Some(2048);
	limits.max_alloc = Some(16 * 1024 * 1024);
	reader.limits(limits);
	let image = reader.decode().ok()?.thumbnail(width, height).into_rgba8();
	Some(iced::widget::image::Handle::from_rgba(
		image.width(),
		image.height(),
		image.into_raw(),
	))
}

/// Decode supported XML entities once. Unknown or invalid entities remain literal.
fn entities(input: &str) -> String {
	let mut result = String::new();
	let mut rest = input;
	while let Some(start) = rest.find('&') {
		result.push_str(&rest[..start]);
		rest = &rest[start..];
		let Some(end) = rest.find(';').filter(|end| *end <= 12) else {
			result.push('&');
			rest = &rest[1..];
			continue;
		};
		let entity = &rest[1..end];
		let decoded = match entity {
			"lt" => Some('<'),
			"gt" => Some('>'),
			"amp" => Some('&'),
			"quot" => Some('"'),
			"apos" => Some('\''),
			_ => entity
				.strip_prefix("#x")
				.and_then(|n| u32::from_str_radix(n, 16).ok())
				.or_else(|| entity.strip_prefix('#').and_then(|n| n.parse().ok()))
				.and_then(char::from_u32)
				.filter(|c| !c.is_control()),
		};
		if let Some(c) = decoded {
			result.push(c);
		} else {
			result.push_str(&rest[..=end]);
		}
		rest = &rest[end + 1..];
	}
	result.push_str(rest);
	result
}
