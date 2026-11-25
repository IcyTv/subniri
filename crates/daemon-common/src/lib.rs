#![allow(clippy::missing_errors_doc)]

use zbus::{
	names::{BusName, InterfaceName},
	zvariant::ObjectPath,
};

pub mod calendar;
mod discovery;
pub use discovery::{
	Discovery, DiscoveryProxy, DiscoveryServer, DiscoveryServerSignals, DiscoveryState,
};

pub const DISCOVERY_OBJECT_PATH: ObjectPath<'_> =
	ObjectPath::from_static_str_checked("/de/icytv/subniri/Discovery");

mod appearance;
pub use appearance::*;

pub const APPEARANCE_OBJECT_PATH: ObjectPath<'_> =
	ObjectPath::from_static_str_checked("/de/icytv/subniri/Appearance");
pub const APPEARANCE_INTERFACE: InterfaceName<'_> =
	InterfaceName::from_static_str_checked(AppearanceProperties::INTERFACE_NAME);

mod nightlight;
pub use nightlight::*;

mod battery;
pub use battery::*;

pub const BATTERY_OBJECT_PATH: ObjectPath<'_> =
	ObjectPath::from_static_str_checked("/de/icytv/subniri/Battery");
pub const BATTERY_INTERFACE: InterfaceName<'_> =
	InterfaceName::from_static_str_checked(BatteryProperties::INTERFACE_NAME);

mod power_profile;
pub use power_profile::*;

pub const POWER_PROFILE_OBJECT_PATH: ObjectPath<'_> =
	ObjectPath::from_static_str_checked("/de/icytv/subniri/PowerProfile");
pub const POWER_PROFILE_INTERFACE: InterfaceName<'_> =
	InterfaceName::from_static_str_checked(PowerProfileProperties::INTERFACE_NAME);

mod charge_limit;
pub use charge_limit::*;

pub const CHARGE_LIMIT_OBJECT_PATH: ObjectPath<'_> =
	ObjectPath::from_static_str_checked("/de/icytv/subniri/ChargeLimit");
pub const CHARGE_LIMIT_INTERFACE: InterfaceName<'_> =
	InterfaceName::from_static_str_checked(ChargeLimitProperties::INTERFACE_NAME);

mod spotify;
pub use spotify::*;

mod homeassistant;
pub use homeassistant::*;

mod notifications;
pub use notifications::*;

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

pub const NOTIFICATIONS_OBJECT_PATH: ObjectPath<'_> =
	ObjectPath::from_static_str_checked("/de/icytv/subniri/Notifications");
pub const NOTIFICATIONS_INTERFACE: InterfaceName<'_> =
	InterfaceName::from_static_str_checked("de.icytv.subniri.Notifications");
