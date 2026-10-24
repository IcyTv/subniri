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
