//! Own a Battery session from initialization through replacement. The bar only
//! routes messages; connection identity and stale work never escape this module.
use std::time::Duration;

use iced::{Element, Rectangle, Subscription, Task};
use neo_widgets::style::Colors;

use super::{Battery, Message as Event};
use crate::modules::{
	connection::{State as ServiceState, initialize},
	loading, unavailable,
};

#[derive(Debug)]
pub struct Integration {
	state: ServiceState<Battery>,
}

#[derive(Debug, Clone)]
pub enum Message {
	Initialized(Result<Battery, String>),
	Action(Event),
	OpenPopup(Rectangle),
}

impl Integration {
	pub fn new() -> Self {
		Self {
			state: ServiceState::Connecting,
		}
	}

	pub fn init(&self) -> Task<Message> {
		if matches!(self.state, ServiceState::Connecting) {
			Task::perform(
				initialize(Duration::ZERO, Battery::new()),
				Message::Initialized,
			)
		} else {
			Task::none()
		}
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Initialized(Ok(battery)) => self.state = ServiceState::Ready(battery),
			Message::Initialized(Err(error)) => {
				log::warn!("Failed to initialize Battery module: {error}");
				let delay = self.state.failed(error);
				return Task::perform(initialize(delay, Battery::new()), Message::Initialized);
			}
			Message::Action(Event::Disconnected(error)) => {
				// A retry is already in flight; a stale stream's disconnect must not restart it.
				if let ServiceState::Ready(battery) = &self.state {
					log::warn!("Battery module disconnected: {error}");
					let connection = battery.connection();
					let delay = self.state.failed(error);
					return Task::perform(
						async move {
							let _ = connection.close().await;
							initialize(delay, Battery::new()).await
						},
						Message::Initialized,
					);
				}
			}
			Message::Action(event) => {
				if let ServiceState::Ready(battery) = &mut self.state {
					return battery.update(event).map(Message::Action);
				}
			}
			Message::OpenPopup(_) => {}
		}
		Task::none()
	}

	pub fn subscription(&self) -> Subscription<Message> {
		match &self.state {
			ServiceState::Ready(battery) => battery.subscription().map(Message::Action),
			_ => Subscription::none(),
		}
	}

	pub fn view(&self, colors: Colors) -> Element<'_, Message> {
		match &self.state {
			ServiceState::Connecting => loading(colors.decorative.yellow, colors),
			ServiceState::Unavailable { error, .. } => unavailable(
				"Battery unavailable",
				error,
				colors.decorative.yellow,
				colors,
			),
			ServiceState::Ready(battery) => battery
				.view_tile(colors)
				.map(Message::Action)
				.on_press_with_bounds(Message::OpenPopup)
				.into(),
		}
	}

	pub fn view_popup(&self, colors: Colors) -> Element<'_, Message> {
		match &self.state {
			ServiceState::Ready(battery) => battery.view_popup(colors).map(Message::Action),
			ServiceState::Connecting => {
				super::super::connection_popup("Battery", "Connecting…", colors)
			}
			ServiceState::Unavailable { .. } => super::super::connection_popup(
				"Battery",
				"Battery unavailable. Retrying automatically.",
				colors,
			),
		}
	}
}
