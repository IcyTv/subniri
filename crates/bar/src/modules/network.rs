use std::{
	collections::HashSet,
	time::{Duration, Instant},
};

use futures::StreamExt;
use iced::{
	Alignment, Animation, Element, Font, Length, Subscription, Task,
	alignment::Vertical,
	font,
	widget::{Svg, column, container, row, rule, space, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button, neo_card, neo_toggle},
};
use nmrs::{
	AccessPoint, ActiveConnection, ActiveConnectionState, ConnectivityState, NetworkManager,
	NetworkSnapshot,
};
use utilities::Hashable;

use crate::modules::{ICON_HEIGHT, MODULE_HEIGHT, MODULE_RADIUS};

#[derive(Debug, Clone)]
pub enum Message {
	SnapshotChanged(NetworkSnapshot),

	ToggleWifi(bool),
	ScanWifi,
	StoppedScanningWifi,
	Tick(Instant),
	Noop,
}

#[derive(Clone, Debug)]
pub struct Network {
	manager: Hashable<NetworkManager>,
	snapshot: NetworkSnapshot,

	scanning: Animation<bool>,
	stopping_rotation: Option<f32>,
}

impl Network {
	pub async fn new() -> Result<Self, String> {
		let manager = NetworkManager::new().await.map_err(|e| e.to_string())?;
		let snapshot = Self::snapshot(&manager).await.map_err(|e| e.to_string())?;

		Ok(Self {
			manager: Hashable::new(manager),
			snapshot,

			scanning: Animation::new(false)
				.repeat_forever()
				.duration(Duration::from_secs(1)),
			stopping_rotation: None,
		})
	}

	pub fn subscription(&self) -> Subscription<Message> {
		let events = Subscription::run_with(self.manager.clone(), |manager| {
			let manager = manager.clone();
			async_stream::stream! {
				let mut stream = match manager.network_events().await {
					Ok(stream) => stream,
					Err(error) => {
						log::warn!("Failed to subscribe to network events: {error}");
						return;
					}
				};

				while let Some(event) = stream.next().await {
					match event {
						Ok(_) => {
							match Self::snapshot(&manager).await {
								Ok(snapshot) => yield Message::SnapshotChanged(snapshot),
								Err(error) => log::warn!("Failed to refresh network snapshot: {error}"),
							}
						}
						Err(error) => {
							log::warn!("Network event error: {error}");
						}
					}
				}
			}
		});

		let anim_tick = if self.scanning.value() {
			iced::time::every(Duration::from_millis(10)).map(Message::Tick)
		} else {
			Subscription::none()
		};

		Subscription::batch([events, anim_tick])
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::SnapshotChanged(snapshot) => {
				self.snapshot = snapshot;
			}
			Message::ToggleWifi(on) => {
				// TODO: Let's try to be optimistic about this update.
				let manager = self.manager.clone();
				return Task::future(async move {
					if let Err(e) = manager.set_wireless_enabled(on).await {
						log::warn!("Failed to set wireless enabled: {}", e);
					};

					Message::Noop
				});
			}
			Message::ScanWifi => {
				log::debug!("Scanning for wifi networks");
				self.stopping_rotation = None;
				self.scanning.go_mut(true, Instant::now());
				let manager = self.manager.clone();
				return Task::future(async move {
					tokio::time::sleep(Duration::from_secs_f32(2.5)).await;
					if let Err(e) = manager.scan_networks(None).await {
						log::warn!("Failed to rescan networks: {e}");
					}

					Message::StoppedScanningWifi
				});
			}
			Message::StoppedScanningWifi => {
				log::debug!("Stopped scanning for wifi networks");
				let now = Instant::now();
				self.stopping_rotation =
					Some(self.scanning.interpolate(std::f32::consts::TAU, 0.0, now));
			}
			Message::Tick(now) => {
				if let Some(previous_rotation) = self.stopping_rotation {
					let rotation = self.scanning.interpolate(std::f32::consts::TAU, 0.0, now);

					if rotation > previous_rotation {
						self.scanning = Animation::new(false)
							.repeat_forever()
							.duration(Duration::from_secs(1));
						self.stopping_rotation = None;
					} else {
						self.stopping_rotation = Some(rotation);
					}
				}
			}
			Message::Noop => (),
		}
		Task::none()
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		neo_button(
			row![
				self.connectivity_icon()
					.width(Length::Shrink)
					.height(ICON_HEIGHT),
				text(self.active_name())
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
		let svg_rotation = self
			.scanning
			.interpolate(std::f32::consts::TAU, 0.0, Instant::now());

		let top_row = row![
			svg(phosphor_icon!("wifi-high", "bold"))
				.width(ICON_HEIGHT * 1.5)
				.height(ICON_HEIGHT * 1.5),
			text("Network").weight(font::Weight::Bold),
			space::horizontal(),
			neo_button(
				svg(phosphor_icon!("arrow-counter-clockwise", "bold"))
					.rotation(svg_rotation)
					.width(ICON_HEIGHT)
					.height(ICON_HEIGHT)
			)
			.on_press(Message::ScanWifi),
			neo_toggle()
				.toggled(self.wifi_enabled())
				.enabled(self.snapshot.wifi.present && self.snapshot.wifi.hardware_enabled)
				.on_toggled(Message::ToggleWifi)
				.height(18.0)
				.width(40.0)
		]
		.align_y(Alignment::Center)
		.spacing(5.0);

		let primary_connection = self.primary_connection(ActiveConnectionState::Activated);

		let active_text = if primary_connection
			.map(|c| matches!(c, ActiveConnection::Wired(_)))
			.unwrap_or_default()
		{
			"Ethernet"
		} else {
			self.active_name()
		};

		let active_subtext = if let Some(conn) = primary_connection {
			let (ip_addr, link_speed) = match conn {
				ActiveConnection::Wired(wired) => (
					wired
						.ip4_address
						.as_deref()
						.or_else(|| wired.ip6_address.as_deref()),
					wired
						.speed_mbps
						.map(|s| s.to_string())
						.unwrap_or_else(|| "Unknown".to_string()),
				),
				ActiveConnection::Wifi(wifi) => (
					wifi.ip4_address
						.as_deref()
						.or_else(|| wifi.ip6_address.as_deref()),
					"Wifi".to_string(),
				),
				ActiveConnection::Vpn(vpn) => (
					vpn.ip4_address
						.as_deref()
						.or_else(|| vpn.ip6_address.as_deref()),
					"Vpn".to_string(),
				),
				ActiveConnection::Other(other) => (
					other
						.ip4_address
						.as_deref()
						.or_else(|| other.ip6_address.as_deref()),
					other
						.connection_type
						.clone()
						.unwrap_or_else(|| "Other".to_string()),
				),
				_ => (None, "Unknown".to_string()),
			};

			format!(
				"{} - {} Mbps",
				ip_addr
					.map(|addr| addr.trim_end_matches(|c: char| c == '/' || c.is_numeric()))
					.unwrap_or("Unknown IP"),
				link_speed
			)
		} else {
			"None - 0 Mbps".to_string()
		};

		let connection_widget = neo_card(
			row![
				self.connectivity_icon().width(28.0).height(28.0),
				column![text(active_text), text(active_subtext).size(10.0)]
			]
			.align_y(Alignment::Center)
			.spacing(10.0)
			.width(Length::Fill),
		)
		.width(Length::Fill);

		let wifi_networks: Element<Message> = if self.wifi_enabled() {
			column(
				self.snapshot
					.access_points
					.iter()
					.map(Self::view_access_point),
			)
			.into()
		} else {
			container(
				text("Enable wifi to see available networks")
					.wrapping(text::Wrapping::Word)
					.weight(font::Weight::Bold),
			)
			.center_x(Length::Fill)
			.into()
		};

		let content = column![
			top_row,
			rule::horizontal(2.0),
			text("Active Connections").weight(font::Weight::Bold),
			connection_widget,
			rule::horizontal(2.0),
			wifi_networks,
		]
		.spacing(10.0);

		neo_card(content)
			.width(320.0)
			.background(COLORS.decorative.green)
			.into()
	}

	fn connectivity_icon(&self) -> Svg<'_> {
		let active_transport = self.primary_transport(ActiveConnectionState::Activated);
		let connecting_transport = self.primary_transport(ActiveConnectionState::Activating);

		let (icon, muted) = if let Some(transport) = active_transport {
			match self.snapshot.connectivity.state {
				ConnectivityState::Full => (transport.icon(), false),
				ConnectivityState::Portal => (phosphor_icon!("globe-x", "bold"), false),
				ConnectivityState::Limited | ConnectivityState::None => (transport.x_icon(), false),
				_ => (transport.icon(), true),
			}
		} else if let Some(transport) = connecting_transport {
			(transport.icon(), true)
		} else if self.snapshot.wifi.present {
			if self.snapshot.wifi.hardware_enabled && self.snapshot.wifi.enabled {
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
		self.primary_connection(state).map(Transport::from)
	}

	fn primary_connection(&self, state: ActiveConnectionState) -> Option<&ActiveConnection> {
		self.snapshot
			.active_connections
			.iter()
			.find(|connection| {
				matches!(
					connection,
					ActiveConnection::Wired(wired) if wired.state == state
				)
			})
			.or_else(|| {
				self.snapshot
					.active_connections
					.iter()
					.filter_map(|connection| match connection {
						ActiveConnection::Wifi(wifi) if wifi.state == state => Some(connection),
						_ => None,
					})
					.max_by_key(|connection| match connection {
						ActiveConnection::Wifi(wifi) => wifi.strength.unwrap_or_default(),
						_ => 0,
					})
			})
	}

	fn active_connection(&self) -> Option<&ActiveConnection> {
		self.primary_connection(ActiveConnectionState::Activated)
			.or_else(|| self.primary_connection(ActiveConnectionState::Activating))
	}

	fn network_name(active_connection: &ActiveConnection) -> &str {
		match active_connection {
			ActiveConnection::Wired(connection) => &connection.id,
			ActiveConnection::Wifi(connection) => &connection.ssid,
			ActiveConnection::Vpn(connection) => &connection.id,
			ActiveConnection::Other(connection) => &connection.id,
			_ => "Unknown",
		}
	}

	fn active_name(&self) -> &str {
		self.active_connection()
			.map(Self::network_name)
			.unwrap_or("No connection")
	}

	fn wifi_enabled(&self) -> bool {
		self.snapshot.wifi.present
			&& self.snapshot.wifi.hardware_enabled
			&& self.snapshot.wifi.enabled
	}

	fn view_access_point<'a>(ap: &'a AccessPoint) -> Element<'a, Message> {
		neo_button(
			row![
				Self::wifi_icon_for_signal_strength(ap.strength)
					.width(28.0)
					.height(28.0),
				column![text(&ap.ssid).ellipsis(text::Ellipsis::End),].spacing(2.0)
			]
			.align_y(Alignment::Center)
			.width(Length::Fill)
			.spacing(8.0),
		)
		.padding([0.0, 8.0])
		.width(Length::Fill)
		.height(60.0)
		.into()
	}

	fn wifi_icon_for_signal_strength(strength: u8) -> Svg<'static> {
		match strength {
			0..=39 => svg(phosphor_icon!("wifi-low", "bold")),
			40..=69 => svg(phosphor_icon!("wifi-medium", "bold")),
			40..=u8::MAX => svg(phosphor_icon!("wifi-high", "bold")),
		}
	}

	async fn snapshot(manager: &NetworkManager) -> nmrs::Result<NetworkSnapshot> {
		let mut snapshot = manager.snapshot().await?;
		snapshot
			.access_points
			.sort_by_key(|ap| (ap.is_active, u8::MAX - ap.strength));

		let mut seen = HashSet::new();
		snapshot
			.access_points
			.retain(|ap| seen.insert(ap.ssid.clone()));

		Ok(snapshot)
	}
}

#[derive(Clone, Copy)]
enum Transport {
	Wired,
	Wifi(Option<u8>),
}

impl From<&ActiveConnection> for Transport {
	fn from(connection: &ActiveConnection) -> Self {
		match connection {
			ActiveConnection::Wired(_) => Self::Wired,
			ActiveConnection::Wifi(wifi) => Self::Wifi(wifi.strength),
			// TODO: Handle VPN and other connection types
			ActiveConnection::Vpn(_) | ActiveConnection::Other(_) | _ => Self::Wired,
		}
	}
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
