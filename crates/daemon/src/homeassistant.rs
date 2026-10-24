use std::{
	borrow::Cow,
	collections::{HashMap, HashSet},
	sync::{
		Arc, Weak,
		atomic::{AtomicBool, Ordering},
	},
	time::Duration,
};

use daemon_common::{
	HOMEASSISTANT_OBJECT_PATH, HOMEASSISTANT_REDIRECT_URI, Homeassistant, HomeassistantEntity,
	HomeassistantEntityInfo, HomeassistantEntityServer, HomeassistantServer, HomeassistantStatus,
};
use futures::{SinkExt, StreamExt};
use hass_rs::{HassClient, HassEntity, HassError, HassRegistryEntity};
use rand::distr::{Alphanumeric, SampleString};
use secret_store::{SecretAttributes, SecretStore};
use serde::{Deserialize, Serialize};
use tokio::{
	sync::{Mutex, RwLock, oneshot, watch},
	task::JoinHandle,
};
use tokio_tungstenite::tungstenite::Message;
use zbus::{
	message::Header,
	names::{BusName, OwnedUniqueName, UniqueName},
	zvariant::{Optional, OwnedObjectPath},
};

use crate::observed::{ObservationLimitReached, Observed, watch_disconnections};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

const OAUTH_CLIENT_ID: &str = "http://127.0.0.1:8765";
const LONG_LIVED_TOKEN_LIFESPAN_DAYS: u64 = 3650;
const MAX_OBSERVERS: usize = 10;
const MAX_ENTITIES_PER_CLIENT: usize = 100;
const MAX_OBSERVED_ENTITIES: usize = 1_000;
const REGISTRY_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const REGISTRY_EVENT_DEBOUNCE: Duration = Duration::from_millis(250);
const CATALOG_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const RECONNECT_INITIAL_DELAY: Duration = Duration::from_secs(1);
const RECONNECT_MAX_DELAY: Duration = Duration::from_mins(1);
const FORWARDED_EVENT_CAPACITY: usize = 1_024;

#[derive(Clone)]
pub struct HomeassistantService {
	inner: Arc<HomeassistantInner>,
}

struct HomeassistantInner {
	connection: zbus::Connection,
	state: RwLock<HomeassistantState>,
	registry: Mutex<EntityRegistry>,
	reconcile: Mutex<()>,
	tracked: RwLock<HashSet<HomeassistantResource>>,
	shutdown_tx: watch::Sender<bool>,
	tasks: Mutex<Vec<JoinHandle<()>>>,
	catalog_task: Mutex<Option<JoinHandle<()>>>,
}

struct HomeassistantState {
	store: Option<Arc<SecretStore>>,
	client: Option<Arc<HassClient>>,
	url: Option<url::Url>,
	status: HomeassistantStatus,
	authenticated: bool,
	generation: u64,
	connection_config: Option<(bool, Option<url::Url>)>,
	csrf_tokens: HashMap<String, AuthorizationSession>,
}

#[derive(Clone)]
struct AuthorizationSession {
	token: String,
	url: url::Url,
	generation: u64,
	in_progress: bool,
}

struct AuthorizationGuard {
	service: Weak<HomeassistantInner>,
	caller: String,
	generation: u64,
	armed: bool,
}

struct AbortTask(JoinHandle<()>);

impl Drop for AbortTask {
	fn drop(&mut self) {
		self.0.abort();
	}
}

impl AuthorizationGuard {
	fn disarm(&mut self) {
		self.armed = false;
	}
}

impl Drop for AuthorizationGuard {
	fn drop(&mut self) {
		if !self.armed {
			return;
		}
		let Some(inner) = self.service.upgrade() else {
			return;
		};
		let caller = self.caller.clone();
		let generation = self.generation;
		tokio::spawn(async move {
			HomeassistantService { inner }
				.cancel_authorization_for(&caller, Some(generation))
				.await;
		});
	}
}

struct EntityRegistry {
	observed: Observed<HomeassistantResource>,
	entities: HashMap<String, HomeassistantEntityService>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum HomeassistantResource {
	Entity(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EntitySnapshot {
	info: HomeassistantEntityInfo,
	registry_name: String,
	registry_icon: String,
}

impl std::ops::Deref for EntitySnapshot {
	type Target = HomeassistantEntityInfo;

	fn deref(&self) -> &Self::Target {
		&self.info
	}
}

impl std::ops::DerefMut for EntitySnapshot {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.info
	}
}

impl HomeassistantService {
	pub fn new(connection: zbus::Connection) -> Self {
		let (shutdown_tx, _) = watch::channel(false);
		Self {
			inner: Arc::new(HomeassistantInner {
				connection,
				state: RwLock::new(HomeassistantState {
					store: None,
					client: None,
					url: None,
					status: HomeassistantStatus::NoUrl,
					authenticated: false,
					generation: 0,
					connection_config: None,
					csrf_tokens: HashMap::new(),
				}),
				registry: Mutex::new(EntityRegistry {
					observed: Observed::with_limits(
						MAX_OBSERVERS,
						MAX_ENTITIES_PER_CLIENT,
						MAX_OBSERVED_ENTITIES,
					),
					entities: HashMap::new(),
				}),
				reconcile: Mutex::new(()),
				tracked: RwLock::new(HashSet::new()),
				shutdown_tx,
				tasks: Mutex::new(Vec::new()),
				catalog_task: Mutex::new(None),
			}),
		}
	}

	pub async fn register(
		connection: zbus::Connection, config: config::Homeassistant,
	) -> Result<Self, Error> {
		let service = Self::new(connection.clone());
		service.start_tasks().await?;
		let added = match connection
			.object_server()
			.at(
				HOMEASSISTANT_OBJECT_PATH,
				HomeassistantServer(service.clone()),
			)
			.await
		{
			Ok(added) => added,
			Err(error) => {
				service.stop_tasks().await;
				return Err(error.into());
			}
		};
		if !added {
			service.stop_tasks().await;
			return Err("Homeassistant service is already registered".into());
		}
		let manager_added = connection
			.object_server()
			.at(HOMEASSISTANT_OBJECT_PATH, zbus::fdo::ObjectManager)
			.await;
		if !matches!(manager_added, Ok(true)) {
			let _ = connection
				.object_server()
				.remove::<HomeassistantServer<HomeassistantService>, _>(HOMEASSISTANT_OBJECT_PATH)
				.await;
			service.stop_tasks().await;
			return match manager_added {
				Ok(false) => Err("Homeassistant ObjectManager is already registered".into()),
				Err(error) => Err(error.into()),
				Ok(true) => unreachable!(),
			};
		}

		service.reconfigure(config).await;
		Ok(service)
	}

	async fn start_tasks(&self) -> Result<(), Error> {
		let watcher = self.clone();
		let connection = self.inner.connection.clone();
		let shutdown = self.inner.shutdown_tx.subscribe();
		let (ready_tx, ready_rx) = oneshot::channel();
		let task = tokio::spawn(async move {
			if let Err(error) = watch_disconnections(&connection, shutdown, ready_tx, |client| {
				let watcher = watcher.clone();
				async move { watcher.release_client(&client).await }
			})
			.await
			{
				log::error!("Failed to watch D-Bus clients for Home Assistant: {error}");
			}
		});
		self.inner.tasks.lock().await.push(task);
		ready_rx.await.map_err(|_| {
			"Home Assistant D-Bus disconnect watcher stopped during initialization".into()
		})
	}

	async fn stop_tasks(&self) {
		let _ = self.inner.shutdown_tx.send(true);
		self.stop_catalog_task().await;
		for task in std::mem::take(&mut *self.inner.tasks.lock().await) {
			task.abort();
			if let Err(error) = task.await
				&& !error.is_cancelled()
			{
				log::warn!("Home Assistant background task failed: {error}");
			}
		}
	}

	pub async fn reconfigure(&self, config: config::Homeassistant) {
		*self.inner.tracked.write().await = config
			.tracked_devices
			.iter()
			.cloned()
			.map(HomeassistantResource::Entity)
			.collect();
		let connection_config = (config.enabled, config.url.clone());
		let generation = {
			let mut state = self.inner.state.write().await;
			if state.connection_config.as_ref() == Some(&connection_config) {
				return;
			}
			state.connection_config = Some(connection_config);
			state.generation = state.generation.wrapping_add(1);
			state.client = None;
			state.url = config.url.clone();
			state.authenticated = false;
			state.csrf_tokens.clear();
			state.status = if config.enabled {
				HomeassistantStatus::Connecting
			} else {
				HomeassistantStatus::Disabled
			};
			state.generation
		};
		self.stop_catalog_task().await;
		self.clear_entities().await;

		if !config.enabled {
			return;
		}

		let store = match self.secret_store().await {
			Ok(store) => store,
			Err(error) => {
				log::error!("Failed to initialize secret store: {error}");
				self.set_connection_state(generation, None, HomeassistantStatus::Error, false)
					.await;
				return;
			}
		};

		let Some(url) = config.url else {
			self.set_connection_state(generation, None, HomeassistantStatus::NoUrl, false)
				.await;
			return;
		};

		if let Err(error) = check_url(&url).await {
			log::error!("Invalid Home Assistant URL: {error}");
			self.set_connection_state(generation, None, HomeassistantStatus::InvalidUrl, false)
				.await;
			return;
		}

		let Some(websocket_url) = websocket_url(&url) else {
			self.set_connection_state(generation, None, HomeassistantStatus::InvalidUrl, false)
				.await;
			return;
		};

		let client = match HassClient::new(&websocket_url).await {
			Ok(client) => Arc::new(client),
			Err(error) => {
				log::error!("Failed to create Home Assistant client: {error}");
				self.set_connection_state(generation, None, HomeassistantStatus::Error, false)
					.await;
				return;
			}
		};

		// if client.ping().await.is_err() {
		// 	log::error!("Failed to ping Home Assistant at {url}");
		// 	self.set_connection_state(generation, None, HomeassistantStatus::InvalidUrl, false)
		// 		.await;
		// 	return;
		// }

		let attributes = homeassistant_attributes();
		let token = match store
			.get(&attributes.clone().insert("type", "long_lived_token"))
			.await
		{
			Ok(token) => token,
			Err(error) => {
				log::error!("Failed to retrieve Home Assistant token: {error}");
				self.set_connection_state(generation, None, HomeassistantStatus::Error, false)
					.await;
				return;
			}
		};

		let token = match token {
			Some(token) => token,
			None => {
				log::info!("No Home Assistant long-lived token found");
				self.set_connection_state(
					generation,
					Some(client),
					HomeassistantStatus::NoLongLivedToken,
					false,
				)
				.await;
				return;
			}
		};

		// TODO: Check if an invalid token will fail this...
		if let Err(e) = client.auth_with_longlivedtoken(&token).await {
			// log::error!("Failed to authenticate with Home Assistant: {e}");
			// self.set_connection_state(generation, Some(client), HomeassistantStatus::Error, false)
			// 	.await;

			match e {
				HassError::AuthenticationFailed(msg)
					if msg == "Invalid access token or password" =>
				{
					let _ = store
						.delete(&attributes.insert("type", "long_lived_token"))
						.await
						.ok();

					self.set_connection_state(
						generation,
						Some(client),
						HomeassistantStatus::NoLongLivedToken,
						false,
					)
					.await
				}
				_ => {
					self.set_connection_state(
						generation,
						Some(client),
						HomeassistantStatus::Error,
						false,
					)
					.await
				}
			}

			return;
		}

		self.set_connection_state(
			generation,
			Some(client),
			HomeassistantStatus::Connecting,
			true,
		)
		.await;
		if let Some(client) = self.client_for_generation(generation).await {
			if self.start_catalog_task(client.clone(), generation).await {
				self.mark_catalog_ready(&client, generation).await;
			}
		}
	}

	async fn set_connection_state(
		&self, generation: u64, client: Option<Arc<HassClient>>, status: HomeassistantStatus,
		authenticated: bool,
	) {
		let mut state = self.inner.state.write().await;
		if state.generation != generation {
			return;
		}
		state.client = client;
		state.status = status;
		state.authenticated = authenticated;
	}

	async fn secret_store(&self) -> Result<Arc<SecretStore>, secret_store::Error> {
		if let Some(store) = self.inner.state.read().await.store.clone() {
			return Ok(store);
		}

		let store = Arc::new(SecretStore::from_connection(self.inner.connection.clone()).await?);
		self.inner.state.write().await.store = Some(store.clone());
		Ok(store)
	}

	async fn client_for_generation(&self, generation: u64) -> Option<Arc<HassClient>> {
		let state = self.inner.state.read().await;
		(state.generation == generation && state.authenticated)
			.then(|| state.client.clone())
			.flatten()
	}

	async fn start_catalog_task(&self, client: Arc<HassClient>, generation: u64) -> bool {
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

	async fn is_current_client(&self, client: &Arc<HassClient>, generation: u64) -> bool {
		let state = self.inner.state.read().await;
		state.generation == generation
			&& state.authenticated
			&& state
				.client
				.as_ref()
				.is_some_and(|current| Arc::ptr_eq(current, client))
	}

	async fn mark_catalog_ready(&self, client: &Arc<HassClient>, generation: u64) -> bool {
		let mut state = self.inner.state.write().await;
		if state.generation != generation
			|| !state.authenticated
			|| !state
				.client
				.as_ref()
				.is_some_and(|current| Arc::ptr_eq(current, client))
		{
			return false;
		}
		state.status = HomeassistantStatus::Ready;
		true
	}

	async fn stop_catalog_task(&self) {
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

	async fn mark_reconnecting(&self, client: &Arc<HassClient>, generation: u64) -> bool {
		let mut state = self.inner.state.write().await;
		if state.generation != generation
			|| !state
				.client
				.as_ref()
				.is_some_and(|current| Arc::ptr_eq(current, client))
		{
			return false;
		}
		state.client = None;
		state.authenticated = false;
		state.status = HomeassistantStatus::Connecting;
		true
	}

	async fn wait_to_reconnect(&self, generation: u64, delay: Duration) -> bool {
		if !self.is_generation_enabled(generation).await {
			return false;
		}
		let mut shutdown = self.inner.shutdown_tx.subscribe();
		tokio::select! {
			() = tokio::time::sleep(delay) => self.is_generation_enabled(generation).await,
			changed = shutdown.changed() => changed.is_ok() && !*shutdown.borrow() && self.is_generation_enabled(generation).await,
		}
	}

	async fn is_generation_enabled(&self, generation: u64) -> bool {
		let state = self.inner.state.read().await;
		state.generation == generation
			&& state
				.connection_config
				.as_ref()
				.is_some_and(|(enabled, _)| *enabled)
	}

	async fn reconnect_client(&self, generation: u64) -> Result<Option<Arc<HassClient>>, String> {
		let url = {
			let state = self.inner.state.read().await;
			if state.generation != generation
				|| !state
					.connection_config
					.as_ref()
					.is_some_and(|(enabled, _)| *enabled)
			{
				return Ok(None);
			}
			state.url.clone()
		};
		let Some(url) = url else {
			return Ok(None);
		};
		let websocket_url = websocket_url(&url)
			.ok_or_else(|| "Failed to construct Home Assistant websocket URL".to_string())?;
		let store = self
			.secret_store()
			.await
			.map_err(|error| error.to_string())?;
		let token = store
			.get(&homeassistant_attributes().insert("type", "long_lived_token"))
			.await
			.map_err(|error| error.to_string())?
			.ok_or_else(|| "Home Assistant long-lived token is missing".to_string())?;
		let client = tokio::time::timeout(CATALOG_REQUEST_TIMEOUT, async {
			let client = HassClient::new(&websocket_url).await?;
			client.auth_with_longlivedtoken(&token).await?;
			Ok::<_, HassError>(client)
		})
		.await
		.map_err(|_| "Home Assistant connection attempt timed out".to_string())?
		.map_err(|error| error.to_string())?;
		let client = Arc::new(client);
		let mut state = self.inner.state.write().await;
		if state.generation != generation
			|| !state
				.connection_config
				.as_ref()
				.is_some_and(|(enabled, _)| *enabled)
		{
			return Ok(None);
		}
		state.client = Some(client.clone());
		state.authenticated = true;
		state.status = HomeassistantStatus::Connecting;
		Ok(Some(client))
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

	async fn clear_entities(&self) {
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

	async fn release_client(&self, client: &OwnedUniqueName) {
		self.inner.registry.lock().await.observed.release(client);
		self.cancel_authorization_for(client.as_str(), None).await;
	}

	async fn cancel_authorization_for(&self, caller: &str, generation: Option<u64>) {
		let mut state = self.inner.state.write().await;
		let removed = state.csrf_tokens.get(caller).is_some_and(|session| {
			generation.is_none_or(|expected| session.generation == expected)
		});
		if !removed {
			return;
		}
		state.csrf_tokens.remove(caller);
		if state.status == HomeassistantStatus::Authorizing {
			state.status = if state.authenticated {
				HomeassistantStatus::Ready
			} else {
				HomeassistantStatus::NoLongLivedToken
			};
		}
	}

	pub async fn unregister(&self) -> Result<(), zbus::Error> {
		self.stop_tasks().await;
		self.clear_entities().await;
		let _ = self
			.inner
			.connection
			.object_server()
			.remove::<zbus::fdo::ObjectManager, _>(HOMEASSISTANT_OBJECT_PATH)
			.await;
		self.inner
			.connection
			.object_server()
			.remove::<HomeassistantServer<HomeassistantService>, _>(HOMEASSISTANT_OBJECT_PATH)
			.await
			.map(|_| ())
	}

	fn caller<'a>(header: &'a Header<'_>) -> zbus::fdo::Result<&'a UniqueName<'a>> {
		header
			.sender()
			.ok_or_else(|| zbus::fdo::Error::Failed("D-Bus caller has no unique name".to_string()))
	}
}

impl Homeassistant for HomeassistantService {
	async fn status(&self) -> HomeassistantStatus {
		self.inner.state.read().await.status
	}

	async fn authenticated(&self) -> bool {
		self.inner.state.read().await.authenticated
	}

	async fn begin_authorization(&self, header: Header<'_>) -> zbus::fdo::Result<String> {
		let caller = Self::caller(&header)?;
		let csrf_token = Alphanumeric.sample_string(&mut rand::rng(), 16);
		let url = {
			let mut state = self.inner.state.write().await;
			let Some(url) = state.url.clone() else {
				return Err(zbus::fdo::Error::Failed(
					"Home Assistant URL is not configured".to_string(),
				));
			};
			let generation = state.generation;
			state.csrf_tokens.insert(
				caller.to_string(),
				AuthorizationSession {
					token: csrf_token.clone(),
					url: url.clone(),
					generation,
					in_progress: false,
				},
			);
			state.status = HomeassistantStatus::Authorizing;
			url
		};

		let auth_url = match authorization_url(&url, &csrf_token) {
			Ok(url) => url,
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to construct authorization URL: {e}"
				)));
			}
		};

		Ok(auth_url)
	}

	async fn cancel_authorization(&self, header: Header<'_>) -> zbus::fdo::Result<()> {
		let caller = Self::caller(&header)?;
		self.cancel_authorization_for(caller.as_str(), None).await;
		Ok(())
	}

	async fn complete_authorization(
		&self, code: &str, csrf_token: &str, header: Header<'_>,
	) -> zbus::fdo::Result<bool> {
		let caller = Self::caller(&header)?;
		let caller = caller.to_string();
		let (url, generation) = {
			let mut state = self.inner.state.write().await;
			let generation = state.generation;
			let configured_url = state.url.clone();
			let Some(session) = state.csrf_tokens.get_mut(caller.as_str()) else {
				return Err(zbus::fdo::Error::Failed(
					"CSRF token not found for caller".to_string(),
				));
			};
			if session.generation != generation || configured_url.as_ref() != Some(&session.url) {
				return Err(zbus::fdo::Error::Failed(
					"Home Assistant configuration changed during authorization".to_string(),
				));
			}
			if session.token != csrf_token {
				return Err(zbus::fdo::Error::Failed(
					"CSRF token does not match".to_string(),
				));
			}
			if session.in_progress {
				return Err(zbus::fdo::Error::Failed(
					"Home Assistant authorization is already being completed".to_string(),
				));
			}
			session.in_progress = true;
			(session.url.clone(), session.generation)
		};
		let mut authorization_guard = AuthorizationGuard {
			service: Arc::downgrade(&self.inner),
			caller: caller.clone(),
			generation,
			armed: true,
		};

		let client = reqwest::Client::new();
		let params = [
			("grant_type", "authorization_code"),
			("code", code),
			("client_id", OAUTH_CLIENT_ID),
		];

		let token_url = url
			.join("/auth/token")
			.map_err(|e| zbus::fdo::Error::Failed(format!("Failed to construct token URL: {e}")))?;

		let resp = match client.post(token_url).form(&params).send().await {
			Ok(resp) if !resp.status().is_success() => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to exchange code for token: {}",
					resp.status()
				)));
			}
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to exchange code for token: {e}"
				)));
			}
			Ok(resp) => resp,
		};

		let res = match resp.json::<TokenResponse>().await {
			Ok(res) => res,
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to parse token response: {e}"
				)));
			}
		};

		let Some(websocket_url) = websocket_url(&url) else {
			return Err(zbus::fdo::Error::Failed(
				"Failed to construct websocket URL".to_string(),
			));
		};

		// TODO: Move this to hass-rs...
		let (mut stream, _) = match tokio_tungstenite::connect_async(&websocket_url).await {
			Ok(stream) => stream,
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to connect to Home Assistant websocket: {e}"
				)));
			}
		};

		let msg = match next_websocket_json(&mut stream).await {
			Ok(msg) => msg,
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to receive message from Home Assistant websocket: {e}"
				)));
			}
		};

		match msg.get("type").and_then(|t| t.as_str()) {
			Some("auth_required") => (),
			Some("auth_invalid") => {
				let msg = msg
					.get("message")
					.and_then(|m| m.as_str())
					.unwrap_or("Unknown error");
				return Err(zbus::fdo::Error::Failed(format!(
					"Home Assistant websocket authentication failed: {msg}"
				)));
			}
			_ => {
				return Err(zbus::fdo::Error::Failed(
					"Unexpected message from Home Assistant websocket".to_string(),
				));
			}
		}

		let auth_msg = serde_json::json!({
			"type": "auth",
			"access_token": res.access_token,
		});
		let auth_msg = serde_json::to_string(&auth_msg).map_err(|e| {
			zbus::fdo::Error::Failed(format!("Failed to serialize auth message: {e}"))
		})?;

		if let Err(e) = stream.send(Message::Text(auth_msg.into())).await {
			return Err(zbus::fdo::Error::Failed(format!(
				"Failed to send auth message to Home Assistant websocket: {e}"
			)));
		}

		let msg = match next_websocket_json(&mut stream).await {
			Ok(msg) => msg,
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to receive message from Home Assistant websocket: {e}"
				)));
			}
		};

		match msg.get("type").and_then(|t| t.as_str()) {
			Some("auth_ok") => (),
			Some("auth_invalid") => {
				let msg = msg
					.get("message")
					.and_then(|m| m.as_str())
					.unwrap_or("Unknown error");
				return Err(zbus::fdo::Error::Failed(format!(
					"Home Assistant websocket authentication failed: {msg}"
				)));
			}
			_ => {
				return Err(zbus::fdo::Error::Failed(
					"Unexpected message from Home Assistant websocket".to_string(),
				));
			}
		}

		let token_req = serde_json::json!({
			"id": 1,
			"type": "auth/long_lived_access_token",
			"client_name": "Subniri Shell",
			"lifespan": LONG_LIVED_TOKEN_LIFESPAN_DAYS,
		});
		let token_req = serde_json::to_string(&token_req).map_err(|e| {
			zbus::fdo::Error::Failed(format!("Failed to serialize token request: {e}"))
		})?;

		if let Err(e) = stream.send(Message::Text(token_req.into())).await {
			return Err(zbus::fdo::Error::Failed(format!(
				"Failed to send auth message to Home Assistant websocket: {e}"
			)));
		}

		let long_lived_token = loop {
			let msg = match next_websocket_json(&mut stream).await {
				Ok(msg) => msg,
				Err(e) => {
					return Err(zbus::fdo::Error::Failed(format!(
						"Failed to receive message from Home Assistant websocket: {e}"
					)));
				}
			};

			if msg.get("id").and_then(|id| id.as_u64()) != Some(1) {
				continue;
			}

			if msg
				.get("type")
				.and_then(|t| t.as_str())
				.map_or(true, |t| t != "result")
			{
				return Err(zbus::fdo::Error::Failed(
					"Unexpected message from Home Assistant websocket".to_string(),
				));
			}

			if msg.get("success").and_then(|s| s.as_bool()) != Some(true) {
				let error = msg
					.get("error")
					.and_then(|e| e.get("message"))
					.and_then(|m| m.as_str())
					.unwrap_or("Unknown error");
				log::warn!("Failed to create long-lived token: {msg}");
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to create long-lived token: {error}"
				)));
			}

			let Some(token) = msg
				.get("result")
				.and_then(|t| t.as_str())
				.map(|t| t.to_string())
			else {
				return Err(zbus::fdo::Error::Failed(
					"Failed to get long-lived token from response".to_string(),
				));
			};

			break token;
		};

		// let revoke_url = url
		// 	.join("/auth/revoke")
		// 	.map_err(|e| zbus::fdo::Error::Failed(format!("Failed to construct revoke URL: {e}")))?;

		'revoke: {
			// Try to revoke refresh token, if it fails, continue gracefully
			let revoke_url = match url.join("/auth/revoke") {
				Ok(u) => u,
				Err(e) => {
					log::warn!("Failed to construct revoke URL: {e}");
					break 'revoke;
				}
			};
			let params = [("token", res.refresh_token.as_str())];

			if let Err(e) = client.post(revoke_url).form(&params).send().await {
				log::warn!("Failed to revoke refresh token: {e}");
			}
		}

		drop(stream);

		let client = match HassClient::new(&websocket_url).await {
			Ok(client) => Arc::new(client),
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to create Home Assistant client: {e}"
				)));
			}
		};
		if let Err(e) = client.auth_with_longlivedtoken(&long_lived_token).await {
			return Err(zbus::fdo::Error::Failed(format!(
				"Failed to authenticate with Home Assistant: {e}"
			)));
		}

		let store = match self.secret_store().await {
			Ok(store) => Some(store),
			Err(error) => {
				log::warn!("Failed to access secret store: {error}");
				None
			}
		};
		// Prevent reconfiguration between validating this authorization and persisting its token.
		let mut state = self.inner.state.write().await;
		if state.generation != generation || state.url.as_ref() != Some(&url) {
			return Err(zbus::fdo::Error::Failed(
				"Home Assistant configuration changed during authorization".to_string(),
			));
		}
		let session_active = state
			.csrf_tokens
			.remove(caller.as_str())
			.is_some_and(|session| {
				session.generation == generation
					&& session.url == url
					&& session.token == csrf_token
					&& session.in_progress
			});
		if !session_active {
			return Err(zbus::fdo::Error::Failed(
				"Home Assistant authorization was cancelled".to_string(),
			));
		}
		if let Some(store) = store {
			let attributes = homeassistant_attributes().insert("type", "long_lived_token");
			if let Err(error) = store
				.store(
					"Homeassistant Long Lived Token",
					&attributes,
					&long_lived_token,
				)
				.await
			{
				log::warn!("Failed to store Home Assistant long-lived token: {error}");
			}
		}
		state.client = Some(client.clone());
		state.status = HomeassistantStatus::Connecting;
		state.authenticated = true;
		drop(state);
		if !self.start_catalog_task(client.clone(), generation).await {
			return Err(zbus::fdo::Error::Failed(
				"Failed to initialize Home Assistant entity catalog".to_string(),
			));
		}
		if !self.mark_catalog_ready(&client, generation).await {
			return Err(zbus::fdo::Error::Failed(
				"Home Assistant entity catalog stopped during initialization".to_string(),
			));
		}
		authorization_guard.disarm();

		Ok(true)
	}
}

fn homeassistant_attributes() -> SecretAttributes {
	SecretAttributes::new()
		.insert("application", "subniri")
		.insert("service", "homeassistant")
}

fn entity_path(entity_id: &str) -> zbus::Result<OwnedObjectPath> {
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

async fn check_url(url: &url::Url) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
	if url.scheme() != "http" && url.scheme() != "https" {
		return Err(format!("Invalid URL scheme: {}", url.scheme()).into());
	}
	if url.host_str().is_none() {
		return Err("URL must have a host".into());
	}
	if !url.path().starts_with('/') {
		return Err("URL path must start with '/'".into());
	}

	let mut url = url.clone();
	url.set_path(format!("{}/manifest.json", url.path()).as_str());

	let response = reqwest::get(url.as_str()).await?;
	let data = response.json::<serde_json::Value>().await?;

	match data.get("name") {
		Some(name) if name == "Home Assistant" => Ok(()),
		_ => Err("Invalid Home Assistant manifest".into()),
	}
}

fn websocket_url(url: &url::Url) -> Option<String> {
	let scheme = match url.scheme() {
		"http" => "ws",
		"https" => "wss",
		_ => return None,
	};
	let host = url.host_str()?;
	let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
	let path = url.path();
	Some(format!("{scheme}://{host}{port}{path}/api/websocket"))
}

fn authorization_url(url: &url::Url, csrf_token: &str) -> Result<String, url::ParseError> {
	let mut url = url.join("/auth/authorize")?;
	url.query_pairs_mut()
		.append_pair("response_type", "code")
		.append_pair("client_id", OAUTH_CLIENT_ID)
		.append_pair("redirect_uri", HOMEASSISTANT_REDIRECT_URI)
		.append_pair("state", csrf_token)
		.append_pair("scope", "read write");
	Ok(url.to_string())
}

#[derive(Deserialize, Serialize, Debug)]
struct TokenResponse {
	access_token: String,
	token_type: String,
	refresh_token: String,
	expires_in: u64,
}

async fn next_websocket_json<S>(stream: &mut S) -> Result<serde_json::Value, String>
where
	S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
		+ SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error>
		+ Unpin,
{
	while let Some(msg) = stream.next().await {
		match msg {
			Ok(Message::Text(text)) => match serde_json::from_str::<serde_json::Value>(&text) {
				Ok(msg) => return Ok(msg),
				Err(e) => return Err(format!("Error parsing json: {e}")),
			},
			Ok(Message::Ping(bytes)) => {
				if let Err(e) = stream.send(Message::Pong(bytes)).await {
					return Err(format!("Failed to respond to websocket ping: {e}"));
				}
			}
			Ok(Message::Close(_)) => return Err("Websocket stream closed".to_string()),
			Ok(_) => continue,
			Err(e) => {
				return Err(format!("Websocket api error: {e}"));
			}
		}
	}

	Err("Websocket stream closed".to_string())
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

impl EntitySnapshot {
	fn new(state: HassEntity, registry: Option<&HassRegistryEntity>) -> Self {
		let registry_name = registry
			.and_then(|entity| entity.name.clone().or_else(|| entity.original_name.clone()))
			.unwrap_or_default();
		let registry_icon = registry
			.and_then(|entity| entity.icon.clone())
			.unwrap_or_default();
		let mut snapshot = Self {
			info: HomeassistantEntityInfo {
				entity_id: state.entity_id.clone(),
				domain: state
					.entity_id
					.split_once('.')
					.map_or_else(String::new, |(domain, _)| domain.to_string()),
				device_id: registry
					.and_then(|entity| entity.device_id.clone())
					.unwrap_or_default(),
				area_id: registry
					.and_then(|entity| entity.area_id.clone())
					.unwrap_or_default(),
				platform: registry.map_or_else(String::new, |entity| entity.platform.clone()),
				entity_category: registry
					.and_then(|entity| entity.entity_category.clone())
					.into(),
				..Default::default()
			},
			registry_name,
			registry_icon,
		};
		snapshot.apply_state(state);
		snapshot
	}

	fn with_state(&self, state: HassEntity) -> Self {
		let mut snapshot = self.clone();
		snapshot.apply_state(state);
		snapshot
	}

	fn apply_state(&mut self, state: HassEntity) {
		self.entity_id = state.entity_id;
		self.state = state.state;
		self.available = self.state != "unavailable";
		self.friendly_name = attribute_string(&state.attributes, "friendly_name")
			.unwrap_or_else(|| self.registry_name.clone());
		self.icon = attribute_string(&state.attributes, "icon")
			.unwrap_or_else(|| self.registry_icon.clone());
		self.device_class = attribute_string(&state.attributes, "device_class").unwrap_or_default();
		self.unit_of_measurement =
			attribute_string(&state.attributes, "unit_of_measurement").unwrap_or_default();
		self.attributes_json =
			serde_json::to_string(&state.attributes).unwrap_or_else(|_| "{}".to_string());
		self.last_changed = state.last_changed;
		self.last_updated = state.last_updated;
	}
}

fn attribute_string(attributes: &serde_json::Value, name: &str) -> Option<String> {
	attributes.get(name)?.as_str().map(ToString::to_string)
}

#[derive(Clone)]
struct HomeassistantEntityService {
	data: Arc<RwLock<EntitySnapshot>>,
	observation: Arc<Mutex<()>>,
	retiring: Arc<AtomicBool>,
	registered: Arc<AtomicBool>,
	notifications_pending: Arc<AtomicBool>,
	service: Weak<HomeassistantInner>,
}

impl HomeassistantEntityService {
	fn new(data: EntitySnapshot, service: Weak<HomeassistantInner>) -> Self {
		Self {
			data: Arc::new(RwLock::new(data)),
			observation: Arc::new(Mutex::new(())),
			retiring: Arc::new(AtomicBool::new(false)),
			registered: Arc::new(AtomicBool::new(false)),
			notifications_pending: Arc::new(AtomicBool::new(false)),
			service,
		}
	}

	fn service(&self) -> zbus::fdo::Result<HomeassistantService> {
		self.service
			.upgrade()
			.map(|inner| HomeassistantService { inner })
			.ok_or_else(|| {
				zbus::fdo::Error::Failed("Home Assistant service is shutting down".to_string())
			})
	}

	async fn update_state(&self, state: HassEntity, notify: bool) -> Result<(), String> {
		let data = self.data.read().await.with_state(state);
		self.update(data, notify).await
	}

	async fn update(&self, data: EntitySnapshot, notify: bool) -> Result<(), String> {
		let entity_id = data.entity_id.clone();
		let changed = {
			let mut current = self.data.write().await;
			if *current == data {
				false
			} else {
				*current = data;
				true
			}
		};
		if !notify {
			self.notifications_pending.store(false, Ordering::Release);
			return Ok(());
		}
		if changed {
			self.notifications_pending.store(true, Ordering::Release);
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
		self.notifications_pending.store(false, Ordering::Release);
		Ok(())
	}
}

impl HomeassistantEntity for HomeassistantEntityService {
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

	#[test]
	fn derives_entity_properties_from_state_attributes() {
		let snapshot = EntitySnapshot::new(
			HassEntity {
				entity_id: "sensor.office_temperature".to_string(),
				last_changed: "2026-01-01T00:00:00Z".to_string(),
				state: "21.5".to_string(),
				attributes: serde_json::json!({
					"friendly_name": "Office temperature",
					"device_class": "temperature",
					"unit_of_measurement": "C"
				}),
				last_updated: "2026-01-01T00:00:01Z".to_string(),
				context: None,
			},
			None,
		);

		assert_eq!(snapshot.domain, "sensor");
		assert_eq!(snapshot.state, "21.5");
		assert!(snapshot.available);
		assert_eq!(snapshot.friendly_name, "Office temperature");
		assert_eq!(snapshot.device_class, "temperature");
		assert_eq!(snapshot.unit_of_measurement, "C");
	}
}
