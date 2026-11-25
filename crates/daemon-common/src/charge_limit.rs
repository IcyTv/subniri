/// One coherent charge-threshold snapshot. Hardware without sysfs threshold support
/// publishes the default (unavailable) state.
#[derive(
	Debug,
	Default,
	Clone,
	Copy,
	PartialEq,
	serde::Serialize,
	serde::Deserialize,
	zvariant::Type,
	zvariant::Value,
	zvariant::OwnedValue,
)]
pub struct ChargeLimitState {
	pub available: bool,
	/// 0-100; the battery stops charging once this percentage is reached.
	pub end_threshold: u32,
}

#[zbus::interface(
	name = "de.icytv.subniri.ChargeLimit",
	proxy(
		default_service = "de.icytv.subniri.Daemon",
		default_path = "/de/icytv/subniri/ChargeLimit",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait ChargeLimit {
	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn state(&self) -> ChargeLimitState;

	async fn set_end_threshold(&self, value: u32) -> zbus::fdo::Result<()>;
}
