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
	/// Watts; positive while charging or discharging, 0.0 if unknown.
	pub energy_rate: f64,
	/// Seconds until full; 0 if not charging or unknown.
	pub time_to_full: u64,
	/// Seconds until empty; 0 if not discharging or unknown.
	pub time_to_empty: u64,
	/// Wh; current full-charge capacity of the physical battery, 0.0 if unknown.
	pub energy_full: f64,
	/// Wh; design capacity when new, 0.0 if unknown. `energy_full / energy_full_design`
	/// is the battery health ratio; UPower's synthetic DisplayDevice never reports this,
	/// so it is sourced from the underlying physical battery device instead.
	pub energy_full_design: f64,
	/// Charge cycles reported by firmware, 0 if unknown.
	pub cycle_count: u32,
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
