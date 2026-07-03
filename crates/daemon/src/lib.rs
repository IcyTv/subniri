#![allow(clippy::missing_errors_doc)]

pub mod nightlight;

pub const NIGHTLIGHT_BUS_NAME: &str = "de.icytv.subniri.Daemon";
pub const NIGHTLIGHT_OBJECT_PATH: &str = "/de/icytv/subniri/Nightlight";
pub const NIGHTLIGHT_INTERFACE: &str = "de.icytv.subniri.Nightlight";

#[zbus::proxy(
	interface = "de.icytv.subniri.Nightlight",
	default_service = "de.icytv.subniri.Daemon",
	default_path = "/de/icytv/subniri/Nightlight"
)]
pub trait Nightlight {
	#[zbus(property)]
	fn brightness(&self) -> zbus::Result<f64>;

	#[zbus(property)]
	fn set_brightness(&self, brightness: f64) -> zbus::Result<()>;

	#[zbus(property)]
	fn temperature(&self) -> zbus::Result<u32>;

	#[zbus(property)]
	fn set_temperature(&self, temperature: u32) -> zbus::Result<()>;

	#[zbus(property)]
	fn preset(&self) -> zbus::Result<String>;

	#[zbus(property)]
	fn set_preset(&self, preset: &str) -> zbus::Result<()>;

	#[zbus(property)]
	fn enabled(&self) -> zbus::Result<bool>;

	#[zbus(property)]
	fn set_enabled(&self, enabled: bool) -> zbus::Result<()>;

	#[zbus(property)]
	fn state(&self) -> zbus::Result<NightlightStateTuple>;

	fn toggle(&self) -> zbus::Result<()>;

	fn suspend(&self, duration_secs: u64) -> zbus::Result<()>;

	fn unsuspend(&self) -> zbus::Result<()>;
}

type NightlightStateTuple = (bool, bool, f64, u32, String);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NightlightPreset {
	Day,
	Night,
	Custom,
}

impl NightlightPreset {
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Day => "day",
			Self::Night => "night",
			Self::Custom => "custom",
		}
	}

	/// Parse Nightlight Preset value (day, night or custom)
	///
	/// # Errors
	///
	/// `InvalidArgs` if the value could not be parsed into one of the presets
	pub fn parse(value: &str) -> zbus::fdo::Result<Self> {
		match value {
			"day" | "Day" => Ok(Self::Day),
			"night" | "Night" => Ok(Self::Night),
			"custom" | "Custom" => Ok(Self::Custom),
			_ => Err(zbus::fdo::Error::InvalidArgs(format!(
				"invalid nightlight preset: {value}"
			))),
		}
	}
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
