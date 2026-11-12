use crate::{Message, ui::style};
use iced::{
	Alignment, Element,
	widget::{container, row, svg, text, tooltip},
};
use neo_widgets::style::Colors;
use std::{borrow::Cow, time::Duration};

/// Visible place labels need no duplicate hover text. Compact icon buttons get
/// an actual, self-contained tooltip rather than a transparent text overlay.
pub(crate) fn place_hint<'a>(
	button: impl Into<Element<'a, Message>>, label: impl Into<Cow<'a, str>>, compact: bool,
	colors: Colors,
) -> Element<'a, Message> {
	let button = button.into();
	if !compact {
		return button;
	}

	let label: Cow<'a, str> = label.into();
	const POINTER: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 7 12"><path d="M7 0L0 6L7 12Z" fill="currentColor"/></svg>"##;
	let bubble = container(text(label).size(12))
		.padding([7, 10])
		.style(move |_| bubble_style(colors));
	let content = row![
		svg(svg::Handle::from_memory(POINTER))
			.width(7)
			.height(12)
			.style(move |_, _| svg::Style {
				color: Some(colors.black),
			}),
		bubble
	]
	.align_y(Alignment::Center);

	tooltip(button, content, tooltip::Position::Right)
		.delay(Duration::from_millis(300))
		.gap(6)
		.padding(2)
		.into()
}

fn bubble_style(colors: Colors) -> container::Style {
	container::Style {
		text_color: Some(colors.text),
		border: iced::Border {
			color: colors.black,
			width: 1.5,
			radius: 0.0.into(),
		},
		shadow: iced::Shadow {
			color: colors.black,
			offset: iced::Vector::new(2.0, 2.0),
			blur_radius: 0.0,
		},
		..style::panel(colors.body)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn tooltip_body_is_opaque_bordered_and_high_contrast() {
		for colors in [neo_widgets::style::LIGHT, neo_widgets::style::DARK] {
			let style = bubble_style(colors);

			assert_eq!(style.background, Some(colors.body.into()));
			assert_eq!(style.text_color, Some(colors.text));
			assert_eq!(style.border.color, colors.black);
			assert!(style.border.width > 0.0);
			assert_eq!(style.shadow.color, colors.black);
			assert_eq!(style.shadow.blur_radius, 0.0);
		}
	}
}
