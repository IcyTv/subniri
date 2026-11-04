//! Own a Network session from initialization through replacement. The bar only
//! routes messages; connection identity and stale work never escape this module.
use std::time::Duration;

use iced::{Element, Rectangle, Subscription, Task};
use neo_widgets::style::COLORS;
use utilities::Hashable;

use super::{Message as Event, Network};
use crate::modules::{
	connection::{State as ServiceState, initialize},
	loading, unavailable,
};

#[derive(Debug)]
pub struct Integration {
	state: ServiceState<Network>,
}

#[derive(Debug, Clone)]
pub enum Message {
	Initialized(Result<Network, String>),
	Action(Event),
	Session(Hashable<nmrs::NetworkManager>, Event),
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
				initialize(Duration::ZERO, Network::new()),
				Message::Initialized,
			)
		} else {
			Task::none()
		}
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Initialized(Ok(network)) => self.state = ServiceState::Ready(network),
			Message::Initialized(Err(error)) => {
				log::warn!("Failed to initialize Network module: {error}");
				let delay = self.state.failed(error);
				return Task::perform(initialize(delay, Network::new()), Message::Initialized);
			}
			Message::Session(connection, event) => {
				let ServiceState::Ready(network) = &mut self.state else {
					return Task::none();
				};
				if network.connection().as_ptr() != connection.as_ptr() {
					return Task::none();
				}
				if let Event::Disconnected(error) = event {
					log::warn!("Network module disconnected: {error}");
					let delay = self.state.failed(error);
					return Task::perform(
						async move {
							let _ = connection.dbus_connection().clone().close().await;
							initialize(delay, Network::new()).await
						},
						Message::Initialized,
					);
				}
				return network
					.update(event)
					.map(move |event| Message::Session(connection.clone(), event));
			}
			Message::Action(event) => {
				if let ServiceState::Ready(network) = &mut self.state {
					let connection = network.connection();
					return network
						.update(event)
						.map(move |event| Message::Session(connection.clone(), event));
				}
			}
			Message::OpenPopup(_) => {}
		}
		Task::none()
	}

	pub fn subscription(&self) -> Subscription<Message> {
		match &self.state {
			ServiceState::Ready(network) => network
				.subscription()
				.with(network.connection())
				.map(|(connection, event)| Message::Session(connection, event)),
			_ => Subscription::none(),
		}
	}

	pub fn view(&self) -> Element<'_, Message> {
		match &self.state {
			ServiceState::Connecting => loading(COLORS.decorative.green),
			ServiceState::Unavailable { error, .. } => {
				unavailable("Network unavailable", error, COLORS.decorative.green)
			}
			ServiceState::Ready(network) => network
				.view()
				.map(Message::Action)
				.on_press_with_bounds(Message::OpenPopup)
				.into(),
		}
	}

	pub fn view_popup(&self) -> Element<'_, Message> {
		match &self.state {
			ServiceState::Ready(network) => network.view_popup().map(Message::Action),
			_ => self.view(),
		}
	}

	#[cfg(test)]
	pub(super) fn connected(network: Network) -> Self {
		Self {
			state: ServiceState::Ready(network),
		}
	}

	#[cfg(test)]
	pub(super) fn ready(&self) -> Option<&Network> {
		match &self.state {
			ServiceState::Ready(network) => Some(network),
			_ => None,
		}
	}
}
