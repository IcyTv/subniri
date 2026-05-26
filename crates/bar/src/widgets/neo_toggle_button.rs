use iced::{
	Background, Border, Length,
	alignment::{Horizontal, Vertical},
	widget::{column, container, row, svg, text},
};

use crate::style::COLORS;

use super::{NeoButton, neo_button, neo_toggle};

pub fn neo_toggle_button<'a, Message>(
	icon: svg::Handle, title: &'a str, subtitle: &'a str, toggled: bool,
	icon_color: Option<iced::Color>,
) -> NeoButton<'a, Message>
where
	Message: Clone + 'a,
{
	let toggle: iced::Element<Message> = neo_toggle::<Message>()
		.toggled(toggled)
		.track_style(super::NeoSurfaceStyle {
			shadow_width: 0.0,
			..Default::default()
		})
		.handle_style(super::NeoSurfaceStyle {
			shadow_width: 0.0,
			..Default::default()
		})
		.width(Length::Fixed(36.0))
		.into();

	let content = row![
		container(svg(icon).height(Length::Fill))
			.width(42)
			.height(42)
			.style(move |_| {
				container::Style {
					background: icon_color.map(Background::Color),
					border: Border {
						color: COLORS.black,
						width: 2.0,
						radius: 3.0.into(),
					},
					..Default::default()
				}
			})
			.padding(12),
		column![
			text(title).color(COLORS.text).font(iced::Font {
				weight: iced::font::Weight::Bold,
				..Default::default()
			}),
			text(subtitle).color(COLORS.text).size(12)
		]
		.spacing(4)
		.align_x(Horizontal::Left)
		.width(Length::Fill),
		toggle
	]
	.align_y(Vertical::Center)
	.spacing(12);

	neo_button(content)
}
