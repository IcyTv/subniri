#![allow(clippy::missing_errors_doc)]

pub mod calendar;
pub mod nightlight;
pub mod spotify;

pub use daemon_common::*;

pub struct NightlightClient {
	connection: zbus::Connection,
}

impl NightlightClient {
	pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
		Ok(Self {
			connection: futures::executor::block_on(zbus::Connection::session())?,
		})
	}

	pub async fn new_async() -> Result<Self, Box<dyn std::error::Error>> {
		Ok(Self {
			connection: zbus::Connection::session().await?,
		})
	}

	#[must_use]
	pub fn from_connection(connection: zbus::Connection) -> Self {
		Self { connection }
	}

	pub fn send(
		&self, command: NightlightCommand,
	) -> Result<NightlightResponse, Box<dyn std::error::Error>> {
		futures::executor::block_on(self.send_async(command))
	}

	pub async fn send_async(
		&self, command: NightlightCommand,
	) -> Result<NightlightResponse, Box<dyn std::error::Error>> {
		let proxy = NightlightProxy::new(&self.connection).await?;

		let result = match command {
			NightlightCommand::SetBrightness(brightness) => {
				proxy.set_brightness(f64::from(brightness)).await
			}
			NightlightCommand::SetTemperature(temperature) => {
				proxy.set_temperature(temperature).await
			}
			NightlightCommand::SetNightlight(preset) => {
				let preset = preset.as_str();
				proxy.set_preset(preset).await
			}
			NightlightCommand::SetEnabled(enabled) => proxy.set_enabled(enabled).await,
			NightlightCommand::ToggleNightlight => proxy.toggle().await,
			NightlightCommand::Suspend(duration_secs) => proxy.suspend(duration_secs).await,
			NightlightCommand::Unsuspend => proxy.unsuspend().await,
		};

		match result {
			Ok(()) => {
				let (active, available, brightness, temperature, preset) = proxy.state().await?;

				Ok(NightlightResponse::State(NightlightState {
					active,
					available,
					brightness,
					temperature,
					preset,
				}))
			}
			Err(error) => Ok(NightlightResponse::Error(error.to_string())),
		}
	}
}

#[derive(Debug, Clone, Copy)]
pub enum NightlightCommand {
	SetBrightness(f32),
	SetTemperature(u32),
	SetNightlight(NightlightPreset),
	SetEnabled(bool),
	ToggleNightlight,
	Suspend(u64),
	Unsuspend,
}

#[derive(Debug, Clone)]
pub struct NightlightState {
	pub active: bool,
	pub available: bool,
	pub brightness: f64,
	pub temperature: u32,
	pub preset: String,
}

#[derive(Debug, Clone)]
pub enum NightlightResponse {
	State(NightlightState),
	Error(String),
}
