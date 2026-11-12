use std::collections::HashMap;

use iced::{
	Alignment, Element, Length, Theme,
	widget::{Space, column, container, image, markdown, row, scrollable, svg, text, tooltip},
};

use crate::{
	FileManager, Message,
	filesystem::entry::format_size,
	ui::{icons::ui_icon, style},
};

use super::{Message as PreviewMessage, load::Body};

impl FileManager {
	pub(crate) fn preview_panel(&self, width: f32) -> Element<'_, Message> {
		let colors = self.appearance.colors();
		let visible = self.preview.visible(width);
		let pane_width = if visible {
			self.preview.pane_width(width)
		} else {
			0.0
		};

		let mut panel = column![].spacing(12).width(Length::Fill);
		if visible && let Some(target) = &self.preview.target {
			let name = target
				.path
				.file_name()
				.unwrap_or_default()
				.to_string_lossy();

			let dismiss = style::control(
				svg(ui_icon!("x"))
					.width(12)
					.height(12)
					.style(move |_, _| svg::Style {
						color: Some(colors.text),
					}),
				false,
				colors,
			)
			.padding(6)
			.on_press(PreviewMessage::Dismiss.into());
			let dismiss = tooltip(
				dismiss,
				container(text("Dismiss preview").size(11))
					.padding(6)
					.style(move |_| style::panel(colors.body)),
				tooltip::Position::Bottom,
			)
			.delay(std::time::Duration::from_millis(300));

			panel = panel
				.push(
					row![
						text(name)
							.size(15)
							.weight(iced::font::Weight::Bold)
							.width(Length::Fill),
						dismiss,
					]
					.spacing(8)
					.align_y(Alignment::Start),
				)
				.push(
					text(target.path.to_string_lossy())
						.size(10)
						.color(colors.secondary),
				);
			if let Some(entry) = self.entries.iter().find(|entry| entry.path == target.path) {
				panel = panel.push(
					text(if entry.is_dir {
						"Folder".into()
					} else {
						format_size(
							self.preview
								.content
								.as_ref()
								.map_or(entry.size, |content| content.size),
						)
					})
					.size(11)
					.color(colors.secondary),
				);
			}
			panel = panel.push(style::rule(colors));

			let body: Element<'_, Message> = if self.preview.loading {
				text("Loading preview…")
					.size(13)
					.color(colors.secondary)
					.into()
			} else if let Some(error) = &self.preview.error {
				column![
					text("Preview unavailable").size(14),
					text(error).size(12).color(colors.secondary)
				]
				.spacing(8)
				.into()
			} else if let Some(content) = &self.preview.content {
				match &content.body {
					Body::Text(source) => text(source).font(style::MONO).size(12).into(),
					Body::Markdown { items, images } => markdown::view_with(
						items,
						markdown::Settings {
							font: iced::Font::new("DM Sans"),
							inline_code_font: style::MONO,
							code_block_font: style::MONO,
							..markdown::Settings::with_text_size(14)
						},
						&LocalViewer {
							theme: colors.iced_theme(),
							images,
						},
					),
					Body::Image(handle) => image(handle.clone())
						.width(Length::Fill)
						.content_fit(iced::ContentFit::Contain)
						.into(),
					Body::Metadata(reason) => text(reason).size(13).color(colors.secondary).into(),
				}
			} else {
				Space::new().into()
			};
			panel = panel.push(
				scrollable(container(body).width(Length::Fill))
					.id("file-preview")
					.height(Length::Fill),
			);

			if let Some(content) = &self.preview.content {
				if let Some(note) = &content.note {
					panel = panel.push(text(note).size(10).color(colors.secondary));
				}
			}

			if self.preview.pages > 1 {
				let page = self.preview.page;
				let pages = self.preview.pages;

				panel = panel.push(
					row![
						style::control(text("Previous").size(11), false, colors)
							.on_press(PreviewMessage::Page(page.saturating_sub(1)).into())
							.enabled(page > 1 && !self.preview.loading),
						Space::new().width(Length::Fill),
						text(format!("{page} / {pages}")).size(11),
						Space::new().width(Length::Fill),
						style::control(text("Next").size(11), false, colors)
							.on_press(PreviewMessage::Page(page.saturating_add(1)).into())
							.enabled(page < pages && !self.preview.loading),
					]
					.align_y(Alignment::Center)
					.spacing(6),
				);
			}
		}

		// Keep the panel's container in the row even when closed; files retain their tree.
		container(panel.padding(if visible { 16 } else { 0 }))
			.width(pane_width)
			.height(Length::Fill)
			.clip(true)
			.style(move |_| style::panel(colors.surface))
			.into()
	}
}

struct LocalViewer<'a> {
	theme: Theme,
	images: &'a HashMap<String, image::Handle>,
}

impl<'a> markdown::Viewer<'a, Message> for LocalViewer<'a> {
	fn theme(&self) -> &Theme {
		&self.theme
	}

	fn highlighter(&self) -> &dyn text::Highlighter<iced::Code, Theme> {
		markdown::Catalog::highlighter(&self.theme)
	}

	fn on_link_click(url: markdown::Uri) -> Message {
		PreviewMessage::IgnoreLink(url).into()
	}

	fn image(
		&self, settings: markdown::Settings, url: &'a markdown::Uri, title: &'a str,
		alt: &markdown::Text,
	) -> Element<'a, Message> {
		if let Some(handle) = self.images.get(url) {
			image(handle.clone())
				.width(Length::Fill)
				.content_fit(iced::ContentFit::Contain)
				.into()
		} else {
			let _ = title;
			column![
				text("Image unavailable or blocked").size(11),
				markdown::paragraph(self, settings, alt, Self::on_link_click),
			]
			.spacing(4)
			.into()
		}
	}
}
