use std::{pin::pin, time::Instant};

use iced::{Animation, Length, Subscription, Task};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_toggle_button},
};
use nmrs::{DeviceState, NetworkManager};

#[derive(Debug, Clone)]
pub enum Message {
	AsyncDataLoaded(AsyncData),
	StateChanged(WifiState),
	Toggle,
	Toggled(WifiState),
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
}

impl Wifi {
	pub fn new() -> Self {
		Self {
			async_data: None,
			enabled: Animation::new(false).very_quick(),
			active_connection: None,
			extra_active_connections: 0,
			wired_connected: false,
		}
	}

	pub fn init(&self) -> Task<Message> {
		let _ = self;
		Task::perform(load_data(), Message::AsyncDataLoaded)
	}

	pub fn subscription() -> Subscription<Message> {
		Subscription::run(|| {
			async_stream::stream! {
				let nm = match NetworkManager::new().await {
					Ok(nm) => nm,
					Err(error) => {
						log::warn!("Failed to connect to NetworkManager for Wi-Fi monitoring: {error}");
						return;
					}
				};

				let (tx, rx) = async_channel::unbounded::<()>();
				let device_tx = tx.clone();
				let device_monitor_nm = nm.clone();
				let device_monitor = device_monitor_nm.monitor_device_changes(move || {
					let _ = device_tx.try_send(());
				});
				let mut device_monitor = pin!(device_monitor);
				let network_monitor_nm = nm.clone();
				let network_monitor = network_monitor_nm.monitor_network_changes(move || {
					let _ = tx.try_send(());
				});
				let mut network_monitor = pin!(network_monitor);
				let mut device_monitor_running = true;
				let mut network_monitor_running = true;

				loop {
					tokio::select! {
						result = &mut device_monitor, if device_monitor_running => {
							if let Err(error) = result {
								log::warn!("Wi-Fi device monitor disconnected: {error}");
							}

							device_monitor_running = false;
							if !network_monitor_running {
								break;
							}
						}
						result = &mut network_monitor, if network_monitor_running => {
							if let Err(error) = result {
								log::warn!("Wi-Fi network monitor disconnected: {error}");
							}

							network_monitor_running = false;
							if !device_monitor_running {
								break;
							}
						}
						event = rx.recv() => {
							if event.is_err() {
								break;
							}

							match load_state(&nm).await {
								Ok(state) => yield Message::StateChanged(state),
								Err(error) => log::warn!("Failed to refresh Wi-Fi state: {error}"),
							}
						}
					}
				}
			}
		})
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::AsyncDataLoaded(data) => {
				self.set_state(data.state.clone());
				self.async_data = Some(data);
			}
			Message::Toggle if let Some(data) = &self.async_data => {
				let nm = data.nm.clone();
				return Task::perform(
					async move {
						let radios = nm.airplane_mode_state().await.unwrap();
						let enabled = !radios.wifi.enabled;
						nm.set_wireless_enabled(enabled).await.unwrap();
						load_state(&nm).await.unwrap_or_else(|_| WifiState {
							enabled,
							..Default::default()
						})
					},
					Message::Toggled,
				);
			}
			Message::Toggled(state) | Message::StateChanged(state) => self.set_state(state),
			Message::Toggle => (),
		}

		Task::none()
	}

	fn set_state(&mut self, state: WifiState) {
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
		.width(Length::Fill)
		.height(64)
	}

	fn subtitle(&self) -> String {
		if let Some(connection) = &self.active_connection {
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

async fn load_data() -> AsyncData {
	let nm = NetworkManager::new()
		.await
		// FIXME: Don't panic here
		.expect("Failed to connect to NetworkManager");

	let state = load_state(&nm).await.unwrap_or_default();

	log::trace!("Wifi enabled: {}", state.enabled);

	AsyncData { nm, state }
}

async fn load_state(nm: &NetworkManager) -> nmrs::Result<WifiState> {
	let enabled = nm.airplane_mode_state().await?.wifi.enabled;
	let mut active_networks = nm
		.list_networks(None)
		.await
		.inspect_err(|error| log::warn!("Failed to list Wi-Fi networks: {error}"))
		.unwrap_or_default()
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
