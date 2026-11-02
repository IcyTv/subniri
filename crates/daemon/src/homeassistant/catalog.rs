//! Catalog synchronization, event subscriptions, and entity registration.

use std::{
	collections::HashMap,
	sync::{Arc, atomic::Ordering},
	time::Duration,
};

use daemon_common::HomeassistantEntityServer;
use hass_rs::HassClient;
use tokio::{sync::oneshot, task::JoinHandle};

use super::{
	CATALOG_REQUEST_TIMEOUT, HomeassistantResource, HomeassistantService,
	entity::{HomeassistantEntityService, entity_path},
	snapshot::EntitySnapshot,
};

const REGISTRY_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const REGISTRY_EVENT_DEBOUNCE: Duration = Duration::from_millis(250);
const RECONNECT_INITIAL_DELAY: Duration = Duration::from_secs(1);
const RECONNECT_MAX_DELAY: Duration = Duration::from_mins(1);
const FORWARDED_EVENT_CAPACITY: usize = 1_024;

struct AbortTask(JoinHandle<()>);

impl Drop for AbortTask {
	fn drop(&mut self) {
		self.0.abort();
	}
}

impl HomeassistantService {
	pub(super) async fn start_catalog_task(
		&self, client: Arc<HassClient>, generation: u64,
	) -> bool {
		if !self.is_current_client(&client, generation).await {
			return false;
		}

		let mut catalog_task = self.inner.catalog_task.lock().await;
		if !self.is_current_client(&client, generation).await {
			return false;
		}

		if let Some(task) = catalog_task.take() {
			task.abort();
			if let Err(error) = task.await
				&& !error.is_cancelled()
			{
				log::warn!("Home Assistant catalog task failed: {error}");
			}
		}

		self.remove_unregistered_entities().await;

		if !self.is_current_client(&client, generation).await {
			return false;
		}

		let (ready_tx, ready_rx) = oneshot::channel();
		let service = self.clone();

		let task = tokio::spawn(async move {
			service.run_catalog(client, generation, ready_tx).await;
		});

		*catalog_task = Some(task);
		drop(catalog_task);

		matches!(ready_rx.await, Ok(true))
	}

	pub(super) async fn stop_catalog_task(&self) {
		let Some(task) = self.inner.catalog_task.lock().await.take() else {
			return;
		};
		task.abort();

		if let Err(error) = task.await
			&& !error.is_cancelled()
		{
			log::warn!("Home Assistant catalog task failed: {error}");
		}
	}

	async fn run_catalog(
		&self, client: Arc<HassClient>, generation: u64, ready: oneshot::Sender<bool>,
	) {
		let mut client = client;
		let mut ready = Some(ready);
		let mut reconnect_delay = RECONNECT_INITIAL_DELAY;

		loop {
			match self
				.run_catalog_session(client.clone(), generation, &mut ready)
				.await
			{
				Ok(()) => return,
				Err(reason) => {
					log::warn!("Home Assistant catalog stopped: {reason}");

					if !self.mark_reconnecting(&client, generation).await {
						if let Some(ready) = ready.take() {
							let _ = ready.send(false);
						}
						return;
					}

					if let Some(ready) = ready.take() {
						let _ = ready.send(false);
					}
				}
			}

			loop {
				if !self.wait_to_reconnect(generation, reconnect_delay).await {
					return;
				}

				match self.reconnect_client(generation).await {
					Ok(Some(reconnected)) => {
						log::info!("Reconnected to Home Assistant");
						client = reconnected;
						reconnect_delay = RECONNECT_INITIAL_DELAY;
						break;
					}
					Ok(None) => return,
					Err(error) => {
						reconnect_delay = (reconnect_delay * 2).min(RECONNECT_MAX_DELAY);
						log::warn!(
							"Failed to reconnect to Home Assistant; retrying in {}s: {error}",
							reconnect_delay.as_secs()
						);
					}
				}
			}
		}
	}

	async fn run_catalog_session(
		&self, client: Arc<HassClient>, generation: u64, ready: &mut Option<oneshot::Sender<bool>>,
	) -> Result<(), String> {
		let hass_entity_events = match subscribe_event(&client, "entity_registry_updated").await {
			Ok(events) => events,
			Err(error) => return Err(error),
		};

		let (entity_event_tx, mut entity_events) =
			tokio::sync::mpsc::channel(FORWARDED_EVENT_CAPACITY);

		let _entity_forwarder = AbortTask(tokio::spawn(forward_events(
			hass_entity_events,
			entity_event_tx,
		)));

		let hass_state_events = match subscribe_event(&client, "state_changed").await {
			Ok(events) => events,
			Err(error) => return Err(error),
		};

		let (state_event_tx, mut state_events) =
			tokio::sync::mpsc::channel(FORWARDED_EVENT_CAPACITY);

		let _state_forwarder = AbortTask(tokio::spawn(forward_events(
			hass_state_events,
			state_event_tx,
		)));

		if !self.sync_entities(&client, generation).await? {
			return Ok(());
		}

		if !self.mark_catalog_ready(&client, generation).await {
			return Ok(());
		}

		if let Some(ready) = ready.take() {
			let _ = ready.send(true);
		}

		let mut shutdown = self.inner.shutdown_tx.subscribe();
		let mut refresh = tokio::time::interval(REGISTRY_REFRESH_INTERVAL);
		refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
		refresh.tick().await;

		loop {
			tokio::select! {
				changed = shutdown.changed() => {
					if changed.is_err() || *shutdown.borrow() {
						return Ok(());
					}
				}
				_ = refresh.tick() => {
					if !self.sync_entities(&client, generation).await? {
						return Ok(());
					}
				}
				event = entity_events.recv() => {
					if event.is_some() {
						tokio::time::sleep(REGISTRY_EVENT_DEBOUNCE).await;
						if !self.sync_entities(&client, generation).await? {
							return Ok(());
						}
					} else {
						return Err("Home Assistant entity event stream closed".to_string());
					}
				}
				event = state_events.recv() => {
					match event {
						Some(event) => match self.apply_state_event(event, &client, generation).await {
							Ok(true) => {}
							Ok(false) => return Ok(()),
							Err(error) => log::warn!("Failed to apply Home Assistant state update: {error}"),
						},
						None => {
							return Err("Home Assistant state event stream closed".to_string());
						},
					}
				}
			}
		}
	}

	async fn sync_entities(
		&self, client: &Arc<HassClient>, generation: u64,
	) -> Result<bool, String> {
		let _reconcile = self.inner.reconcile.lock().await;
		if !self.is_current_client(client, generation).await {
			return Ok(false);
		}

		let (states, registry_entities) = tokio::time::timeout(CATALOG_REQUEST_TIMEOUT, async {
			tokio::try_join!(client.get_states(), client.get_entity_registry_list())
		})
		.await
		.map_err(|_| "Home Assistant catalog request timed out".to_string())?
		.map_err(|error| error.to_string())?;

		if !self.is_current_client(client, generation).await {
			return Ok(false);
		}

		let registry_entities = registry_entities
			.into_iter()
			.map(|entity| (entity.entity_id.clone(), entity))
			.collect::<HashMap<_, _>>();

		let snapshots = states
			.into_iter()
			.map(|state| {
				let registry = registry_entities.get(&state.entity_id);
				let snapshot = EntitySnapshot::new(state, registry);
				(snapshot.entity_id.clone(), snapshot)
			})
			.collect::<HashMap<_, _>>();

		let (updates, removals) = {
			let registry = self.inner.registry.lock().await;
			let updates = snapshots
				.iter()
				.filter_map(|(entity_id, snapshot)| {
					registry
						.entities
						.get(entity_id)
						.cloned()
						.map(|entity| (entity, snapshot.clone()))
				})
				.collect::<Vec<_>>();

			let removals = registry
				.entities
				.iter()
				.filter(|(entity_id, _)| !snapshots.contains_key(entity_id.as_str()))
				.map(|(entity_id, entity)| (entity_id.clone(), entity.clone()))
				.collect::<Vec<_>>();

			(updates, removals)
		};

		for (entity, snapshot) in updates {
			let _observation = entity.observation.lock().await;
			let notify = self.should_notify_entity(&snapshot.entity_id).await;
			entity.update(snapshot, notify).await?;
		}

		for (entity_id, entity) in removals {
			self.remove_entity(&entity_id, &entity).await;
		}

		for (entity_id, snapshot) in snapshots {
			if self
				.inner
				.registry
				.lock()
				.await
				.entities
				.contains_key(&entity_id)
			{
				continue;
			}

			let path = match entity_path(&entity_id) {
				Ok(path) => path,
				Err(error) => {
					log::warn!("Skipping invalid Home Assistant entity ID {entity_id}: {error}");
					continue;
				}
			};

			let entity = HomeassistantEntityService::new(snapshot, Arc::downgrade(&self.inner));
			self.inner
				.registry
				.lock()
				.await
				.entities
				.insert(entity_id.clone(), entity.clone());

			let added = self
				.inner
				.connection
				.object_server()
				.at(path, HomeassistantEntityServer(entity.clone()))
				.await;

			let added = match added {
				Ok(added) => added,
				Err(error) => {
					self.remove_entity(&entity_id, &entity).await;
					return Err(error.to_string());
				}
			};

			if !added {
				self.remove_entity(&entity_id, &entity).await;
				return Err(format!(
					"Home Assistant entity object {entity_id} was already registered"
				));
			}

			entity.registered.store(true, Ordering::Release);
			entity.sync_interfaces().await?;
		}

		let tracked = self.inner.tracked.read().await;
		for resource in tracked.iter() {
			let HomeassistantResource::Entity(entity_id) = resource;
			if !self
				.inner
				.registry
				.lock()
				.await
				.entities
				.contains_key(entity_id)
			{
				log::debug!(
					"Configured Home Assistant entity {entity_id} is not present in the registry"
				);
			}
		}

		Ok(true)
	}

	async fn apply_state_event(
		&self, event: hass_rs::WSEvent, client: &Arc<HassClient>, generation: u64,
	) -> Result<bool, String> {
		let _reconcile = self.inner.reconcile.lock().await;

		if !self.is_current_client(client, generation).await {
			return Ok(false);
		}

		let entity_id = event.event.data.entity_id.clone().or_else(|| {
			event
				.event
				.data
				.new_state
				.as_ref()
				.map(|state| state.entity_id.clone())
		});

		let Some(entity_id) = entity_id else {
			return Ok(true);
		};

		let entity = self
			.inner
			.registry
			.lock()
			.await
			.entities
			.get(&entity_id)
			.cloned();

		match (entity, event.event.data.new_state) {
			(Some(entity), Some(state)) => {
				let _observation = entity.observation.lock().await;
				let notify = self.should_notify_entity(&entity_id).await;
				entity.update_state(state, notify).await?;
			}
			(Some(entity), None) => self.remove_entity(&entity_id, &entity).await,
			(None, Some(_)) => {
				drop(_reconcile);
				return self.sync_entities(client, generation).await;
			}
			(None, None) => {}
		}

		Ok(true)
	}

	async fn should_notify_entity(&self, entity_id: &str) -> bool {
		let resource = HomeassistantResource::Entity(entity_id.to_string());

		if self.inner.tracked.read().await.contains(&resource) {
			return true;
		}

		self.inner
			.registry
			.lock()
			.await
			.observed
			.is_observed(&resource)
	}

	async fn remove_entity(&self, entity_id: &str, entity: &HomeassistantEntityService) {
		let _observation = entity.observation.lock().await;
		entity.retiring.store(true, Ordering::Release);

		let path = match entity_path(entity_id) {
			Ok(path) => path,
			Err(error) => {
				log::warn!("Invalid Home Assistant entity ID {entity_id}: {error}");
				return;
			}
		};
		if let Err(error) = entity.remove_interfaces().await {
			log::warn!("Failed to remove Home Assistant capabilities for {entity_id}: {error}");
		}

		if let Err(error) = self
			.inner
			.connection
			.object_server()
			.remove::<HomeassistantEntityServer<HomeassistantEntityService>, _>(path)
			.await && !matches!(error, zbus::Error::InterfaceNotFound)
		{
			log::warn!("Failed to remove Home Assistant entity {entity_id}: {error}");
		}

		let mut registry = self.inner.registry.lock().await;
		if registry
			.entities
			.get(entity_id)
			.is_some_and(|current| Arc::ptr_eq(&current.retiring, &entity.retiring))
		{
			registry.entities.remove(entity_id);
			registry
				.observed
				.remove_key(&HomeassistantResource::Entity(entity_id.to_string()));
		}
	}

	pub(super) async fn clear_entities(&self) {
		let _reconcile = self.inner.reconcile.lock().await;

		let entities = self
			.inner
			.registry
			.lock()
			.await
			.entities
			.iter()
			.map(|(entity_id, entity)| (entity_id.clone(), entity.clone()))
			.collect::<Vec<_>>();

		for (entity_id, entity) in entities {
			self.remove_entity(&entity_id, &entity).await;
		}
	}

	async fn remove_unregistered_entities(&self) {
		let _reconcile = self.inner.reconcile.lock().await;

		let entities = self
			.inner
			.registry
			.lock()
			.await
			.entities
			.iter()
			.filter(|(_, entity)| !entity.registered.load(Ordering::Acquire))
			.map(|(entity_id, entity)| (entity_id.clone(), entity.clone()))
			.collect::<Vec<_>>();

		for (entity_id, entity) in entities {
			self.remove_entity(&entity_id, &entity).await;
		}
	}
}

async fn subscribe_event(
	client: &HassClient, event: &str,
) -> Result<tokio::sync::mpsc::Receiver<hass_rs::WSEvent>, String> {
	tokio::time::timeout(CATALOG_REQUEST_TIMEOUT, client.subscribe_event(event))
		.await
		.map_err(|_| format!("Home Assistant {event} subscription timed out"))?
		.map_err(|error| error.to_string())
}

async fn forward_events(
	mut receiver: tokio::sync::mpsc::Receiver<hass_rs::WSEvent>,
	sender: tokio::sync::mpsc::Sender<hass_rs::WSEvent>,
) {
	while let Some(event) = receiver.recv().await {
		match sender.try_send(event) {
			Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {}
			Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return,
		}
	}
}
