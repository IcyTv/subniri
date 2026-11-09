//! Fileberg's palette is deliberately local: shell widgets retain their own theme.
use iced::{Color, Element, Length, widget::container};
use neo_widgets::{
	style::rgb,
	widgets::{NeoButton, NeoButtonStyle, NeoContentSurfaceStyle, NeoSurfaceStyle, neo_button},
};

pub const INK: Color = rgb(0x222_31f);
pub const PAPER: Color = rgb(0xf7f_5ed);
pub const SIDEBAR: Color = rgb(0xeee_ee5);
pub const YELLOW: Color = rgb(0xffc_f3f);
pub const BLUE: Color = rgb(0xb6c_afa);
pub const MUTED: Color = rgb(0x777_86d);
pub const LINE: Color = rgb(0xdad_bd0);
pub const MONO: iced::Font = iced::Font::new("Space Mono");

pub fn panel(background: Color) -> container::Style {
	container::Style {
		background: Some(background.into()),
		text_color: Some(INK),
		..Default::default()
	}
}

/// Event-only background context target; never paints a button around the grid.
pub fn context_area<'a, Message: Clone + 'a>(
	content: impl Into<Element<'a, Message>>,
) -> NeoButton<'a, Message> {
	let surface = NeoSurfaceStyle {
		background: PAPER,
		border: INK,
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
			disabled_background: PAPER,
		})
		.padding(0)
		.focusable(false)
		.focus_on_click(false)
}

pub fn control<'a, Message: Clone + 'a>(
	content: impl Into<Element<'a, Message>>, selected: bool,
) -> NeoButton<'a, Message> {
	let surface = NeoSurfaceStyle {
		background: if selected { BLUE } else { PAPER },
		border: INK,
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
				background: if selected { BLUE } else { SIDEBAR },
				..surface
			},
			focused: NeoContentSurfaceStyle {
				surface: NeoSurfaceStyle {
					border_width: 2.0,
					border: INK,
					..surface
				},
				padding: 8.into(),
			},
			disabled_background: SIDEBAR,
		})
		.focusable(true)
		.focus_on_click(false)
}

pub fn primary<'a, Message: Clone + 'a>(
	content: impl Into<Element<'a, Message>>,
) -> NeoButton<'a, Message> {
	control(content, true)
		.background(YELLOW)
		.hover_background(rgb(0xffd_c6a))
}

/// Flat adjoining menu controls; the containing row owns the shared outline.
pub fn menu_control<'a, Message: Clone + 'a>(
	content: impl Into<Element<'a, Message>>, selected: bool,
) -> NeoButton<'a, Message> {
	let surface = NeoSurfaceStyle {
		background: if selected { BLUE } else { PAPER },
		border: INK,
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
				background: if selected { BLUE } else { SIDEBAR },
				..surface
			},
			focused: NeoContentSurfaceStyle {
				surface: NeoSurfaceStyle {
					border_width: 1.5,
					..surface
				},
				padding: 0.into(),
			},
			disabled_background: PAPER,
		})
		.focusable(true)
		.focus_on_click(false)
}

pub fn dark_control<'a, Message: Clone + 'a>(
	content: impl Into<Element<'a, Message>>,
) -> NeoButton<'a, Message> {
	control(content, false)
		.background(INK)
		.hover_background(rgb(0x444_53f))
		.focus_color(INK)
}

pub fn rule<'a, Message: 'a>() -> Element<'a, Message> {
	container(iced::widget::Space::new().height(1))
		.width(Length::Fill)
		.style(|_| panel(LINE))
		.into()
}

pub fn divider<'a, Message: 'a>(vertical: bool) -> Element<'a, Message> {
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
		.style(|_| panel(INK))
		.into()
}

pub fn search_input(
	theme: &iced::Theme, status: iced::widget::text_input::Status,
) -> iced::widget::text_input::Style {
	let mut style = input(theme, status);
	style.border.width = 0.0;
	style
}

pub fn menu_search(
	theme: &iced::Theme, status: iced::widget::text_input::Status,
) -> iced::widget::text_input::Style {
	let mut style = search_input(theme, status);
	style.background = SIDEBAR.into();
	style
}

#[cfg(test)]
mod menu_tests {
	use super::*;
	#[test]
	fn search_is_borderless_and_tinted_even_when_focused() {
		use iced::widget::text_input::Status;
		for status in [
			Status::Active,
			Status::Hovered,
			Status::Focused { is_hovered: false },
			Status::Focused { is_hovered: true },
		] {
			let style = menu_search(&iced::Theme::Light, status);
			assert_eq!(style.border.width, 0.0);
			assert_eq!(style.background, SIDEBAR.into());
		}
	}
}

pub fn input(
	_theme: &iced::Theme, status: iced::widget::text_input::Status,
) -> iced::widget::text_input::Style {
	iced::widget::text_input::Style {
		background: PAPER.into(),
		border: iced::Border {
			color: INK,
			width: if matches!(status, iced::widget::text_input::Status::Focused { .. }) {
				2.0
			} else {
				1.0
			},
			radius: 0.0.into(),
		},
		placeholder: MUTED,
		value: INK,
		selection: BLUE,
	}
}

pub fn pick_list(
	_theme: &iced::Theme, status: iced::widget::pick_list::Status,
) -> iced::widget::pick_list::Style {
	iced::widget::pick_list::Style {
		text_color: INK,
		placeholder_color: MUTED,
		handle_color: INK,
		background: if matches!(status, iced::widget::pick_list::Status::Active) {
			PAPER
		} else {
			SIDEBAR
		}
		.into(),
		border: iced::Border {
			color: INK,
			width: 1.0,
			radius: 0.0.into(),
		},
	}
}
