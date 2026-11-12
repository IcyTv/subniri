//! Bluetooth's lifecycle owner; transport identity never escapes to the bar.
use std::time::Duration;

use iced::{Element, Rectangle, Subscription, Task};
use neo_widgets::style::Colors;
use utilities::Hashable;

use super::{Bluetooth, Message as Event};
use crate::modules::{
	connection::{State, initialize},
	loading, unavailable,
};

#[derive(Debug)]
pub struct Integration {
	state: State<Bluetooth>,
}

#[derive(Debug, Clone)]
pub enum Message {
	Initialized(Result<Bluetooth, String>),
	Session(Hashable<zbus::Connection>, Event),
	Action(Event),
	OpenPopup(Rectangle),
}

impl Integration {
	pub fn new() -> Self {
		Self {
			state: State::Connecting,
		}
	}

	pub fn init(&self) -> Task<Message> {
		if matches!(self.state, State::Connecting) {
			Task::perform(
				initialize(Duration::ZERO, Bluetooth::new()),
				Message::Initialized,
			)
		} else {
			Task::none()
		}
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Initialized(Ok(bluetooth)) => self.state = State::Ready(bluetooth),
			Message::Initialized(Err(error)) => {
				log::warn!("Failed to initialize Bluetooth module: {error}");
				let delay = self.state.failed(error);
				return Task::perform(initialize(delay, Bluetooth::new()), Message::Initialized);
			}
			Message::Session(connection, event) => {
				let State::Ready(bluetooth) = &mut self.state else {
					return Task::none();
				};
				if bluetooth.connection().as_ptr() != connection.as_ptr() {
					return Task::none();
				}
				if let Event::Disconnected(error) = event {
					let delay = self.state.failed(error);
					return Task::perform(
						async move {
							let _ = (*connection).clone().close().await;
							initialize(delay, Bluetooth::new()).await
						},
						Message::Initialized,
					);
				}
				bluetooth.update(event);
			}
			Message::Action(event) => {
				if let State::Ready(bluetooth) = &mut self.state {
					bluetooth.update(event);
				}
			}
			Message::OpenPopup(_) => {}
		}
		Task::none()
	}

	pub fn subscription(&self) -> Subscription<Message> {
		match &self.state {
			State::Ready(bluetooth) => bluetooth
				.subscription()
				.with(bluetooth.connection())
				.map(|(connection, event)| Message::Session(connection, event)),
			_ => Subscription::none(),
		}
	}

	pub fn view(&self, colors: Colors) -> Element<'_, Message> {
		match &self.state {
			State::Connecting => loading(colors.decorative.blue, colors),
			State::Unavailable { error, .. } => unavailable(
				"Bluetooth unavailable",
				error,
				colors.decorative.blue,
				colors,
			),
			State::Ready(bluetooth) => bluetooth
				.view(colors)
				.map(Message::Action)
				.on_press_with_bounds(Message::OpenPopup)
				.into(),
		}
	}

	pub fn view_popup(&self, colors: Colors) -> Element<'_, Message> {
		match &self.state {
			State::Ready(bluetooth) => bluetooth.view_popup(colors).map(Message::Action),
			State::Connecting => super::super::connection_popup("Bluetooth", "Connecting…", colors),
			State::Unavailable { .. } => super::super::connection_popup(
				"Bluetooth",
				"Bluetooth unavailable. Retrying automatically.",
				colors,
			),
		}
	}

	#[cfg(test)]
	pub(super) fn connected(bluetooth: Bluetooth) -> Self {
		Self {
			state: State::Ready(bluetooth),
		}
	}

	#[cfg(test)]
	pub(super) fn ready(&self) -> bool {
		matches!(self.state, State::Ready(_))
	}
}
