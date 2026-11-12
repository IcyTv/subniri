use iced::{
	Background, Border, Length,
	alignment::{Horizontal, Vertical},
	widget::{column, container, row, space, svg, text},
};

use crate::style::{COLORS, Colors};

use super::{NeoButton, neo_button, neo_toggle};

pub fn neo_toggle_button<'a, Message>(
	icon: svg::Handle, title: impl text::IntoFragment<'a>, subtitle: impl text::IntoFragment<'a>,
	toggled: bool, icon_color: Option<iced::Color>,
) -> NeoButton<'a, Message>
where
	Message: Clone + 'a,
{
	neo_toggle_button_with_colors(COLORS, icon, title, subtitle, toggled, icon_color)
}

pub fn neo_toggle_button_with_colors<'a, Message>(
	colors: Colors, icon: svg::Handle, title: impl text::IntoFragment<'a>,
	subtitle: impl text::IntoFragment<'a>, toggled: bool, icon_color: Option<iced::Color>,
) -> NeoButton<'a, Message>
where
	Message: Clone + 'a,
{
	let toggle: iced::Element<Message> = neo_toggle::<Message>()
		.with_colors(colors)
		.toggled(toggled)
		.track_style(super::NeoSurfaceStyle {
			shadow_width: 0.0,
			..super::NeoSurfaceStyle::with_colors(colors)
		})
		.handle_style(super::NeoSurfaceStyle {
			shadow_width: 0.0,
			..super::NeoSurfaceStyle::with_colors(colors)
		})
		.width(Length::Fixed(36.0))
		.height(Length::Fixed(18.0))
		.into();

	let icon_tint = if icon_color.is_some() {
		colors.on_accent
	} else {
		colors.text
	};
	let content = row![
		container(svg(icon).height(Length::Fill).style(move |_, _| {
			svg::Style {
				color: Some(icon_tint),
			}
		}))
		.width(42)
		.height(42)
		.style(move |_| {
			container::Style {
				background: icon_color.map(Background::Color),
				border: Border {
					color: colors.black,
					width: 2.0,
					radius: 3.0.into(),
				},
				..Default::default()
			}
		})
		.padding(12),
		column![
			text(title).color(colors.text).font(iced::Font {
				weight: iced::font::Weight::Bold,
				..Default::default()
			}),
			text(subtitle).color(colors.text).size(12)
		]
		.spacing(4)
		.align_x(Horizontal::Left)
		.width(Length::Fill),
		space::horizontal(),
		toggle
	]
	.align_y(Vertical::Center)
	.width(Length::Fill)
	.spacing(12);

	neo_button(content).with_colors(colors)
}
