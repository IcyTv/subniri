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

pub fn file_art(entry: &DirEntry, scale: f32) -> Element<'_, Message> {
	if entry.is_dir {
		return folder(entry, scale);
	}
	if let Some(preview) = &entry.thumbnail {
		return container(
			image(preview.clone())
				.width(79.0 * scale)
				.height(60.0 * scale)
				.content_fit(iced::ContentFit::Cover),
		)
		.padding(2.0 * scale)
		.style(move |_| framed(neo_widgets::style::rgb(0xd9e_5eb), scale))
		.into();
	}
	let (background, symbol) = match entry.kind {
		FileKind::Pdf => (neo_widgets::style::rgb(0xefb_6a9), ui_icon!("file-pdf")),
		FileKind::Code => (neo_widgets::style::rgb(0xcbd_5bc), ui_icon!("code")),
		FileKind::Archive => (neo_widgets::style::rgb(0xe0c_6ea), ui_icon!("file-archive")),
		FileKind::Image => (neo_widgets::style::rgb(0xd9e_5eb), ui_icon!("image")),
		FileKind::Audio => (neo_widgets::style::rgb(0xe0c_6ea), ui_icon!("music-notes")),
		FileKind::Video => (neo_widgets::style::rgb(0xefb_6a9), ui_icon!("film-strip")),
		FileKind::Text => (
			neo_widgets::style::rgb(0xfff_ef8),
			ui_icon!("text-align-left"),
		),
		FileKind::Document => (neo_widgets::style::rgb(0xfff_ef8), ui_icon!("file")),
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
		container(svg(symbol).width(28.0 * scale).height(28.0 * scale))
			.height(Length::Fill)
			.center_x(Length::Fill)
			.align_y(Alignment::Center),
		text(extension)
			.font(style::MONO)
			.size(8.0 * scale)
			.color(style::INK),
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
		.style(move |_| framed(background, scale))
		.into()
}

fn folder(entry: &DirEntry, scale: f32) -> Element<'_, Message> {
	// The body is 70×49 plus an 11px tab, exactly like .folder-icon in the
	// mockup. No whitespace-heavy square viewBox that flattens the silhouette.
	const FOLDER: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 75 65"><path d="M1 12V1h25l7 11h37v49H1z" fill="#22231f" transform="translate(3 3)"/><path d="M1 12V1h25l7 11h37v49H1z" fill="#ffcf3f" stroke="#22231f" stroke-width="2"/></svg>"##;
	let symbol = match entry.name.to_lowercase().as_str() {
		"projects" => ui_icon!("code"),
		"design assets" => ui_icon!("pen-nib"),
		"work" => ui_icon!("briefcase"),
		"personal" => ui_icon!("smiley"),
		"archive" => ui_icon!("archive"),
		"very important" => ui_icon!("star"),
		_ => ui_icon!("folder"),
	};
	stack![
		svg(svg::Handle::from_memory(FOLDER))
			.width(FOLDER_WIDTH * scale)
			.height(FOLDER_HEIGHT * scale),
		container(svg(symbol).width(20.0 * scale).height(20.0 * scale))
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

pub fn framed(background: iced::Color, scale: f32) -> container::Style {
	container::Style {
		background: Some(background.into()),
		text_color: Some(style::INK),
		border: Border {
			color: style::INK,
			width: 2.0 * scale,
			radius: 0.0.into(),
		},
		shadow: Shadow {
			color: style::INK,
			offset: Vector::new(3.0 * scale, 3.0 * scale),
			blur_radius: 0.0,
		},
		..Default::default()
	}
}
