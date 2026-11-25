use std::{collections::HashMap, sync::Arc, time::Duration};

use daemon_common::{BATTERY_INTERFACE, BATTERY_OBJECT_PATH, Battery, BatteryServer, BatteryState};
use futures::StreamExt;
use tokio::sync::{Mutex, watch};
use zbus::{Connection, object_server::SignalEmitter, zvariant::OwnedValue};

const UPOWER_SERVICE: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const DEVICE_INTERFACE: &str = "org.freedesktop.UPower.Device";

#[derive(Clone)]
pub struct BatteryService {
	connection: Connection,
	state: Arc<Mutex<BatteryState>>,
}

impl BatteryService {
	pub async fn register(connection: Connection) -> zbus::Result<Self> {
		let service = Self {
			connection,
			state: Arc::default(),
		};
		if !service
			.connection
			.object_server()
			.at(BATTERY_OBJECT_PATH, BatteryServer(service.clone()))
			.await?
		{
			return Err(zbus::Error::Failure(
				"battery interface is already registered".into(),
			));
		}
		Ok(service)
	}

	pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
		let mut backoff = Duration::from_secs(1);
		while !*shutdown.borrow() {
			let observe = async {
				let connection = Connection::system().await?;
				self.observe(&connection, &mut backoff).await
			};
			tokio::select! {
				result = observe => {
					if let Err(error) = result {
						log::debug!("Battery backend unavailable: {error}");
					}
				}
				_ = shutdown.changed() => break,
			}
			self.publish(BatteryState::default()).await;

			tokio::select! {
				() = tokio::time::sleep(backoff) => {}
				_ = shutdown.changed() => break,
			}
			backoff = (backoff * 2).min(Duration::from_secs(30));
		}

		self.publish(BatteryState::default()).await;
		if let Err(error) = self
			.connection
			.object_server()
			.remove::<BatteryServer<Self>, _>(BATTERY_OBJECT_PATH)
			.await
		{
			log::warn!("Failed to unregister battery interface: {error}");
		}
	}

	async fn publish(&self, snapshot: BatteryState) {
		let mut state = self.state.lock().await;
		if *state == snapshot {
			return;
		}
		*state = snapshot;
		drop(state);

		let result = async {
			let emitter = SignalEmitter::new(&self.connection, BATTERY_OBJECT_PATH)?;
			zbus::fdo::Properties::properties_changed(
				&emitter,
				BATTERY_INTERFACE,
				HashMap::new(),
				std::borrow::Cow::Borrowed(
					daemon_common::BatteryProperties::MUTABLE_PROPERTY_NAMES,
				),
			)
			.await
		}
		.await;
		if let Err(error) = result {
			log::warn!("Failed to notify battery state: {error}");
		}
	}

	async fn observe(&self, connection: &Connection, backoff: &mut Duration) -> zbus::Result<()> {
		let root =
			zbus::Proxy::new(connection, UPOWER_SERVICE, UPOWER_PATH, UPOWER_SERVICE).await?;
		let mut owners = root.receive_owner_changed().await?;
		let bus = zbus::fdo::DBusProxy::new(connection).await?;
		let owner = bus.get_name_owner(UPOWER_SERVICE.try_into()?).await?;
		// Pin reads to one service instance, never combine snapshots across a restart.
		let root =
			zbus::Proxy::new(connection, owner.as_str(), UPOWER_PATH, UPOWER_SERVICE).await?;
		let path: zbus::zvariant::OwnedObjectPath = root.call("GetDisplayDevice", &()).await?;
		let properties = zbus::fdo::PropertiesProxy::builder(connection)
			.destination(owner.clone())?
			.path(path.clone())?
			.build()
			.await?;
		let mut changes = properties
			.receive_properties_changed_with_args(&[(0, DEVICE_INTERFACE)])
			.await?;
		let mut messages = zbus::MessageStream::from(connection);

		// Health (EnergyFullDesign/ChargeCycles) is never carried by UPower's synthetic
		// DisplayDevice; a lookup failure here must not take down the primary percentage feed.
		let health_properties =
			match find_health_device(connection, &owner, &root, &path).await {
				Ok(Some(device)) => zbus::fdo::PropertiesProxy::builder(connection)
					.destination(owner.clone())?
					.path(device)?
					.build()
					.await
					.ok(),
				Ok(None) => None,
				Err(error) => {
					log::debug!("Battery health device lookup failed: {error}");
					None
				}
			};

		loop {
			let values = properties.get_all(DEVICE_INTERFACE.try_into()?).await?;
			let health = match &health_properties {
				Some(health_properties) => {
					match health_properties.get_all(DEVICE_INTERFACE.try_into()?).await {
						Ok(values) => Some(values),
						Err(error) => {
							log::debug!("Battery health property refresh failed: {error}");
							None
						}
					}
				}
				None => None,
			};
			self.publish(snapshot(&values, health.as_ref())?).await;
			*backoff = Duration::from_secs(1);

			loop {
				tokio::select! {
					change = changes.next() => {
						let Some(change) = change else { return Ok(()); };
						change.args()?;
						// Refresh the complete snapshot for changed AND invalidated properties.
						break;
					}
					_ = owners.next() => return Ok(()),
					message = messages.next() => {
						if !matches!(message, Some(Ok(_))) { return Ok(()); }
					}
				}
			}
		}
	}
}

impl Battery for BatteryService {
	async fn state(&self) -> BatteryState {
		*self.state.lock().await
	}
}

fn snapshot(
	values: &HashMap<String, OwnedValue>, health: Option<&HashMap<String, OwnedValue>>,
) -> zbus::Result<BatteryState> {
	let missing = |name| zbus::Error::Failure(format!("missing UPower property {name}"));
	let present = values
		.get("IsPresent")
		.ok_or_else(|| missing("IsPresent"))?
		.downcast_ref::<bool>()?;
	let kind = values
		.get("Type")
		.ok_or_else(|| missing("Type"))?
		.downcast_ref::<u32>()?;
	if !present || kind != 2 {
		return Ok(BatteryState::default());
	}

	let percentage = values
		.get("Percentage")
		.ok_or_else(|| missing("Percentage"))?
		.downcast_ref::<f64>()?;
	if !percentage.is_finite() {
		return Err(zbus::Error::Failure("invalid UPower percentage".into()));
	}
	let state = values
		.get("State")
		.ok_or_else(|| missing("State"))?
		.downcast_ref::<u32>()?;
	let energy_rate = optional_finite_f64(values, "EnergyRate");
	let time_to_full = optional_u64(values, "TimeToFull");
	let time_to_empty = optional_u64(values, "TimeToEmpty");
	let (energy_full, energy_full_design, cycle_count) = health.map_or(
		(0.0, 0.0, 0),
		|health| {
			(
				optional_finite_f64(health, "EnergyFull"),
				optional_finite_f64(health, "EnergyFullDesign"),
				optional_cycle_count(health),
			)
		},
	);
	Ok(BatteryState {
		present: true,
		percentage: percentage.clamp(0.0, 100.0),
		state,
		energy_rate,
		time_to_full,
		time_to_empty,
		energy_full,
		energy_full_design,
		cycle_count,
	})
}

/// Finds the physical battery device (`Type == 2`) backing UPower's synthetic DisplayDevice,
/// which never reports `EnergyFullDesign`/`ChargeCycles` itself.
async fn find_health_device(
	connection: &Connection, owner: &zbus::names::OwnedUniqueName, root: &zbus::Proxy<'_>,
	display_path: &zbus::zvariant::OwnedObjectPath,
) -> zbus::Result<Option<zbus::zvariant::OwnedObjectPath>> {
	let devices: Vec<zbus::zvariant::OwnedObjectPath> = root.call("EnumerateDevices", &()).await?;
	for device in devices {
		if device == *display_path {
			continue;
		}
		let properties = zbus::fdo::PropertiesProxy::builder(connection)
			.destination(owner.clone())?
			.path(device.clone())?
			.build()
			.await?;
		let values = properties.get_all(DEVICE_INTERFACE.try_into()?).await?;
		let kind = values
			.get("Type")
			.and_then(|value| value.downcast_ref::<u32>().ok());
		if kind == Some(2) {
			return Ok(Some(device));
		}
	}
	Ok(None)
}

/// `EnergyRate` is legitimately absent (or momentarily invalid) on some devices/states;
/// unlike `Percentage` this must degrade to 0.0 instead of failing the whole snapshot.
fn optional_finite_f64(values: &HashMap<String, OwnedValue>, name: &str) -> f64 {
	values
		.get(name)
		.and_then(|value| value.downcast_ref::<f64>().ok())
		.filter(|value| value.is_finite())
		.unwrap_or(0.0)
}

/// `ChargeCycles` is `i32` and reports -1 when the firmware doesn't expose it.
fn optional_cycle_count(values: &HashMap<String, OwnedValue>) -> u32 {
	values
		.get("ChargeCycles")
		.and_then(|value| value.downcast_ref::<i32>().ok())
		.filter(|value| *value >= 0)
		.map_or(0, |value| value as u32)
}

fn optional_u64(values: &HashMap<String, OwnedValue>, name: &str) -> u64 {
	values
		.get(name)
		.and_then(|value| value.downcast_ref::<i64>().ok())
		.filter(|value| *value >= 0)
		.map_or(0, |value| value as u64)
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::{
		io::BufRead,
		process::{Command, Stdio},
	};

	struct Broker(std::process::Child);

	impl Drop for Broker {
		fn drop(&mut self) {
			let _ = self.0.kill();
			let _ = self.0.wait();
		}
	}

	struct DisplayDevice(Arc<Mutex<(bool, u32, f64, u32, f64, i64, i64)>>);

	#[zbus::interface(name = "org.freedesktop.UPower.Device")]
	impl DisplayDevice {
		#[zbus(property)]
		async fn is_present(&self) -> bool {
			self.0.lock().await.0
		}

		#[zbus(property, name = "Type")]
		async fn kind(&self) -> u32 {
			self.0.lock().await.1
		}

		#[zbus(property)]
		async fn percentage(&self) -> f64 {
			self.0.lock().await.2
		}

		#[zbus(property)]
		async fn state(&self) -> u32 {
			self.0.lock().await.3
		}

		#[zbus(property)]
		async fn energy_rate(&self) -> f64 {
			self.0.lock().await.4
		}

		#[zbus(property)]
		async fn time_to_full(&self) -> i64 {
			self.0.lock().await.5
		}

		#[zbus(property)]
		async fn time_to_empty(&self) -> i64 {
			self.0.lock().await.6
		}
	}

	struct UPower;

	#[zbus::interface(name = "org.freedesktop.UPower")]
	impl UPower {
		fn get_display_device(&self) -> zbus::zvariant::ObjectPath<'static> {
			zbus::zvariant::ObjectPath::from_static_str_checked(
				"/org/freedesktop/UPower/devices/DisplayDevice",
			)
		}
	}

	async fn fake_upower(
		address: &str, state: Arc<Mutex<(bool, u32, f64, u32, f64, i64, i64)>>,
	) -> Connection {
		zbus::connection::Builder::address(address)
			.unwrap()
			.name(UPOWER_SERVICE)
			.unwrap()
			.serve_at(UPOWER_PATH, UPower)
			.unwrap()
			.serve_at(
				"/org/freedesktop/UPower/devices/DisplayDevice",
				DisplayDevice(state),
			)
			.unwrap()
			.build()
			.await
			.unwrap()
	}

	async fn wait_for_state(service: &BatteryService, expected: BatteryState) {
		tokio::time::timeout(Duration::from_secs(3), async {
			while service.state().await != expected {
				tokio::task::yield_now().await;
			}
		})
		.await
		.expect("battery snapshot did not converge");
	}

	fn observe_private(
		service: BatteryService, connection: Connection,
	) -> tokio::task::JoinHandle<()> {
		tokio::spawn(async move {
			let _ = service
				.observe(&connection, &mut Duration::from_secs(1))
				.await;
			service.publish(BatteryState::default()).await;
		})
	}

	#[tokio::test]
	async fn battery_upower_invalidations_owner_restart_and_bus_disconnect() {
		// Explicit private address on every connection: never use either live user bus.
		let mut broker = Broker(
			Command::new("dbus-daemon")
				.args(["--session", "--nofork", "--print-address=1"])
				.stdout(Stdio::piped())
				.spawn()
				.unwrap(),
		);
		let mut address = String::new();
		std::io::BufReader::new(broker.0.stdout.take().unwrap())
			.read_line(&mut address)
			.unwrap();
		let address = address.trim();
		assert!(address.starts_with("unix:"));
		let connection = zbus::connection::Builder::address(address)
			.unwrap()
			.build()
			.await
			.unwrap();
		let service = BatteryService::register(connection.clone()).await.unwrap();
		let device = Arc::new(Mutex::new((true, 2, 51.0, 1, 12.5, 0, 600)));
		let upower = fake_upower(address, device.clone()).await;
		let observer = observe_private(service.clone(), connection.clone());
		wait_for_state(
			&service,
			BatteryState {
				present: true,
				percentage: 51.0,
				state: 1,
				energy_rate: 12.5,
				time_to_full: 0,
				time_to_empty: 600,
				energy_full: 0.0,
				energy_full_design: 0.0,
				cycle_count: 0,
			},
		)
		.await;

		*device.lock().await = (true, 2, 52.0, 2, 8.0, 0, 500);
		let emitter =
			SignalEmitter::new(&upower, "/org/freedesktop/UPower/devices/DisplayDevice").unwrap();
		zbus::fdo::Properties::properties_changed(
			&emitter,
			DEVICE_INTERFACE.try_into().unwrap(),
			HashMap::new(),
			std::borrow::Cow::Borrowed(&["Percentage", "State"]),
		)
		.await
		.unwrap();
		wait_for_state(
			&service,
			BatteryState {
				present: true,
				percentage: 52.0,
				state: 2,
				energy_rate: 8.0,
				time_to_full: 0,
				time_to_empty: 500,
				energy_full: 0.0,
				energy_full_design: 0.0,
				cycle_count: 0,
			},
		)
		.await;

		upower.close().await.unwrap();
		tokio::time::timeout(Duration::from_secs(3), observer)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(service.state().await, BatteryState::default());

		*device.lock().await = (true, 2, 75.0, 4, 0.0, 0, 0);
		let _replacement = fake_upower(address, device).await;
		let observer = observe_private(service.clone(), connection);
		wait_for_state(
			&service,
			BatteryState {
				present: true,
				percentage: 75.0,
				state: 4,
				energy_rate: 0.0,
				time_to_full: 0,
				time_to_empty: 0,
				energy_full: 0.0,
				energy_full_design: 0.0,
				cycle_count: 0,
			},
		)
		.await;

		broker.0.kill().unwrap();
		tokio::time::timeout(Duration::from_secs(3), observer)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(service.state().await, BatteryState::default());
	}

	fn values(
		present: bool, kind: u32, percentage: f64, state: u32,
	) -> HashMap<String, OwnedValue> {
		HashMap::from([
			("IsPresent".into(), present.into()),
			("Type".into(), kind.into()),
			("Percentage".into(), percentage.into()),
			("State".into(), state.into()),
		])
	}

	#[test]
	fn battery_snapshot_uses_only_present_system_battery_without_power_supply() {
		for kind in [0, 1, 3, 5, 8] {
			assert_eq!(
				snapshot(&values(true, kind, 70.0, 1), None).unwrap(),
				BatteryState::default()
			);
		}
		assert_eq!(
			snapshot(&values(false, 2, 70.0, 1), None).unwrap(),
			BatteryState::default()
		);
		assert_eq!(
			snapshot(&values(true, 2, 70.5, 1), None).unwrap(),
			BatteryState {
				present: true,
				percentage: 70.5,
				state: 1,
				energy_rate: 0.0,
				time_to_full: 0,
				time_to_empty: 0,
				energy_full: 0.0,
				energy_full_design: 0.0,
				cycle_count: 0,
			}
		);
		assert_eq!(
			snapshot(&values(true, 2, 120.0, 4), None).unwrap().percentage,
			100.0
		);
		assert!(snapshot(&values(true, 2, f64::NAN, 1), None).is_err());
		assert!(snapshot(&HashMap::new(), None).is_err());
	}

	#[test]
	fn battery_snapshot_reads_optional_energy_fields_and_tolerates_invalid_ones() {
		let mut present = values(true, 2, 80.0, 1);
		present.insert("EnergyRate".into(), 15.25.into());
		present.insert("TimeToFull".into(), 1200_i64.into());
		present.insert("TimeToEmpty".into(), (-1_i64).into());
		let state = snapshot(&present, None).unwrap();
		assert_eq!(state.energy_rate, 15.25);
		assert_eq!(state.time_to_full, 1200);
		assert_eq!(state.time_to_empty, 0, "a negative TimeToEmpty is invalid");

		let mut invalid = values(true, 2, 80.0, 1);
		invalid.insert("EnergyRate".into(), f64::NAN.into());
		assert_eq!(snapshot(&invalid, None).unwrap().energy_rate, 0.0);
	}

	#[test]
	fn battery_snapshot_reads_health_from_the_physical_device_and_tolerates_unknown_cycles() {
		let present = values(true, 2, 80.0, 1);
		let mut health = HashMap::new();
		health.insert("EnergyFull".into(), 75.76.into());
		health.insert("EnergyFullDesign".into(), 75.0.into());
		health.insert("ChargeCycles".into(), 1_i32.into());
		let state = snapshot(&present, Some(&health)).unwrap();
		assert_eq!(state.energy_full, 75.76);
		assert_eq!(state.energy_full_design, 75.0);
		assert_eq!(state.cycle_count, 1);

		let mut unknown_cycles = health.clone();
		unknown_cycles.insert("ChargeCycles".into(), (-1_i32).into());
		assert_eq!(
			snapshot(&present, Some(&unknown_cycles)).unwrap().cycle_count,
			0,
			"a negative ChargeCycles means the firmware doesn't report it"
		);

		assert_eq!(
			snapshot(&present, None).unwrap(),
			BatteryState {
				present: true,
				percentage: 80.0,
				state: 1,
				energy_rate: 0.0,
				time_to_full: 0,
				time_to_empty: 0,
				energy_full: 0.0,
				energy_full_design: 0.0,
				cycle_count: 0,
			},
			"a missing health device must degrade to unknown health, not fail the snapshot"
		);
	}

	#[tokio::test]
	async fn battery_state_notifies_updates_unavailability_and_recovery() {
		let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
		let server = zbus::connection::Builder::unix_stream(server_socket)
			.p2p()
			.server(zbus::Guid::generate())
			.unwrap()
			.build();
		let client = zbus::connection::Builder::unix_stream(client_socket)
			.p2p()
			.build();
		let (server, client) = tokio::join!(server, client);
		let service = BatteryService::register(server.unwrap()).await.unwrap();
		let client = client.unwrap();
		let proxy = daemon_common::BatteryProxy::new(&client).await.unwrap();
		let mut changes = proxy.receive_state_changed().await;

		let initial = tokio::time::timeout(Duration::from_secs(2), changes.next())
			.await
			.unwrap()
			.unwrap();
		assert_eq!(initial.get().await.unwrap(), BatteryState::default());

		for state in [
			BatteryState {
				present: true,
				percentage: 65.0,
				state: 1,
				energy_rate: 10.0,
				time_to_full: 300,
				time_to_empty: 0,
				energy_full: 0.0,
				energy_full_design: 0.0,
				cycle_count: 0,
			},
			BatteryState {
				present: true,
				percentage: 66.0,
				state: 2,
				energy_rate: 9.0,
				time_to_full: 0,
				time_to_empty: 3600,
				energy_full: 0.0,
				energy_full_design: 0.0,
				cycle_count: 0,
			},
			BatteryState::default(),
			BatteryState {
				present: true,
				percentage: 67.0,
				state: 4,
				energy_rate: 0.0,
				time_to_full: 0,
				time_to_empty: 0,
				energy_full: 0.0,
				energy_full_design: 0.0,
				cycle_count: 0,
			},
		] {
			service.publish(state).await;
			let changed = tokio::time::timeout(Duration::from_secs(2), changes.next())
				.await
				.unwrap()
				.unwrap();
			assert_eq!(changed.get().await.unwrap(), state);
		}
	}
}
