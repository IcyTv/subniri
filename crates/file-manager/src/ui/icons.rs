//! Native equivalents of the HTML's folder, document-card and thumbnail artwork.
use crate::{
	Message,
	filesystem::entry::{DirEntry, FileKind},
	ui::style,
};
use iced::{
	Alignment, Border, Element, Length, Shadow, Vector,
	widget::{column, container, image, stack, svg, text},
};
use neo_widgets::style::Colors;

// Phosphor's regular SVGs do not have the "-regular" filename suffix used by
// other weights. Keep this adapter local instead of changing existing consumers.
macro_rules! ui_icon {
	($name:literal) => {
		iced::widget::svg::Handle::from_memory(include_bytes!(concat!(
			env!("PHOSPHOR_ICONS"),
			"/regular/",
			$name,
			".svg"
		)))
	};
}
pub(crate) use ui_icon;

pub const FOLDER_WIDTH: f32 = 75.0;
pub const FOLDER_HEIGHT: f32 = 65.0;

pub fn file_art(entry: &DirEntry, scale: f32, colors: Colors) -> Element<'_, Message> {
	let art = base_art(entry, scale, colors);
	if !entry.is_symlink {
		return art;
	}

	with_link_badge(art, scale, entry.unresolved_link, colors)
}

fn with_link_badge<'a, R: iced::advanced::svg::Renderer + 'a>(
	art: Element<'a, Message, iced::Theme, R>, scale: f32, unresolved: bool, colors: Colors,
) -> Element<'a, Message, iced::Theme, R> {
	let badge = container(
		svg(ui_icon!("link"))
			.width(12.0 * scale)
			.height(12.0 * scale)
			.style(move |_, _| svg::Style {
				color: Some(colors.text),
			}),
	)
	.padding(2.0 * scale)
	.style(move |_| {
		framed(
			if unresolved {
				colors.decorative.coral90
			} else {
				colors.surface
			},
			scale,
			colors,
		)
	});

	stack![
		art,
		container(badge)
			.width(Length::Fill)
			.height(Length::Fill)
			.align_y(Alignment::End)
	]
	.into()
}

fn base_art(entry: &DirEntry, scale: f32, colors: Colors) -> Element<'_, Message> {
	if entry.is_dir {
		return folder(entry, scale, colors);
	}
	if let Some(preview) = &entry.thumbnail {
		return container(
			image(preview.clone())
				.width(79.0 * scale)
				.height(60.0 * scale)
				.content_fit(iced::ContentFit::Cover),
		)
		.padding(2.0 * scale)
		.style(move |_| framed(colors.decorative.blue90, scale, colors))
		.into();
	}
	let (background, symbol) = match entry.kind {
		FileKind::Pdf => (colors.decorative.coral90, ui_icon!("file-pdf")),
		FileKind::Code => (colors.decorative.green90, ui_icon!("code")),
		FileKind::Archive => (colors.decorative.purple90, ui_icon!("file-archive")),
		FileKind::Image => (colors.decorative.blue90, ui_icon!("image")),
		FileKind::Audio => (colors.decorative.purple90, ui_icon!("music-notes")),
		FileKind::Video => (colors.decorative.coral90, ui_icon!("film-strip")),
		FileKind::Text => (colors.surface, ui_icon!("text-align-left")),
		FileKind::Document => (colors.surface, ui_icon!("file")),
		FileKind::WordProcessing => (colors.surface, ui_icon!("file-doc")),
		FileKind::Spreadsheet => (colors.decorative.green90, ui_icon!("table")),
		FileKind::Presentation => (colors.decorative.coral90, ui_icon!("presentation-chart")),
		FileKind::Config => (colors.surface, ui_icon!("gear")),
		FileKind::Font => (colors.decorative.purple90, ui_icon!("text-aa")),
		FileKind::Executable => (colors.decorative.green90, ui_icon!("terminal-window")),
		FileKind::Book => (colors.decorative.blue90, ui_icon!("book-open")),
	};
	let extension = entry.path.extension().map_or_else(
		|| "FILE".into(),
		|ext| {
			ext.to_string_lossy()
				.chars()
				.take(6)
				.collect::<String>()
				.to_uppercase()
		},
	);
	let content = column![
		container(
			svg(symbol)
				.width(28.0 * scale)
				.height(28.0 * scale)
				.style(move |_, _| svg::Style {
					color: Some(colors.text)
				})
		)
		.height(Length::Fill)
		.center_x(Length::Fill)
		.align_y(Alignment::Center),
		text(extension)
			.font(style::MONO)
			.size(8.0 * scale)
			.color(colors.text),
	]
	.spacing(3.0 * scale)
	.align_x(Alignment::Center);
	container(content)
		.padding(iced::Padding {
			top: 7.0 * scale,
			bottom: 4.0 * scale,
			left: 2.0 * scale,
			right: 2.0 * scale,
		})
		.width(52.0 * scale)
		.height(65.0 * scale)
		.style(move |_| framed(background, scale, colors))
		.into()
}

#[cfg(test)]
mod tests {
	use super::*;
	use iced::{
		Size,
		advanced::{layout, widget::Tree},
		widget::Space,
	};

	#[test]
	fn bundled_folder_names_match_configuration_validation() {
		for name in config::FILEBERG_FOLDER_ICONS {
			assert!(folder_symbol(name).is_some(), "{name}");
		}

		assert!(folder_symbol("not-an-icon").is_none());
	}

	#[test]
	fn link_badge_preserves_folder_card_and_thumbnail_sizes_at_every_view_scale() {
		let colors = neo_widgets::appearance::Appearance::default().colors();

		for scale in [0.4, 0.6, 1.0] {
			for (width, height) in [(75.0, 65.0), (52.0, 65.0), (83.0, 64.0)] {
				for unresolved in [false, true] {
					let art: Element<'_, Message, iced::Theme, ()> = Space::new()
						.width(width * scale)
						.height(height * scale)
						.into();

					let mut badge = with_link_badge(art, scale, unresolved, colors);
					let mut tree = Tree::new(badge.as_widget());
					badge.as_widget_mut().diff(&mut tree);
					let node = badge.as_widget_mut().layout(
						&mut tree,
						&(),
						&layout::Limits::new(Size::ZERO, Size::new(500.0, 500.0)),
					);

					assert_eq!(node.size(), Size::new(width * scale, height * scale));
				}
			}
		}
	}
}

fn folder(entry: &DirEntry, scale: f32, colors: Colors) -> Element<'_, Message> {
	// The body is 70×49 plus an 11px tab, exactly like .folder-icon in the
	// mockup. No whitespace-heavy square viewBox that flattens the silhouette.
	let yellow = colors.decorative.yellow;
	let folder_svg = format!(
		r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 75 65"><path d="M1 12V1h25l7 11h37v49H1z" fill="black" transform="translate(3 3)"/><path d="M1 12V1h25l7 11h37v49H1z" fill="rgb({},{},{})" stroke="black" stroke-width="2"/></svg>"#,
		(yellow.r * 255.0) as u8,
		(yellow.g * 255.0) as u8,
		(yellow.b * 255.0) as u8,
	);

	let symbol = entry
		.folder_identity
		.as_ref()
		.map(|identity| identity.icon.clone())
		.unwrap_or_else(|| ui_icon!("folder"));
	let symbolic = entry
		.folder_identity
		.as_ref()
		.is_none_or(|identity| identity.symbolic);

	stack![
		svg(svg::Handle::from_memory(folder_svg.into_bytes()))
			.width(FOLDER_WIDTH * scale)
			.height(FOLDER_HEIGHT * scale),
		container(
			svg(symbol)
				.width(20.0 * scale)
				.height(20.0 * scale)
				.style(move |_, _| svg::Style {
					color: symbolic.then_some(colors.on_accent)
				})
		)
		.padding(iced::Padding {
			right: 10.0 * scale,
			bottom: 11.0 * scale,
			..Default::default()
		})
		.width(FOLDER_WIDTH * scale)
		.height(FOLDER_HEIGHT * scale)
		.align_x(Alignment::End)
		.align_y(Alignment::End),
	]
	.into()
}

/// Only bundled names are accepted; custom SVG files are loaded by directory workers.
pub(crate) fn folder_symbol(name: &str) -> Option<svg::Handle> {
	Some(match name {
		"folder" => ui_icon!("folder"),
		"house" => ui_icon!("house"),
		"desktop" => ui_icon!("desktop"),
		"download-simple" => ui_icon!("download-simple"),
		"file-text" => ui_icon!("file-text"),
		"image" => ui_icon!("image"),
		"music-notes" => ui_icon!("music-notes"),
		"film-strip" => ui_icon!("film-strip"),
		"users" => ui_icon!("users"),
		"copy" => ui_icon!("copy"),
		"code" => ui_icon!("code"),
		"git-branch" => ui_icon!("git-branch"),
		"pen-nib" => ui_icon!("pen-nib"),
		"briefcase" => ui_icon!("briefcase"),
		"smiley" => ui_icon!("smiley"),
		"archive" => ui_icon!("archive"),
		"star" => ui_icon!("star"),
		"gear" => ui_icon!("gear"),
		"package" => ui_icon!("package"),
		_ => return None,
	})
}

pub fn framed(background: iced::Color, scale: f32, colors: Colors) -> container::Style {
	container::Style {
		background: Some(background.into()),
		text_color: Some(colors.text),
		border: Border {
			color: colors.border,
			width: 2.0 * scale,
			radius: 0.0.into(),
		},
		shadow: Shadow {
			color: colors.black,
			offset: Vector::new(3.0 * scale, 3.0 * scale),
			blur_radius: 0.0,
		},
		..Default::default()
	}
}
