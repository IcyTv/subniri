use std::{
	collections::{HashMap, HashSet},
	path::PathBuf,
	sync::{
		Arc, Weak,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
	time::Duration,
};

use daemon_common::{
	SPOTIFY_OBJECT_PATH, SPOTIFY_REDIRECT_URL, Spotify, SpotifyServer, SpotifyStatus, SpotifyTrack,
	SpotifyTrackProperties, SpotifyTrackServer,
};
use rspotify::{
	AuthCodePkceSpotify, Credentials, OAuth, Token, TokenCallback,
	model::{LibraryId, TrackId},
	prelude::{BaseClient, Id, OAuthClient},
	scopes,
};
use secret_store::{SecretAttributes, SecretStore};
use tokio::{
	sync::{Mutex, RwLock, mpsc, oneshot, watch},
	task::JoinHandle,
	time::MissedTickBehavior,
};
use zbus::{
	message::Header,
	names::{BusName, UniqueName},
	object_server::SignalEmitter,
	zvariant::OwnedObjectPath,
};

use crate::observed::{ObservationLimitReached, Observed, watch_disconnections};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

// Spotify's current /me/library/contains endpoint accepts at most 40 URIs per request.
const MAX_OBSERVERS: usize = 40;
const UNOBSERVED_TRACK_LIFETIME: Duration = Duration::from_secs(30);
const SCHEDULER_QUEUE_CAPACITY: usize = 64;
const MIN_REQUEST_SPACING: Duration = Duration::from_secs(1);
const TRACK_OBJECT_PATH_PREFIX: &str = "/de/icytv/subniri/Spotify/Tracks";

#[derive(Clone)]
pub struct SpotifyDbus {
	inner: Arc<SpotifyInner>,
}

struct SpotifyInner {
	connection: zbus::Connection,
	state: Arc<RwLock<SpotifyState>>,
	registry: Mutex<TrackRegistry>,
	scheduler: SpotifyScheduler,
	poll_interval_tx: watch::Sender<u64>,
	shutdown_tx: watch::Sender<bool>,
	tasks: Mutex<Vec<JoinHandle<()>>>,
}

struct SpotifyState {
	store: Option<Arc<SecretStore>>,
	client: Option<AuthCodePkceSpotify>,
	status: SpotifyStatus,
	authenticated: bool,
	generation: u64,
	connection_config: Option<(bool, String)>,
}

struct TrackRegistry {
	observed: Observed<String>,
	tracks: HashMap<String, SpotifyTrackDbus>,
}

#[derive(Clone)]
struct SpotifyScheduler {
	tx: mpsc::Sender<SchedulerRequest>,
	state: Arc<RwLock<SpotifyState>>,
}

enum SchedulerRequest {
	Contains {
		track_ids: Vec<String>,
		generation: u64,
		reply: oneshot::Sender<Result<Scheduled<Vec<bool>>, String>>,
	},
	SetSaved {
		track_id: String,
		saved: bool,
		generation: u64,
		reply: oneshot::Sender<Result<Scheduled<bool>, String>>,
	},
}

struct Scheduled<T> {
	sequence: u64,
	value: T,
}

#[derive(Clone)]
struct SpotifyTrackDbus {
	track_id: String,
	saved: Arc<RwLock<CachedSaved>>,
	observation: Arc<Mutex<()>>,
	retiring: Arc<AtomicBool>,
	expiry_generation: Arc<AtomicU64>,
	scheduler: SpotifyScheduler,
	service: Weak<SpotifyInner>,
}

struct CachedSaved {
	value: bool,
	sequence: u64,
}

enum TrackRemoval {
	Removed,
	Stale,
	Retained,
}

impl SpotifyDbus {
	pub async fn register(
		connection: zbus::Connection, config: config::Spotify,
	) -> Result<Self, Error> {
		let (service, scheduler_rx, poll_interval_rx) =
			Self::new(connection.clone(), config.poll_interval_seconds);
		if let Err(error) = service.start_tasks(scheduler_rx, poll_interval_rx).await {
			service.stop_tasks().await;
			return Err(error);
		}
		let added = match connection
			.object_server()
			.at(SPOTIFY_OBJECT_PATH, SpotifyServer(service.clone()))
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
			return Err("Spotify D-Bus interface is already registered".into());
		}
		service.reconfigure(config).await;
		Ok(service)
	}

	fn new(
		connection: zbus::Connection, poll_interval_seconds: u64,
	) -> (Self, mpsc::Receiver<SchedulerRequest>, watch::Receiver<u64>) {
		let (scheduler_tx, scheduler_rx) = mpsc::channel(SCHEDULER_QUEUE_CAPACITY);
		let (poll_interval_tx, poll_interval_rx) = watch::channel(poll_interval_seconds);
		let (shutdown_tx, _) = watch::channel(false);
		let state = Arc::new(RwLock::new(SpotifyState {
			store: None,
			client: None,
			status: SpotifyStatus::Unconfigured,
			authenticated: false,
			generation: 0,
			connection_config: None,
		}));
		let service = Self {
			inner: Arc::new(SpotifyInner {
				connection,
				state: state.clone(),
				registry: Mutex::new(TrackRegistry {
					observed: Observed::with_limits(MAX_OBSERVERS, MAX_OBSERVERS, MAX_OBSERVERS),
					tracks: HashMap::new(),
				}),
				scheduler: SpotifyScheduler {
					tx: scheduler_tx,
					state,
				},
				poll_interval_tx,
				shutdown_tx,
				tasks: Mutex::new(Vec::new()),
			}),
		};
		(service, scheduler_rx, poll_interval_rx)
	}

	async fn start_tasks(
		&self, scheduler_rx: mpsc::Receiver<SchedulerRequest>,
		poll_interval_rx: watch::Receiver<u64>,
	) -> Result<(), Error> {
		let scheduler_state = self.inner.state.clone();
		let scheduler_shutdown = self.inner.shutdown_tx.subscribe();
		let scheduler_task = tokio::spawn(run_scheduler(
			scheduler_state,
			scheduler_rx,
			scheduler_shutdown,
		));

		let poller = self.clone();
		let poller_shutdown = self.inner.shutdown_tx.subscribe();
		let poller_task = tokio::spawn(async move {
			poller.run_poller(poll_interval_rx, poller_shutdown).await;
		});

		let watcher = self.clone();
		let watcher_connection = self.inner.connection.clone();
		let watcher_shutdown = self.inner.shutdown_tx.subscribe();
		let (watcher_ready_tx, watcher_ready_rx) = oneshot::channel();
		let watcher_task = tokio::spawn(async move {
			if let Err(error) = watch_disconnections(
				&watcher_connection,
				watcher_shutdown,
				watcher_ready_tx,
				|client| {
					let watcher = watcher.clone();
					async move { watcher.release_client(&client).await }
				},
			)
			.await
			{
				log::error!("Failed to watch D-Bus clients for Spotify: {error}");
			}
		});

		self.inner
			.tasks
			.lock()
			.await
			.extend([scheduler_task, poller_task, watcher_task]);
		watcher_ready_rx
			.await
			.map_err(|_| "Spotify D-Bus disconnect watcher stopped during initialization".into())
	}

	async fn stop_tasks(&self) {
		let _ = self.inner.shutdown_tx.send(true);
		for task in std::mem::take(&mut *self.inner.tasks.lock().await) {
			task.abort();
			if let Err(error) = task.await
				&& !error.is_cancelled()
			{
				log::warn!("Spotify background task failed: {error}");
			}
		}
	}

	async fn run_poller(
		&self, mut poll_interval: watch::Receiver<u64>, mut shutdown: watch::Receiver<bool>,
	) {
		let mut timer =
			tokio::time::interval(Duration::from_secs(*poll_interval.borrow_and_update()));
		timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

		loop {
			tokio::select! {
				changed = shutdown.changed() => {
					if changed.is_err() || *shutdown.borrow() {
						break;
					}
				}
				changed = poll_interval.changed() => {
					if changed.is_err() {
						break;
					}
					timer = tokio::time::interval(Duration::from_secs(*poll_interval.borrow_and_update()));
					timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
				}
				_ = timer.tick() => self.poll_observed_tracks().await,
			}
		}
	}

	async fn poll_observed_tracks(&self) {
		let track_ids = self
			.inner
			.registry
			.lock()
			.await
			.observed
			.observed_keys()
			.into_iter()
			.collect::<Vec<_>>();
		if track_ids.is_empty() {
			return;
		}

		let result = match self.inner.scheduler.contains(track_ids.clone()).await {
			Ok(result) => result,
			Err(error) => {
				log::warn!("Failed to poll observed Spotify tracks: {error}");
				return;
			}
		};
		if result.value.len() != track_ids.len() {
			log::warn!(
				"Spotify returned {} saved states for {} observed tracks",
				result.value.len(),
				track_ids.len()
			);
			return;
		}

		for (track_id, saved) in track_ids.into_iter().zip(result.value) {
			let track = self
				.inner
				.registry
				.lock()
				.await
				.tracks
				.get(&track_id)
				.cloned();
			let Some(track) = track else {
				continue;
			};
			let _observation = track.observation.lock().await;
			if track.retiring.load(Ordering::Acquire) {
				continue;
			}
			if !track.update_saved(result.sequence, saved).await {
				continue;
			}

			let path = track_path(&track_id);
			match path.and_then(|path| SignalEmitter::new(&self.inner.connection, path)) {
				Ok(emitter) => {
					if let Err(error) = track.emit_saved_changed(&emitter).await {
						log::warn!("Failed to emit Spotify saved-state change: {error}");
					}
				}
				Err(error) => log::warn!("Failed to create Spotify track signal emitter: {error}"),
			}
		}
	}

	async fn release_client(&self, client: &UniqueName<'_>) {
		let orphaned = {
			let mut registry = self.inner.registry.lock().await;
			registry
				.observed
				.release(client)
				.into_iter()
				.filter_map(|track_id| {
					registry
						.tracks
						.get(&track_id)
						.cloned()
						.map(|track| (track_id, track))
				})
				.collect::<Vec<_>>()
		};
		for (track_id, track) in orphaned {
			self.schedule_unobserved_track_expiry(track_id, &track);
		}
	}

	async fn remove_if_unobserved(
		&self, track_id: &str, expected_expiry: &Arc<AtomicU64>, expiry_generation: u64,
	) -> TrackRemoval {
		let track = self
			.inner
			.registry
			.lock()
			.await
			.tracks
			.get(track_id)
			.cloned();
		let Some(track) = track else {
			return TrackRemoval::Stale;
		};
		if !Arc::ptr_eq(&track.expiry_generation, expected_expiry) {
			return TrackRemoval::Stale;
		}
		let _observation = track.observation.lock().await;
		if track.expiry_generation.load(Ordering::Acquire) != expiry_generation {
			return TrackRemoval::Stale;
		}
		let mut registry = self.inner.registry.lock().await;
		if track.expiry_generation.load(Ordering::Acquire) != expiry_generation
			|| !registry
				.tracks
				.get(track_id)
				.is_some_and(|current| Arc::ptr_eq(&current.expiry_generation, expected_expiry))
		{
			return TrackRemoval::Stale;
		}
		if registry.observed.is_observed(&track_id.to_string()) {
			return TrackRemoval::Retained;
		}
		track.retiring.store(true, Ordering::Release);

		let result = async {
			self.inner
				.connection
				.object_server()
				.remove::<SpotifyTrackServer<SpotifyTrackDbus>, _>(track_path(track_id)?)
				.await
				.map(|_| ())
		}
		.await;
		match result {
			Ok(()) => {
				registry.tracks.remove(track_id);
				TrackRemoval::Removed
			}
			Err(error) => {
				track.retiring.store(false, Ordering::Release);
				log::warn!("Failed to remove orphaned Spotify track {track_id}: {error}");
				TrackRemoval::Retained
			}
		}
	}

	async fn remove_observation(&self, client: &UniqueName<'_>, track_id: &String) {
		let track = {
			let mut registry = self.inner.registry.lock().await;
			registry
				.observed
				.remove(client, track_id)
				.then(|| registry.tracks.get(track_id).cloned())
				.flatten()
		};
		if let Some(track) = track {
			self.schedule_unobserved_track_expiry(track_id.clone(), &track);
		}
	}

	fn schedule_unobserved_track_expiry(&self, track_id: String, track: &SpotifyTrackDbus) {
		let expected_expiry = track.expiry_generation.clone();
		let expiry_generation = expected_expiry
			.fetch_add(1, Ordering::AcqRel)
			.wrapping_add(1);
		let service = Arc::downgrade(&self.inner);
		tokio::spawn(async move {
			tokio::time::sleep(UNOBSERVED_TRACK_LIFETIME).await;
			if let Some(inner) = service.upgrade() {
				let _ = SpotifyDbus { inner }
					.remove_if_unobserved(&track_id, &expected_expiry, expiry_generation)
					.await;
			}
		});
	}

	async fn remove_track_object(&self, track_id: &str) {
		let path = match track_path(track_id) {
			Ok(path) => path,
			Err(error) => {
				log::warn!("Invalid Spotify track object path for {track_id}: {error}");
				return;
			}
		};
		if let Err(error) = self
			.inner
			.connection
			.object_server()
			.remove::<SpotifyTrackServer<SpotifyTrackDbus>, _>(path)
			.await
		{
			log::warn!("Failed to remove Spotify track object for {track_id}: {error}");
		}
	}

	fn caller<'a>(header: &'a Header<'_>) -> zbus::fdo::Result<&'a UniqueName<'a>> {
		header
			.sender()
			.ok_or_else(|| zbus::fdo::Error::Failed("D-Bus caller has no unique name".to_string()))
	}

	pub async fn reconfigure(&self, config: config::Spotify) {
		if config.poll_interval_seconds < 15 {
			log::warn!(
				"Spotify polling interval is below 15 seconds and may exceed API rate limits"
			);
		}
		let _ = self
			.inner
			.poll_interval_tx
			.send(config.poll_interval_seconds);
		let generation = {
			let mut state = self.inner.state.write().await;
			let connection_config = (config.enabled, config.client_id.clone());
			if state.connection_config.as_ref() == Some(&connection_config) {
				return;
			}
			state.connection_config = Some(connection_config);
			state.generation = state.generation.wrapping_add(1);
			state.client = None;
			state.authenticated = false;
			state.status = if config.enabled {
				SpotifyStatus::Connecting
			} else {
				SpotifyStatus::Disabled
			};
			state.generation
		};

		if !config.enabled {
			return;
		}
		if !valid_client_id(&config.client_id) {
			self.set_connection_state(generation, None, SpotifyStatus::Unconfigured, false)
				.await;
			return;
		}

		let store = match self.secret_store().await {
			Ok(store) => store,
			Err(error) => {
				log::error!("Failed to connect to Secret Service: {error}");
				self.set_connection_state(generation, None, SpotifyStatus::Error, false)
					.await;
				return;
			}
		};

		match initialize_client(config.client_id, store).await {
			Ok((client, has_token)) => {
				let refresh_token = current_refresh_token(&client).await;
				let authenticated = has_token && client.current_user().await.is_ok();
				restore_refresh_token(&client, refresh_token).await;
				let status = if authenticated {
					SpotifyStatus::Ready
				} else {
					SpotifyStatus::AuthenticationRequired
				};
				self.set_connection_state(generation, Some(client), status, authenticated)
					.await;
			}
			Err(error) => {
				log::error!("Failed to initialize Spotify: {error}");
				self.set_connection_state(generation, None, SpotifyStatus::Error, false)
					.await;
			}
		}
	}

	async fn set_connection_state(
		&self, generation: u64, client: Option<AuthCodePkceSpotify>, status: SpotifyStatus,
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

	pub async fn unregister(&self) -> zbus::Result<()> {
		self.stop_tasks().await;

		let track_ids = {
			let mut registry = self.inner.registry.lock().await;
			registry
				.tracks
				.drain()
				.map(|(id, _)| id)
				.collect::<Vec<_>>()
		};
		for track_id in track_ids {
			self.remove_track_object(&track_id).await;
		}

		self.inner
			.connection
			.object_server()
			.remove::<SpotifyServer<Self>, _>(SPOTIFY_OBJECT_PATH)
			.await
			.map(|_| ())
	}
}

impl Spotify for SpotifyDbus {
	async fn status(&self) -> SpotifyStatus {
		self.inner.state.read().await.status
	}

	async fn authenticated(&self) -> bool {
		self.inner.state.read().await.authenticated
	}

	async fn begin_authorization(&self) -> zbus::fdo::Result<String> {
		let mut state = self.inner.state.write().await;
		let client = state
			.client
			.as_mut()
			.ok_or_else(|| zbus::fdo::Error::Failed("Spotify is not configured".to_string()))?;
		let authorization_url = client
			.get_authorize_url(None)
			.map_err(|error| spotify_error(&error))?;
		state.status = SpotifyStatus::Authorizing;
		state.authenticated = false;
		Ok(authorization_url)
	}

	async fn complete_authorization(&self, code: &str, state: &str) -> zbus::fdo::Result<bool> {
		let (client, generation) = {
			let current = self.inner.state.read().await;
			if current.status != SpotifyStatus::Authorizing {
				return Err(zbus::fdo::Error::Failed(
					"No Spotify authorization is in progress".to_string(),
				));
			}
			let client = current
				.client
				.clone()
				.ok_or_else(|| zbus::fdo::Error::Failed("Spotify is not configured".to_string()))?;
			if client.oauth.state != state {
				return Err(zbus::fdo::Error::InvalidArgs(
					"Spotify OAuth state did not match".to_string(),
				));
			}
			(client, current.generation)
		};

		let result = match client.request_token(code).await {
			Ok(()) => client
				.current_user()
				.await
				.map(|_| ())
				.map_err(|error| spotify_error(&error)),
			Err(error) => Err(spotify_error(&error)),
		};
		let mut current = self.inner.state.write().await;
		if current.generation != generation {
			return Err(zbus::fdo::Error::Failed(
				"Spotify configuration changed during authorization".to_string(),
			));
		}
		match result {
			Ok(()) => {
				current.client = Some(client);
				current.status = SpotifyStatus::Ready;
				current.authenticated = true;
				Ok(true)
			}
			Err(error) => {
				current.status = SpotifyStatus::Error;
				current.authenticated = false;
				Err(error)
			}
		}
	}

	async fn cancel_authorization(&self) -> zbus::fdo::Result<()> {
		let mut state = self.inner.state.write().await;
		if state.status == SpotifyStatus::Authorizing {
			state.status = SpotifyStatus::AuthenticationRequired;
		}
		Ok(())
	}

	async fn get_track(&self, track_id: &str) -> zbus::fdo::Result<OwnedObjectPath> {
		let track_id = TrackId::from_id(track_id)
			.map_err(|error| zbus::fdo::Error::InvalidArgs(error.to_string()))?;
		let track_id = track_id.id().to_string();
		let path = track_path(&track_id)
			.map_err(|error| zbus::fdo::Error::InvalidArgs(error.to_string()))?;
		let mut protected_evictions = HashSet::new();

		loop {
			let (track, eviction) = {
				let registry = self.inner.registry.lock().await;
				if let Some(track) = registry.tracks.get(&track_id) {
					(Some(track.clone()), None)
				} else if registry.tracks.len() < MAX_OBSERVERS {
					(None, None)
				} else {
					let eviction = registry
						.tracks
						.iter()
						.find(|(id, _)| {
							!registry.observed.is_observed(id)
								&& !protected_evictions.contains(id.as_str())
						})
						.map(|(id, track)| {
							(
								id.clone(),
								track.expiry_generation.clone(),
								track.expiry_generation.load(Ordering::Acquire),
							)
						});
					(None, eviction)
				}
			};
			if let Some(track) = track {
				let _observation = track.observation.lock().await;
				let current = self
					.inner
					.registry
					.lock()
					.await
					.tracks
					.get(&track_id)
					.is_some_and(|current| {
						Arc::ptr_eq(&current.retiring, &track.retiring)
							&& !track.retiring.load(Ordering::Acquire)
					});
				if current {
					self.schedule_unobserved_track_expiry(track_id.clone(), &track);
					return Ok(path);
				}
				continue;
			}
			if let Some((eviction, expected_expiry, expiry_generation)) = eviction {
				match self
					.remove_if_unobserved(&eviction, &expected_expiry, expiry_generation)
					.await
				{
					TrackRemoval::Removed | TrackRemoval::Stale => continue,
					TrackRemoval::Retained => {
						protected_evictions.insert(eviction);
						continue;
					}
				}
			}

			let mut registry = self.inner.registry.lock().await;
			if registry.tracks.contains_key(&track_id) {
				continue;
			}
			if registry.tracks.len() >= MAX_OBSERVERS {
				return Err(zbus::fdo::Error::LimitsExceeded(format!(
					"No more than {MAX_OBSERVERS} Spotify track objects may exist at once"
				)));
			}

			let track = SpotifyTrackDbus::new(
				track_id.clone(),
				self.inner.scheduler.clone(),
				Arc::downgrade(&self.inner),
			);
			let added = self
				.inner
				.connection
				.object_server()
				.at(path.clone(), SpotifyTrackServer(track.clone()))
				.await
				.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
			if !added {
				return Err(zbus::fdo::Error::Failed(
					"Spotify track object was already registered".to_string(),
				));
			}
			registry.tracks.insert(track_id.clone(), track.clone());
			drop(registry);
			self.schedule_unobserved_track_expiry(track_id, &track);
			return Ok(path);
		}
	}
}

impl SpotifyScheduler {
	async fn contains(&self, track_ids: Vec<String>) -> Result<Scheduled<Vec<bool>>, String> {
		if track_ids.len() > MAX_OBSERVERS {
			return Err(format!(
				"Spotify saved-state batches are limited to {MAX_OBSERVERS} tracks"
			));
		}
		let generation = self.state.read().await.generation;
		let (reply, response) = oneshot::channel();
		self.tx
			.send(SchedulerRequest::Contains {
				track_ids,
				generation,
				reply,
			})
			.await
			.map_err(|_| "Spotify scheduler is unavailable".to_string())?;
		response
			.await
			.map_err(|_| "Spotify scheduler stopped before replying".to_string())?
	}

	async fn set_saved(&self, track_id: String, saved: bool) -> Result<Scheduled<bool>, String> {
		let generation = self.state.read().await.generation;
		let (reply, response) = oneshot::channel();
		self.tx
			.send(SchedulerRequest::SetSaved {
				track_id,
				saved,
				generation,
				reply,
			})
			.await
			.map_err(|_| "Spotify scheduler is unavailable".to_string())?;
		response
			.await
			.map_err(|_| "Spotify scheduler stopped before replying".to_string())?
	}
}

impl SpotifyTrackDbus {
	fn new(track_id: String, scheduler: SpotifyScheduler, service: Weak<SpotifyInner>) -> Self {
		Self {
			track_id,
			saved: Arc::new(RwLock::new(CachedSaved {
				value: false,
				sequence: 0,
			})),
			observation: Arc::new(Mutex::new(())),
			retiring: Arc::new(AtomicBool::new(false)),
			expiry_generation: Arc::new(AtomicU64::new(0)),
			scheduler,
			service,
		}
	}

	fn service(&self) -> zbus::fdo::Result<SpotifyDbus> {
		self.service
			.upgrade()
			.map(|inner| SpotifyDbus { inner })
			.ok_or_else(|| zbus::fdo::Error::Failed("Spotify service is shutting down".to_string()))
	}

	async fn update_saved(&self, sequence: u64, saved: bool) -> bool {
		let mut cached = self.saved.write().await;
		if sequence < cached.sequence {
			return false;
		}
		cached.sequence = sequence;
		if cached.value == saved {
			return false;
		}
		cached.value = saved;
		true
	}

	async fn emit_saved_changed(&self, emitter: &SignalEmitter<'_>) -> zbus::Result<()> {
		zbus::fdo::Properties::properties_changed(
			emitter,
			zbus::names::InterfaceName::from_static_str_checked(
				SpotifyTrackProperties::INTERFACE_NAME,
			),
			HashMap::new(),
			std::borrow::Cow::Borrowed(&["Saved"]),
		)
		.await
	}
}

impl SpotifyTrack for SpotifyTrackDbus {
	fn track_id(&self) -> String {
		self.track_id.clone()
	}

	async fn saved(&self) -> bool {
		self.saved.read().await.value
	}

	async fn observe(
		&self, header: Header<'_>, emitter: SignalEmitter<'_>,
	) -> zbus::fdo::Result<bool> {
		let service = self.service()?;
		let caller = SpotifyDbus::caller(&header)?;
		let _observation = self.observation.lock().await;
		if self.retiring.load(Ordering::Acquire) {
			return Err(zbus::fdo::Error::Failed(
				"Spotify track object is being released".to_string(),
			));
		}
		let added = service
			.inner
			.registry
			.lock()
			.await
			.observed
			.add(caller, self.track_id.clone())
			.map_err(observation_limit_error)?;
		let dbus = match zbus::fdo::DBusProxy::new(&service.inner.connection).await {
			Ok(dbus) => dbus,
			Err(error) => {
				service.remove_observation(caller, &self.track_id).await;
				return Err(zbus::fdo::Error::Failed(error.to_string()));
			}
		};
		let caller_connected = match dbus.name_has_owner(BusName::Unique(caller.clone())).await {
			Ok(connected) => connected,
			Err(error) => {
				service.remove_observation(caller, &self.track_id).await;
				return Err(zbus::fdo::Error::Failed(error.to_string()));
			}
		};
		if !caller_connected {
			service.remove_observation(caller, &self.track_id).await;
			return Err(zbus::fdo::Error::Failed(
				"D-Bus caller disconnected while observing Spotify track".to_string(),
			));
		}
		if !added.added {
			return Ok(self.saved.read().await.value);
		}
		if !added.newly_observed {
			let cached = self.saved.read().await;
			if cached.sequence > 0 {
				return Ok(cached.value);
			}
		}

		let result = match self.scheduler.contains(vec![self.track_id.clone()]).await {
			Ok(result) if result.value.len() == 1 => result,
			Ok(result) => {
				service.remove_observation(caller, &self.track_id).await;
				return Err(zbus::fdo::Error::Failed(format!(
					"Spotify returned {} states for one track",
					result.value.len()
				)));
			}
			Err(error) => {
				service.remove_observation(caller, &self.track_id).await;
				return Err(zbus::fdo::Error::Failed(error));
			}
		};
		let saved = result.value.into_iter().next().ok_or_else(|| {
			zbus::fdo::Error::Failed("Spotify returned no track state".to_string())
		})?;
		if self.update_saved(result.sequence, saved).await {
			self.emit_saved_changed(&emitter)
				.await
				.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
		}
		Ok(self.saved.read().await.value)
	}

	async fn unobserve(&self, header: Header<'_>) -> zbus::fdo::Result<()> {
		let service = self.service()?;
		let caller = SpotifyDbus::caller(&header)?;
		let _observation = self.observation.lock().await;
		service.remove_observation(caller, &self.track_id).await;
		Ok(())
	}

	async fn refresh(&self, emitter: SignalEmitter<'_>) -> zbus::fdo::Result<bool> {
		let _observation = self.observation.lock().await;
		let result = self
			.scheduler
			.contains(vec![self.track_id.clone()])
			.await
			.map_err(zbus::fdo::Error::Failed)?;
		let saved = result.value.into_iter().next().ok_or_else(|| {
			zbus::fdo::Error::Failed("Spotify returned no track state".to_string())
		})?;
		if self.update_saved(result.sequence, saved).await {
			self.emit_saved_changed(&emitter)
				.await
				.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
		}
		Ok(self.saved.read().await.value)
	}

	async fn set_saved(&self, saved: bool, emitter: SignalEmitter<'_>) -> zbus::fdo::Result<bool> {
		let _observation = self.observation.lock().await;
		let result = self
			.scheduler
			.set_saved(self.track_id.clone(), saved)
			.await
			.map_err(zbus::fdo::Error::Failed)?;
		let (applied, current) = {
			let mut cached = self.saved.write().await;
			if result.sequence < cached.sequence {
				(false, cached.value)
			} else {
				let changed = cached.value != result.value;
				cached.sequence = result.sequence;
				cached.value = result.value;
				(changed, cached.value)
			}
		};
		if applied {
			self.emit_saved_changed(&emitter)
				.await
				.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
		}
		Ok(current)
	}
}

fn observation_limit_error(error: ObservationLimitReached) -> zbus::fdo::Error {
	let limit = match error {
		ObservationLimitReached::Clients => "clients",
		ObservationLimitReached::ObjectsPerClient => "tracks per client",
		ObservationLimitReached::Objects => "observed tracks",
	};
	zbus::fdo::Error::LimitsExceeded(format!(
		"Spotify supports no more than {MAX_OBSERVERS} {limit}"
	))
}

async fn run_scheduler(
	state: Arc<RwLock<SpotifyState>>, mut requests: mpsc::Receiver<SchedulerRequest>,
	mut shutdown: watch::Receiver<bool>,
) {
	let mut sequence = 0_u64;
	let mut next_request = tokio::time::Instant::now();
	loop {
		tokio::select! {
			biased;
			changed = shutdown.changed() => {
				if changed.is_err() || *shutdown.borrow() {
					break;
				}
			}
			request = requests.recv() => {
				let Some(request) = request else {
					break;
				};
				if let Err(error) = scheduler_request_ready(&state, &request).await {
					reject_scheduler_request(request, error);
					continue;
				}
				tokio::time::sleep_until(next_request).await;
				next_request = tokio::time::Instant::now() + MIN_REQUEST_SPACING;
				sequence = sequence.wrapping_add(1);
				if let Some(retry_after) = handle_scheduler_request(&state, request, sequence).await {
					next_request = next_request.max(tokio::time::Instant::now() + retry_after);
				}
			}
		}
	}
}

async fn scheduler_request_ready(
	state: &RwLock<SpotifyState>, request: &SchedulerRequest,
) -> Result<(), String> {
	let generation = scheduler_request_generation(request);
	let state = state.read().await;
	if state.generation != generation {
		return Err("Spotify configuration changed before the request ran".to_string());
	}
	if !state.authenticated || state.client.is_none() {
		return Err("Spotify is not authenticated".to_string());
	}
	Ok(())
}

fn scheduler_request_generation(request: &SchedulerRequest) -> u64 {
	match request {
		SchedulerRequest::Contains { generation, .. }
		| SchedulerRequest::SetSaved { generation, .. } => *generation,
	}
}

fn reject_scheduler_request(request: SchedulerRequest, error: String) {
	match request {
		SchedulerRequest::Contains { reply, .. } => {
			let _ = reply.send(Err(error));
		}
		SchedulerRequest::SetSaved { reply, .. } => {
			let _ = reply.send(Err(error));
		}
	}
}

async fn handle_scheduler_request(
	state: &RwLock<SpotifyState>, request: SchedulerRequest, sequence: u64,
) -> Option<Duration> {
	let request_generation = scheduler_request_generation(&request);
	let client = {
		let state = state.read().await;
		if !state.authenticated || state.generation != request_generation {
			None
		} else {
			state.client.clone()
		}
	};
	let Some(client) = client else {
		match request {
			SchedulerRequest::Contains { reply, .. } => {
				let _ = reply.send(Err("Spotify is not authenticated".to_string()));
			}
			SchedulerRequest::SetSaved { reply, .. } => {
				let _ = reply.send(Err("Spotify is not authenticated".to_string()));
			}
		}
		return None;
	};

	match request {
		SchedulerRequest::Contains {
			track_ids,
			generation: _,
			reply,
		} => {
			handle_contains_request(
				state,
				client,
				track_ids,
				request_generation,
				sequence,
				reply,
			)
			.await
		}
		SchedulerRequest::SetSaved {
			track_id,
			saved,
			generation: _,
			reply,
		} => {
			handle_set_saved_request(
				state,
				client,
				track_id,
				saved,
				request_generation,
				sequence,
				reply,
			)
			.await
		}
	}
}

async fn handle_contains_request(
	state: &RwLock<SpotifyState>, client: AuthCodePkceSpotify, track_ids: Vec<String>,
	generation: u64, sequence: u64, reply: oneshot::Sender<Result<Scheduled<Vec<bool>>, String>>,
) -> Option<Duration> {
	let tracks = track_ids
		.into_iter()
		.map(|id| TrackId::from_id(id).map_err(|error| error.to_string()))
		.collect::<Result<Vec<_>, _>>();
	let tracks = match tracks {
		Ok(tracks) => tracks,
		Err(error) => {
			let _ = reply.send(Err(error));
			return None;
		}
	};
	let refresh_token = current_refresh_token(&client).await;
	let result = client
		.library_contains(tracks.into_iter().map(LibraryId::Track))
		.await;
	restore_refresh_token(&client, refresh_token).await;
	match result {
		Ok(value) => {
			reply_if_current(state, generation, reply, Scheduled { sequence, value }).await;
			None
		}
		Err(error) => handle_api_error(state, generation, reply, error).await,
	}
}

async fn handle_set_saved_request(
	state: &RwLock<SpotifyState>, client: AuthCodePkceSpotify, track_id: String, saved: bool,
	generation: u64, sequence: u64, reply: oneshot::Sender<Result<Scheduled<bool>, String>>,
) -> Option<Duration> {
	let track = match TrackId::from_id(track_id) {
		Ok(track) => track,
		Err(error) => {
			let _ = reply.send(Err(error.to_string()));
			return None;
		}
	};
	let refresh_token = current_refresh_token(&client).await;
	let result = if saved {
		client.library_add([LibraryId::Track(track)]).await
	} else {
		client.library_remove([LibraryId::Track(track)]).await
	};
	restore_refresh_token(&client, refresh_token).await;
	match result {
		Ok(()) => {
			reply_if_current(
				state,
				generation,
				reply,
				Scheduled {
					sequence,
					value: saved,
				},
			)
			.await;
			None
		}
		Err(error) => handle_api_error(state, generation, reply, error).await,
	}
}

async fn reply_if_current<T>(
	state: &RwLock<SpotifyState>, generation: u64,
	reply: oneshot::Sender<Result<Scheduled<T>, String>>, result: Scheduled<T>,
) {
	if state.read().await.generation == generation {
		let _ = reply.send(Ok(result));
	} else {
		let _ = reply.send(Err(
			"Spotify configuration changed during the request".to_string()
		));
	}
}

async fn handle_api_error<T>(
	state: &RwLock<SpotifyState>, generation: u64,
	reply: oneshot::Sender<Result<Scheduled<T>, String>>, error: rspotify::ClientError,
) -> Option<Duration> {
	let retry_after = spotify_retry_after(&error);
	log::warn!("Spotify API request failed: {error}");
	if matches!(error, rspotify::ClientError::InvalidToken)
		|| spotify_status_code(&error) == Some(401)
	{
		mark_authentication_required(state, generation).await;
	}
	let _ = reply.send(Err(error.to_string()));
	retry_after
}

fn spotify_retry_after(error: &rspotify::ClientError) -> Option<Duration> {
	if spotify_status_code(error) != Some(429) {
		return None;
	}
	let rspotify::ClientError::Http(error) = error else {
		return None;
	};
	let rspotify::http::HttpError::StatusCode(response) = error.as_ref() else {
		return None;
	};
	response
		.headers()
		.get("retry-after")?
		.to_str()
		.ok()?
		.parse()
		.ok()
		.map(Duration::from_secs)
}

fn spotify_status_code(error: &rspotify::ClientError) -> Option<u16> {
	let rspotify::ClientError::Http(error) = error else {
		return None;
	};
	let rspotify::http::HttpError::StatusCode(response) = error.as_ref() else {
		return None;
	};
	Some(response.status().as_u16())
}

async fn mark_authentication_required(state: &RwLock<SpotifyState>, generation: u64) {
	let mut state = state.write().await;
	if state.generation == generation {
		state.authenticated = false;
		state.status = SpotifyStatus::AuthenticationRequired;
	}
}

async fn current_refresh_token(client: &AuthCodePkceSpotify) -> Option<String> {
	client
		.get_token()
		.lock()
		.await
		.ok()
		.and_then(|token| token.as_ref().and_then(|token| token.refresh_token.clone()))
}

async fn restore_refresh_token(client: &AuthCodePkceSpotify, refresh_token: Option<String>) {
	let Some(refresh_token) = refresh_token else {
		return;
	};
	let token = client.get_token();
	let Ok(mut token) = token.lock().await else {
		return;
	};
	if let Some(token) = token.as_mut()
		&& token.refresh_token.is_none()
	{
		token.refresh_token = Some(refresh_token);
	}
}

fn track_path(track_id: &str) -> zbus::Result<OwnedObjectPath> {
	OwnedObjectPath::try_from(format!("{TRACK_OBJECT_PATH_PREFIX}/{track_id}"))
		.map_err(zbus::Error::from)
}

async fn initialize_client(
	client_id: String, store: Arc<SecretStore>,
) -> Result<(AuthCodePkceSpotify, bool), Error> {
	let credentials = Credentials::new_pkce(&client_id);
	let oauth = OAuth {
		redirect_uri: SPOTIFY_REDIRECT_URL.to_string(),
		scopes: scopes!("user-library-read user-library-modify"),
		..Default::default()
	};

	let (token_tx, mut token_rx) = tokio::sync::mpsc::unbounded_channel();
	let callback = TokenCallback(Box::new(move |token| {
		let _ = token_tx.send(token);
		Ok(())
	}));
	let token_store = store.clone();
	tokio::spawn(async move {
		while let Some(token) = token_rx.recv().await {
			if let Err(error) = persist_token(&token_store, token).await {
				log::error!("Failed to persist Spotify token: {error}");
			}
		}
	});

	let rspotify_config = rspotify::Config {
		token_cached: false,
		token_refreshing: true,
		token_callback_fn: Arc::new(Some(callback)),
		cache_path: PathBuf::from("/dev/null/invalid"),
		..Default::default()
	};

	let attributes = spotify_attributes();
	let access_token = store
		.get(&attributes.clone().insert("type", "access_token"))
		.await?;
	let expiry = store
		.get(&attributes.clone().insert("type", "access_token_expiry"))
		.await?;
	let refresh_token = store
		.get(&attributes.insert("type", "refresh_token"))
		.await?;
	let token = access_token.zip(expiry).map(|(access_token, expiry)| {
		let expires_at = chrono::DateTime::parse_from_rfc3339(&expiry).map_or_else(
			|_| chrono::Utc::now(),
			|date| date.with_timezone(&chrono::Utc),
		);
		Token {
			access_token,
			expires_in: expires_at - chrono::Utc::now(),
			expires_at: Some(expires_at),
			refresh_token,
			scopes: oauth.scopes.clone(),
		}
	});
	let has_token = token.is_some();
	let client = if let Some(token) = token {
		AuthCodePkceSpotify::from_token_with_config(token, credentials, oauth, rspotify_config)
	} else {
		AuthCodePkceSpotify::with_config(credentials, oauth, rspotify_config)
	};
	Ok((client, has_token))
}

async fn persist_token(store: &SecretStore, token: Token) -> Result<(), secret_store::Error> {
	if token.is_expired() {
		return Ok(());
	}
	let expiry = token
		.expires_at
		.unwrap_or_else(|| chrono::Utc::now() + token.expires_in)
		.to_rfc3339();
	let attributes = spotify_attributes();
	store
		.store(
			"Spotify Access Token",
			&attributes.clone().insert("type", "access_token"),
			token.access_token,
		)
		.await?;
	store
		.store(
			"Spotify Access Token Expiry",
			&attributes.clone().insert("type", "access_token_expiry"),
			expiry,
		)
		.await?;
	if let Some(refresh_token) = token.refresh_token {
		store
			.store(
				"Spotify Refresh Token",
				&attributes.insert("type", "refresh_token"),
				refresh_token,
			)
			.await?;
	}
	Ok(())
}

fn spotify_attributes() -> SecretAttributes {
	SecretAttributes::new()
		.insert("application", "subniri")
		.insert("service", "spotify")
}

fn valid_client_id(client_id: &str) -> bool {
	client_id.len() == 32 && client_id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn spotify_error(error: &rspotify::ClientError) -> zbus::fdo::Error {
	zbus::fdo::Error::Failed(error.to_string())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn creates_stable_track_object_paths() {
		assert!(matches!(
			track_path("4iV5W9uYEdYUVa79Axb7Rh"),
			Ok(path)
				if path.as_str()
					== "/de/icytv/subniri/Spotify/Tracks/4iV5W9uYEdYUVa79Axb7Rh"
		));
	}

	#[test]
	fn rejects_invalid_track_object_paths() {
		assert!(track_path("not-a-track").is_err());
	}

	#[tokio::test]
	async fn restores_refresh_token_omitted_by_pkce_refresh_response() {
		let client = AuthCodePkceSpotify::from_token(Token {
			access_token: "access".to_string(),
			expires_in: chrono::Duration::hours(1),
			expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
			refresh_token: None,
			scopes: std::collections::HashSet::new(),
		});

		restore_refresh_token(&client, Some("refresh".to_string())).await;

		assert_eq!(
			current_refresh_token(&client).await.as_deref(),
			Some("refresh")
		);
	}
}
