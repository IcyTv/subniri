use config::AppearanceMode;
use iced::{
	Element, Length, font,
	widget::{column, row, text},
};
use neo_widgets::{
	appearance::Appearance,
	style::Colors,
	widgets::{neo_button, neo_card},
};

use crate::Message;

pub fn view(appearance: &Appearance) -> Element<'_, Message> {
	let colors = appearance.colors();
	let selected = appearance.theme.selected();
	let mut content = column![
		text("Appearance")
			.size(crate::sizing::HEADER_TITLE)
			.weight(font::Weight::Bold)
			.color(colors.text),
		text("Choose a theme. Changes apply immediately across Subniri.").color(colors.secondary),
		row![
			theme_card(AppearanceMode::Light, selected, appearance.connected),
			theme_card(AppearanceMode::Dark, selected, appearance.connected),
		]
		.spacing(20),
	]
	.spacing(20);

	if !appearance.connected {
		content = content.push(
			neo_card(
				text(
					"Appearance service unavailable. Theme selection will be enabled when it reconnects.",
				)
				.color(colors.text),
			)
			.with_colors(colors)
			.background(colors.feedback.warning90)
			.padding(16)
			.width(Length::Fill),
		);
	}

	if appearance.saving {
		content = content.push(text("Saving appearance…").color(colors.secondary));
	}

	if !appearance.error.is_empty() {
		content = content.push(
			neo_card(
				column![
					text("Appearance could not be saved.")
						.weight(font::Weight::Bold)
						.color(colors.text),
					text(
						"The theme is active for this session, but may reset when the appearance service restarts."
					)
					.color(colors.text),
					text(appearance.error.as_str()).color(colors.text),
					neo_button(text("Retry saving").color(colors.text))
						.with_colors(colors)
						.padding([8, 16])
						.focusable(true)
						.enabled(appearance.connected && !appearance.saving)
						.on_press(Message::RetrySavingAppearance),
				]
				.spacing(12),
			)
			.with_colors(colors)
			.background(colors.feedback.warning90)
			.padding(16)
			.width(Length::Fill),
		);
	}

	content.into()
}

fn theme_card(
	theme: AppearanceMode, selected: AppearanceMode, connected: bool,
) -> Element<'static, Message> {
	let colors = Colors::for_theme(theme);
	let name = match theme {
		AppearanceMode::Light => "Light",
		AppearanceMode::Dark => "Dark",
	};
	let marker = if theme == selected {
		"Selected"
	} else {
		"Select theme"
	};

	let preview = neo_card(
		column![
			text("Subniri")
				.size(22)
				.weight(font::Weight::Bold)
				.color(colors.text),
			neo_card(text("Your desktop, your style").color(colors.text))
				.with_colors(colors)
				.background(colors.decorative.pink90)
				.padding(12)
				.width(Length::Fill),
		]
		.spacing(16),
	)
	.with_colors(colors)
	.background(colors.body)
	.padding(16)
	.width(Length::Fill);

	neo_button(
		column![
			preview,
			text(name)
				.size(22)
				.weight(font::Weight::Bold)
				.color(colors.text),
			text(marker).color(colors.secondary),
		]
		.spacing(12),
	)
	.with_colors(colors)
	.background(colors.surface)
	.disabled_background(colors.surface)
	.padding(16)
	.width(Length::Fill)
	.focusable(true)
	.enabled(connected)
	.on_press(Message::SelectTheme(theme))
	.into()
}
