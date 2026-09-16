use std::time::Duration;

use iced::{
	Element, Font, Length, Rectangle, Subscription,
	alignment::Vertical,
	font, time,
	widget::{column, container, row, rule, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button, neo_card},
};

use super::{MODULE_HEIGHT, MODULE_RADIUS};
use crate::modules::{ICON_HEIGHT, clock::tray::Tray};

use self::calendar::Calendar;

mod calendar;
mod tray;

#[derive(Debug)]
pub struct Clock {
	time: String,
	time_secs: String,
	date: String,

	calendar: Calendar,
	tray: Tray,
}

#[derive(Debug, Clone)]
pub enum Message {
	Tick,
	Pressed,

	Calendar(calendar::Message),
	Tray(tray::Message),
}

#[derive(Debug, Clone)]
pub enum PopupAction {
	Open(Rectangle),
	NativeContextMenu { service: String, bounds: Rectangle },
	CloseAll,
}

impl Clock {
	pub fn new() -> Self {
		Self {
			time: current_time(),
			time_secs: current_time_secs(),
			date: current_date(),

			calendar: Calendar::new(),
			tray: Tray::new(),
		}
	}

	pub fn update(&mut self, message: Message) -> Option<PopupAction> {
		match message {
			Message::Tick => {
				self.time = current_time();
				self.time_secs = current_time_secs();
				self.date = current_date();
				self.calendar.update(calendar::Message::Tick);
			}
			Message::Pressed => log::trace!("Pressed"),
			Message::Calendar(calendar_message) => {
				self.calendar.update(calendar_message);
			}
			Message::Tray(message) => {
				return self.tray.update(message).map(|action| match action {
					tray::PopupAction::Open(bounds) => PopupAction::Open(bounds),
					tray::PopupAction::NativeContextMenu { service, bounds } => {
						PopupAction::NativeContextMenu { service, bounds }
					}
					tray::PopupAction::CloseAll => PopupAction::CloseAll,
				});
			}
		}

		None
	}

	pub fn subscription(&self) -> Subscription<Message> {
		let tick = time::every(Duration::from_secs(1)).map(|_| Message::Tick);

		let tray = self.tray.subscription().map(Message::Tray);

		let calendar = self.calendar.subscription().map(Message::Calendar);

		Subscription::batch([tick, tray, calendar])
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
			column![
				// neo_card(
				container(text(&self.time_secs).size(38).weight(font::Weight::Bold),)
					.center_x(Length::Fill)
					.padding(10),
				// )
				rule::horizontal(2).style(|_| rule::Style {
					color: COLORS.black,
					radius: 0.0.into(),
					fill_mode: rule::FillMode::Full,
					snap: false,
				}),
				self.tray.view().map(Message::Tray),
				rule::horizontal(2).style(|_| rule::Style {
					color: COLORS.black,
					radius: 0.0.into(),
					fill_mode: rule::FillMode::Full,
					snap: false,
				}),
				self.calendar.view().map(Message::Calendar),
			]
			.spacing(10),
		)
		.background(COLORS.decorative.purple)
		.width(400.0)
		.into()
	}

	pub fn on_popup_closed(&mut self) {
		self.calendar.update(calendar::Message::Closed);
	}

	pub fn open_tray_context_menu(&mut self, service: String, x: i32, y: i32) {
		self.tray
			.update(tray::Message::ContextMenuAt { service, x, y });
	}

	pub fn view_context_menu(&self, depth: usize) -> Element<'_, Message> {
		self.tray.view_context_menu(depth).map(Message::Tray)
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
