#![allow(clippy::missing_errors_doc)]

use zbus::{
	names::{BusName, InterfaceName},
	zvariant::ObjectPath,
};

pub mod calendar;

pub const DEFAULT_BUS_NAME: BusName<'_> =
	BusName::from_static_str_checked("de.icytv.subniri.Daemon");

pub const NIGHTLIGHT_OBJECT_PATH: ObjectPath<'_> =
	ObjectPath::from_static_str_checked("/de/icytv/subniri/Nightlight");
pub const NIGHTLIGHT_INTERFACE: InterfaceName<'_> =
	InterfaceName::from_static_str_checked("de.icytv.subniri.Nightlight");

pub const CALENDAR_OBJECT_PATH: ObjectPath<'_> =
	ObjectPath::from_static_str_checked("/de/icytv/subniri/Calendar");
pub const CALENDAR_INTERFACE: InterfaceName<'_> =
	InterfaceName::from_static_str_checked("de.icytv.subniri.Calendar");

pub const SPOTIFY_OBJECT_PATH: ObjectPath<'_> =
	ObjectPath::from_static_str_checked("/de/icytv/subniri/Spotify");
pub const SPOTIFY_INTERFACE: InterfaceName<'_> =
	InterfaceName::from_static_str_checked("de.icytv.subniri.Spotify");

#[zbus::proxy(
	interface = NIGHTLIGHT_INTERFACE,
	default_service = DEFAULT_BUS_NAME,
	default_path = NIGHTLIGHT_OBJECT_PATH
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

#[zbus::proxy(
	interface = SPOTIFY_INTERFACE,
	default_service = DEFAULT_BUS_NAME,
	default_path = SPOTIFY_OBJECT_PATH
)]
pub trait Spotify {
	#[zbus(property)]
	fn status(&self) -> zbus::Result<String>;

	#[zbus(property)]
	fn authenticated(&self) -> zbus::Result<bool>;

	fn begin_authorization(&self) -> zbus::Result<String>;
	fn track_saved(&self, track_id: &str) -> zbus::Result<bool>;
	fn set_track_saved(&self, track_id: &str, saved: bool) -> zbus::Result<bool>;
}
