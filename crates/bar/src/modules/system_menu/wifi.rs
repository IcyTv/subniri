use std::time::{Duration, Instant};

use futures::StreamExt;

use iced::{Animation, Length, Subscription, Task};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_toggle_button},
};
use nmrs::{DeviceState, NetworkManager};

#[derive(Debug, Clone)]
pub enum Message {
	AsyncDataLoaded(Result<AsyncData, String>),
	StateChanged(WifiState),
	Toggle,
	Toggled {
		revision: u64,
		result: Result<WifiState, String>,
	},
}

#[derive(Debug, Clone)]
pub struct AsyncData {
	nm: NetworkManager,
	state: WifiState,
}

#[derive(Debug, Clone, Default)]
pub struct WifiState {
	enabled: bool,
	active_connection: Option<WifiConnection>,
	extra_active_connections: usize,
	wired_connected: bool,
}

#[derive(Debug, Clone)]
struct WifiConnection {
	ssid: String,
	strength: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct Wifi {
	async_data: Option<AsyncData>,
	enabled: Animation<bool>,
	active_connection: Option<WifiConnection>,
	extra_active_connections: usize,
	wired_connected: bool,
	revision: u64,
	pending: Option<u64>,
	error: Option<String>,
}

impl Wifi {
	pub fn new() -> Self {
		Self {
			async_data: None,
			enabled: Animation::new(false).very_quick(),
			active_connection: None,
			extra_active_connections: 0,
			wired_connected: false,
			revision: 0,
			pending: None,
			error: None,
		}
	}

	pub fn subscription() -> Subscription<Message> {
		Subscription::run(|| {
			async_stream::stream! {
				let mut backoff = Duration::from_secs(1);
				loop {
					let mut events = Box::pin(state_events());
					let error = loop {
						match events.next().await {
							Some(Ok(message)) => {
								backoff = Duration::from_secs(1);
								yield message;
							}
							Some(Err(error)) => break error,
							None => break "Wi-Fi monitor stopped".to_string(),
						}
					};
					drop(events); // Drop both nmrs MonitorHandles before reconnecting.
					yield Message::AsyncDataLoaded(Err(error));
					tokio::time::sleep(backoff).await;
					backoff = (backoff * 2).min(Duration::from_secs(30));
				}
			}
		})
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::AsyncDataLoaded(Ok(data)) => {
				self.set_state(data.state.clone());
				self.pending = None;
				self.async_data = Some(data);
			}
			Message::AsyncDataLoaded(Err(error)) => {
				log::warn!("Wi-Fi unavailable: {error}");
				self.async_data = None;
				self.pending = None;
				self.revision = self.revision.wrapping_add(1);
				self.error = Some(error);
			}
			Message::Toggle
				if self.pending.is_none()
					&& let Some(data) = &self.async_data =>
			{
				let nm = data.nm.clone();
				self.revision = self.revision.wrapping_add(1);
				let revision = self.revision;
				self.pending = Some(revision);
				return Task::perform(
					request(async move {
						let radios = nm.airplane_mode_state().await?;
						nm.set_wireless_enabled(!radios.wifi.enabled).await?;
						load_state(&nm).await
					}),
					move |result| Message::Toggled { revision, result },
				);
			}
			Message::Toggled { revision, result } if self.pending == Some(revision) => {
				self.pending = None;
				match result {
					Ok(state) if self.revision == revision => self.set_state(state),
					Ok(_) => (),
					Err(error) => {
						log::warn!("Failed to toggle Wi-Fi: {error}");
						self.error = Some(error);
					}
				}
			}
			Message::StateChanged(state) => self.set_state(state),
			_ => (),
		}

		Task::none()
	}

	fn set_state(&mut self, state: WifiState) {
		self.revision = self.revision.wrapping_add(1);
		self.error = None;
		self.enabled.go_mut(state.enabled, Instant::now());
		self.active_connection = state.active_connection;
		self.extra_active_connections = state.extra_active_connections;
		self.wired_connected = state.wired_connected;
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		let icon_color =
			self.enabled
				.interpolate(COLORS.white, COLORS.decorative.green, Instant::now());
		let background =
			self.enabled
				.interpolate(COLORS.white, COLORS.decorative.green90, Instant::now());

		neo_toggle_button(
			phosphor_icon!("wifi-high"),
			"Wifi",
			self.subtitle(),
			self.enabled.value(),
			Some(icon_color),
		)
		.background(background)
		.on_press(Message::Toggle)
		.enabled(self.async_data.is_some() && self.pending.is_none())
		.width(Length::Fill)
		.height(64)
	}

	fn subtitle(&self) -> String {
		if self.async_data.is_none() {
			"Unavailable · retrying".to_string()
		} else if self.pending.is_some() {
			"Updating…".to_string()
		} else if self.error.is_some() {
			"Update failed".to_string()
		} else if let Some(connection) = &self.active_connection {
			let strength = connection
				.strength
				.map_or_else(|| "-- %".to_string(), |strength| format!("{strength}%"));

			if self.extra_active_connections > 0 {
				format!(
					"{} · {strength} · +{}",
					connection.ssid, self.extra_active_connections
				)
			} else {
				format!("{} · {strength}", connection.ssid)
			}
		} else if self.enabled.value() && self.wired_connected {
			"Wired connected".to_string()
		} else if self.enabled.value() {
			"Not connected".to_string()
		} else {
			"Disabled".to_string()
		}
	}
}

async fn request<T, E: std::fmt::Display>(
	future: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, String> {
	tokio::time::timeout(Duration::from_secs(5), future)
		.await
		.map_err(|_| "Wi-Fi request timed out".to_string())?
		.map_err(|error| error.to_string())
}

fn state_events() -> impl futures::Stream<Item = Result<Message, String>> {
	async_stream::try_stream! {
		let nm = request(NetworkManager::new()).await?;
		let (tx, rx) = async_channel::bounded::<()>(1);
		let device_tx = tx.clone();
		// These methods return handles after setup, not long-running futures.
		// Keep the handles alive; dropping them immediately stops monitoring.
		let _monitors = request(async {
			let devices = nm.monitor_device_changes(move || { let _ = device_tx.try_send(()); }).await?;
			let networks = nm.monitor_network_changes(move || { let _ = tx.try_send(()); }).await?;
			Ok::<_, nmrs::ConnectionError>((devices, networks))
		}).await?;
		let state = request(load_state(&nm)).await?;
		yield Message::AsyncDataLoaded(Ok(AsyncData { nm: nm.clone(), state }));
		let mut refresh = tokio::time::interval(Duration::from_secs(5));
		refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
		refresh.tick().await;
		loop {
			let event = tokio::select! {
				() = nm.dbus_connection().closed() => Err("Wi-Fi D-Bus connection closed".to_string()),
				event = rx.recv() => event.map_err(|_| "Wi-Fi monitors stopped".to_string()),
				_ = refresh.tick() => Ok(()),
			};
			event?;
			yield Message::StateChanged(request(load_state(&nm)).await?);
		}
	}
}

async fn load_state(nm: &NetworkManager) -> nmrs::Result<WifiState> {
	let enabled = nm.airplane_mode_state().await?.wifi.enabled;
	let mut active_networks = nm
		.list_networks(None)
		.await?
		.into_iter()
		.filter(|network| network.is_active)
		.collect::<Vec<_>>();

	active_networks.sort_by_key(|network| std::cmp::Reverse(network.strength.unwrap_or(0)));

	let active_connection = active_networks.first().map(|network| WifiConnection {
		ssid: network.ssid.clone(),
		strength: network.strength,
	});
	let extra_active_connections = active_networks.len().saturating_sub(1);
	let wired_connected = nm
		.list_devices()
		.await?
		.into_iter()
		.any(|device| device.is_wired() && device.state == DeviceState::Activated);

	Ok(WifiState {
		enabled,
		active_connection,
		extra_active_connections,
		wired_connected,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::modules::test_bus;
	use futures::StreamExt;

	#[test]
	#[ignore = "requires dbus-daemon; uses only a private bus"]
	fn missing_networkmanager_is_reported() -> Result<(), test_bus::Error> {
		test_bus::run("modules::system_menu::wifi::tests::missing_service_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn missing_service_child() -> Result<(), test_bus::Error> {
		test_bus::require_child()?;
		let recipe = iced::advanced::subscription::into_recipes(Wifi::subscription())
			.into_iter()
			.next()
			.ok_or("missing subscription")?;
		let mut stream = recipe.stream(futures::stream::pending().boxed());
		let message =
			tokio::time::timeout(std::time::Duration::from_secs(2), stream.next()).await?;
		assert!(
			matches!(message, Some(Message::AsyncDataLoaded(Err(_)))),
			"Wi-Fi monitor must report service failure instead of silently ending"
		);
		Ok(())
	}

	#[test]
	fn newer_observation_wins_over_delayed_toggle() {
		let mut wifi = Wifi::new();
		wifi.revision = 3;
		wifi.pending = Some(3);
		let _ = wifi.update(Message::StateChanged(WifiState::default()));
		let _ = wifi.update(Message::Toggled {
			revision: 3,
			result: Ok(WifiState {
				enabled: true,
				..WifiState::default()
			}),
		});
		assert!(!wifi.enabled.value());
		assert!(wifi.pending.is_none());
	}

	#[tokio::test(start_paused = true)]
	async fn nonreplying_wifi_request_is_bounded() {
		assert!(request::<(), String>(std::future::pending()).await.is_err());
	}

	struct Manager {
		enabled: bool,
	}

	#[zbus::interface(name = "org.freedesktop.NetworkManager")]
	impl Manager {
		fn get_devices(&self) -> Vec<zvariant::OwnedObjectPath> {
			vec![]
		}
		#[zbus(property)]
		fn wireless_enabled(&self) -> bool {
			self.enabled
		}
		#[zbus(property)]
		fn set_wireless_enabled(&mut self, value: bool) {
			self.enabled = value;
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
	}

	#[test]
	#[ignore = "requires dbus-daemon; uses only a private bus"]
	fn restores_state_and_controls_when_service_appears() -> Result<(), test_bus::Error> {
		test_bus::run("modules::system_menu::wifi::tests::recovery_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn recovery_child() -> Result<(), test_bus::Error> {
		test_bus::require_child()?;
		let recipe = iced::advanced::subscription::into_recipes(Wifi::subscription())
			.into_iter()
			.next()
			.ok_or("missing subscription")?;
		let mut stream = recipe.stream(futures::stream::pending().boxed());
		let first = tokio::time::timeout(Duration::from_secs(2), stream.next()).await?;
		assert!(matches!(first, Some(Message::AsyncDataLoaded(Err(_)))));
		let _manager = zbus::connection::Builder::session()?
			.name("org.freedesktop.NetworkManager")?
			.serve_at("/org/freedesktop/NetworkManager", Manager { enabled: true })?
			.build()
			.await?;
		let next = tokio::time::timeout(Duration::from_secs(3), stream.next())
			.await?
			.ok_or("Wi-Fi monitor ended instead of recovering")?;
		let mut wifi = Wifi::new();
		let _ = wifi.update(next);
		assert!(wifi.enabled.value());
		assert!(wifi.async_data.is_some());
		let task = wifi.update(Message::Toggle);
		assert_eq!(wifi.update(Message::Toggle).units(), 0);
		let message =
			tokio::time::timeout(Duration::from_secs(2), test_bus::task_message(task)).await??;
		let _ = wifi.update(message);
		assert!(
			!wifi.enabled.value(),
			"the recovered backend must accept a real property write"
		);
		Ok(())
	}
}
