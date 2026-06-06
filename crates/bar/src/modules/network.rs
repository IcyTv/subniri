use iced::{
	Font, Length,
	alignment::Vertical,
	font,
	widget::{row, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button},
};

use crate::modules::{ICON_HEIGHT, MODULE_HEIGHT, MODULE_RADIUS};

pub fn network<'a>() -> NeoButton<'a, Message> {
	neo_button(
		row![
			svg(phosphor_icon!("network"))
				.width(Length::Shrink)
				.height(ICON_HEIGHT),
			text("todo")
				.font(Font {
					weight: font::Weight::Bold,
					..Font::DEFAULT
				})
				.color(COLORS.text)
				.size(18)
				.align_y(Vertical::Center)
		]
		.spacing(5.)
		.align_y(Vertical::Center),
	)
	.height(MODULE_HEIGHT)
	.radius(MODULE_RADIUS)
	.background(COLORS.decorative.green)
}

#[derive(Debug, Clone)]
pub enum Message {}

#[allow(dead_code)]
pub struct Network {}
