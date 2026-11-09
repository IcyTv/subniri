use crate::{Message, ui::style};
use iced::{
	Alignment, Element,
	widget::{container, row, svg, text, tooltip},
};
use std::{borrow::Cow, time::Duration};

/// Visible place labels need no duplicate hover text. Compact icon buttons get
/// an actual, self-contained tooltip rather than a transparent text overlay.
pub(crate) fn place_hint<'a>(
	button: impl Into<Element<'a, Message>>, label: impl Into<Cow<'a, str>>, compact: bool,
) -> Element<'a, Message> {
	let button = button.into();
	if !compact {
		return button;
	}
	let label: Cow<'a, str> = label.into();
	const POINTER: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 7 12"><path d="M7 0L0 6L7 12Z" fill="#22231f"/></svg>"##;
	let bubble = container(text(label).size(12))
		.padding([7, 10])
		.style(|_| bubble_style());
	let content = row![
		svg(svg::Handle::from_memory(POINTER)).width(7).height(12),
		bubble
	]
	.align_y(Alignment::Center);
	tooltip(button, content, tooltip::Position::Right)
		.delay(Duration::from_millis(300))
		.gap(6)
		.padding(2)
		.into()
}

fn bubble_style() -> container::Style {
	container::Style {
		border: iced::Border {
			color: style::INK,
			width: 1.5,
			radius: 0.0.into(),
		},
		shadow: iced::Shadow {
			color: style::INK,
			offset: iced::Vector::new(2.0, 2.0),
			blur_radius: 0.0,
		},
		..style::panel(style::PAPER)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn tooltip_body_is_opaque_bordered_and_high_contrast() {
		let style = bubble_style();
		assert_eq!(style.background, Some(crate::ui::style::PAPER.into()));
		assert_eq!(style.text_color, Some(crate::ui::style::INK));
		assert_eq!(style.border.color, crate::ui::style::INK);
		assert!(style.border.width > 0.0);
		assert_eq!(style.shadow.blur_radius, 0.0);
	}
}
