use std::time::Duration;

use iced::{
	Element, Font, Length, Subscription,
	alignment::Vertical,
	font, time,
	widget::{column, row, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button, neo_card},
};

use super::{MODULE_HEIGHT, MODULE_RADIUS};
use crate::modules::ICON_HEIGHT;

#[derive(Debug)]
pub struct Clock {
	time: String,
	time_secs: String,
	date: String,
}

#[derive(Debug, Clone, Copy)]
pub enum Message {
	Tick,
	Pressed,
}

impl Clock {
	pub fn new() -> Self {
		Self {
			time: current_time(),
			time_secs: current_time_secs(),
			date: current_date(),
		}
	}

	pub fn update(&mut self, message: Message) {
		match message {
			Message::Tick => {
				self.time = current_time();
				self.time_secs = current_time_secs();
				self.date = current_date();
			}
			Message::Pressed => log::trace!("Pressed"),
		}
	}

	pub fn subscription() -> Subscription<Message> {
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
	}

	pub fn view_popup(&self) -> Element<'_, Message> {
		neo_card(
			column![neo_card(
				text(&self.time_secs).size(38).weight(font::Weight::Bold)
			)]
			.spacing(10),
		)
		.background(COLORS.decorative.purple)
		.into()
	}

	fn text(label: &str) -> Element<'_, Message> {
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

#[inline]
fn current_time_secs() -> String {
	let time = jiff::Zoned::now();

	jiff::fmt::strtime::format("%H:%M:%S", &time).unwrap_or_else(|error| {
		log::warn!("Failed to format current time: {error}");
		"--:--:--".to_string()
	})
}

#[inline]
fn current_time() -> String {
	// jiff::Timestamp::now().format("%H:%M").to_string()
	let time = jiff::Zoned::now();

	jiff::fmt::strtime::format("%H:%M", &time).unwrap_or_else(|error| {
		log::warn!("Failed to format current time: {error}");
		"--:--".to_string()
	})
}

#[inline]
fn current_date() -> String {
	// Local::now().format("%a %B %d").to_string()
	let time = jiff::Zoned::now();

	jiff::fmt::strtime::format("%a %B %d", &time).unwrap_or_else(|error| {
		log::warn!("Failed to format current date: {error}");
		"--- --".to_string()
	})
}
