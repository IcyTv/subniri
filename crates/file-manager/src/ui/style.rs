//! Fileberg controls use the shared appearance palette.
use iced::{Color, Element, Length, widget::container};
use neo_widgets::{
	style::Colors,
	widgets::{NeoButton, NeoButtonStyle, NeoContentSurfaceStyle, NeoSurfaceStyle, neo_button},
};

pub const MONO: iced::Font = iced::Font::new("Space Mono");

pub fn panel(background: Color) -> container::Style {
	container::Style {
		background: Some(background.into()),
		..Default::default()
	}
}

/// Event-only background context target; never paints a button around the grid.
pub fn context_area<'a, Message: Clone + 'a>(
	content: impl Into<Element<'a, Message>>, colors: Colors,
) -> NeoButton<'a, Message> {
	let surface = NeoSurfaceStyle {
		background: colors.surface,
		border: colors.black,
		border_width: 0.0,
		shadow_width: 0.0,
		radius: 0.0,
	};
	let content_style = NeoContentSurfaceStyle {
		surface,
		padding: 0.into(),
	};

	neo_button(content)
		.style(NeoButtonStyle {
			surface: content_style,
			hovered: surface,
			focused: content_style,
			disabled_background: colors.surface,
		})
		.padding(0)
		.focusable(false)
		.focus_on_click(false)
}

pub fn control<'a, Message: Clone + 'a>(
	content: impl Into<Element<'a, Message>>, selected: bool, colors: Colors,
) -> NeoButton<'a, Message> {
	let surface = NeoSurfaceStyle {
		background: if selected {
			colors.selection
		} else {
			colors.surface
		},
		border: colors.black,
		radius: 0.0,
		border_width: if selected { 1.5 } else { 0.0 },
		shadow_width: if selected { 3.0 } else { 0.0 },
	};

	neo_button(content)
		.style(NeoButtonStyle {
			surface: NeoContentSurfaceStyle {
				surface,
				padding: 8.into(),
			},
			hovered: NeoSurfaceStyle {
				background: if selected {
					colors.selection
				} else {
					colors.body
				},
				..surface
			},
			focused: NeoContentSurfaceStyle {
				surface: NeoSurfaceStyle {
					border_width: 2.0,
					border: colors.black,
					..surface
				},
				padding: 8.into(),
			},
			disabled_background: colors.body,
		})
		.focusable(true)
		.focus_on_click(false)
}

pub fn primary<'a, Message: Clone + 'a>(
	content: impl Into<Element<'a, Message>>, colors: Colors,
) -> NeoButton<'a, Message> {
	control(content, true, colors)
		.background(colors.decorative.yellow)
		.hover_background(colors.decorative.yellow70)
}

/// Flat adjoining menu controls; the containing row owns the shared outline.
pub fn menu_control<'a, Message: Clone + 'a>(
	content: impl Into<Element<'a, Message>>, selected: bool, colors: Colors,
) -> NeoButton<'a, Message> {
	let surface = NeoSurfaceStyle {
		background: if selected {
			colors.selection
		} else {
			colors.surface
		},
		border: colors.black,
		radius: 0.0,
		border_width: 0.0,
		shadow_width: 0.0,
	};

	neo_button(content)
		.style(NeoButtonStyle {
			surface: NeoContentSurfaceStyle {
				surface,
				padding: 0.into(),
			},
			hovered: NeoSurfaceStyle {
				background: if selected {
					colors.selection
				} else {
					colors.body
				},
				..surface
			},
			focused: NeoContentSurfaceStyle {
				surface: NeoSurfaceStyle {
					border_width: 1.5,
					..surface
				},
				padding: 0.into(),
			},
			disabled_background: colors.surface,
		})
		.focusable(true)
		.focus_on_click(false)
}

pub fn dark_control<'a, Message: Clone + 'a>(
	content: impl Into<Element<'a, Message>>, colors: Colors,
) -> NeoButton<'a, Message> {
	control(content, false, colors)
		.background(colors.black)
		.hover_background(colors.black)
		.focus_color(colors.black)
}

pub fn rule<'a, Message: 'a>(colors: Colors) -> Element<'a, Message> {
	container(iced::widget::Space::new().height(1))
		.width(Length::Fill)
		.style(move |_| panel(colors.separator))
		.into()
}

pub fn divider<'a, Message: 'a>(vertical: bool, colors: Colors) -> Element<'a, Message> {
	container(iced::widget::Space::new())
		.width(if vertical {
			Length::Fixed(2.0)
		} else {
			Length::Fill
		})
		.height(if vertical {
			Length::Fill
		} else {
			Length::Fixed(2.0)
		})
		.style(move |_| panel(colors.black))
		.into()
}

pub fn search_input(
	theme: &iced::Theme, status: iced::widget::text_input::Status, colors: Colors,
) -> iced::widget::text_input::Style {
	let mut style = input(theme, status, colors);
	style.border.width = 0.0;

	style
}

pub fn menu_search(
	theme: &iced::Theme, status: iced::widget::text_input::Status, colors: Colors,
) -> iced::widget::text_input::Style {
	let mut style = search_input(theme, status, colors);
	style.background = colors.body.into();

	style
}

#[cfg(test)]
mod menu_tests {
	use super::*;

	#[test]
	fn search_is_borderless_and_tinted_even_when_focused() {
		use iced::widget::text_input::Status;

		for colors in [neo_widgets::style::LIGHT, neo_widgets::style::DARK] {
			for status in [
				Status::Active,
				Status::Hovered,
				Status::Focused { is_hovered: false },
				Status::Focused { is_hovered: true },
			] {
				let style = menu_search(&colors.iced_theme(), status, colors);

				assert_eq!(style.border.width, 0.0);
				assert_eq!(style.background, colors.body.into());
				assert_eq!(style.value, colors.text);
			}
		}
	}
}

pub fn input(
	_theme: &iced::Theme, status: iced::widget::text_input::Status, colors: Colors,
) -> iced::widget::text_input::Style {
	iced::widget::text_input::Style {
		background: colors.surface.into(),
		border: iced::Border {
			color: colors.black,
			width: if matches!(status, iced::widget::text_input::Status::Focused { .. }) {
				2.0
			} else {
				1.0
			},
			radius: 0.0.into(),
		},
		placeholder: colors.secondary,
		value: colors.text,
		selection: colors.selection,
	}
}

pub fn pick_list(
	_theme: &iced::Theme, status: iced::widget::pick_list::Status, colors: Colors,
) -> iced::widget::pick_list::Style {
	iced::widget::pick_list::Style {
		text_color: colors.text,
		placeholder_color: colors.secondary,
		handle_color: colors.text,
		background: if matches!(status, iced::widget::pick_list::Status::Active) {
			colors.surface
		} else {
			colors.body
		}
		.into(),
		border: iced::Border {
			color: colors.black,
			width: 1.0,
			radius: 0.0.into(),
		},
	}
}
