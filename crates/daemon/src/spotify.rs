use std::{
	collections::{HashMap, HashSet},
	sync::{
		Arc, Weak,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
	time::Duration,
};

use daemon_common::{
	SPOTIFY_OBJECT_PATH, Spotify, SpotifyPairing, SpotifyServer, SpotifyStatus, SpotifyTrack,
	SpotifyTrackProperties, SpotifyTrackServer,
};
use secret_store::SecretStore;
use tokio::{
	sync::{Mutex, Notify, RwLock, mpsc, oneshot, watch},
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

#[doc(hidden)]
pub mod auth;
mod collection;
mod connection;
pub mod protocol;
mod rate_limit;
mod requests;
use rate_limit::Failure;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

// Local bounded batch/observation policy, not a claimed native-service quota.
const MAX_OBSERVERS: usize = collection::MAX_ITEMS;
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
	pairing_task: Mutex<Option<JoinHandle<()>>>,
	lifecycle: watch::Sender<u64>,
	sequence: Arc<AtomicU64>,
	push_healthy: AtomicBool,
	reconcile: Notify,
}

struct SpotifyState {
	store: Option<Arc<SecretStore>>,
	client: Option<Arc<requests::Api>>,
	auth: Option<Arc<auth::Auth>>,
	status: SpotifyStatus,
	authenticated: bool,
	generation: u64,
	connection_config: Option<bool>,
	retry_at: u64,
	pairing_owner: Option<String>,
	push_connected: bool,
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
	generation: u64,
	value: T,
}

#[derive(Clone)]
struct SpotifyTrackDbus {
	track_id: String,
	saved: Arc<RwLock<CachedSaved>>,
	observation: Arc<Mutex<()>>,
	initialization: Arc<Mutex<()>>,
	retiring: Arc<AtomicBool>,
	expiry_generation: Arc<AtomicU64>,
	scheduler: SpotifyScheduler,
	service: Weak<SpotifyInner>,
}

struct CachedSaved {
	value: bool,
	sequence: u64,
	generation: u64,
}

impl CachedSaved {
	fn apply(&mut self, generation: u64, sequence: u64, saved: bool) -> bool {
		if sequence < self.sequence {
			return false;
		}
		let changed = self.sequence == 0 || self.generation != generation || self.value != saved;
		self.sequence = sequence;
		self.generation = generation;
		self.value = saved;
		changed
	}
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
		let (lifecycle, _) = watch::channel(0);
		let state = Arc::new(RwLock::new(SpotifyState {
			store: None,
			client: None,
			auth: None,
			status: SpotifyStatus::Unconfigured,
			authenticated: false,
			generation: 0,
			connection_config: None,
			retry_at: 0,
			pairing_owner: None,
			push_connected: false,
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
				pairing_task: Mutex::new(None),
				lifecycle,
				sequence: Arc::new(AtomicU64::new(1)),
				push_healthy: AtomicBool::new(false),
				reconcile: Notify::new(),
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
			self.inner.sequence.clone(),
		));

		let poller = self.clone();
		let poller_shutdown = self.inner.shutdown_tx.subscribe();
		let poller_task = tokio::spawn(async move {
			poller.run_poller(poll_interval_rx, poller_shutdown).await;
		});
		let sync = self.clone();
		let sync_task = tokio::spawn(async move {
			sync.run_sync().await;
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

		self.inner.tasks.lock().await.extend([
			scheduler_task,
			poller_task,
			watcher_task,
			sync_task,
		]);
		watcher_ready_rx
			.await
			.map_err(|_| "Spotify D-Bus disconnect watcher stopped during initialization".into())
	}

	async fn stop_tasks(&self) {
		let _ = self.inner.shutdown_tx.send(true);
		if let Some(task) = self.inner.pairing_task.lock().await.take() {
			task.abort();
		}
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
		loop {
			let mut interval = reconciliation_interval(
				self.inner.push_healthy.load(Ordering::Acquire),
				*poll_interval.borrow_and_update(),
			);
			let retry_at = self.inner.state.read().await.retry_at;
			if retry_at > unix_seconds() {
				interval = interval.min(Duration::from_secs(
					retry_at.saturating_sub(unix_seconds()).saturating_add(1),
				));
			}
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
				}
				() = self.inner.reconcile.notified() => self.poll_observed_tracks().await,
				() = tokio::time::sleep(interval) => self.poll_observed_tracks().await,
			}
		}
	}

	#[allow(clippy::too_many_lines)] // Keep lifecycle cancellation and reconnect policy together.
	async fn run_sync(&self) {
		let mut lifecycle = self.inner.lifecycle.subscribe();
		let mut shutdown = self.inner.shutdown_tx.subscribe();
		let mut failures = 0_u32;
		loop {
			let (auth, api, generation, enabled) = {
				let state = self.inner.state.read().await;
				(
					state.auth.clone(),
					state.client.clone(),
					state.generation,
					state.connection_config == Some(true) && state.authenticated,
				)
			};
			if !enabled || auth.is_none() || api.is_none() {
				tokio::select! {
					_ = lifecycle.changed() => {},
					_ = shutdown.changed() => break,
				}
				continue;
			}
			let Some(auth) = auth else {
				continue;
			};
			let Some(api) = api else {
				continue;
			};
			let (tx, mut rx) = mpsc::channel(64);
			let mut session = ConnectionTask(tokio::spawn(connection::run(auth.clone(), api, tx)));
			let mut maintenance = tokio::time::interval(Duration::from_secs(30));
			maintenance.set_missed_tick_behavior(MissedTickBehavior::Skip);
			let mut last_reconcile = tokio::time::Instant::now() - Duration::from_secs(60);
			let failure = loop {
				tokio::select! {
					_ = shutdown.changed() => return,
					_ = lifecycle.changed() => break None,
					result = &mut session.0 => break Some(result.unwrap_or(Err(Failure::Unavailable)).err().unwrap_or(Failure::Unavailable)),
					_ = maintenance.tick() => {
						if let Err(failure) = auth.access_token().await { break Some(failure); }
						if self.inner.state.read().await.status == SpotifyStatus::RateLimited { mark_recovered(&self.inner.state,generation).await; }
					},
					update = rx.recv() => match update {
						Some(connection::Update::Connected) => {
							self.inner.push_healthy.store(true,Ordering::Release);
							failures = 0;
							let mut state = self.inner.state.write().await;
							state.push_connected = state.generation == generation;
							if state.generation == generation && state.retry_at <= unix_seconds() {
								state.status = SpotifyStatus::Ready;
								state.retry_at = 0;
							}
							drop(state);
							self.inner.reconcile.notify_one();
						},
						Some(connection::Update::Event(protocol::Event::Collection { changes,.. })) => {
							let sequence = self.inner.sequence.fetch_add(1,Ordering::AcqRel);
							for change in changes { self.apply_change(generation,sequence,&change.id,change.saved).await; }
						},
						Some(connection::Update::Event(protocol::Event::Reconcile)) if last_reconcile.elapsed() >= Duration::from_secs(60) => {
								last_reconcile = tokio::time::Instant::now();
								self.inner.reconcile.notify_one();
						},
						None => break Some(Failure::Unavailable),
						_ => {},
					}
				}
			};
			drop(session);
			if self.inner.push_healthy.swap(false, Ordering::AcqRel) {
				self.inner.reconcile.notify_one();
			}
			{
				let mut state = self.inner.state.write().await;
				if state.generation == generation {
					state.push_connected = false;
				}
			}
			let Some(failure) = failure else {
				continue;
			};
			set_failure(&self.inner.state, generation, &failure).await;
			if failure == Failure::SessionThrottled {
				log::warn!(
					"Spotify session throttled without retry timing; automatic reconnect paused until re-enabled"
				);
				tokio::select! { _ = shutdown.changed() => break, _ = lifecycle.changed() => {} }
				continue;
			}
			failures = failures.saturating_add(1);
			let delay = match failure {
				Failure::RateLimited(delay) => delay,
				_ => Duration::from_secs(15_u64.saturating_mul(1 << failures.min(5)).min(900)),
			};
			log::warn!(retry_seconds = delay.as_secs(), failure = %failure,
				"Spotify connection supervisor waiting before reconnect; see stage diagnostic for cause");
			if tokio::time::Instant::now().checked_add(delay).is_none() {
				tokio::select! { _ = shutdown.changed() => break, _ = lifecycle.changed() => {} }
				continue;
			}
			tokio::select! {
				_ = shutdown.changed() => break,
				_ = lifecycle.changed() => {},
				() = tokio::time::sleep(delay) => {},
			}
		}
	}

	async fn apply_change(&self, generation: u64, sequence: u64, id: &str, saved: bool) {
		let state = self.inner.state.read().await;
		if generation != state.generation {
			return;
		}
		let track = self.inner.registry.lock().await.tracks.get(id).cloned();
		let Some(track) = track else {
			return;
		};
		let _observation = track.observation.lock().await;
		if track.retiring.load(Ordering::Acquire)
			|| !track.update_saved(generation, sequence, saved).await
		{
			return;
		}
		if let Ok(path) = track_path(id)
			&& let Ok(emitter) = SignalEmitter::new(&self.inner.connection, path)
		{
			let _ = track.emit_saved_changed(&emitter).await;
		}
	}

	async fn poll_observed_tracks(&self) {
		if !self.inner.state.read().await.authenticated {
			return;
		}
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
			self.apply_change(result.generation, result.sequence, &track_id, saved)
				.await;
		}
	}

	async fn release_client(&self, client: &UniqueName<'_>) {
		self.cancel_pairing_for(client.as_str()).await;
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
		if !config.client_id.is_empty() {
			log::warn!("spotify.client_id is deprecated and ignored; use pairing in snowconf");
		}
		if config.poll_interval_seconds < 60 {
			log::warn!("Legacy Spotify poll interval is clamped to 60 seconds for recovery only");
		}
		let _ = self
			.inner
			.poll_interval_tx
			.send(config.poll_interval_seconds.max(60));
		let generation = {
			let mut state = self.inner.state.write().await;
			let connection_config = config.enabled;
			if state.connection_config.as_ref() == Some(&connection_config) {
				return;
			}
			state.connection_config = Some(connection_config);
			state.generation = state.generation.wrapping_add(1);
			// Keep quota owners across disable/enable; toggling must not clear cooldowns.
			if let Some(client) = &state.client {
				client.retire();
			}
			state.pairing_owner = None;
			state.push_connected = false;
			state.authenticated = false;
			state.status = if config.enabled {
				SpotifyStatus::Connecting
			} else {
				SpotifyStatus::Disabled
			};
			state.generation
		};
		if let Some(task) = self.inner.pairing_task.lock().await.take() {
			task.abort();
		}
		self.inner.push_healthy.store(false, Ordering::Release);
		self.inner.lifecycle.send_replace(generation);
		self.invalidate_tracks().await;
		if !config.enabled {
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

		let existing_auth = self.inner.state.read().await.auth.clone();
		let auth = if let Some(existing) = existing_auth {
			Ok(existing)
		} else {
			auth::Auth::new(store).map(Arc::new)
		};
		let Ok(auth) = auth else {
			self.set_connection_state(generation, None, SpotifyStatus::Unavailable, false)
				.await;
			return;
		};
		// Malformed/obsolete credentials must not prevent the user pairing again.
		let loaded = auth.load().await.unwrap_or(false);
		let previous_api = self.inner.state.read().await.client.clone();
		let api = previous_api.map_or_else(requests::Api::new, |api| Ok(api.replacement()));
		let Ok(api) = api else {
			return;
		};
		{
			let mut state = self.inner.state.write().await;
			if state.generation != generation {
				return;
			}
			state.auth = Some(auth);
			state.client = Some(Arc::new(api));
			state.authenticated = loaded;
			state.status = if loaded {
				SpotifyStatus::Connecting
			} else {
				SpotifyStatus::AuthenticationRequired
			};
		}
		self.inner.lifecycle.send_replace(generation);
	}

	async fn cancel_pairing_for(&self, owner: &str) {
		let mut task = self.inner.pairing_task.lock().await;
		let mut state = self.inner.state.write().await;
		if state.pairing_owner.as_deref() != Some(owner) {
			return;
		}
		state.pairing_owner = None;
		state.generation = state.generation.wrapping_add(1);
		if let Some(task) = task.take() {
			task.abort();
		}
		if !state.authenticated {
			state.status = SpotifyStatus::AuthenticationRequired;
		}
		self.inner.lifecycle.send_replace(state.generation);
	}

	async fn invalidate_tracks(&self) {
		let tracks: Vec<_> = self
			.inner
			.registry
			.lock()
			.await
			.tracks
			.values()
			.cloned()
			.collect();
		for track in tracks {
			track.saved.write().await.sequence = 0;
			if let Ok(path) = track_path(&track.track_id)
				&& let Ok(emitter) = SignalEmitter::new(&self.inner.connection, path)
			{
				let _ = track.emit_saved_changed(&emitter).await;
			}
		}
	}

	async fn set_connection_state(
		&self, generation: u64, client: Option<Arc<requests::Api>>, status: SpotifyStatus,
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

	async fn retry_at(&self) -> u64 {
		self.inner.state.read().await.retry_at
	}

	#[allow(clippy::too_many_lines)] // Keep generation validation and pairing commit together.
	async fn begin_pairing(&self, header: Header<'_>) -> zbus::fdo::Result<SpotifyPairing> {
		let caller = Self::caller(&header)?.to_string();
		let mut task_slot = self.inner.pairing_task.lock().await;
		if self
			.inner
			.state
			.read()
			.await
			.pairing_owner
			.as_ref()
			.is_some_and(|owner| owner != &caller)
		{
			return Err(zbus::fdo::Error::Failed(
				"A different settings client is already pairing Spotify".into(),
			));
		}
		if let Some(task) = task_slot.take() {
			task.abort();
		}
		let (auth, generation) = {
			let mut state = self.inner.state.write().await;
			if state.connection_config != Some(true) {
				return Err(zbus::fdo::Error::Failed(
					"Enable Spotify before pairing".into(),
				));
			}
			let previous_auth = state.auth.clone().ok_or_else(|| {
				zbus::fdo::Error::Failed("Spotify credential store is unavailable".into())
			})?;
			// An in-flight operation for the previous account must never acquire the
			// newly paired account's token. Share budgets, not credential ownership.
			let auth = Arc::new(previous_auth.fresh_identity());
			if let Some(api) = state.client.as_ref() {
				state.client = Some(Arc::new(api.replacement()));
			}
			state.auth = Some(auth.clone());
			state.pairing_owner = Some(caller);
			state.push_connected = false;
			state.generation = state.generation.wrapping_add(1);
			state.status = SpotifyStatus::Authorizing;
			state.authenticated = false;
			(auth, state.generation)
		};
		self.inner.lifecycle.send_replace(generation);
		self.invalidate_tracks().await;
		let grant = match auth.begin_pairing().await {
			Ok(grant) => grant,
			Err(error) => {
				self.inner.state.write().await.pairing_owner = None;
				set_failure(&self.inner.state, generation, &error).await;
				return Err(zbus::fdo::Error::Failed(error.to_string()));
			}
		};
		let public = grant
			.public()
			.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
		let expires_at = public.expires_at;
		let service = self.clone();
		*task_slot = Some(tokio::spawn(async move {
			let mut interval = grant.interval.max(1);
			loop {
				if unix_seconds() >= expires_at {
					set_failure(
						&service.inner.state,
						generation,
						&Failure::AuthenticationRequired,
					)
					.await;
					break;
				}
				tokio::time::sleep(Duration::from_secs(
					interval.min(expires_at.saturating_sub(unix_seconds())),
				))
				.await;
				match auth.poll_pairing(&grant).await {
					Ok(auth::Approval::Pending) => {}
					Ok(auth::Approval::SlowDown) => interval = interval.saturating_add(5),
					Ok(auth::Approval::Authorized(token)) => {
						let mut state = service.inner.state.write().await;
						if state.generation != generation
							|| state.connection_config != Some(true)
							|| unix_seconds() >= expires_at
						{
							break;
						}
						match auth.accept(token).await {
							Ok(()) => {
								state.authenticated = true;
								state.status = SpotifyStatus::Connecting;
								state.retry_at = 0;
							}
							Err(_) => state.status = SpotifyStatus::Unavailable,
						}
						state.pairing_owner = None;
						drop(state);
						service.inner.lifecycle.send_replace(generation);
						break;
					}
					Err(Failure::RateLimited(delay)) => {
						set_failure(
							&service.inner.state,
							generation,
							&Failure::RateLimited(delay),
						)
						.await;
						interval = interval.max(delay.as_secs().saturating_add(1));
					}
					Err(Failure::Unavailable) => interval = interval.saturating_mul(2).min(60),
					Err(error) => {
						set_failure(&service.inner.state, generation, &error).await;
						break;
					}
				}
			}
			let mut state = service.inner.state.write().await;
			if state.generation == generation {
				state.pairing_owner = None;
			}
		}));
		Ok(public)
	}

	async fn cancel_pairing(&self, header: Header<'_>) -> zbus::fdo::Result<()> {
		self.cancel_pairing_for(Self::caller(&header)?.as_str())
			.await;
		Ok(())
	}

	async fn get_track(&self, track_id: &str) -> zbus::fdo::Result<OwnedObjectPath> {
		if !protocol::valid_track_id(track_id) {
			return Err(zbus::fdo::Error::InvalidArgs(
				"Invalid Spotify track ID".into(),
			));
		}
		let track_id = track_id.to_owned();
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
				generation: 0,
			})),
			observation: Arc::new(Mutex::new(())),
			initialization: Arc::new(Mutex::new(())),
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

	async fn update_saved(&self, generation: u64, sequence: u64, saved: bool) -> bool {
		let mut cached = self.saved.write().await;
		cached.apply(generation, sequence, saved)
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

	async fn saved(&self) -> zbus::fdo::Result<bool> {
		let service = self.service()?;
		let state = service.inner.state.read().await;
		let cached = self.saved.read().await;
		if cached.sequence == 0 || cached.generation != state.generation {
			return Err(zbus::fdo::Error::Failed(
				"Spotify track state is not known yet".into(),
			));
		}
		Ok(cached.value)
	}

	async fn observe(
		&self, header: Header<'_>, emitter: SignalEmitter<'_>,
	) -> zbus::fdo::Result<bool> {
		let service = self.service()?;
		let _initialization = self.initialization.lock().await;
		let generation = service.inner.state.read().await.generation;
		let caller = SpotifyDbus::caller(&header)?;
		let observation = self.observation.lock().await;
		if self.retiring.load(Ordering::Acquire) {
			return Err(zbus::fdo::Error::Failed(
				"Spotify track object is being released".to_string(),
			));
		}
		let _added = service
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
		{
			let cached = self.saved.read().await;
			if cached.sequence > 0 && cached.generation == generation {
				return Ok(cached.value);
			}
		}
		drop(observation);

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
				// The value is unknown, but observation is still useful: Dealer can
				// establish it during an HTTP cooldown. Retain it until Unobserve or
				// caller disconnect rather than evicting a subscribed track.
				return Err(zbus::fdo::Error::Failed(error));
			}
		};
		let saved = result.value.into_iter().next().ok_or_else(|| {
			zbus::fdo::Error::Failed("Spotify returned no track state".to_string())
		})?;
		let _ = emitter;
		service
			.apply_change(result.generation, result.sequence, &self.track_id, saved)
			.await;
		self.saved().await
	}

	async fn unobserve(&self, header: Header<'_>) -> zbus::fdo::Result<()> {
		let service = self.service()?;
		let caller = SpotifyDbus::caller(&header)?;
		let _observation = self.observation.lock().await;
		service.remove_observation(caller, &self.track_id).await;
		Ok(())
	}

	async fn refresh(&self, emitter: SignalEmitter<'_>) -> zbus::fdo::Result<bool> {
		let result = self
			.scheduler
			.contains(vec![self.track_id.clone()])
			.await
			.map_err(zbus::fdo::Error::Failed)?;
		let saved = result.value.into_iter().next().ok_or_else(|| {
			zbus::fdo::Error::Failed("Spotify returned no track state".to_string())
		})?;
		let _ = emitter;
		self.service()?
			.apply_change(result.generation, result.sequence, &self.track_id, saved)
			.await;
		self.saved().await
	}

	async fn set_saved(&self, saved: bool, emitter: SignalEmitter<'_>) -> zbus::fdo::Result<bool> {
		let result = match self.scheduler.set_saved(self.track_id.clone(), saved).await {
			Ok(result) => result,
			Err(error) => {
				self.service()?.inner.reconcile.notify_one();
				return Err(zbus::fdo::Error::Failed(error));
			}
		};
		let _ = emitter;
		self.service()?
			.apply_change(
				result.generation,
				result.sequence,
				&self.track_id,
				result.value,
			)
			.await;
		self.saved().await
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

#[allow(clippy::too_many_lines)] // Ordered actor dispatch; reads cannot jump across mutations.
async fn run_scheduler(
	state: Arc<RwLock<SpotifyState>>, mut requests: mpsc::Receiver<SchedulerRequest>,
	mut shutdown: watch::Receiver<bool>, sequence: Arc<AtomicU64>,
) {
	let mut next_request = tokio::time::Instant::now();
	let mut pending = std::collections::VecDeque::new();
	loop {
		let request = if let Some(request) = pending.pop_front() {
			request
		} else {
			tokio::select! {
				biased;
				_ = shutdown.changed() => break,
				request = requests.recv() => match request { Some(request) => request, None => break },
			}
		};
		if let Err(error) = scheduler_request_ready(&state, &request).await {
			reject_scheduler_request(request, error);
			continue;
		}
		tokio::select! {
			_ = shutdown.changed() => break,
			() = tokio::time::sleep_until(next_request.max(tokio::time::Instant::now() + Duration::from_millis(25))) => {},
		}
		next_request = tokio::time::Instant::now() + MIN_REQUEST_SPACING;
		let sequence = sequence.fetch_add(1, Ordering::AcqRel);
		if let SchedulerRequest::Contains {
			track_ids,
			generation,
			reply,
		} = request
		{
			let mut batch = vec![(track_ids.clone(), reply)];
			let mut ids = track_ids;
			while let Ok(request) = requests.try_recv() {
				match request {
					SchedulerRequest::Contains {
						track_ids,
						generation: next_generation,
						reply,
					} if next_generation == generation => {
						let mut union = ids.clone();
						for id in &track_ids {
							if !union.contains(id) {
								union.push(id.clone());
							}
						}
						if union.len() > MAX_OBSERVERS {
							pending.push_back(SchedulerRequest::Contains {
								track_ids,
								generation: next_generation,
								reply,
							});
							break;
						}
						ids = union;
						batch.push((track_ids, reply));
					}
					other => {
						pending.push_back(other);
						break;
					}
				}
			}
			let client = {
				let current = state.read().await;
				if current.generation == generation && current.authenticated {
					current.client.clone()
				} else {
					None
				}
			};
			let result = if let Some(client) = client {
				client.contains(&ids).await
			} else {
				Err(Failure::AuthenticationRequired)
			};
			match result {
				Ok(values) => {
					mark_recovered(&state, generation).await;
					let values: HashMap<_, _> = ids.into_iter().zip(values).collect();
					for (ids, reply) in batch {
						let value = ids
							.iter()
							.filter_map(|id| values.get(id).copied())
							.collect();
						reply_if_current(
							&state,
							generation,
							reply,
							Scheduled {
								sequence,
								generation,
								value,
							},
						)
						.await;
					}
				}
				Err(error) => {
					set_failure(&state, generation, &error).await;
					for (_, reply) in batch {
						let _ = reply.send(Err(error.to_string()));
					}
				}
			}
		} else {
			tokio::select! {
				_ = shutdown.changed() => break,
				_ = handle_scheduler_request(&state,request,sequence) => {},
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
	if let Some(client) = &state.client {
		client.ready().await.map_err(|error| error.to_string())?;
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
	state: &RwLock<SpotifyState>, client: Arc<requests::Api>, track_ids: Vec<String>,
	generation: u64, sequence: u64, reply: oneshot::Sender<Result<Scheduled<Vec<bool>>, String>>,
) -> Option<Duration> {
	let result = client.contains(&track_ids).await;
	match result {
		Ok(value) => {
			reply_if_current(
				state,
				generation,
				reply,
				Scheduled {
					sequence,
					generation,
					value,
				},
			)
			.await;
			None
		}
		Err(error) => handle_api_error(state, generation, reply, error).await,
	}
}

async fn handle_set_saved_request(
	state: &RwLock<SpotifyState>, client: Arc<requests::Api>, track_id: String, saved: bool,
	generation: u64, sequence: u64, reply: oneshot::Sender<Result<Scheduled<bool>, String>>,
) -> Option<Duration> {
	let result = client.set_saved(track_id, saved).await;
	match result {
		Ok(_) => {
			mark_recovered(state, generation).await;
			reply_if_current(
				state,
				generation,
				reply,
				Scheduled {
					sequence,
					generation,
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
	reply: oneshot::Sender<Result<Scheduled<T>, String>>, error: Failure,
) -> Option<Duration> {
	set_failure(state, generation, &error).await;
	let _ = reply.send(Err(error.to_string()));
	None
}

fn unix_seconds() -> u64 {
	u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0)
}

fn reconciliation_interval(push_healthy: bool, recovery_seconds: u64) -> Duration {
	Duration::from_secs(if push_healthy {
		900
	} else {
		recovery_seconds.max(60)
	})
}

struct ConnectionTask(JoinHandle<Result<(), Failure>>);
impl Drop for ConnectionTask {
	fn drop(&mut self) {
		self.0.abort();
	}
}

async fn mark_recovered(state: &RwLock<SpotifyState>, generation: u64) {
	let mut state = state.write().await;
	if state.generation == generation && state.authenticated && state.retry_at <= unix_seconds() {
		state.retry_at = 0;
		state.status = if state.push_connected {
			SpotifyStatus::Ready
		} else {
			SpotifyStatus::Connecting
		};
	}
}

async fn set_failure(state: &RwLock<SpotifyState>, generation: u64, failure: &Failure) {
	let mut state = state.write().await;
	if state.generation != generation {
		return;
	}
	match failure {
		Failure::AuthenticationRequired => {
			state.authenticated = false;
			state.status = SpotifyStatus::AuthenticationRequired;
		}
		Failure::RateLimited(delay) => {
			state.retry_at = state.retry_at.max(
				unix_seconds()
					.saturating_add(delay.as_secs())
					.saturating_add(1),
			);
			state.status = SpotifyStatus::RateLimited;
		}
		Failure::SessionThrottled => {
			state.status = SpotifyStatus::RateLimited;
			state.retry_at = 0;
		}
		Failure::Forbidden | Failure::InvalidResponse => state.status = SpotifyStatus::Error,
		Failure::Unavailable => state.status = SpotifyStatus::Unavailable,
	}
}

fn track_path(track_id: &str) -> zbus::Result<OwnedObjectPath> {
	if !protocol::valid_track_id(track_id) {
		return Err(zbus::Error::Failure("Invalid Spotify track ID".into()));
	}
	OwnedObjectPath::try_from(format!("{TRACK_OBJECT_PATH_PREFIX}/{track_id}"))
		.map_err(zbus::Error::from)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn initial_cooldown_retains_observation_and_push_establishes_state_without_http() {
		use daemon_common::SpotifyTrackProxy;
		use futures::StreamExt;
		use std::{
			io::BufRead,
			process::{Command, Stdio},
		};
		use tokio::io::{AsyncReadExt, AsyncWriteExt};
		struct PrivateBus(std::process::Child);
		impl Drop for PrivateBus {
			fn drop(&mut self) {
				let _ = self.0.kill();
				let _ = self.0.wait();
			}
		}
		// A private bus, never the user's real session bus. No AP/Dealer workers.
		let mut bus = PrivateBus(
			Command::new("dbus-daemon")
				.args(["--session", "--nofork", "--print-address=1"])
				.stdout(Stdio::piped())
				.stderr(Stdio::null())
				.spawn()
				.expect("private D-Bus regression requires dbus-daemon"),
		);
		let mut address = String::new();
		std::io::BufReader::new(bus.0.stdout.take().unwrap())
			.read_line(&mut address)
			.unwrap();
		let server = zbus::connection::Builder::address(address.trim())
			.unwrap()
			.name(daemon_common::DEFAULT_BUS_NAME.clone())
			.unwrap()
			.build()
			.await
			.unwrap();
		let client = zbus::connection::Builder::address(address.trim())
			.unwrap()
			.build()
			.await
			.unwrap();
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let api = Arc::new(requests::Api::test_with_base(format!(
			"http://{}",
			listener.local_addr().unwrap()
		)));
		let http = tokio::spawn(async move {
			let (mut socket, _) = listener.accept().await.unwrap();
			let mut bytes = [0; 8192];
			socket.read(&mut bytes).await.unwrap();
			socket.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7200\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
			assert!(
				tokio::time::timeout(Duration::from_millis(150), listener.accept())
					.await
					.is_err()
			);
		});
		let (service, rx, _interval) = SpotifyDbus::new(server.clone(), 60);
		{
			let mut state = service.inner.state.write().await;
			state.client = Some(api.clone());
			state.authenticated = true;
			state.generation = 1;
		}
		let (stop, shutdown) = watch::channel(false);
		let worker = tokio::spawn(run_scheduler(
			service.inner.state.clone(),
			rx,
			shutdown,
			service.inner.sequence.clone(),
		));
		let id = "3uADmRFuyQBoCwUn19B6lV";
		let track = SpotifyTrackDbus::new(
			id.into(),
			service.inner.scheduler.clone(),
			Arc::downgrade(&service.inner),
		);
		service
			.inner
			.registry
			.lock()
			.await
			.tracks
			.insert(id.into(), track.clone());
		let path = track_path(id).unwrap();
		server
			.object_server()
			.at(path.clone(), SpotifyTrackServer(track))
			.await
			.unwrap();
		let proxy = SpotifyTrackProxy::builder(&client)
			.path(path)
			.unwrap()
			.cache_properties(zbus::proxy::CacheProperties::Lazily)
			.build()
			.await
			.unwrap();
		let mut changes = proxy.receive_saved_changed().await;
		assert!(proxy.observe().await.is_err());
		assert!(
			service
				.inner
				.registry
				.lock()
				.await
				.observed
				.is_observed(&id.to_owned())
		);
		assert!(matches!(api.ready().await, Err(Failure::RateLimited(_))));
		service.apply_change(1, 2, id, true).await;
		let change = tokio::time::timeout(Duration::from_secs(3), changes.next())
			.await
			.unwrap()
			.unwrap();
		assert!(change.get().await.unwrap());
		assert!(matches!(api.ready().await, Err(Failure::RateLimited(_))));
		proxy.unobserve().await.unwrap();
		assert!(
			!service
				.inner
				.registry
				.lock()
				.await
				.observed
				.is_observed(&id.to_owned())
		);
		http.await.unwrap();
		stop.send(true).unwrap();
		worker.await.unwrap();
	}

	fn mock_state(api: Arc<requests::Api>) -> Arc<RwLock<SpotifyState>> {
		Arc::new(RwLock::new(SpotifyState {
			store: None,
			client: Some(api),
			auth: None,
			status: SpotifyStatus::Ready,
			authenticated: true,
			generation: 1,
			connection_config: Some(true),
			retry_at: 0,
			pairing_owner: None,
			push_connected: true,
		}))
	}

	#[tokio::test]
	async fn simultaneous_initial_observations_are_one_batched_http_request() {
		use tokio::io::{AsyncReadExt, AsyncWriteExt};
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let base = format!("http://{}", listener.local_addr().unwrap());
		let server = tokio::spawn(async move {
			let (mut socket, _) = listener.accept().await.unwrap();
			let mut data = Vec::new();
			loop {
				let mut bytes = [0; 1024];
				let n = socket.read(&mut bytes).await.unwrap();
				assert_ne!(n, 0);
				data.extend_from_slice(&bytes[..n]);
				if let Some(end) = data.windows(4).position(|chunk| chunk == b"\r\n\r\n") {
					let head = String::from_utf8_lossy(&data[..end]);
					let len: usize = head
						.lines()
						.find_map(|line| {
							line.to_ascii_lowercase()
								.strip_prefix("content-length: ")
								.map(str::to_owned)
						})
						.unwrap()
						.parse()
						.unwrap();
					if data.len() >= end + 4 + len {
						break;
					}
				}
			}
			let text = String::from_utf8(data).unwrap();
			assert!(text.contains("3uADmRFuyQBoCwUn19B6lV"));
			assert!(text.contains("4iV5W9uYEdYUVa79Axb7Rh"));
			assert!(text.starts_with("POST /collection/v2/contains "));
			let body = [10, 2, 1, 0];
			socket
				.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n")
				.await
				.unwrap();
			socket.write_all(&body).await.unwrap();
		});
		let api = Arc::new(requests::Api::test_with_base(base));
		let state = mock_state(api);
		let (tx, rx) = mpsc::channel(64);
		let (stop, shutdown) = watch::channel(false);
		let worker = tokio::spawn(run_scheduler(
			state,
			rx,
			shutdown,
			Arc::new(AtomicU64::new(1)),
		));
		let (a, first) = oneshot::channel();
		let (b, second) = oneshot::channel();
		tx.send(SchedulerRequest::Contains {
			track_ids: vec!["3uADmRFuyQBoCwUn19B6lV".into()],
			generation: 1,
			reply: a,
		})
		.await
		.unwrap();
		tx.send(SchedulerRequest::Contains {
			track_ids: vec!["4iV5W9uYEdYUVa79Axb7Rh".into()],
			generation: 1,
			reply: b,
		})
		.await
		.unwrap();
		assert_eq!(first.await.unwrap().unwrap().value, [true]);
		assert_eq!(second.await.unwrap().unwrap().value, [false]);
		server.await.unwrap();
		stop.send(true).unwrap();
		worker.await.unwrap();
	}

	#[tokio::test(start_paused = true)]
	async fn generation_is_rechecked_after_pacing_before_any_http_request() {
		let api = Arc::new(requests::Api::test_with_base("http://127.0.0.1:9".into()));
		let state = mock_state(api);
		let (tx, rx) = mpsc::channel(64);
		let (stop, shutdown) = watch::channel(false);
		let worker = tokio::spawn(run_scheduler(
			state.clone(),
			rx,
			shutdown,
			Arc::new(AtomicU64::new(1)),
		));
		let (reply, response) = oneshot::channel();
		tx.send(SchedulerRequest::Contains {
			track_ids: vec!["3uADmRFuyQBoCwUn19B6lV".into()],
			generation: 1,
			reply,
		})
		.await
		.unwrap();
		tokio::task::yield_now().await;
		state.write().await.generation = 2;
		tokio::time::advance(Duration::from_secs(1)).await;
		let error = response.await.unwrap().err().unwrap();
		assert_eq!(error, Failure::AuthenticationRequired.to_string()); // not a loopback transport error
		stop.send(true).unwrap();
		worker.await.unwrap();
	}

	#[test]
	fn healthy_push_never_uses_the_legacy_fast_poll_interval() {
		assert_eq!(reconciliation_interval(true, 5), Duration::from_secs(900));
		assert_eq!(reconciliation_interval(false, 5), Duration::from_secs(60));
		assert_eq!(
			reconciliation_interval(false, 300),
			Duration::from_secs(300)
		);
	}

	#[tokio::test]
	async fn cooldown_and_transient_failures_preserve_pairing_and_recover() {
		let state = RwLock::new(SpotifyState {
			store: None,
			client: None,
			auth: None,
			status: SpotifyStatus::Ready,
			authenticated: true,
			generation: 7,
			connection_config: Some(true),
			retry_at: 0,
			pairing_owner: None,
			push_connected: true,
		});
		set_failure(&state, 7, &Failure::RateLimited(Duration::from_secs(3600))).await;
		assert!(state.read().await.authenticated);
		mark_recovered(&state, 7).await;
		assert_eq!(state.read().await.status, SpotifyStatus::RateLimited);
		state.write().await.retry_at = unix_seconds().saturating_sub(1);
		mark_recovered(&state, 7).await;
		assert_eq!(state.read().await.status, SpotifyStatus::Ready);
		set_failure(&state, 7, &Failure::Unavailable).await;
		assert!(state.read().await.authenticated);
		set_failure(&state, 6, &Failure::AuthenticationRequired).await;
		assert!(state.read().await.authenticated); // old generation cannot revoke a new grant
	}

	#[tokio::test]
	async fn websocket_changes_reach_the_same_dbus_property_stream_used_by_the_bar() {
		use daemon_common::SpotifyTrackProxy;
		use futures::StreamExt;
		let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
		let guid = zbus::Guid::generate();
		let server = zbus::connection::Builder::unix_stream(server_socket)
			.p2p()
			.server(guid)
			.unwrap()
			.build();
		let client = zbus::connection::Builder::unix_stream(client_socket)
			.p2p()
			.build();
		let (server, client) = tokio::join!(server, client);
		let server = server.unwrap();
		let client = client.unwrap();
		// Deliberately do NOT start network workers or install an HTTP/auth client.
		let (service, _requests, _interval) = SpotifyDbus::new(server.clone(), 60);
		service.inner.state.write().await.generation = 1;
		let id = "3uADmRFuyQBoCwUn19B6lV";
		let track = SpotifyTrackDbus::new(
			id.into(),
			service.inner.scheduler.clone(),
			Arc::downgrade(&service.inner),
		);
		service
			.inner
			.registry
			.lock()
			.await
			.tracks
			.insert(id.into(), track.clone());
		let path = track_path(id).unwrap();
		server
			.object_server()
			.at(path.clone(), SpotifyTrackServer(track))
			.await
			.unwrap();
		let proxy = SpotifyTrackProxy::builder(&client)
			.path(path)
			.unwrap()
			.cache_properties(zbus::proxy::CacheProperties::Lazily)
			.build()
			.await
			.unwrap();
		assert!(proxy.saved().await.is_err()); // unknown must never be false
		let mut changes = proxy.receive_saved_changed().await;
		for (sequence, saved) in [(2, true), (3, false)] {
			let value: serde_json::Value = serde_json::json!({"uri":"hm://collection/collection/user/json","payloads":[{"items":[{"type":"track","identifier":id,"removed":!saved}]}]});
			let protocol::Event::Collection { changes: items, .. } =
				protocol::decode(&value, "user")
			else {
				panic!("Expected collection event");
			};
			for item in items {
				service
					.apply_change(1, sequence, &item.id, item.saved)
					.await;
			}
			let change = tokio::time::timeout(Duration::from_secs(3), changes.next())
				.await
				.unwrap()
				.unwrap();
			assert_eq!(change.get().await.unwrap(), saved);
		}
	}

	#[test]
	fn push_cannot_be_overwritten_by_an_older_http_read_or_write_completion() {
		let mut cache = CachedSaved {
			value: false,
			sequence: 0,
			generation: 1,
		};
		assert!(cache.apply(1, 2, true));
		assert!(!cache.apply(1, 1, false));
		assert!(cache.value);
		assert!(!cache.apply(1, 3, true)); // duplicate remote confirmation emits no signal
		assert!(cache.apply(1, 4, false));
		assert!(!cache.apply(1, 3, true));
		assert!(!cache.value);
	}

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
}
