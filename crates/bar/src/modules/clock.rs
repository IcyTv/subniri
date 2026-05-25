use std::time::Duration;

use chrono::Local;
use iced::{
	Element, Font, Length, Subscription,
	alignment::Vertical,
	font, time,
	widget::{row, svg, text},
};

use super::{MODULE_HEIGHT, MODULE_RADIUS};
use crate::{
	modules::ICON_HEIGHT,
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button},
};

#[derive(Debug)]
pub struct Clock {
	time: String,
	date: String,
}

#[derive(Debug, Clone)]
pub enum Message {
	Tick,
	Pressed,
}

impl Clock {
	pub fn new() -> Self {
		Self {
			time: current_time(),
			date: current_date(),
		}
	}

	pub fn update(&mut self, message: Message) {
		match message {
			Message::Tick => {
				self.time = current_time();
				self.date = current_date();
			}
			Message::Pressed => log::trace!("Pressed"),
		}
	}

	pub fn subscription(&self) -> Subscription<Message> {
		time::every(Duration::from_secs(1)).map(|_| Message::Tick)
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		neo_button(
			row![
				svg(phosphor_icon!("calendar", "bold"))
					.width(Length::Shrink)
					.height(ICON_HEIGHT),
				Self::text(&self.date),
				svg(phosphor_icon!("clock", "bold"))
					.width(Length::Shrink)
					.height(ICON_HEIGHT),
				Self::text(&self.time),
			]
			.spacing(5.)
			.align_y(Vertical::Center),
		)
		.on_press(Message::Pressed)
		.height(MODULE_HEIGHT)
		.radius(MODULE_RADIUS)
		.background(COLORS.decorative.purple)
		.into()
	}

	fn text<'a>(label: &'a str) -> Element<'a, Message> {
		text(label)
			.font(Font {
				weight: font::Weight::Bold,
				..Font::DEFAULT
			})
			.color(COLORS.text)
			.size(18)
			.align_y(Vertical::Center)
			.into()
	}
}

fn current_time() -> String {
	Local::now().format("%H:%M").to_string()
}

fn current_date() -> String {
	Local::now().format("%a %B %d").to_string()
}
