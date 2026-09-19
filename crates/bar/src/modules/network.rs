use futures::StreamExt;
use iced::{
	Element, Font, Length, Subscription, Task,
	alignment::Vertical,
	font,
	widget::{Svg, row, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button, neo_card},
};
use nmrs::{
	ActiveConnection, ActiveConnectionState, ConnectivityState, NetworkEvent, NetworkManager,
	RadioState,
};
use utilities::Hashable;

use crate::modules::{ICON_HEIGHT, MODULE_HEIGHT, MODULE_RADIUS};

#[derive(Debug, Clone)]
pub enum Message {
	ConnectivityChanged(ConnectivityState),
	EnabledChanged(RadioState),
	ActiveConnectionsChanged(Vec<ActiveConnection>),
}

#[derive(Clone, Debug)]
pub struct Network {
	manager: Hashable<NetworkManager>,

	connectivity: ConnectivityState,
	enabled: RadioState,
	active_connections: Vec<ActiveConnection>,
}

impl Network {
	pub async fn new() -> Result<Self, String> {
		let manager = NetworkManager::new().await.map_err(|e| e.to_string())?;

		let connectivity = manager.connectivity().await.map_err(|e| e.to_string())?;
		let enabled = manager.wifi_state().await.map_err(|e| e.to_string())?;
		let active_connections = manager
			.list_active_connections()
			.await
			.map_err(|e| e.to_string())?;

		Ok(Self {
			manager: Hashable::new(manager),

			connectivity,
			enabled,
			active_connections,
		})
	}

	pub fn subscription(&self) -> Subscription<Message> {
		Subscription::run_with(self.manager.clone(), |manager| {
			let manager = manager.clone();
			async_stream::stream! {
				let mut stream = manager.network_events().await.unwrap();
				while let Some(event) = stream.next().await {
					match event {
						Ok(NetworkEvent::ConnectivityChanged) => {
							let connectivity = match manager.connectivity().await {
								Ok(conn) => conn,
								Err(e) => {
									log::warn!("Failed to get connectivity: {}", e);
									continue;
								}
							};
							yield Message::ConnectivityChanged(connectivity);
						}
						Ok(NetworkEvent::WirelessEnabledChanged) => {
							let enabled = match manager.wifi_state().await {
								Ok(state) => state,
								Err(e) => {
									log::warn!("Failed to get radio state: {}", e);
									continue;
								}
							};
							yield Message::EnabledChanged(enabled);
						}
						Ok(NetworkEvent::ActiveConnectionsChanged) => {
							let active_connections = match manager.list_active_connections().await {
								Ok(conns) => conns,
								Err(e) => {
									log::warn!("Failed to get active connections: {}", e);
									continue;
								}
							};
							yield Message::ActiveConnectionsChanged(active_connections);
						}
						Ok(ev) => {
							log::trace!("TODO network event: {ev:?}");
						}
						Err(e) => {
							log::warn!("Network event error: {}", e);
						}
					}
				}
			}
		})
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::ConnectivityChanged(connectivity) => {
				self.connectivity = connectivity;
			}
			Message::EnabledChanged(enabled) => {
				self.enabled = enabled;
			}
			Message::ActiveConnectionsChanged(active_connections) => {
				self.active_connections = active_connections;
			}
		}
		Task::none()
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		neo_button(
			row![
				self.connectivity_icon()
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

	pub fn view_popup(&self) -> Element<'_, Message> {
		neo_card("").background(COLORS.decorative.green).into()
	}

	fn connectivity_icon(&self) -> Svg<'_> {
		let active_transport = self.primary_transport(ActiveConnectionState::Activated);
		let connecting_transport = self.primary_transport(ActiveConnectionState::Activating);

		let (icon, muted) = if let Some(transport) = active_transport {
			match self.connectivity {
				ConnectivityState::Full => (transport.icon(), false),
				ConnectivityState::Portal => (phosphor_icon!("globe-x", "bold"), false),
				ConnectivityState::Limited | ConnectivityState::None => (transport.x_icon(), false),
				_ => (transport.icon(), true),
			}
		} else if let Some(transport) = connecting_transport {
			(transport.icon(), true)
		} else if self.enabled.present {
			if self.enabled.hardware_enabled && self.enabled.enabled {
				(phosphor_icon!("wifi-x", "bold"), true)
			} else {
				(phosphor_icon!("wifi-slash", "bold"), true)
			}
		} else {
			(phosphor_icon!("network-x", "bold"), true)
		};

		let color = if muted {
			COLORS.black.scale_alpha(0.5)
		} else {
			COLORS.black
		};

		svg(icon).style(move |_, _| svg::Style { color: Some(color) })
	}

	fn primary_transport(&self, state: ActiveConnectionState) -> Option<Transport> {
		if self.active_connections.iter().any(|connection| {
			matches!(
				connection,
				ActiveConnection::Wired(wired) if wired.state == state
			)
		}) {
			return Some(Transport::Wired);
		}

		self.active_connections
			.iter()
			.filter_map(|connection| match connection {
				ActiveConnection::Wifi(wifi) if wifi.state == state => Some(wifi.strength),
				_ => None,
			})
			.max_by_key(|strength| strength.unwrap_or_default())
			.map(Transport::Wifi)
	}
}

#[derive(Clone, Copy)]
enum Transport {
	Wired,
	Wifi(Option<u8>),
}

impl Transport {
	fn icon(self) -> svg::Handle {
		match self {
			Self::Wired => phosphor_icon!("network", "bold"),
			Self::Wifi(Some(0..=39)) => phosphor_icon!("wifi-low", "bold"),
			Self::Wifi(Some(40..=69)) | Self::Wifi(None) => {
				phosphor_icon!("wifi-medium", "bold")
			}
			Self::Wifi(Some(70..=u8::MAX)) => phosphor_icon!("wifi-high", "bold"),
		}
	}

	fn x_icon(self) -> svg::Handle {
		match self {
			Self::Wired => phosphor_icon!("network-x", "bold"),
			Self::Wifi(_) => phosphor_icon!("wifi-x", "bold"),
		}
	}
}
