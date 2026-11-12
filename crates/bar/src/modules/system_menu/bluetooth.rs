use std::time::{Duration, Instant};

use bluer::Session;
use iced::{Animation, Length, Subscription, Task};
use neo_widgets::{phosphor_icon, style::Colors, widgets::NeoButton};

#[derive(Debug, Clone)]
pub enum Message {
	AsyncDataLoaded(Result<AsyncData, String>),
	StateChanged(BluetoothState),
	Toggle,
	Toggled {
		revision: u64,
		result: Result<BluetoothState, String>,
	},
}

#[derive(Debug, Clone)]
pub struct AsyncData {
	session: Session,
	state: BluetoothState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BluetoothState {
	enabled: bool,
	active_devices: usize,
}

#[derive(Debug, Clone)]
pub struct Bluetooth {
	async_data: Option<AsyncData>,
	enabled: Animation<bool>,
	active_devices: usize,
	revision: u64,
	pending: Option<u64>,
	error: Option<String>,
}

impl Bluetooth {
	pub fn new() -> Self {
		Self {
			async_data: None,
			enabled: Animation::new(false).very_quick(),
			active_devices: 0,
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
					match request(load_data()).await {
						Ok(data) => {
							let session = data.session.clone();
							yield Message::AsyncDataLoaded(Ok(data));
							backoff = Duration::from_secs(1);
							let mut interval = tokio::time::interval(Duration::from_secs(5));
							interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
							interval.tick().await;
							loop {
								interval.tick().await;
								match request(async { load_state(&session).await.map_err(|e| e.to_string()) }).await {
									Ok(state) => yield Message::StateChanged(state),
									Err(error) => {
										yield Message::AsyncDataLoaded(Err(error));
										break;
									}
								}
							}
						}
						Err(error) => yield Message::AsyncDataLoaded(Err(error)),
					}
					tokio::time::sleep(backoff).await;
					backoff = (backoff * 2).min(Duration::from_secs(30));
				}
			}
		})
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::AsyncDataLoaded(Ok(data)) => {
				self.set_state(&data.state);
				self.pending = None;
				self.async_data = Some(data);
			}
			Message::AsyncDataLoaded(Err(error)) => {
				log::warn!("Failed to initialize Bluetooth system menu widget: {error}");
				self.async_data = None;
				self.pending = None;
				self.revision = self.revision.wrapping_add(1);
				self.error = Some(error);
			}
			Message::Toggle
				if self.pending.is_none()
					&& let Some(data) = &self.async_data =>
			{
				let session = data.session.clone();
				self.revision = self.revision.wrapping_add(1);
				let revision = self.revision;
				self.pending = Some(revision);

				return Task::perform(
					request(async move {
						let adapter = default_adapter(&session)
							.await
							.map_err(|e| e.to_string())?
							.ok_or_else(|| "No Bluetooth adapter found".to_string())?;
						let powered = adapter.is_powered().await.map_err(|e| e.to_string())?;
						adapter
							.set_powered(!powered)
							.await
							.map_err(|e| e.to_string())?;
						load_state(&session).await.map_err(|e| e.to_string())
					}),
					move |result| Message::Toggled { revision, result },
				);
			}
			Message::Toggled { revision, result } if self.pending == Some(revision) => {
				self.pending = None;
				match result {
					Ok(state) if self.revision == revision => self.set_state(&state),
					Ok(_) => (), // A newer monitoring snapshot wins.
					Err(error) => {
						log::warn!("Failed to toggle Bluetooth: {error}");
						self.error = Some(error);
					}
				}
			}
			Message::StateChanged(state) => self.set_state(&state),
			_ => (),
		}

		Task::none()
	}

	fn set_state(&mut self, state: &BluetoothState) {
		self.revision = self.revision.wrapping_add(1);
		self.error = None;
		self.enabled.go_mut(state.enabled, Instant::now());
		self.active_devices = state.active_devices;
	}

	pub fn view_tile(&self, size: iced::Size, colors: Colors) -> NeoButton<'_, Message> {
		let icon_color =
			self.enabled
				.interpolate(colors.surface, colors.decorative.green, Instant::now());

		neo_widgets::widgets::neo_tile_with_colors(
			colors,
			phosphor_icon!("bluetooth"),
			"Bluetooth",
			self.subtitle(),
			Some(self.enabled.value()),
			Some(icon_color),
			size,
		)
		.on_press(Message::Toggle)
		.enabled(self.async_data.is_some() && self.pending.is_none())
		.width(Length::Fill)
		.height(Length::Fill)
	}

	pub fn is_animating(&self, now: Instant) -> bool {
		self.enabled.is_animating(now)
	}

	fn subtitle(&self) -> String {
		if self.async_data.is_none() {
			"Unavailable · retrying".to_string()
		} else if self.pending.is_some() {
			"Updating…".to_string()
		} else if self.error.is_some() {
			"Update failed".to_string()
		} else if self.enabled.value() {
			format!("{} devices", self.active_devices)
		} else {
			String::new()
		}
	}
}

async fn request<T>(
	future: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
	tokio::time::timeout(Duration::from_secs(5), future)
		.await
		.map_err(|_| "Bluetooth request timed out".to_string())?
}

async fn load_data() -> Result<AsyncData, String> {
	let session = Session::new().await.map_err(|error| error.to_string())?;
	let state = load_state(&session)
		.await
		.map_err(|error| error.to_string())?;

	Ok(AsyncData { session, state })
}

async fn load_state(session: &Session) -> bluer::Result<BluetoothState> {
	let mut active_devices = 0;

	let adapter = default_adapter(session).await?;

	let powered_on = if let Some(adapter) = &adapter {
		adapter.is_powered().await?
	} else {
		false
	};

	for name in session.adapter_names().await? {
		let adapter = session.adapter(&name)?;
		for addr in adapter.device_addresses().await? {
			let device = adapter.device(addr)?;

			if device.is_connected().await? {
				active_devices += 1;
			}
		}
	}

	Ok(BluetoothState {
		enabled: powered_on,
		active_devices,
	})
}

async fn default_adapter(session: &Session) -> bluer::Result<Option<bluer::Adapter>> {
	match session.default_adapter().await {
		Ok(adapter) => Ok(Some(adapter)),
		Err(bluer::Error {
			kind: bluer::ErrorKind::NotFound,
			..
		}) => Ok(None),
		Err(error) => Err(error),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::modules::test_bus;
	use futures::StreamExt;
	use std::sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	};

	#[test]
	fn newer_observation_wins_over_delayed_toggle_result() {
		let mut bluetooth = Bluetooth::new();
		bluetooth.revision = 7;
		bluetooth.pending = Some(7);
		let _ = bluetooth.update(Message::StateChanged(BluetoothState::default()));
		let _ = bluetooth.update(Message::Toggled {
			revision: 7,
			result: Ok(BluetoothState {
				enabled: true,
				active_devices: 0,
			}),
		});
		assert!(!bluetooth.enabled.value());
		assert!(bluetooth.pending.is_none());
	}

	#[tokio::test(start_paused = true)]
	async fn nonreplying_bluetooth_request_is_bounded() {
		assert!(request::<()>(std::future::pending()).await.is_err());
	}

	struct Adapter {
		readable: Arc<AtomicBool>,
		deny_write: bool,
		powered: bool,
	}

	#[zbus::interface(name = "org.bluez.Adapter1")]
	impl Adapter {
		#[zbus(property)]
		fn powered(&self) -> zbus::fdo::Result<bool> {
			if self.readable.load(Ordering::SeqCst) {
				Ok(self.powered)
			} else {
				Err(zbus::fdo::Error::Failed("injected read failure".into()))
			}
		}

		#[zbus(property)]
		fn set_powered(&mut self, value: bool) -> zbus::fdo::Result<()> {
			if self.deny_write {
				self.readable.store(false, Ordering::SeqCst);
				Err(zbus::fdo::Error::AccessDenied(
					"injected write rejection".into(),
				))
			} else {
				self.powered = value;
				Ok(())
			}
		}
	}

	async fn bluez(deny_write: bool) -> Result<zbus::Connection, test_bus::Error> {
		Ok(zbus::connection::Builder::session()?
			.name("org.bluez")?
			.serve_at("/", zbus::fdo::ObjectManager)?
			.serve_at(
				"/org/bluez/hci0",
				Adapter {
					readable: Arc::new(AtomicBool::new(true)),
					deny_write,
					powered: true,
				},
			)?
			.build()
			.await?)
	}

	#[test]
	#[ignore = "requires dbus-daemon; uses only a private bus"]
	fn initial_snapshot_reaches_ui() -> Result<(), test_bus::Error> {
		test_bus::run("modules::system_menu::bluetooth::tests::initial_snapshot_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn initial_snapshot_child() -> Result<(), test_bus::Error> {
		test_bus::require_child()?;
		let _bluez = bluez(false).await?;
		let recipe = iced::advanced::subscription::into_recipes(Bluetooth::subscription())
			.into_iter()
			.next()
			.ok_or("missing subscription")?;
		let mut stream = recipe.stream(futures::stream::pending().boxed());
		let message = tokio::time::timeout(Duration::from_secs(1), stream.next())
			.await?
			.ok_or("Bluetooth monitor ended")?;
		let mut bluetooth = Bluetooth::new();
		let _ = bluetooth.update(message);
		assert!(
			bluetooth.enabled.value(),
			"the first confirmed powered state must reach the UI even if it never changes"
		);
		Ok(())
	}

	#[test]
	#[ignore = "requires dbus-daemon; uses only a private bus"]
	fn failed_toggle_preserves_confirmed_state() -> Result<(), test_bus::Error> {
		test_bus::run("modules::system_menu::bluetooth::tests::failed_toggle_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn failed_toggle_child() -> Result<(), test_bus::Error> {
		test_bus::require_child()?;
		let _bluez = bluez(true).await?;
		let mut bluetooth = Bluetooth::new();
		let data = tokio::time::timeout(Duration::from_secs(2), load_data()).await??;
		let _ = bluetooth.update(Message::AsyncDataLoaded(Ok(data)));
		let task = bluetooth.update(Message::Toggle);
		assert_eq!(
			bluetooth.update(Message::Toggle).units(),
			0,
			"toggles must not overlap"
		);
		let message =
			tokio::time::timeout(Duration::from_secs(2), test_bus::task_message(task)).await??;
		let _ = bluetooth.update(message);
		assert!(
			bluetooth.enabled.value(),
			"rejected writes and failed reads must not invent the requested state"
		);
		Ok(())
	}

	#[test]
	#[ignore = "requires dbus-daemon; uses only a private bus"]
	fn recovers_when_bluez_appears() -> Result<(), test_bus::Error> {
		test_bus::run("modules::system_menu::bluetooth::tests::recovery_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn recovery_child() -> Result<(), test_bus::Error> {
		test_bus::require_child()?;
		let recipe = iced::advanced::subscription::into_recipes(Bluetooth::subscription())
			.into_iter()
			.next()
			.ok_or("missing subscription")?;
		let mut stream = recipe.stream(futures::stream::pending().boxed());
		let first = tokio::time::timeout(Duration::from_secs(2), stream.next()).await?;
		assert!(matches!(first, Some(Message::AsyncDataLoaded(Err(_)))));
		let _bluez = bluez(false).await?;
		let next = tokio::time::timeout(Duration::from_secs(3), stream.next())
			.await?
			.ok_or("Bluetooth monitor ended instead of recovering")?;
		let mut bluetooth = Bluetooth::new();
		let _ = bluetooth.update(next);
		assert!(bluetooth.enabled.value());
		assert!(
			bluetooth.async_data.is_some(),
			"recovery must restore commands, not just the indicator"
		);
		let task = bluetooth.update(Message::Toggle);
		let message =
			tokio::time::timeout(Duration::from_secs(2), test_bus::task_message(task)).await??;
		let _ = bluetooth.update(message);
		assert!(
			!bluetooth.enabled.value(),
			"the recovered backend must accept a real property write"
		);
		Ok(())
	}
}
