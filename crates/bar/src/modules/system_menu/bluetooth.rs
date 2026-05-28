use std::time::{Duration, Instant};

use bluer::Session;
use iced::{Animation, Length, Subscription, Task};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_toggle_button},
};

#[derive(Debug, Clone)]
pub enum Message {
	AsyncDataLoaded(Result<AsyncData, String>),
	StateChanged(BluetoothState),
	Toggle,
	Toggled(BluetoothState),
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
}

impl Bluetooth {
	pub fn new() -> Self {
		Self {
			async_data: None,
			enabled: Animation::new(false).very_quick(),
			active_devices: 0,
		}
	}

	pub fn init(&self) -> Task<Message> {
		Task::perform(load_data(), Message::AsyncDataLoaded)
	}

	pub fn subscription(&self) -> Subscription<Message> {
		Subscription::run(|| {
			async_stream::stream! {
				let session = match Session::new().await {
					Ok(s) => s,
					Err(error) => {
						log::warn!("Failed to connect to bluez for Bluetooth monitoring: {error}");
						return;
					}
				};

				let mut state = load_state(&session).await.unwrap_or_default();
				let mut interval = tokio::time::interval(Duration::from_secs(5));

				loop {
					interval.tick().await;

					match load_state(&session).await {
						Ok(new_state) if new_state != state => {
							state = new_state.clone();
							yield Message::StateChanged(new_state);
						}
						Ok(_) => (),
						Err(error) => log::warn!("Failed to refresh Bluetooth state: {error}"),
					}
				}
			}
		})
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::AsyncDataLoaded(Ok(data)) => {
				self.set_state(data.state.clone());
				self.async_data = Some(data);
			}
			Message::AsyncDataLoaded(Err(error)) => {
				log::warn!("Failed to initialize Bluetooth system menu widget: {error}");
			}
			Message::StateChanged(state) => self.set_state(state),
			Message::Toggle if let Some(data) = &self.async_data => {
				let session = data.session.clone();

				return Task::perform(
					async move {
						let state = load_state(&session).await.unwrap_or_default();
						let enabled = !state.enabled;

						match default_adapter(&session).await {
							Ok(Some(adapter)) => {
								if let Err(error) = adapter.set_powered(enabled).await {
									log::warn!("Failed to toggle Bluetooth power: {error}");
								}
							}
							Ok(None) => log::warn!("Cannot toggle Bluetooth: no adapter found"),
							Err(error) => log::warn!("Failed to find Bluetooth adapter: {error}"),
						}

						load_state(&session).await.unwrap_or(BluetoothState {
							enabled,
							active_devices: state.active_devices,
						})
					},
					Message::Toggled,
				);
			}
			Message::Toggled(state) => self.set_state(state),
			_ => (),
		}

		Task::none()
	}

	fn set_state(&mut self, state: BluetoothState) {
		self.enabled.go_mut(state.enabled, Instant::now());
		self.active_devices = state.active_devices;
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		let icon_color =
			self.enabled
				.interpolate(COLORS.white, COLORS.decorative.green, Instant::now());
		let background =
			self.enabled
				.interpolate(COLORS.white, COLORS.decorative.green90, Instant::now());

		neo_toggle_button(
			phosphor_icon!("bluetooth"),
			"Bluetooth",
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
		if self.enabled.value() {
			format!("{} devices", self.active_devices)
		} else {
			"Disabled".to_string()
		}
	}
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
