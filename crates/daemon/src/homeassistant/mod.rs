//! Home Assistant integration: shared state and daemon lifecycle.
//!
//! Authorization, connection management, catalog reconciliation, entity
//! observation, snapshots, and interaction capabilities live in private modules.

mod authorization;
mod capabilities;
mod catalog;
mod connection;
mod entity;
mod snapshot;

use std::{
	collections::{HashMap, HashSet},
	sync::Arc,
	time::Duration,
};

use daemon_common::{HOMEASSISTANT_OBJECT_PATH, HomeassistantServer, HomeassistantStatus};
use hass_rs::HassClient;
use secret_store::SecretStore;
use tokio::{
	sync::{Mutex, RwLock, oneshot, watch},
	task::JoinHandle,
};
use zbus::{
	message::Header,
	names::{OwnedUniqueName, UniqueName},
};

use crate::observed::{Observed, watch_disconnections};

use self::{authorization::AuthorizationSession, entity::HomeassistantEntityService};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

const MAX_OBSERVERS: usize = 10;
const MAX_ENTITIES_PER_CLIENT: usize = 100;
const MAX_OBSERVED_ENTITIES: usize = 1_000;
const CATALOG_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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

struct EntityRegistry {
	observed: Observed<HomeassistantResource>,
	entities: HashMap<String, HomeassistantEntityService>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum HomeassistantResource {
	Entity(String),
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

	async fn release_client(&self, client: &OwnedUniqueName) {
		self.inner.registry.lock().await.observed.release(client);
		self.cancel_authorization_for(client.as_str(), None).await;
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
