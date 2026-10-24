#![allow(clippy::missing_errors_doc)]

use zbus::{
	names::{BusName, InterfaceName},
	zvariant::ObjectPath,
};

pub mod calendar;

mod nightlight;
pub use nightlight::*;

mod spotify;
pub use spotify::*;

mod homeassistant;
pub use homeassistant::*;

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
pub const SPOTIFY_TRACK_INTERFACE: InterfaceName<'_> =
	InterfaceName::from_static_str_checked("de.icytv.subniri.Spotify.Track");

pub const HOMEASSISTANT_OBJECT_PATH: ObjectPath<'_> =
	ObjectPath::from_static_str_checked("/de/icytv/subniri/Homeassistant");
pub const HOMEASSISTANT_INTERFACE: InterfaceName<'_> =
	InterfaceName::from_static_str_checked("de.icytv.subniri.Homeassistant");
