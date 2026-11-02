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
	Disconnected(String),
}

#[derive(Clone, Debug)]
pub struct Network {
	manager: Hashable<NetworkManager>,
	snapshot: NetworkSnapshot,

	scanning: Animation<bool>,
	stopping_rotation: Option<f32>,
}

impl Network {
	pub fn connection(&self) -> Hashable<NetworkManager> {
		self.manager.clone()
	}

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
			network_events(manager.clone())
		});

		let anim_tick = if self.scanning.value() {
			iced::window::frames().map(Message::Tick)
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
				let manager = self.manager.clone();
				return Task::future(async move {
					match request(async {
						manager.set_wireless_enabled(on).await?;
						Self::snapshot(&manager).await
					})
					.await
					{
						Ok(snapshot) => Message::SnapshotChanged(snapshot),
						Err(error) => Message::Disconnected(error),
					}
				});
			}
			Message::ScanWifi => {
				log::debug!("Scanning for wifi networks");
				self.stopping_rotation = None;
				self.scanning.go_mut(true, Instant::now());
				return Task::future(scan(self.manager.clone()));
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
			Message::Disconnected(_) => (), // Handled by the module's connection lifecycle.
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

fn network_events(manager: Hashable<NetworkManager>) -> impl futures::Stream<Item = Message> {
	async_stream::stream! {
		let error = match request(manager.network_events()).await {
			Ok(mut stream) => loop {
				let event = tokio::select! {
					() = manager.dbus_connection().closed() => None,
					event = stream.next() => event,
				};
				match event {
					Some(Ok(nmrs::NetworkEvent::NetworkManagerRestarted)) => {
						break "NetworkManager owner changed".to_string();
					}
					Some(Ok(_)) => match request(Network::snapshot(&manager)).await {
						Ok(snapshot) => yield Message::SnapshotChanged(snapshot),
						Err(error) => break error,
					},
					Some(Err(error)) => break error.to_string(),
					None => break "Network event stream disconnected".to_string(),
				}
			},
			Err(error) => error,
		};
		// nmrs spawns monitor tasks. Closing this module's old transport also
		// terminates their signal streams before a replacement is started.
		let _ = manager.dbus_connection().clone().close().await;
		yield Message::Disconnected(error);
	}
}

async fn request<T, E: std::fmt::Display>(
	future: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, String> {
	tokio::time::timeout(Duration::from_secs(5), future)
		.await
		.map_err(|_| "NetworkManager request timed out".to_string())?
		.map_err(|error| error.to_string())
}

async fn scan(manager: Hashable<NetworkManager>) -> Message {
	match request(manager.scan_networks(None)).await {
		Ok(()) => Message::StoppedScanningWifi,
		Err(error) => Message::Disconnected(error),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::modules::test_bus;

	#[test]
	#[ignore = "requires dbus-daemon; uses only a private bus"]
	fn failed_monitor_setup_is_reported() -> Result<(), test_bus::Error> {
		test_bus::run("modules::network::tests::monitor_failure_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn monitor_failure_child() -> Result<(), test_bus::Error> {
		test_bus::require_child()?;
		let manager = NetworkManager::new().await?;
		let mut stream = Box::pin(network_events(Hashable::new(manager)));
		let message = tokio::time::timeout(Duration::from_secs(2), stream.next()).await?;
		assert!(
			matches!(message, Some(Message::Disconnected(_))),
			"a failed monitor must notify the module instead of silently ending"
		);
		Ok(())
	}

	#[tokio::test(start_paused = true)]
	async fn nonreplying_network_request_is_bounded() {
		assert!(request::<(), String>(std::future::pending()).await.is_err());
	}

	struct NonreplyingManager;

	#[zbus::interface(name = "org.freedesktop.NetworkManager")]
	impl NonreplyingManager {
		async fn get_devices(&self) -> Vec<zvariant::OwnedObjectPath> {
			std::future::pending().await
		}
	}

	#[test]
	#[ignore = "requires dbus-daemon; uses only a private bus"]
	fn nonreplying_scan_completes() -> Result<(), test_bus::Error> {
		test_bus::run("modules::network::tests::scan_timeout_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn scan_timeout_child() -> Result<(), test_bus::Error> {
		test_bus::require_child()?;
		let _service = zbus::connection::Builder::session()?
			.name("org.freedesktop.NetworkManager")?
			.serve_at("/org/freedesktop/NetworkManager", NonreplyingManager)?
			.build()
			.await?;
		let manager = Hashable::new(NetworkManager::new().await?);
		let message = tokio::time::timeout(Duration::from_secs(8), scan(manager)).await?;
		assert!(matches!(message, Message::Disconnected(_)));
		Ok(())
	}

	struct SnapshotManager;

	#[zbus::interface(name = "org.freedesktop.NetworkManager")]
	impl SnapshotManager {
		fn get_devices(&self) -> Vec<zvariant::OwnedObjectPath> {
			vec![]
		}
		#[zbus(property)]
		fn wireless_enabled(&self) -> bool {
			true
		}
		#[zbus(property)]
		fn wireless_hardware_enabled(&self) -> bool {
			true
		}
		#[zbus(property)]
		fn wwan_enabled(&self) -> bool {
			false
		}
		#[zbus(property)]
		fn wwan_hardware_enabled(&self) -> bool {
			false
		}
		#[zbus(property)]
		fn active_connections(&self) -> Vec<zvariant::OwnedObjectPath> {
			vec![]
		}
	}

	struct Settings;

	#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings")]
	impl Settings {
		fn list_connections(&self) -> Vec<zvariant::OwnedObjectPath> {
			vec![]
		}
		#[zbus(property)]
		fn connections(&self) -> Vec<zvariant::OwnedObjectPath> {
			vec![]
		}
	}

	async fn snapshot_service() -> Result<zbus::Connection, test_bus::Error> {
		Ok(zbus::connection::Builder::session()?
			.name("org.freedesktop.NetworkManager")?
			.serve_at("/org/freedesktop/NetworkManager", SnapshotManager)?
			.serve_at("/org/freedesktop/NetworkManager/Settings", Settings)?
			.build()
			.await?)
	}

	#[test]
	#[ignore = "requires dbus-daemon; uses only a private bus"]
	fn duplicate_and_stale_disconnects_do_not_restart_new_connection() -> Result<(), test_bus::Error>
	{
		test_bus::run("modules::network::tests::connection_identity_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn connection_identity_child() -> Result<(), test_bus::Error> {
		use crate::modules::{Module, ModuleMessage, ServiceState};
		test_bus::require_child()?;
		let _service = snapshot_service().await?;
		let network = tokio::time::timeout(Duration::from_secs(2), Network::new()).await??;
		let old = network.connection();
		let mut module = Module::Network(ServiceState::Ready(network));
		let task = module.update(ModuleMessage::NetworkEvent(
			old.clone(),
			Message::Disconnected("lost".into()),
		));
		assert!(task.units() > 0);
		let duplicate = module.update(ModuleMessage::NetworkEvent(
			old.clone(),
			Message::Disconnected("duplicate".into()),
		));
		assert_eq!(
			duplicate.units(),
			0,
			"only one replacement may be in flight"
		);
		let response =
			tokio::time::timeout(Duration::from_secs(3), test_bus::task_message(task)).await??;
		let _ = module.update(response);
		assert!(matches!(module, Module::Network(ServiceState::Ready(_))));
		let stale = module.update(ModuleMessage::NetworkEvent(
			old,
			Message::Disconnected("stale".into()),
		));
		assert_eq!(stale.units(), 0);
		assert!(
			matches!(module, Module::Network(ServiceState::Ready(_))),
			"an old task must not tear down its replacement"
		);
		Ok(())
	}

	#[test]
	#[ignore = "requires dbus-daemon; uses only a private bus"]
	fn owner_loss_is_reported() -> Result<(), test_bus::Error> {
		test_bus::run("modules::network::tests::owner_loss_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn owner_loss_child() -> Result<(), test_bus::Error> {
		test_bus::require_child()?;
		let service = snapshot_service().await?;
		let manager = Hashable::new(NetworkManager::new().await?);
		let mut stream = Box::pin(network_events(manager));
		assert!(
			tokio::time::timeout(Duration::from_millis(200), stream.next())
				.await
				.is_err()
		);
		service.close().await?;
		let message = tokio::time::timeout(Duration::from_secs(2), stream.next()).await?;
		assert!(matches!(message, Some(Message::Disconnected(_))));
		Ok(())
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
