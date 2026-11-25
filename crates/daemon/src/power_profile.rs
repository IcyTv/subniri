use std::{collections::HashMap, sync::Arc, time::Duration};

use daemon_common::{
	POWER_PROFILE_INTERFACE, POWER_PROFILE_OBJECT_PATH, PowerProfile, PowerProfileServer,
	PowerProfileState,
};
use futures::StreamExt;
use tokio::sync::{Mutex, watch};
use zbus::{
	Connection,
	object_server::SignalEmitter,
	zvariant::{Array, Dict, OwnedValue},
};

const POWER_PROFILES_SERVICE: &str = "net.hadess.PowerProfiles";
const POWER_PROFILES_PATH: &str = "/net/hadess/PowerProfiles";
const POWER_PROFILES_INTERFACE: &str = "net.hadess.PowerProfiles";

#[derive(Clone)]
pub struct PowerProfileService {
	connection: Connection,
	state: Arc<Mutex<PowerProfileState>>,
}

impl PowerProfileService {
	pub async fn register(connection: Connection) -> zbus::Result<Self> {
		let service = Self {
			connection,
			state: Arc::default(),
		};
		if !service
			.connection
			.object_server()
			.at(POWER_PROFILE_OBJECT_PATH, PowerProfileServer(service.clone()))
			.await?
		{
			return Err(zbus::Error::Failure(
				"power profile interface is already registered".into(),
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
						log::debug!("Power profile backend unavailable: {error}");
					}
				}
				_ = shutdown.changed() => break,
			}
			self.publish(PowerProfileState::default()).await;

			tokio::select! {
				() = tokio::time::sleep(backoff) => {}
				_ = shutdown.changed() => break,
			}
			backoff = (backoff * 2).min(Duration::from_secs(30));
		}

		self.publish(PowerProfileState::default()).await;
		if let Err(error) = self
			.connection
			.object_server()
			.remove::<PowerProfileServer<Self>, _>(POWER_PROFILE_OBJECT_PATH)
			.await
		{
			log::warn!("Failed to unregister power profile interface: {error}");
		}
	}

	async fn publish(&self, snapshot: PowerProfileState) {
		let mut state = self.state.lock().await;
		if *state == snapshot {
			return;
		}
		*state = snapshot;
		drop(state);

		let result = async {
			let emitter = SignalEmitter::new(&self.connection, POWER_PROFILE_OBJECT_PATH)?;
			zbus::fdo::Properties::properties_changed(
				&emitter,
				POWER_PROFILE_INTERFACE,
				HashMap::new(),
				std::borrow::Cow::Borrowed(
					daemon_common::PowerProfileProperties::MUTABLE_PROPERTY_NAMES,
				),
			)
			.await
		}
		.await;
		if let Err(error) = result {
			log::warn!("Failed to notify power profile state: {error}");
		}
	}

	async fn observe(&self, connection: &Connection, backoff: &mut Duration) -> zbus::Result<()> {
		let root = zbus::Proxy::new(
			connection,
			POWER_PROFILES_SERVICE,
			POWER_PROFILES_PATH,
			POWER_PROFILES_SERVICE,
		)
		.await?;
		let mut owners = root.receive_owner_changed().await?;
		let bus = zbus::fdo::DBusProxy::new(connection).await?;
		let owner = bus
			.get_name_owner(POWER_PROFILES_SERVICE.try_into()?)
			.await?;
		// Pin reads to one service instance, never combine snapshots across a restart.
		let properties = zbus::fdo::PropertiesProxy::builder(connection)
			.destination(owner.as_str())?
			.path(POWER_PROFILES_PATH)?
			.build()
			.await?;
		let mut changes = properties
			.receive_properties_changed_with_args(&[(0, POWER_PROFILES_INTERFACE)])
			.await?;
		let mut messages = zbus::MessageStream::from(connection);

		loop {
			let values = properties
				.get_all(POWER_PROFILES_INTERFACE.try_into()?)
				.await?;
			self.publish(snapshot(&values)?).await;
			*backoff = Duration::from_secs(1);

			loop {
				tokio::select! {
					change = changes.next() => {
						let Some(change) = change else { return Ok(()); };
						change.args()?;
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

impl PowerProfile for PowerProfileService {
	async fn state(&self) -> PowerProfileState {
		self.state.lock().await.clone()
	}

	async fn set_active_profile(&self, profile: String) -> zbus::fdo::Result<()> {
		let failed = |error: zbus::Error| zbus::fdo::Error::Failed(error.to_string());

		let connection = Connection::system().await.map_err(failed)?;
		let bus = zbus::fdo::DBusProxy::new(&connection).await.map_err(failed)?;
		let service_name = POWER_PROFILES_SERVICE
			.try_into()
			.map_err(|error: zbus::names::Error| zbus::fdo::Error::Failed(error.to_string()))?;
		let owner = bus.get_name_owner(service_name).await.map_err(|_| {
			zbus::fdo::Error::Failed("power-profiles-daemon is not running".into())
		})?;
		let properties = zbus::fdo::PropertiesProxy::builder(&connection)
			.destination(owner.as_str())
			.map_err(failed)?
			.path(POWER_PROFILES_PATH)
			.map_err(failed)?
			.build()
			.await
			.map_err(failed)?;
		properties
			.set(
				POWER_PROFILES_INTERFACE
					.try_into()
					.map_err(|error: zbus::names::Error| zbus::fdo::Error::Failed(error.to_string()))?,
				"ActiveProfile",
				zbus::zvariant::Value::from(profile),
			)
			.await
	}
}

fn snapshot(values: &HashMap<String, OwnedValue>) -> zbus::Result<PowerProfileState> {
	let missing = |name| zbus::Error::Failure(format!("missing PowerProfiles property {name}"));
	let active = values
		.get("ActiveProfile")
		.ok_or_else(|| missing("ActiveProfile"))?
		.downcast_ref::<String>()?;
	let entries = values
		.get("Profiles")
		.ok_or_else(|| missing("Profiles"))?
		.downcast_ref::<Array>()?;
	let profiles = entries
		.iter()
		.filter_map(|entry| {
			let dict = entry.downcast_ref::<Dict>().ok()?;
			let key = "Profile".to_string();
			dict.get::<String, String>(&key).ok().flatten()
		})
		.collect();

	Ok(PowerProfileState {
		available: true,
		active,
		profiles,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use zbus::zvariant::{Signature, Value};

	fn profile_entry(name: &str) -> Value<'static> {
		let mut dict = Dict::new(&Signature::Str, &Signature::Str);
		dict.add("Profile", name.to_string()).unwrap();
		Value::Dict(dict)
	}

	fn values(active: &str, profiles: &[&str]) -> HashMap<String, OwnedValue> {
		let dict_signature = Signature::dict(Signature::Str, Signature::Str);
		let mut array = Array::new(&dict_signature);
		for name in profiles {
			array.append(profile_entry(name)).unwrap();
		}
		HashMap::from([
			(
				"ActiveProfile".into(),
				OwnedValue::try_from(Value::new(active.to_owned())).unwrap(),
			),
			("Profiles".into(), OwnedValue::try_from(array).unwrap()),
		])
	}

	#[test]
	fn power_profile_snapshot_reads_active_and_available_profiles() {
		let state = snapshot(&values("balanced", &["power-saver", "balanced", "performance"]))
			.unwrap();
		assert!(state.available);
		assert_eq!(state.active, "balanced");
		assert_eq!(
			state.profiles,
			vec!["power-saver", "balanced", "performance"]
		);
	}

	#[test]
	fn power_profile_snapshot_requires_both_properties() {
		assert!(snapshot(&HashMap::new()).is_err());
	}
}
