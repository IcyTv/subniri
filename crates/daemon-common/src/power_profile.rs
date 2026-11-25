/// One coherent power-profiles-daemon snapshot. Unavailable backends publish the default state.
#[derive(
	Debug,
	Default,
	Clone,
	PartialEq,
	serde::Serialize,
	serde::Deserialize,
	zvariant::Type,
	zvariant::Value,
	zvariant::OwnedValue,
)]
pub struct PowerProfileState {
	pub available: bool,
	pub active: String,
	pub profiles: Vec<String>,
}

#[zbus::interface(
	name = "de.icytv.subniri.PowerProfile",
	proxy(
		default_service = "de.icytv.subniri.Daemon",
		default_path = "/de/icytv/subniri/PowerProfile",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait PowerProfile {
	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn state(&self) -> PowerProfileState;

	async fn set_active_profile(&self, profile: String) -> zbus::fdo::Result<()>;
}
