use iced::widget::{row, svg};

use crate::{phosphor_icon, widgets::neo_button};

// TODO: Press and Hold
pub fn neo_tumbler<'a, Message: Clone + 'a>(
	child: impl Into<iced::Element<'a, Message, iced::Theme, iced::Renderer>>, on_forward: Message,
	on_backward: Message, spacing: f32,
) -> iced::widget::Row<'a, Message> {
	neo_tumbler_with_colors(child, on_forward, on_backward, spacing, crate::style::LIGHT)
}

pub fn neo_tumbler_with_colors<'a, Message: Clone + 'a>(
	child: impl Into<iced::Element<'a, Message>>, on_forward: Message, on_backward: Message,
	spacing: f32, colors: crate::style::Colors,
) -> iced::widget::Row<'a, Message> {
	row![
		neo_button(
			svg(phosphor_icon!("caret-left", "bold"))
				.style(move |_, _| svg::Style {
					color: Some(colors.text)
				})
				.width(12.0)
				.height(12.0),
		)
		.with_colors(colors)
		.on_press(on_backward),
		child.into(),
		neo_button(
			svg(phosphor_icon!("caret-right", "bold"))
				.style(move |_, _| svg::Style {
					color: Some(colors.text)
				})
				.width(12.0)
				.height(12.0),
		)
		.with_colors(colors)
		.on_press(on_forward),
	]
	.align_y(iced::Alignment::Center)
	.spacing(spacing)
	.into()
}
