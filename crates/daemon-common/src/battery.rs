/// One coherent display-device snapshot. Unavailable backends publish the default state.
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
pub struct BatteryState {
	pub present: bool,
	pub percentage: f64,
	/// UPower device state: charging=1, discharging=2, empty=3, full=4, pending=5/6.
	pub state: u32,
}

#[zbus::interface(
	name = "de.icytv.subniri.Battery",
	proxy(
		default_service = "de.icytv.subniri.Daemon",
		default_path = "/de/icytv/subniri/Battery",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait Battery {
	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn state(&self) -> BatteryState;
}
