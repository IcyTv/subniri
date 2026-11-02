pub const HOMEASSISTANT_REDIRECT_URI: &str = "http://127.0.0.1:8765/callback";

#[derive(
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
	serde::Serialize,
	serde::Deserialize,
	zvariant::Type,
	zvariant::Value,
	zvariant::OwnedValue,
)]
#[zvariant(signature = "s")]
pub enum HomeassistantStatus {
	Authorizing,
	NoLongLivedToken,
	NoUrl,
	Connecting,
	Disabled,
	Ready,
	Error,
	InvalidUrl,
}

impl HomeassistantStatus {
	pub fn human_string<'a>(&'a self) -> &'a str {
		match self {
			HomeassistantStatus::Authorizing => "Currently Authorizing",
			HomeassistantStatus::NoLongLivedToken => "Not Authorized",
			HomeassistantStatus::NoUrl => "Missing URL",
			HomeassistantStatus::Connecting => "Connecting to instance",
			HomeassistantStatus::Disabled => "Disabled",
			HomeassistantStatus::Ready => "Ready to rumble",
			HomeassistantStatus::Error => "Error",
			HomeassistantStatus::InvalidUrl => "Invalid URL",
		}
	}
}

#[zbus::interface(
	name = "de.icytv.subniri.Homeassistant",
	proxy(
		default_path = "/de/icytv/subniri/Homeassistant",
		default_service = "de.icytv.subniri.Daemon",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait Homeassistant {
	#[zbus(property)]
	async fn authenticated(&self) -> bool;

	#[zbus(property)]
	async fn status(&self) -> HomeassistantStatus;

	async fn begin_authorization(
		&self, #[zbus(header)] header: zbus::message::Header<'_>,
	) -> zbus::fdo::Result<String>;
	async fn cancel_authorization(
		&self, #[zbus(header)] header: zbus::message::Header<'_>,
	) -> zbus::fdo::Result<()>;
	async fn complete_authorization(
		&self, code: &str, csrf_token: &str, #[zbus(header)] header: zbus::message::Header<'_>,
	) -> zbus::fdo::Result<bool>;
}

#[zbus::interface(
	name = "de.icytv.subniri.Homeassistant.Entity",
	property_snapshot_name = "HomeassistantEntityInfo",
	managed_property_snapshot_name = "HomeassistantEntitySummary",
	property_snapshot(derive(Clone, PartialEq, Eq, Default)),
	proxy(
		default_service = "de.icytv.subniri.Daemon",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait HomeassistantEntity {
	/// Interaction interfaces currently attached at this entity's object path.
	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn capabilities(&self) -> Vec<HomeassistantCapability>;

	#[zbus(property(emits_changed_signal = "const"))]
	async fn entity_id(&self) -> String;

	#[zbus(property(emits_changed_signal = "const"), object_manager(skip))]
	async fn domain(&self) -> String;

	#[zbus(property(emits_changed_signal = "invalidates"), object_manager(skip))]
	async fn state(&self) -> String;

	#[zbus(property(emits_changed_signal = "invalidates"), object_manager(skip))]
	async fn available(&self) -> bool;

	#[zbus(property(emits_changed_signal = "invalidates"), object_manager(skip))]
	async fn attributes_json(&self) -> String;

	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn friendly_name(&self) -> String;

	#[zbus(property(emits_changed_signal = "invalidates"), object_manager(skip))]
	async fn icon(&self) -> String;

	#[zbus(property(emits_changed_signal = "invalidates"), object_manager(skip))]
	async fn device_class(&self) -> String;

	#[zbus(property(emits_changed_signal = "invalidates"), object_manager(skip))]
	async fn unit_of_measurement(&self) -> String;

	#[zbus(property(emits_changed_signal = "invalidates"), object_manager(skip))]
	async fn last_changed(&self) -> String;

	#[zbus(property(emits_changed_signal = "invalidates"), object_manager(skip))]
	async fn last_updated(&self) -> String;

	#[zbus(property(emits_changed_signal = "invalidates"), object_manager(skip))]
	async fn device_id(&self) -> String;

	#[zbus(property(emits_changed_signal = "invalidates"), object_manager(skip))]
	async fn area_id(&self) -> String;

	#[zbus(property(emits_changed_signal = "invalidates"), object_manager(skip))]
	async fn platform(&self) -> String;

	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn entity_category(&self) -> zbus::zvariant::Optional<String>;

	async fn observe(
		&self, #[zbus(header)] header: zbus::message::Header<'_>,
	) -> zbus::fdo::Result<bool>;

	async fn unobserve(
		&self, #[zbus(header)] header: zbus::message::Header<'_>,
	) -> zbus::fdo::Result<()>;
}

#[derive(
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
	serde::Serialize,
	serde::Deserialize,
	zvariant::Type,
	zvariant::Value,
	zvariant::OwnedValue,
)]
#[zvariant(signature = "s")]
pub enum HomeassistantCapability {
	Callable,
	Togglable,
	Brightness,
	ColorTemperature,
	Color,
}

impl HomeassistantCapability {
	pub fn interface_name(self) -> &'static str {
		match self {
			Self::Callable => "de.icytv.subniri.Homeassistant.Callable",
			Self::Togglable => "de.icytv.subniri.Homeassistant.Togglable",
			Self::Brightness => "de.icytv.subniri.Homeassistant.Brightness",
			Self::ColorTemperature => "de.icytv.subniri.Homeassistant.ColorTemperature",
			Self::Color => "de.icytv.subniri.Homeassistant.Color",
		}
	}
}

#[zbus::interface(
	name = "de.icytv.subniri.Homeassistant.Callable",
	proxy(
		default_service = "de.icytv.subniri.Daemon",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait HomeassistantCallable {
	/// Start a script without waiting for completion, activate a scene, or press a button.
	async fn call(&self) -> zbus::fdo::Result<()>;
}

#[zbus::interface(
	name = "de.icytv.subniri.Homeassistant.Togglable",
	proxy(
		default_service = "de.icytv.subniri.Daemon",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait HomeassistantTogglable {
	/// Readings return an error when unknown; they never substitute a default value.
	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn enabled(&self) -> zbus::fdo::Result<bool>;
	async fn set_enabled(&self, enabled: bool) -> zbus::fdo::Result<()>;
	async fn toggle(&self) -> zbus::fdo::Result<()>;
}

#[zbus::interface(
	name = "de.icytv.subniri.Homeassistant.Brightness",
	proxy(
		default_service = "de.icytv.subniri.Daemon",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait HomeassistantBrightness {
	/// Home Assistant's 0–255 scale; setting zero turns the light off.
	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn brightness(&self) -> zbus::fdo::Result<u8>;
	async fn set_brightness(&self, brightness: u8) -> zbus::fdo::Result<()>;
}

#[zbus::interface(
	name = "de.icytv.subniri.Homeassistant.ColorTemperature",
	proxy(
		default_service = "de.icytv.subniri.Daemon",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait HomeassistantColorTemperature {
	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn color_temperature(&self) -> zbus::fdo::Result<u32>;
	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn min_kelvin(&self) -> zbus::fdo::Result<u32>;
	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn max_kelvin(&self) -> zbus::fdo::Result<u32>;
	async fn set_color_temperature(&self, kelvin: u32) -> zbus::fdo::Result<()>;
}

#[zbus::interface(
	name = "de.icytv.subniri.Homeassistant.Color",
	proxy(
		default_service = "de.icytv.subniri.Daemon",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait HomeassistantColor {
	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn rgb(&self) -> zbus::fdo::Result<(u8, u8, u8)>;
	async fn set_rgb(&self, red: u8, green: u8, blue: u8) -> zbus::fdo::Result<()>;
}
