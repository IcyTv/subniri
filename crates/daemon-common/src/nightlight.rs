#[zbus::interface(
	name = "de.icytv.subniri.Nightlight",
	proxy(
		default_service = "de.icytv.subniri.Daemon",
		default_path = "/de/icytv/subniri/Nightlight",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait Nightlight {
	#[zbus(property(emits_changed_signal = "const"))]
	fn available(&self) -> bool;

	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn brightness(&self) -> f64;

	#[zbus(property)]
	async fn set_brightness(
		&self, brightness: f64,
		#[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
	) -> zbus::fdo::Result<()>;

	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn temperature(&self) -> u32;

	#[zbus(property)]
	async fn set_temperature(
		&self, temperature: u32,
		#[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
	) -> zbus::fdo::Result<()>;

	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn preset(&self) -> String;

	#[zbus(property)]
	async fn set_preset(
		&self, preset: &str,
		#[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
	) -> zbus::fdo::Result<()>;

	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn enabled(&self) -> bool;

	#[zbus(property)]
	async fn set_enabled(
		&self, enabled: bool,
		#[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
	) -> zbus::fdo::Result<()>;

	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn state(&self) -> NightlightStateTuple;

	async fn toggle(
		&self, #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
	) -> zbus::fdo::Result<()>;
	async fn suspend(
		&self, duration_secs: u64,
		#[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
	) -> zbus::fdo::Result<()>;
	async fn unsuspend(
		&self, #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
	) -> zbus::fdo::Result<()>;
}

pub type NightlightStateTuple = (bool, bool, f64, u32, String);

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

	/// Parse a nightlight preset value.
	///
	/// # Errors
	///
	/// Returns `InvalidArgs` when the value is not a known preset.
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
