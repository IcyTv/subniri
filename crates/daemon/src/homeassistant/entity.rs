//! Entity properties, observation ownership, and change notifications.

use std::{
	borrow::Cow,
	collections::HashMap,
	sync::{
		Arc, Weak,
		atomic::{AtomicBool, Ordering},
	},
};

use daemon_common::{
	HOMEASSISTANT_OBJECT_PATH, HomeassistantCapability, HomeassistantEntity,
	HomeassistantEntityInfo,
};
use hass_rs::HassEntity;
use tokio::sync::{Mutex, RwLock};
use zbus::{
	message::Header,
	names::BusName,
	zvariant::{Optional, OwnedObjectPath},
};

use crate::observed::ObservationLimitReached;

use super::{
	HomeassistantInner, HomeassistantResource, HomeassistantService, MAX_ENTITIES_PER_CLIENT,
	MAX_OBSERVED_ENTITIES, MAX_OBSERVERS, snapshot::EntitySnapshot,
};

#[derive(Clone)]
pub(super) struct HomeassistantEntityService {
	pub(super) interfaces: Arc<Mutex<Vec<HomeassistantCapability>>>,
	pub(super) data: Arc<RwLock<EntitySnapshot>>,
	pub(super) observation: Arc<Mutex<()>>,
	pub(super) retiring: Arc<AtomicBool>,
	pub(super) registered: Arc<AtomicBool>,
	notifications_pending: Arc<AtomicBool>,
	service: Weak<HomeassistantInner>,
}

impl HomeassistantEntityService {
	pub(super) fn new(data: EntitySnapshot, service: Weak<HomeassistantInner>) -> Self {
		Self {
			interfaces: Arc::new(Mutex::new(Vec::new())),
			data: Arc::new(RwLock::new(data)),
			observation: Arc::new(Mutex::new(())),
			retiring: Arc::new(AtomicBool::new(false)),
			registered: Arc::new(AtomicBool::new(false)),
			notifications_pending: Arc::new(AtomicBool::new(false)),
			service,
		}
	}

	pub(super) fn service(&self) -> zbus::fdo::Result<HomeassistantService> {
		self.service
			.upgrade()
			.map(|inner| HomeassistantService { inner })
			.ok_or_else(|| {
				zbus::fdo::Error::Failed("Home Assistant service is shutting down".to_string())
			})
	}

	pub(super) async fn update_state(&self, state: HassEntity, notify: bool) -> Result<(), String> {
		let data = self.data.read().await.with_state(state);
		self.update(data, notify).await
	}

	pub(super) async fn update(
		&self, mut data: EntitySnapshot, notify: bool,
	) -> Result<(), String> {
		let entity_id = data.entity_id.clone();
		let changed = {
			let mut current = self.data.write().await;
			// Full catalog refreshes create fresh snapshots, unlike state events.
			// Preserve known light capabilities there too when HA omits them.
			if data.domain == "light"
				&& matches!(data.state.as_str(), "unavailable" | "unknown")
				&& data.attributes.get("supported_color_modes").is_none()
			{
				data.capabilities = current.capabilities.clone();
			}
			if *current == data {
				false
			} else {
				*current = data;
				true
			}
		};
		if changed && notify {
			self.notifications_pending.store(true, Ordering::Release);
		}
		self.sync_interfaces().await?;

		if !notify {
			self.notifications_pending.store(false, Ordering::Release);
			return Ok(());
		}

		if !self.notifications_pending.load(Ordering::Acquire) {
			return Ok(());
		}

		let service = self.service().map_err(|error| error.to_string())?;
		let path = entity_path(&entity_id).map_err(|error| error.to_string())?;
		let emitter = zbus::object_server::SignalEmitter::new(&service.inner.connection, path)
			.map_err(|error| error.to_string())?;

		zbus::fdo::Properties::properties_changed(
			&emitter,
			zbus::names::InterfaceName::from_static_str_checked(
				HomeassistantEntityInfo::INTERFACE_NAME,
			),
			HashMap::new(),
			Cow::Borrowed(HomeassistantEntityInfo::MUTABLE_PROPERTY_NAMES),
		)
		.await
		.map_err(|error| error.to_string())?;
		for capability in self.interfaces.lock().await.iter().copied() {
			let properties: &[&str] = match capability {
				HomeassistantCapability::Callable => continue,
				HomeassistantCapability::Togglable => &["Enabled"],
				HomeassistantCapability::Brightness => &["Brightness"],
				HomeassistantCapability::ColorTemperature => {
					&["ColorTemperature", "MinKelvin", "MaxKelvin"]
				}
				HomeassistantCapability::Color => &["Rgb"],
			};
			zbus::fdo::Properties::properties_changed(
				&emitter,
				zbus::names::InterfaceName::from_static_str_checked(capability.interface_name()),
				HashMap::new(),
				Cow::Borrowed(properties),
			)
			.await
			.map_err(|error| error.to_string())?;
		}

		self.notifications_pending.store(false, Ordering::Release);

		Ok(())
	}
}

impl HomeassistantEntity for HomeassistantEntityService {
	async fn capabilities(&self) -> Vec<HomeassistantCapability> {
		self.data.read().await.capabilities.clone()
	}
	async fn entity_id(&self) -> String {
		self.data.read().await.entity_id.clone()
	}

	async fn domain(&self) -> String {
		self.data.read().await.domain.clone()
	}

	async fn state(&self) -> String {
		self.data.read().await.state.clone()
	}

	async fn available(&self) -> bool {
		self.data.read().await.available
	}

	async fn attributes_json(&self) -> String {
		self.data.read().await.attributes_json.clone()
	}

	async fn friendly_name(&self) -> String {
		self.data.read().await.friendly_name.clone()
	}

	async fn icon(&self) -> String {
		self.data.read().await.icon.clone()
	}

	async fn device_class(&self) -> String {
		self.data.read().await.device_class.clone()
	}

	async fn unit_of_measurement(&self) -> String {
		self.data.read().await.unit_of_measurement.clone()
	}

	async fn last_changed(&self) -> String {
		self.data.read().await.last_changed.clone()
	}

	async fn last_updated(&self) -> String {
		self.data.read().await.last_updated.clone()
	}

	async fn device_id(&self) -> String {
		self.data.read().await.device_id.clone()
	}

	async fn area_id(&self) -> String {
		self.data.read().await.area_id.clone()
	}

	async fn platform(&self) -> String {
		self.data.read().await.platform.clone()
	}

	async fn entity_category(&self) -> Optional<String> {
		self.data.read().await.entity_category.clone()
	}

	async fn observe(&self, header: Header<'_>) -> zbus::fdo::Result<bool> {
		let service = self.service()?;
		let caller = HomeassistantService::caller(&header)?;

		let _observation = self.observation.lock().await;

		if self.retiring.load(Ordering::Acquire) {
			return Err(zbus::fdo::Error::Failed(
				"Home Assistant entity is being removed".to_string(),
			));
		}

		let resource = HomeassistantResource::Entity(self.data.read().await.entity_id.clone());

		let added = service
			.inner
			.registry
			.lock()
			.await
			.observed
			.add(caller, resource.clone())
			.map_err(observation_limit_error)?;

		let dbus = match zbus::fdo::DBusProxy::new(&service.inner.connection).await {
			Ok(dbus) => dbus,
			Err(error) => {
				if added.added {
					service
						.inner
						.registry
						.lock()
						.await
						.observed
						.remove(caller, &resource);
				}

				return Err(zbus::fdo::Error::Failed(error.to_string()));
			}
		};

		let connected = match dbus.name_has_owner(BusName::Unique(caller.clone())).await {
			Ok(connected) => connected,
			Err(error) => {
				if added.added {
					service
						.inner
						.registry
						.lock()
						.await
						.observed
						.remove(caller, &resource);
				}

				return Err(zbus::fdo::Error::Failed(error.to_string()));
			}
		};

		if !connected {
			if added.added {
				service
					.inner
					.registry
					.lock()
					.await
					.observed
					.remove(caller, &resource);
			}

			return Err(zbus::fdo::Error::Failed(
				"D-Bus caller disconnected while observing Home Assistant entity".to_string(),
			));
		}
		Ok(added.added)
	}

	async fn unobserve(&self, header: Header<'_>) -> zbus::fdo::Result<()> {
		let service = self.service()?;
		let caller = HomeassistantService::caller(&header)?;

		let _observation = self.observation.lock().await;

		let resource = HomeassistantResource::Entity(self.data.read().await.entity_id.clone());

		service
			.inner
			.registry
			.lock()
			.await
			.observed
			.remove(caller, &resource);

		Ok(())
	}
}

pub(super) fn entity_path(entity_id: &str) -> zbus::Result<OwnedObjectPath> {
	let Some((domain, object_id)) = entity_id.split_once('.') else {
		return Err(zbus::Error::Failure(format!(
			"Home Assistant entity ID has no domain: {entity_id}"
		)));
	};

	if domain.is_empty()
		|| object_id.is_empty()
		|| entity_id.matches('.').count() != 1
		|| !domain
			.bytes()
			.chain(object_id.bytes())
			.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
	{
		return Err(zbus::Error::Failure(format!(
			"Home Assistant entity ID is not valid for a D-Bus path: {entity_id}"
		)));
	}

	OwnedObjectPath::try_from(
		format!("{HOMEASSISTANT_OBJECT_PATH}/entity/{domain}/{object_id}").as_str(),
	)
	.map_err(zbus::Error::from)
}

fn observation_limit_error(error: ObservationLimitReached) -> zbus::fdo::Error {
	let limit = match error {
		ObservationLimitReached::Clients => format!("{MAX_OBSERVERS} clients"),
		ObservationLimitReached::ObjectsPerClient => {
			format!("{MAX_ENTITIES_PER_CLIENT} entities per client")
		}
		ObservationLimitReached::Objects => format!("{MAX_OBSERVED_ENTITIES} observed entities"),
	};

	zbus::fdo::Error::LimitsExceeded(format!("Home Assistant supports no more than {limit}"))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn uses_entity_domain_and_object_id_as_path_elements() {
		let path = entity_path("light.kitchen_ceiling").ok();
		assert_eq!(
			path.as_ref().map(|path| path.as_str()),
			Some("/de/icytv/subniri/Homeassistant/entity/light/kitchen_ceiling"),
		);
	}

	#[test]
	fn rejects_invalid_entity_ids() {
		for id in [
			"",
			"light",
			".kitchen",
			"light.",
			"light.one.two",
			"light.kitchen-light",
			"light.dévïce",
		] {
			assert!(entity_path(id).is_err(), "accepted invalid entity ID {id}");
		}
	}
}
