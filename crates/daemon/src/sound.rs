//! Notification sound playback.
//!
//! Sounds are resolved and decoded off the notification path, then played natively through
//! PipeWire from a dedicated worker thread; `pipewire-pulse` is not required. Every failure is
//! logged and dropped; playback never affects notification delivery.

use std::{
	cell::{Cell, RefCell},
	collections::{HashMap, HashSet, VecDeque},
	fs::File,
	io::{Cursor, Read},
	ops::Range,
	path::{Path, PathBuf},
	rc::Rc,
	sync::{
		Arc, Mutex, RwLock,
		atomic::{AtomicBool, Ordering},
	},
	thread::JoinHandle,
	time::{Duration, Instant},
};

use pipewire::{
	self as pw,
	context::ContextRc,
	core::CoreRc,
	loop_::Timeout,
	main_loop::MainLoopRc,
	properties::properties,
	spa::{
		param::audio::{AudioFormat, AudioInfoRaw},
		pod::{Object, Pod, Value, serialize::PodSerializer},
		sys as spa_sys,
		utils::Direction,
	},
	stream::{StreamFlags, StreamRc, StreamState},
};
use symphonia::core::{
	audio::{Channels, SampleBuffer},
	codecs::DecoderOptions,
	errors::Error as DecodeError,
	formats::FormatOptions,
	io::{MediaSourceStream, MediaSourceStreamOptions},
	meta::MetadataOptions,
	probe::Hint,
};
use tokio::{
	sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch},
	task::JoinSet,
};

const QUEUE_CAPACITY: usize = 8;
const MAX_CONCURRENT: usize = 4;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_DURATION_SECS: u64 = 10;
const MAX_PCM_BYTES: usize = 16 * 1024 * 1024;
const MAX_SAMPLE_RATE: u32 = 384_000;
const MAX_CHANNELS: usize = 32;
#[cfg(test)]
const MAX_NAME_LEN: usize = config::notifications::MAX_SOUND_COMPONENT_LEN;
const MAX_INI_BYTES: u64 = 64 * 1024;
const MAX_INHERIT_DEPTH: usize = 8;
const MAX_THEMES: usize = 64;
const PLAYBACK_TIMEOUT: Duration = Duration::from_secs(MAX_DURATION_SECS + 10);
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_THEME_DIRECTORIES: usize = 32;
const SOUND_EXTENSIONS: [&str; 3] = ["oga", "ogg", "wav"];
const APPLICATION: &str = "subniri notifications";
const NODE_NAME: &str = "subniri-notification-sound";
const EVENT_ID: &str = "event.id";
const SAMPLE_BYTES: usize = std::mem::size_of::<f32>();
/// Consecutive process cycles without progress after which a playback is considered stalled.
const MAX_IDLE_CYCLES: u32 = 64;
/// Linux `EPIPE`: the server closed the connection.
const EPIPE: i32 = 32;
/// Alignment for serialized SPA pods, whose headers are read in place.
const POD_ALIGN: usize = 8;

/// Decoded samples are native-endian `f32`.
#[cfg(target_endian = "little")]
const SAMPLE_FORMAT: AudioFormat = AudioFormat::F32LE;
#[cfg(target_endian = "big")]
const SAMPLE_FORMAT: AudioFormat = AudioFormat::F32BE;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SoundSettings {
	enabled: bool,
	theme: Option<String>,
}

impl From<&config::Notifications> for SoundSettings {
	fn from(config: &config::Notifications) -> Self {
		Self {
			enabled: config.sound,
			theme: config.sound_theme.clone(),
		}
	}
}

/// Sound hints extracted from a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundRequest {
	file: Option<PathBuf>,
	name: Option<String>,
}

impl SoundRequest {
	/// Extracts sound hints. Returns `None` when sound is suppressed or no usable hint exists.
	#[must_use]
	pub fn from_hints(hints: &HashMap<String, zvariant::OwnedValue>) -> Option<Self> {
		let string_hint = |key: &str| {
			hints
				.get(key)
				.and_then(|value| <&str>::try_from(value).ok())
		};

		let suppress = hints
			.get("suppress-sound")
			.and_then(|value| bool::try_from(value).ok())
			.unwrap_or(false);
		if suppress {
			return None;
		}

		let file = string_hint("sound-file").and_then(parse_sound_file);
		let name = string_hint("sound-name")
			.filter(|name| is_valid_sound_name(name))
			.map(str::to_owned);

		(file.is_some() || name.is_some()).then_some(Self { file, name })
	}
}

struct Job {
	request: SoundRequest,
	theme: Option<String>,
}

/// Layout of decoded interleaved native-endian `f32` samples.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PcmFormat {
	rate: u32,
	positions: Vec<u32>,
}

impl PcmFormat {
	fn frame_size(&self) -> usize {
		self.positions.len() * SAMPLE_BYTES
	}

	fn audio_info(&self) -> AudioInfoRaw {
		let mut position =
			[spa_sys::SPA_AUDIO_CHANNEL_UNKNOWN; pw::spa::param::audio::MAX_CHANNELS];
		for (slot, channel) in position.iter_mut().zip(&self.positions) {
			*slot = *channel;
		}

		let mut info = AudioInfoRaw::new();
		info.set_format(SAMPLE_FORMAT);
		info.set_rate(self.rate);
		info.set_channels(u32::try_from(self.positions.len()).unwrap_or(0));
		info.set_position(position);
		info
	}
}

struct Decoded {
	pcm: Vec<u8>,
	format: PcmFormat,
	event_id: Option<String>,
}

struct Shared {
	requests: Option<mpsc::Sender<Job>>,
	settings: RwLock<SoundSettings>,
	last_warning: Mutex<Option<String>>,
}

/// Cheap handle used by the notification service to request sounds.
#[derive(Clone)]
pub struct SoundPlayer {
	shared: Arc<Shared>,
}

impl Default for SoundPlayer {
	/// A player without a worker. Requests are ignored.
	fn default() -> Self {
		Self::with_requests(
			None,
			SoundSettings {
				enabled: false,
				theme: None,
			},
		)
	}
}

/// Owns the sound dispatcher task and worker thread.
pub struct SoundWorker {
	dispatcher: Option<tokio::task::JoinHandle<()>>,
	thread: Option<JoinHandle<()>>,
}

impl SoundWorker {
	/// Waits for the worker to exit after daemon shutdown was signalled.
	pub async fn join(mut self) {
		if let Some(dispatcher) = self.dispatcher.take()
			&& let Err(error) = dispatcher.await
		{
			log::error!("Notification sound dispatcher failed: {error}");
		}

		let Some(thread) = self.thread.take() else {
			return;
		};

		match tokio::task::spawn_blocking(move || thread.join()).await {
			Ok(Ok(())) => {}
			Ok(Err(_)) => log::error!("Notification sound worker panicked"),
			Err(error) => log::error!("Failed to join notification sound worker: {error}"),
		}
	}
}

impl SoundPlayer {
	/// Starts the sound worker. Must be called within a Tokio runtime. Failure to start only
	/// disables sounds.
	pub fn spawn(
		config: &config::Notifications, shutdown: watch::Receiver<bool>,
	) -> (Self, SoundWorker) {
		let settings = SoundSettings::from(config);
		let (requests_tx, requests_rx) = mpsc::channel(QUEUE_CAPACITY);
		let (commands_tx, commands_rx) = pw::channel::channel();
		let alive = Arc::new(AtomicBool::new(true));

		let thread = std::thread::Builder::new()
			.name("notification-sound".into())
			.spawn({
				let alive = WorkerAlive(alive.clone());
				move || run_thread(alive, commands_rx)
			});
		let (requests, dispatcher, thread) = match thread {
			Ok(thread) => {
				let worker = Worker {
					commands: commands_tx,
					alive,
				};
				let dispatcher = tokio::spawn(dispatch(requests_rx, shutdown, worker));
				(Some(requests_tx), Some(dispatcher), Some(thread))
			}
			Err(error) => {
				log::error!("Failed to start notification sound worker: {error}");
				(None, None, None)
			}
		};

		let player = Self::with_requests(requests, settings);
		player.spawn_theme_check();

		(player, SoundWorker { dispatcher, thread })
	}

	fn with_requests(requests: Option<mpsc::Sender<Job>>, settings: SoundSettings) -> Self {
		Self {
			shared: Arc::new(Shared {
				requests,
				settings: RwLock::new(settings),
				last_warning: Mutex::new(None),
			}),
		}
	}

	/// Whether notification sounds are enabled by configuration and the worker is running.
	#[must_use]
	pub fn enabled(&self) -> bool {
		self.settings().enabled
			&& self
				.shared
				.requests
				.as_ref()
				.is_some_and(|requests| !requests.is_closed())
	}

	/// Applies changed notification settings. Must be called within a Tokio runtime.
	pub fn reconfigure(&self, config: &config::Notifications) {
		let settings = SoundSettings::from(config);
		match self.shared.settings.write() {
			Ok(mut current) => *current = settings,
			Err(poisoned) => *poisoned.into_inner() = settings,
		}

		self.spawn_theme_check();
	}

	/// Queues a sound without waiting. Requests are dropped when the queue is full.
	pub fn play(&self, request: SoundRequest) {
		let settings = self.settings();
		if !settings.enabled {
			return;
		}
		let Some(requests) = &self.shared.requests else {
			return;
		};

		let job = Job {
			request,
			theme: settings.theme,
		};
		match requests.try_send(job) {
			Ok(()) => {}
			Err(mpsc::error::TrySendError::Full(_)) => {
				log::debug!("Notification sound queue is full; dropping sound");
			}
			Err(mpsc::error::TrySendError::Closed(_)) => {
				log::debug!("Notification sound worker stopped; dropping sound");
			}
		}
	}

	fn settings(&self) -> SoundSettings {
		match self.shared.settings.read() {
			Ok(settings) => settings.clone(),
			Err(poisoned) => poisoned.into_inner().clone(),
		}
	}

	/// Runs the filesystem-bound theme check on the blocking pool.
	fn spawn_theme_check(&self) {
		let player = self.clone();
		tokio::task::spawn_blocking(move || player.check_theme());
	}

	/// Logs a missing-theme warning once per distinct situation. Blocks on filesystem access.
	fn check_theme(&self) {
		// Hold the lock while checking so concurrent checks serialize and the latest settings win.
		let mut last = match self.shared.last_warning.lock() {
			Ok(last) => last,
			Err(poisoned) => poisoned.into_inner(),
		};
		let warning = theme_warning(&self.settings());

		if let Some(message) = &warning
			&& last.as_ref() != Some(message)
		{
			log::warn!("{message}");
		}
		*last = warning;
	}
}

fn theme_warning(settings: &SoundSettings) -> Option<String> {
	const ADVICE: &str = "Install a sound theme (e.g. sound-theme-freedesktop), set \
	                      services.subniri.settings.notifications.soundThemePackage or \
	                      sound_theme, or set sound=false";

	if !settings.enabled {
		return None;
	}

	let data_dirs = data_dirs();
	match &settings.theme {
		Some(theme) if theme_dirs(&data_dirs, theme).is_empty() => Some(format!(
			"Notification sound theme {theme:?} was not found in any sounds/ data directory. \
			 {ADVICE}."
		)),
		Some(_) => None,
		None if installed_themes(&data_dirs).is_empty() => Some(format!(
			"No notification sound theme is installed in any sounds/ data directory. {ADVICE}."
		)),
		None => None,
	}
}

/// A decoded sound plus the concurrency permit it holds until playback ends.
struct Playable {
	decoded: Decoded,
	_permit: OwnedSemaphorePermit,
}

/// Messages for the PipeWire worker thread.
enum Command {
	Play(Playable),
	Shutdown,
}

/// Clears the worker-alive flag when the worker thread exits, however it exits.
struct WorkerAlive(Arc<AtomicBool>);

impl Drop for WorkerAlive {
	fn drop(&mut self) {
		self.0.store(false, Ordering::Release);
	}
}

/// The dispatcher's handle to the worker thread.
///
/// Sends [`Command::Shutdown`] when dropped, so the worker thread stops on every dispatcher exit.
struct Worker {
	commands: pw::channel::Sender<Command>,
	alive: Arc<AtomicBool>,
}

impl Worker {
	/// `pw::channel` sends keep succeeding after the receiver is gone, so liveness is tracked
	/// separately.
	fn is_alive(&self) -> bool {
		self.alive.load(Ordering::Acquire)
	}
}

impl Drop for Worker {
	fn drop(&mut self) {
		if self.commands.send(Command::Shutdown).is_err() {
			log::debug!("Notification sound worker already stopped");
		}
	}
}

/// Receives requests, decodes them on the blocking pool and forwards them to the worker thread.
///
/// Exits when the worker thread dies, which drops all queued audio and permits and closes the
/// request queue so later sounds are rejected without blocking.
async fn dispatch(
	mut requests: mpsc::Receiver<Job>, mut shutdown: watch::Receiver<bool>, worker: Worker,
) {
	let permits = Arc::new(Semaphore::new(MAX_CONCURRENT));
	let mut permit = None;
	let mut decodes = JoinSet::new();

	loop {
		if *shutdown.borrow() {
			break;
		}
		if !worker.is_alive() {
			log::warn!("Notification sound worker stopped; disabling notification sounds");
			break;
		}

		tokio::select! {
			changed = shutdown.changed() => {
				if changed.is_err() || *shutdown.borrow_and_update() {
					break;
				}
			}
			acquired = permits.clone().acquire_owned(), if permit.is_none() => match acquired {
				Ok(acquired) => permit = Some(acquired),
				Err(_) => break,
			},
			job = requests.recv(), if permit.is_some() => {
				let (Some(job), Some(permit)) = (job, permit.take()) else {
					break;
				};
				decodes.spawn_blocking(move || {
					load(&job).map(|decoded| Playable { decoded, _permit: permit })
				});
			}
			Some(result) = decodes.join_next() => match result {
				Ok(Some(playable)) => {
					if !worker.is_alive() {
						continue;
					}
					if worker.commands.send(Command::Play(playable)).is_err() {
						log::debug!("Notification sound worker stopped; dropping sound");
					}
				}
				Ok(None) => {}
				Err(error) => log::warn!("Notification sound decoder failed: {error}"),
			},
		}
	}

	decodes.abort_all();
	requests.close();
}

/// Runs the PipeWire loop. All PipeWire objects live and die on this thread.
///
/// `_alive` is dropped when this function returns or unwinds, marking the worker dead.
fn run_thread(_alive: WorkerAlive, commands: pw::channel::Receiver<Command>) {
	run_thread_inner(
		_alive,
		commands,
		#[cfg(test)]
		None,
	);
}

fn run_thread_inner(
	_alive: WorkerAlive, commands: pw::channel::Receiver<Command>,
	#[cfg(test)] stats: Option<Arc<TestStats>>,
) {
	pw::init();
	let mainloop = match MainLoopRc::new(None) {
		Ok(mainloop) => mainloop,
		Err(error) => {
			log::error!("Failed to create notification sound loop: {error}");
			return;
		}
	};

	let inbox: Rc<RefCell<VecDeque<Command>>> = Rc::default();
	let receiver = commands.attach(mainloop.loop_(), {
		let inbox = inbox.clone();
		move |command| match inbox.try_borrow_mut() {
			Ok(mut inbox) => inbox.push_back(command),
			Err(_) => log::debug!("Notification sound inbox busy; dropping command"),
		}
	});

	let mut engine = Engine::new(mainloop.clone(), inbox);
	#[cfg(test)]
	{
		engine.stats = stats;
	}
	while engine.tick() {
		let result = mainloop.loop_().iterate(engine.timeout(Instant::now()));
		if result < 0 {
			let error = std::io::Error::from_raw_os_error(-result);
			if error.kind() != std::io::ErrorKind::Interrupted {
				log::error!("Notification sound loop failed: {error}");
				break;
			}
		}
	}

	// Tear down streams and the connection before the loop they are attached to.
	engine.disconnect();
	drop(engine);
	drop(receiver);
}

/// Connection progress, written by core listener callbacks.
#[derive(Default)]
struct ConnectionStatus {
	ready: Cell<bool>,
	failed: Cell<bool>,
}

struct Connection {
	listener: Option<pw::core::Listener>,
	core: CoreRc,
	status: Rc<ConnectionStatus>,
}

impl Drop for Connection {
	/// Unregisters the listener before the core is disconnected by its own drop.
	fn drop(&mut self) {
		self.listener.take();
	}
}

/// State owned by the process callback, which runs on PipeWire's realtime data thread.
struct ProcessData {
	pcm: Vec<u8>,
	offset: usize,
	frame_size: usize,
	flushed: bool,
	idle_cycles: u32,
	stalled: Arc<AtomicBool>,
}

struct Playback {
	process: Option<pw::stream::StreamListener<ProcessData>>,
	events: Option<pw::stream::StreamListener<()>>,
	stream: StreamRc,
	finished: Rc<Cell<bool>>,
	stalled: Arc<AtomicBool>,
	deadline: Instant,
	_permit: OwnedSemaphorePermit,
}

impl Drop for Playback {
	/// Disconnects the stream, then unregisters listeners before the stream itself is destroyed.
	fn drop(&mut self) {
		let _ = self.stream.disconnect();
		self.process.take();
		self.events.take();
	}
}

/// Reconnect backoff: starts at one second, doubles per failure up to a cap, and resets once
/// connected.
#[derive(Debug, Default)]
struct Reconnect {
	retry_at: Option<Instant>,
	backoff: Option<Duration>,
}

impl Reconnect {
	fn is_waiting(&self, now: Instant) -> bool {
		self.retry_at.is_some_and(|retry_at| now < retry_at)
	}

	fn failed(&mut self, now: Instant) {
		let backoff = self
			.backoff
			.map_or(Duration::from_secs(1), |backoff| backoff * 2)
			.min(MAX_RECONNECT_BACKOFF);
		self.backoff = Some(backoff);
		self.retry_at = Some(now + backoff);
	}

	fn connected(&mut self) {
		self.backoff = None;
	}
}

/// Sleeps until the earliest deadline, or indefinitely when none is pending.
fn loop_timeout(deadlines: impl Iterator<Item = Instant>, now: Instant) -> Timeout {
	match deadlines.min() {
		// Round up so the deadline has passed when the loop wakes.
		Some(deadline) => {
			Timeout::Finite(deadline.saturating_duration_since(now) + Duration::from_millis(1))
		}
		None => Timeout::Infinite,
	}
}

#[cfg(test)]
#[derive(Default)]
struct TestStats {
	accepted: std::sync::atomic::AtomicUsize,
	drained: std::sync::atomic::AtomicUsize,
	stream_errors: std::sync::atomic::AtomicUsize,
	stalled: std::sync::atomic::AtomicUsize,
	timed_out: std::sync::atomic::AtomicUsize,
	active_playbacks: std::sync::atomic::AtomicUsize,
	connect_failures: std::sync::atomic::AtomicUsize,
	dropped_backoff: std::sync::atomic::AtomicUsize,
	backoff_secs: std::sync::atomic::AtomicU64,
	notify: tokio::sync::Notify,
}

struct Engine {
	mainloop: MainLoopRc,
	inbox: Rc<RefCell<VecDeque<Command>>>,
	connection: Option<Connection>,
	connect_deadline: Option<Instant>,
	pending: Vec<Playable>,
	playbacks: Vec<Playback>,
	reconnect: Reconnect,
	#[cfg(test)]
	stats: Option<Arc<TestStats>>,
}

impl Engine {
	fn new(mainloop: MainLoopRc, inbox: Rc<RefCell<VecDeque<Command>>>) -> Self {
		Self {
			mainloop,
			inbox,
			connection: None,
			connect_deadline: None,
			pending: Vec::new(),
			playbacks: Vec::new(),
			reconnect: Reconnect::default(),
			#[cfg(test)]
			stats: None,
		}
	}

	/// Advances all state after loop activity. Returns `false` once shutdown was requested.
	fn tick(&mut self) -> bool {
		let commands = match self.inbox.try_borrow_mut() {
			Ok(mut inbox) => std::mem::take(&mut *inbox),
			Err(_) => VecDeque::new(),
		};
		for command in commands {
			match command {
				Command::Play(playable) => self.pending.push(playable),
				Command::Shutdown => return false,
			}
		}

		let now = Instant::now();
		if self.connection.is_none() && !self.pending.is_empty() {
			self.connect(now);
		}
		self.service_connection(now);

		self.playbacks.retain(|playback| {
			if playback.finished.get() {
				return false;
			}
			if playback.stalled.load(Ordering::Relaxed) {
				log::debug!("Notification sound stream stalled");
				#[cfg(test)]
				if let Some(stats) = &self.stats {
					stats.stalled.fetch_add(1, Ordering::Relaxed);
				}
				return false;
			}
			if now >= playback.deadline {
				log::debug!("Notification sound timed out");
				#[cfg(test)]
				if let Some(stats) = &self.stats {
					stats.timed_out.fetch_add(1, Ordering::Relaxed);
				}
				return false;
			}
			true
		});

		#[cfg(test)]
		if let Some(stats) = &self.stats {
			stats
				.active_playbacks
				.store(self.playbacks.len(), Ordering::Release);
			stats.notify.notify_waiters();
		}

		true
	}

	/// How long the loop may sleep before the next deadline must be checked.
	fn timeout(&self, now: Instant) -> Timeout {
		let deadlines = self
			.playbacks
			.iter()
			.map(|playback| playback.deadline)
			.chain(self.connect_deadline);

		loop_timeout(deadlines, now)
	}

	fn service_connection(&mut self, now: Instant) {
		let Some(connection) = &self.connection else {
			return;
		};

		if connection.status.failed.get() {
			log::warn!("Notification sound server connection failed; is PipeWire running?");
			self.reset(now);
		} else if connection.status.ready.get() {
			self.reconnect.connected();
			#[cfg(test)]
			if let Some(stats) = &self.stats {
				stats.backoff_secs.store(0, Ordering::Release);
			}
			self.connect_deadline = None;
			for playable in std::mem::take(&mut self.pending) {
				self.start(playable, now);
			}
		} else if self
			.connect_deadline
			.is_some_and(|deadline| now >= deadline)
		{
			log::warn!("Timed out connecting to notification sound server");
			self.reset(now);
		}
	}

	fn connect(&mut self, now: Instant) {
		if self.reconnect.is_waiting(now) {
			log::debug!("Notification sound server reconnect pending; dropping sound");
			self.pending.clear();
			#[cfg(test)]
			if let Some(stats) = &self.stats {
				stats.dropped_backoff.fetch_add(1, Ordering::Relaxed);
			}
			return;
		}

		match self.open_connection() {
			Ok(connection) => {
				self.connection = Some(connection);
				self.connect_deadline = Some(now + CONNECT_TIMEOUT);
			}
			Err(error) => {
				log::warn!("Failed to connect to notification sound server: {error}");
				self.reset(now);
			}
		}
	}

	/// Connects to the PipeWire server and requests a sync to detect readiness.
	fn open_connection(&self) -> Result<Connection, pw::Error> {
		let context = ContextRc::new(&self.mainloop, None)?;
		let core = context.connect_rc(Some(properties! {
			*pw::keys::APP_NAME => APPLICATION,
		}))?;
		let pending = core.sync(0)?;

		let status = Rc::new(ConnectionStatus::default());
		let listener = core
			.add_listener_local()
			.done({
				let status = status.clone();
				move |id, seq| {
					if id == pw::core::PW_ID_CORE && seq == pending {
						status.ready.set(true);
					}
				}
			})
			// The crate's error trampoline unwraps the message as UTF-8 and would abort on
			// invalid server messages; this is accepted as a crate limitation.
			.error({
				let status = status.clone();
				move |id, _seq, res, message| {
					if is_fatal_core_error(id, res, status.ready.get()) {
						log::debug!("Notification sound server error {res}: {message}");
						status.failed.set(true);
					} else {
						log::debug!("Notification sound server error on {id} ({res}): {message}");
					}
				}
			})
			.register();

		Ok(Connection {
			listener: Some(listener),
			core,
			status,
		})
	}

	/// Tears down all playbacks, then the connection.
	fn disconnect(&mut self) {
		self.playbacks.clear();
		self.pending.clear();
		self.connect_deadline = None;
		self.connection = None;
		#[cfg(test)]
		if let Some(stats) = &self.stats {
			stats.active_playbacks.store(0, Ordering::Release);
			stats.notify.notify_waiters();
		}
	}

	/// Drops the connection and all sounds, then schedules a delayed reconnect.
	fn reset(&mut self, now: Instant) {
		self.disconnect();
		self.reconnect.failed(now);
		#[cfg(test)]
		if let Some(stats) = &self.stats {
			stats.connect_failures.fetch_add(1, Ordering::Relaxed);
			stats.backoff_secs.store(
				self.reconnect
					.backoff
					.map_or(0, |backoff| backoff.as_secs()),
				Ordering::Release,
			);
			stats.notify.notify_waiters();
		}
	}

	fn start(&mut self, playable: Playable, now: Instant) {
		let Some(connection) = &self.connection else {
			return;
		};

		match create_playback(
			&connection.core,
			playable,
			now,
			#[cfg(test)]
			self.stats.clone(),
		) {
			Ok(playback) => {
				self.playbacks.push(playback);
				#[cfg(test)]
				if let Some(stats) = &self.stats {
					stats.accepted.fetch_add(1, Ordering::Relaxed);
				}
			}
			Err(error) => log::warn!("Failed to start notification sound stream: {error}"),
		}
	}
}

/// Creates and connects an output stream that plays `playable` once.
fn create_playback(
	core: &CoreRc, playable: Playable, now: Instant, #[cfg(test)] stats: Option<Arc<TestStats>>,
) -> Result<Playback, PlaybackError> {
	let Playable { decoded, _permit } = playable;

	let mut properties = properties! {
		*pw::keys::MEDIA_TYPE => "Audio",
		*pw::keys::MEDIA_CATEGORY => "Playback",
		*pw::keys::MEDIA_ROLE => "Notification",
		*pw::keys::MEDIA_NAME => "Notification sound",
		*pw::keys::APP_NAME => APPLICATION,
		*pw::keys::NODE_NAME => NODE_NAME,
	};
	// Sound names are validated and never contain NUL, which `insert` would reject by panicking.
	if let Some(event_id) = &decoded.event_id {
		properties.insert(EVENT_ID, event_id.as_str());
	}

	let stream = StreamRc::new(core.clone(), "Notification sound", properties)?;
	let finished = Rc::new(Cell::new(false));

	let events = stream
		.add_local_listener::<()>()
		.state_changed({
			let finished = finished.clone();
			#[cfg(test)]
			let stats = stats.clone();
			move |_stream, (), _old, new| match new {
				StreamState::Error(error) => {
					log::debug!("Notification sound stream failed: {error}");
					#[cfg(test)]
					if let Some(stats) = &stats {
						stats.stream_errors.fetch_add(1, Ordering::Relaxed);
					}
					finished.set(true);
				}
				StreamState::Unconnected => finished.set(true),
				StreamState::Connecting | StreamState::Paused | StreamState::Streaming => {}
			}
		})
		.drained({
			let finished = finished.clone();
			#[cfg(test)]
			let stats = stats;
			move |_stream, ()| {
				#[cfg(test)]
				if let Some(stats) = &stats {
					stats.drained.fetch_add(1, Ordering::Relaxed);
				}
				finished.set(true);
			}
		})
		.register()?;

	let format = decoded.format.audio_info();
	let stalled = Arc::new(AtomicBool::new(false));
	let process = stream
		.add_local_listener_with_user_data(ProcessData {
			frame_size: decoded.format.frame_size(),
			pcm: decoded.pcm,
			offset: 0,
			flushed: false,
			idle_cycles: 0,
			stalled: stalled.clone(),
		})
		.process(process)
		.register()?;

	let playback = Playback {
		process: Some(process),
		events: Some(events),
		stream,
		finished,
		stalled,
		deadline: now + PLAYBACK_TIMEOUT,
		_permit,
	};

	let params = format_param(format)?;
	let pod = params.pod().ok_or(PlaybackError::Format)?;
	playback.stream.connect(
		Direction::Output,
		None,
		StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
		&mut [pod],
	)?;

	Ok(playback)
}

#[derive(Debug)]
enum PlaybackError {
	PipeWire(pw::Error),
	Format,
}

impl From<pw::Error> for PlaybackError {
	fn from(error: pw::Error) -> Self {
		Self::PipeWire(error)
	}
}

impl std::fmt::Display for PlaybackError {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::PipeWire(error) => error.fmt(formatter),
			Self::Format => formatter.write_str("failed to build audio format"),
		}
	}
}

/// Serialized pod bytes stored at an 8-byte aligned offset.
///
/// `Pod::from_bytes` reads the header in place without checking alignment, so the bytes are copied
/// into over-allocated storage at an aligned position instead of relying on `Vec<u8>` alignment.
struct AlignedPod {
	storage: Vec<u8>,
	range: Range<usize>,
}

impl AlignedPod {
	fn new(bytes: &[u8]) -> Option<Self> {
		let mut storage = vec![0; bytes.len() + POD_ALIGN - 1];
		let start = storage.as_ptr().align_offset(POD_ALIGN);
		let range = start..start.checked_add(bytes.len())?;
		storage.get_mut(range.clone())?.copy_from_slice(bytes);

		Some(Self { storage, range })
	}

	fn pod(&self) -> Option<&Pod> {
		Pod::from_bytes(self.storage.get(self.range.clone())?)
	}
}

/// Serializes the stream's `EnumFormat` parameter.
fn format_param(info: AudioInfoRaw) -> Result<AlignedPod, PlaybackError> {
	let value = Value::Object(Object {
		type_: pw::spa::sys::SPA_TYPE_OBJECT_Format,
		id: pw::spa::sys::SPA_PARAM_EnumFormat,
		properties: info.into(),
	});

	let bytes = PodSerializer::serialize(Cursor::new(Vec::new()), &value)
		.map(|(cursor, _)| cursor.into_inner())
		.map_err(|_| PlaybackError::Format)?;

	AlignedPod::new(&bytes).ok_or(PlaybackError::Format)
}

/// Fills the next buffer. Runs on PipeWire's realtime thread: must not block, allocate or panic.
fn process(stream: &pw::stream::Stream, data: &mut ProcessData) {
	if data.offset >= data.pcm.len() {
		// All audio was queued in earlier cycles; ask for a `drained` event once it has played.
		if !data.flushed {
			data.flushed = true;
			let _ = stream.flush(true);
		}
		return;
	}

	let Some(mut buffer) = stream.dequeue_buffer() else {
		return;
	};
	let Some(target) = buffer.datas_mut().first_mut() else {
		return;
	};

	let written = target.data().map_or(0, |slice| {
		let range = next_chunk(data.pcm.len(), data.offset, slice.len(), data.frame_size);
		let length = range.len();
		match (slice.get_mut(..length), data.pcm.get(range)) {
			(Some(target), Some(source)) => {
				target.copy_from_slice(source);
				length
			}
			_ => 0,
		}
	});
	data.offset += written;
	data.idle_cycles = next_idle_cycles(data.idle_cycles, written);
	if data.idle_cycles >= MAX_IDLE_CYCLES {
		data.stalled.store(true, Ordering::Relaxed);
	}

	let chunk = target.chunk_mut();
	*chunk.offset_mut() = 0;
	*chunk.stride_mut() = i32::try_from(data.frame_size).unwrap_or(0);
	*chunk.size_mut() = u32::try_from(written).unwrap_or(0);
}

/// Counts consecutive cycles that wrote no audio.
fn next_idle_cycles(idle_cycles: u32, written: usize) -> u32 {
	if written == 0 {
		idle_cycles.saturating_add(1)
	} else {
		0
	}
}

/// Whether a core error ends the connection: a closed socket, or any core failure before the
/// connection became ready.
fn is_fatal_core_error(id: u32, res: i32, ready: bool) -> bool {
	res == -EPIPE || (id == pw::core::PW_ID_CORE && res < 0 && !ready)
}

/// The frame-aligned byte range of `pcm` to copy into a buffer of `max_bytes`.
fn next_chunk(len: usize, offset: usize, max_bytes: usize, frame_size: usize) -> Range<usize> {
	if frame_size == 0 || offset >= len {
		return offset..offset;
	}

	let length = max_bytes.min(len - offset) / frame_size * frame_size;
	offset..offset + length
}

/// Resolves and decodes a sound. Runs on a blocking thread.
fn load(job: &Job) -> Option<Decoded> {
	let file = job.request.file.as_deref().and_then(|path| {
		read_sound_file(path)
			.inspect_err(|error| {
				log::debug!(
					"Ignoring notification sound file {}: {error}",
					path.display()
				);
			})
			.ok()
	});
	// Only sounds found by name identify an event; an explicit file does not.
	let file = file.map(|file| (file, None)).or_else(|| {
		let name = job.request.name.as_deref()?;
		let path = find_named_sound(&data_dirs(), job.theme.as_deref(), name)?;
		read_sound_file(&path)
			.inspect_err(|error| {
				log::debug!("Ignoring notification sound {}: {error}", path.display());
			})
			.ok()
			.map(|file| (file, Some(name.to_owned())))
	});
	let ((bytes, extension), event_id) = file?;

	match decode(bytes, extension.as_deref()) {
		Ok((pcm, format)) => Some(Decoded {
			pcm,
			format,
			event_id,
		}),
		Err(error) => {
			log::debug!("Failed to decode notification sound: {error}");
			None
		}
	}
}

/// Opens a regular file without blocking on or acquiring special files.
///
/// The path is checked before opening so FIFOs and devices are never opened, and the opened
/// descriptor is checked again to close the race with a concurrent replacement.
fn open_regular_file(path: &Path) -> std::io::Result<File> {
	let is_regular =
		|mode| rustix::fs::FileType::from_raw_mode(mode) == rustix::fs::FileType::RegularFile;

	if !is_regular(rustix::fs::stat(path)?.st_mode) {
		return Err(std::io::Error::other("not a regular file"));
	}

	let fd = rustix::fs::open(
		path,
		rustix::fs::OFlags::RDONLY
			| rustix::fs::OFlags::NONBLOCK
			| rustix::fs::OFlags::CLOEXEC
			| rustix::fs::OFlags::NOCTTY,
		rustix::fs::Mode::empty(),
	)?;
	if !is_regular(rustix::fs::fstat(&fd)?.st_mode) {
		return Err(std::io::Error::other("not a regular file"));
	}

	Ok(File::from(fd))
}

/// Reads a bounded regular file without blocking on special files.
fn read_sound_file(path: &Path) -> std::io::Result<(Vec<u8>, Option<String>)> {
	let file = open_regular_file(path)?;

	let metadata = file.metadata()?;
	if metadata.len() > MAX_FILE_BYTES {
		return Err(std::io::Error::other("file is too large"));
	}

	let mut bytes = Vec::new();
	file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
	if bytes.len() as u64 > MAX_FILE_BYTES {
		return Err(std::io::Error::other("file is too large"));
	}

	let extension = path
		.extension()
		.and_then(|extension| extension.to_str())
		.map(str::to_ascii_lowercase);

	Ok((bytes, extension))
}

/// Decodes the default track to interleaved native-endian `f32`, truncated to the duration cap.
fn decode(bytes: Vec<u8>, extension: Option<&str>) -> Result<(Vec<u8>, PcmFormat), DecodeError> {
	let mut hint = Hint::new();
	if let Some(extension) = extension {
		hint.with_extension(extension);
	}

	let source = MediaSourceStream::new(
		Box::new(Cursor::new(bytes)),
		MediaSourceStreamOptions::default(),
	);
	let mut format = symphonia::default::get_probe()
		.format(
			&hint,
			source,
			&FormatOptions::default(),
			&MetadataOptions::default(),
		)?
		.format;
	let track = format
		.default_track()
		.ok_or(DecodeError::Unsupported("no audio track"))?;
	let track_id = track.id;
	let mut decoder =
		symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

	let mut pcm = Vec::new();
	let mut format_spec: Option<PcmFormat> = None;
	let mut limit = MAX_PCM_BYTES;

	loop {
		let packet = match format.next_packet() {
			Ok(packet) => packet,
			Err(DecodeError::IoError(error))
				if error.kind() == std::io::ErrorKind::UnexpectedEof =>
			{
				break;
			}
			Err(DecodeError::ResetRequired) => break,
			Err(error) => return Err(error),
		};
		if packet.track_id() != track_id {
			continue;
		}

		let decoded = match decoder.decode(&packet) {
			Ok(decoded) => decoded,
			Err(DecodeError::DecodeError(_)) => continue,
			Err(error) => return Err(error),
		};

		let signal = *decoded.spec();
		let packet_format = PcmFormat {
			rate: signal.rate,
			positions: channel_positions(signal.channels),
		};
		match &format_spec {
			None => {
				if packet_format.positions.is_empty()
					|| packet_format.positions.len() > MAX_CHANNELS
					|| packet_format.rate == 0
					|| packet_format.rate > MAX_SAMPLE_RATE
				{
					return Err(DecodeError::Unsupported("unsupported sample format"));
				}
				let duration_bytes = u64::from(packet_format.rate)
					* MAX_DURATION_SECS
					* packet_format.frame_size() as u64;
				limit = limit.min(usize::try_from(duration_bytes).unwrap_or(usize::MAX));
				format_spec = Some(packet_format);
			}
			Some(format) if *format != packet_format => break,
			Some(_) => {}
		}

		let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, signal);
		samples.copy_interleaved_ref(decoded);
		for sample in samples.samples() {
			if pcm.len() + 4 > limit {
				break;
			}
			pcm.extend_from_slice(&sample.to_ne_bytes());
		}
		if pcm.len() + 4 > limit {
			break;
		}
	}

	let format = format_spec.ok_or(DecodeError::Unsupported("no decodable audio"))?;
	let frame_size = format.frame_size();
	pcm.truncate(pcm.len() / frame_size * frame_size);
	if pcm.is_empty() {
		return Err(DecodeError::Unsupported("no decodable audio"));
	}

	Ok((pcm, format))
}

/// Maps Symphonia channels, in interleaving order, to SPA channel positions.
///
/// A lone front-left channel is mono. Channels without a SPA equivalent become auxiliary.
fn channel_positions(channels: Channels) -> Vec<u32> {
	if channels == Channels::FRONT_LEFT {
		return vec![spa_sys::SPA_AUDIO_CHANNEL_MONO];
	}

	channels
		.iter()
		.enumerate()
		.map(|(index, channel)| {
			let known = [
				(Channels::FRONT_LEFT, spa_sys::SPA_AUDIO_CHANNEL_FL),
				(Channels::FRONT_RIGHT, spa_sys::SPA_AUDIO_CHANNEL_FR),
				(Channels::FRONT_CENTRE, spa_sys::SPA_AUDIO_CHANNEL_FC),
				(Channels::LFE1, spa_sys::SPA_AUDIO_CHANNEL_LFE),
				(Channels::REAR_LEFT, spa_sys::SPA_AUDIO_CHANNEL_RL),
				(Channels::REAR_RIGHT, spa_sys::SPA_AUDIO_CHANNEL_RR),
				(Channels::FRONT_LEFT_CENTRE, spa_sys::SPA_AUDIO_CHANNEL_FLC),
				(Channels::FRONT_RIGHT_CENTRE, spa_sys::SPA_AUDIO_CHANNEL_FRC),
				(Channels::REAR_CENTRE, spa_sys::SPA_AUDIO_CHANNEL_RC),
				(Channels::SIDE_LEFT, spa_sys::SPA_AUDIO_CHANNEL_SL),
				(Channels::SIDE_RIGHT, spa_sys::SPA_AUDIO_CHANNEL_SR),
			];
			known
				.iter()
				.find(|(candidate, _)| *candidate == channel)
				.map_or_else(
					|| spa_sys::SPA_AUDIO_CHANNEL_AUX0 + u32::try_from(index).unwrap_or(0),
					|(_, position)| *position,
				)
		})
		.collect()
}

/// Accepts absolute paths and `file://` URIs only.
fn parse_sound_file(value: &str) -> Option<PathBuf> {
	if value.len() > 4096 || value.chars().any(char::is_control) {
		return None;
	}

	let path = if value.starts_with("file://") {
		url::Url::parse(value).ok()?.to_file_path().ok()?
	} else {
		PathBuf::from(value)
	};

	path.is_absolute().then_some(path)
}

fn is_valid_sound_name(name: &str) -> bool {
	is_valid_component(name)
}

fn is_valid_component(name: &str) -> bool {
	config::notifications::is_valid_sound_component(name)
}

/// `$XDG_DATA_HOME` followed by `$XDG_DATA_DIRS`, absolute entries only.
fn data_dirs() -> Vec<PathBuf> {
	let home = std::env::var_os("XDG_DATA_HOME")
		.map(PathBuf::from)
		.filter(|path| path.is_absolute())
		.or_else(|| dirs::home_dir().map(|home| home.join(".local/share")));

	let system = std::env::var_os("XDG_DATA_DIRS")
		.filter(|value| !value.is_empty())
		.unwrap_or_else(|| "/usr/local/share:/usr/share".into());
	let system = std::env::split_paths(&system).filter(|path| path.is_absolute());

	let mut seen = HashSet::new();
	home.into_iter()
		.chain(system)
		.filter(|path| seen.insert(path.clone()))
		.collect()
}

/// Existing `sounds/<theme>` directories in data-dir priority order.
fn theme_dirs(data_dirs: &[PathBuf], theme: &str) -> Vec<PathBuf> {
	if !is_valid_component(theme) {
		return Vec::new();
	}

	data_dirs
		.iter()
		.map(|dir| dir.join("sounds").join(theme))
		.filter(|dir| dir.is_dir())
		.collect()
}

/// All installed theme names, sorted for deterministic lookup.
fn installed_themes(data_dirs: &[PathBuf]) -> Vec<String> {
	let mut themes: Vec<String> = data_dirs
		.iter()
		.filter_map(|dir| std::fs::read_dir(dir.join("sounds")).ok())
		.flatten()
		.filter_map(Result::ok)
		.filter(|entry| entry.path().is_dir())
		.filter_map(|entry| entry.file_name().into_string().ok())
		.filter(|name| is_valid_component(name))
		.collect();

	themes.sort();
	themes.dedup();
	themes.truncate(MAX_THEMES);
	themes
}

/// Reads a bounded INI file into `section -> key -> value`.
fn read_ini(path: &Path) -> HashMap<String, HashMap<String, String>> {
	let mut text = String::new();
	let Ok(file) = open_regular_file(path) else {
		return HashMap::new();
	};
	if file.take(MAX_INI_BYTES).read_to_string(&mut text).is_err() {
		return HashMap::new();
	}

	let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
	let mut section = String::new();
	for line in text.lines().map(str::trim) {
		if line.is_empty() || line.starts_with(['#', ';']) {
			continue;
		}
		if let Some(name) = line
			.strip_prefix('[')
			.and_then(|line| line.strip_suffix(']'))
		{
			name.trim().clone_into(&mut section);
			continue;
		}
		if let Some((key, value)) = line.split_once('=') {
			sections
				.entry(section.clone())
				.or_default()
				.entry(key.trim().to_owned())
				.or_insert_with(|| value.trim().to_owned());
		}
	}

	sections
}

fn index_theme(data_dirs: &[PathBuf], theme: &str) -> HashMap<String, String> {
	theme_dirs(data_dirs, theme)
		.iter()
		.map(|dir| dir.join("index.theme"))
		.find(|path| path.is_file())
		.map(|path| read_ini(&path).remove("Sound Theme").unwrap_or_default())
		.unwrap_or_default()
}

fn list_value(value: Option<&String>) -> impl Iterator<Item = String> + '_ {
	value
		.into_iter()
		.flat_map(|value| value.split(','))
		.map(str::trim)
		.filter(|item| is_valid_component(item))
		.map(str::to_owned)
}

/// Appends `theme` and its `Inherits` chain, breadth first, bounded and cycle-safe.
fn push_theme_chain(
	data_dirs: &[PathBuf], theme: &str, themes: &mut Vec<String>, seen: &mut HashSet<String>,
) {
	let mut level = vec![theme.to_owned()];
	for _ in 0..MAX_INHERIT_DEPTH {
		let mut next = Vec::new();
		for theme in level {
			if themes.len() >= MAX_THEMES
				|| !is_valid_component(&theme)
				|| !seen.insert(theme.clone())
			{
				continue;
			}

			let index = index_theme(data_dirs, &theme);
			next.extend(list_value(index.get("Inherits")));
			themes.push(theme);
		}
		if next.is_empty() {
			break;
		}
		level = next;
	}
}

/// The user's GTK sound theme from `gtk-3.0/settings.ini`.
fn gtk_sound_theme() -> Option<String> {
	let config = std::env::var_os("XDG_CONFIG_HOME")
		.map(PathBuf::from)
		.filter(|path| path.is_absolute())
		.or_else(|| dirs::home_dir().map(|home| home.join(".config")))?;

	read_ini(&config.join("gtk-3.0/settings.ini"))
		.remove("Settings")?
		.remove("gtk-sound-theme-name")
		.map(|value| value.trim_matches('"').to_owned())
		.filter(|theme| is_valid_component(theme))
}

/// Theme lookup order: explicit or GTK theme chain, `freedesktop`, then other installed themes.
fn theme_candidates(data_dirs: &[PathBuf], theme: Option<&str>) -> Vec<String> {
	let mut themes = Vec::new();
	let mut seen = HashSet::new();

	let primary = theme.map(str::to_owned).or_else(gtk_sound_theme);
	if let Some(primary) = primary {
		push_theme_chain(data_dirs, &primary, &mut themes, &mut seen);
	}
	push_theme_chain(data_dirs, "freedesktop", &mut themes, &mut seen);

	for theme in installed_themes(data_dirs) {
		if themes.len() >= MAX_THEMES {
			break;
		}
		if seen.insert(theme.clone()) {
			themes.push(theme);
		}
	}

	themes
}

fn find_named_sound(data_dirs: &[PathBuf], theme: Option<&str>, name: &str) -> Option<PathBuf> {
	if !is_valid_sound_name(name) {
		return None;
	}

	theme_candidates(data_dirs, theme)
		.iter()
		.find_map(|theme| find_in_theme(data_dirs, theme, name))
}

fn find_in_theme(data_dirs: &[PathBuf], theme: &str, name: &str) -> Option<PathBuf> {
	let index = index_theme(data_dirs, theme);
	let mut subdirs = vec!["stereo".to_owned()];
	for directory in list_value(index.get("Directories")).take(MAX_THEME_DIRECTORIES) {
		if !subdirs.contains(&directory) {
			subdirs.push(directory);
		}
	}

	theme_dirs(data_dirs, theme).iter().find_map(|dir| {
		subdirs.iter().find_map(|subdir| {
			SOUND_EXTENSIONS
				.iter()
				.map(|extension| dir.join(subdir).join(format!("{name}.{extension}")))
				.find(|path| path.is_file())
		})
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn chunks_are_frame_aligned_and_bounded() {
		assert_eq!(next_chunk(100, 0, 64, 8), 0..64);
		assert_eq!(next_chunk(100, 0, 60, 8), 0..56);
		assert_eq!(next_chunk(104, 96, 64, 8), 96..104);
		assert_eq!(next_chunk(100, 96, 64, 8), 96..96);
		assert_eq!(next_chunk(100, 90, 64, 8), 90..98);
		assert_eq!(next_chunk(100, 100, 64, 8), 100..100);
		assert_eq!(next_chunk(100, 120, 64, 8), 120..120);
		assert_eq!(next_chunk(100, 0, 4, 8), 0..0);
		assert_eq!(next_chunk(100, 0, 64, 0), 0..0);
	}

	#[test]
	fn maps_channel_positions() {
		assert_eq!(
			channel_positions(Channels::FRONT_LEFT),
			vec![spa_sys::SPA_AUDIO_CHANNEL_MONO]
		);
		assert_eq!(
			channel_positions(Channels::FRONT_LEFT | Channels::FRONT_RIGHT),
			vec![spa_sys::SPA_AUDIO_CHANNEL_FL, spa_sys::SPA_AUDIO_CHANNEL_FR]
		);
		assert_eq!(
			channel_positions(Channels::FRONT_RIGHT | Channels::TOP_CENTRE),
			vec![
				spa_sys::SPA_AUDIO_CHANNEL_FR,
				spa_sys::SPA_AUDIO_CHANNEL_AUX0 + 1
			]
		);
	}

	#[test]
	fn serializes_format_param() {
		let format = PcmFormat {
			rate: 48_000,
			positions: channel_positions(Channels::FRONT_LEFT | Channels::FRONT_RIGHT),
		};
		assert_eq!(format.frame_size(), 8);

		let param = format_param(format.audio_info()).ok();
		let pod = param.as_ref().and_then(AlignedPod::pod);
		assert!(pod.is_some_and(|pod| pod.as_raw_ptr().cast::<u64>().is_aligned()));

		let mut parsed = AudioInfoRaw::new();
		assert!(pod.is_some_and(|pod| parsed.parse(pod).is_ok()));
		assert!(parsed.format() == SAMPLE_FORMAT);
		assert_eq!(parsed.rate(), 48_000);
		assert_eq!(parsed.channels(), 2);
		assert_eq!(
			parsed.position().get(..2),
			Some(&[spa_sys::SPA_AUDIO_CHANNEL_FL, spa_sys::SPA_AUDIO_CHANNEL_FR][..])
		);
	}

	#[test]
	fn aligns_pod_storage() {
		let bytes: Vec<u8> = (0..24).collect();
		let pod = AlignedPod::new(&bytes);

		assert!(pod.as_ref().is_some_and(|pod| {
			pod.storage.get(pod.range.clone()) == Some(&bytes[..])
				&& pod
					.storage
					.as_ptr()
					.wrapping_add(pod.range.start)
					.cast::<u64>()
					.is_aligned()
		}));
	}

	#[test]
	fn timeout_is_finite_while_a_deadline_is_pending() {
		let now = Instant::now();

		assert!(matches!(
			loop_timeout(std::iter::empty(), now),
			Timeout::Infinite
		));
		assert!(matches!(
			loop_timeout([now + CONNECT_TIMEOUT].into_iter(), now),
			Timeout::Finite(duration) if duration > CONNECT_TIMEOUT
		));
		assert!(matches!(
			loop_timeout([now + PLAYBACK_TIMEOUT, now + CONNECT_TIMEOUT].into_iter(), now),
			Timeout::Finite(duration) if duration <= CONNECT_TIMEOUT + Duration::from_millis(1)
		));
		assert!(matches!(
			loop_timeout([now].into_iter(), now + Duration::from_secs(1)),
			Timeout::Finite(duration) if duration == Duration::from_millis(1)
		));
	}

	#[test]
	fn reconnect_backoff_doubles_to_cap_and_resets() {
		let now = Instant::now();
		let mut reconnect = Reconnect::default();
		assert!(!reconnect.is_waiting(now));

		let mut expected = Vec::new();
		for _ in 0..7 {
			reconnect.failed(now);
			expected.push(reconnect.backoff.map(|backoff| backoff.as_secs()));
		}
		assert_eq!(expected, [1, 2, 4, 8, 16, 30, 30].map(Some).to_vec());
		assert!(reconnect.is_waiting(now + Duration::from_secs(29)));
		assert!(!reconnect.is_waiting(now + MAX_RECONNECT_BACKOFF));

		reconnect.connected();
		reconnect.failed(now);
		assert_eq!(reconnect.backoff, Some(Duration::from_secs(1)));
	}

	#[test]
	fn counts_idle_process_cycles() {
		let mut idle = 0;
		for _ in 0..MAX_IDLE_CYCLES {
			idle = next_idle_cycles(idle, 0);
		}
		assert_eq!(idle, MAX_IDLE_CYCLES);
		assert_eq!(next_idle_cycles(idle, 8), 0);
		assert_eq!(next_idle_cycles(u32::MAX, 0), u32::MAX);
	}

	#[test]
	fn classifies_fatal_core_errors() {
		let core = pw::core::PW_ID_CORE;

		assert!(is_fatal_core_error(7, -EPIPE, true));
		assert!(is_fatal_core_error(core, -2, false));
		assert!(!is_fatal_core_error(core, -2, true));
		assert!(!is_fatal_core_error(7, -2, false));
	}

	#[test]
	fn dropping_queued_sounds_releases_permits() {
		let permits = Arc::new(Semaphore::new(MAX_CONCURRENT));
		let mut pending: Vec<Playable> =
			std::iter::from_fn(|| permits.clone().try_acquire_owned().ok())
				.map(|permit| Playable {
					decoded: Decoded {
						pcm: Vec::new(),
						format: PcmFormat {
							rate: 48_000,
							positions: vec![spa_sys::SPA_AUDIO_CHANNEL_MONO],
						},
						event_id: None,
					},
					_permit: permit,
				})
				.collect();
		assert_eq!(pending.len(), MAX_CONCURRENT);
		assert_eq!(permits.available_permits(), 0);

		pending.clear();
		assert_eq!(permits.available_permits(), MAX_CONCURRENT);
	}

	#[test]
	fn validates_sound_names() {
		assert!(is_valid_sound_name("message-new-instant"));

		assert!(!is_valid_sound_name(""));
		assert!(!is_valid_sound_name("../bell"));
		assert!(!is_valid_sound_name("a/b"));
		assert!(!is_valid_sound_name("bell\n"));
		assert!(!is_valid_sound_name(&"a".repeat(MAX_NAME_LEN + 1)));
	}

	#[test]
	fn accepts_only_absolute_sound_files() {
		assert_eq!(
			parse_sound_file("/usr/share/sounds/bell.oga"),
			Some(PathBuf::from("/usr/share/sounds/bell.oga"))
		);
		assert_eq!(
			parse_sound_file("file:///tmp/a%20b.wav"),
			Some(PathBuf::from("/tmp/a b.wav"))
		);

		assert_eq!(parse_sound_file("bell.oga"), None);
		assert_eq!(parse_sound_file("http://example.com/a.wav"), None);
	}

	type TestError = Box<dyn std::error::Error>;

	const PRIVATE_REMOTE: &str = "subniri-pipewire-test";
	const ABSENT_REMOTE: &str = "missing-pipewire-socket";
	const TEST_SINK: &str = "subniri-test-sink";

	// Isolated PipeWire server without WirePlumber, hardware discovery, RT scheduling, or D-Bus.
	const PRIVATE_PIPEWIRE_CONF: &str = r#"
context.properties = {
    core.daemon = true
    core.name = subniri-pipewire-test
    support.dbus = false
}
context.spa-libs = {
    audio.convert.* = audioconvert/libspa-audioconvert
    audio.adapt     = audioconvert/libspa-audioconvert
    support.*       = support/libspa-support
}
context.modules = [
    { name = libpipewire-module-protocol-native }
    { name = libpipewire-module-metadata }
    { name = libpipewire-module-spa-node-factory }
    { name = libpipewire-module-client-node }
    { name = libpipewire-module-adapter }
    { name = libpipewire-module-link-factory }
    { name = libpipewire-module-access args = {
        access.socket = {
            subniri-pipewire-test = "unrestricted"
            subniri-pipewire-test-manager = "unrestricted"
        }
    } }
]
context.objects = [
    { factory = metadata args = {
        metadata.name = default
        metadata.values = [
            { key = default.audio.sink value = { name = subniri-test-sink } }
        ]
    } }
    { factory = adapter args = {
        factory.name = support.null-audio-sink
        node.name = subniri-test-sink
        node.description = "Subniri isolated test sink"
        media.class = Audio/Sink
        audio.position = [ FL FR ]
        node.driver = true
        priority.driver = 1000
        adapter.auto-port-config = {
            mode = dsp
            monitor = false
            control = false
            position = preserve
        }
    } }
]
"#;

	// Without a session manager, client streams need auto-port-config to expose output ports.
	const CLIENT_AUTO_PORT_CONF: &str = r#"
stream.properties = {
    adapter.auto-port-config = { mode = dsp }
}
"#;

	struct PrivatePipeWire {
		directory: PathBuf,
		daemon: Option<std::process::Child>,
	}

	impl Drop for PrivatePipeWire {
		fn drop(&mut self) {
			if let Some(daemon) = &mut self.daemon {
				let _ = daemon.kill();
				let _ = daemon.wait();
			}

			// Remove only known files created inside this test's exclusive directory.
			for name in [
				"pw.conf",
				"pipewire.log",
				PRIVATE_REMOTE,
				"subniri-pipewire-test.lock",
				"subniri-pipewire-test-manager",
				"subniri-pipewire-test-manager.lock",
				"cfg/pipewire/client.conf.d/10-auto-port.conf",
			] {
				let _ = std::fs::remove_file(self.directory.join(name));
			}
			for dir in ["cfg/pipewire/client.conf.d", "cfg/pipewire", "cfg"] {
				let _ = std::fs::remove_dir(self.directory.join(dir));
			}
			let _ = std::fs::remove_dir(&self.directory);
		}
	}

	struct TempRuntimeDir {
		directory: PathBuf,
	}

	impl Drop for TempRuntimeDir {
		fn drop(&mut self) {
			let _ = std::fs::remove_dir(&self.directory);
		}
	}

	fn create_private_dir(prefix: &str) -> Result<PathBuf, TestError> {
		use std::os::unix::fs::DirBuilderExt;

		let nonce = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)?
			.as_nanos();
		let directory =
			std::env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()));
		std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
		Ok(directory)
	}

	struct TestWorker {
		commands: pw::channel::Sender<Command>,
		permits: Arc<Semaphore>,
		stats: Arc<TestStats>,
		shutdown: watch::Sender<bool>,
		worker: Option<SoundWorker>,
	}

	impl TestWorker {
		fn spawn() -> Result<Self, TestError> {
			let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
			let (commands_tx, commands_rx) = pw::channel::channel();
			let alive = Arc::new(AtomicBool::new(true));
			let permits = Arc::new(Semaphore::new(MAX_CONCURRENT));
			let stats = Arc::new(TestStats::default());

			let thread = std::thread::Builder::new()
				.name("notification-sound-test".into())
				.spawn({
					let alive = WorkerAlive(alive.clone());
					let stats = stats.clone();
					move || run_thread_inner(alive, commands_rx, Some(stats))
				})?;

			let worker = Worker {
				commands: commands_tx.clone(),
				alive,
			};
			let dispatcher = tokio::spawn(async move {
				let _worker = worker;
				while !*shutdown_rx.borrow() {
					if shutdown_rx.changed().await.is_err() {
						break;
					}
				}
			});

			Ok(Self {
				commands: commands_tx,
				permits,
				stats,
				shutdown: shutdown_tx,
				worker: Some(SoundWorker {
					dispatcher: Some(dispatcher),
					thread: Some(thread),
				}),
			})
		}

		async fn submit_pcm(&self, pcm: Vec<u8>, format: PcmFormat) -> Result<(), TestError> {
			let permit = self.permits.clone().acquire_owned().await?;
			let playable = Playable {
				decoded: Decoded {
					pcm,
					format,
					event_id: None,
				},
				_permit: permit,
			};
			self.commands
				.send(Command::Play(playable))
				.map_err(|_| "failed to send Play command to sound worker")?;
			Ok(())
		}

		async fn shutdown(mut self) -> Result<(), TestError> {
			let _ = self.shutdown.send(true);
			if let Some(worker) = self.worker.take() {
				tokio::time::timeout(Duration::from_secs(2), worker.join())
					.await
					.map_err(|_| "timed out joining sound worker")?;
			}
			Ok(())
		}
	}

	struct AutoLinker {
		sink_ready: Arc<AtomicBool>,
		linked_count: Arc<std::sync::atomic::AtomicUsize>,
		notify: Arc<tokio::sync::Notify>,
		stop: pw::channel::Sender<()>,
		thread: Option<JoinHandle<()>>,
	}

	impl AutoLinker {
		fn spawn() -> Result<Self, TestError> {
			let sink_ready = Arc::new(AtomicBool::new(false));
			let linked_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
			let notify = Arc::new(tokio::sync::Notify::new());
			let (stop_tx, stop_rx) = pw::channel::channel();

			let thread = std::thread::Builder::new()
				.name("test-pw-linker".into())
				.spawn({
					let sink_ready = sink_ready.clone();
					let linked_count = linked_count.clone();
					let notify = notify.clone();
					move || run_auto_linker(stop_rx, sink_ready, linked_count, notify)
				})?;

			Ok(Self {
				sink_ready,
				linked_count,
				notify,
				stop: stop_tx,
				thread: Some(thread),
			})
		}

		fn stop(mut self) {
			let _ = self.stop.send(());
			if let Some(thread) = self.thread.take() {
				let _ = thread.join();
			}
		}
	}

	impl Drop for AutoLinker {
		fn drop(&mut self) {
			let _ = self.stop.send(());
			if let Some(thread) = self.thread.take() {
				let _ = thread.join();
			}
		}
	}

	#[derive(Default)]
	struct LinkerState {
		sink_node_id: Option<u32>,
		stream_node_id: Option<u32>,
		ports: Vec<(u32, u32, String, String)>,
		linked_channels: HashSet<String>,
		links: Vec<pw::link::Link>,
	}

	fn run_auto_linker(
		stop_rx: pw::channel::Receiver<()>, sink_ready: Arc<AtomicBool>,
		linked_count: Arc<std::sync::atomic::AtomicUsize>, notify: Arc<tokio::sync::Notify>,
	) {
		pw::init();
		let Ok(mainloop) = MainLoopRc::new(None) else {
			return;
		};
		let Ok(context) = ContextRc::new(&mainloop, None) else {
			return;
		};
		let Ok(core) = context.connect_rc(None) else {
			return;
		};
		let Ok(registry) = core.get_registry_rc() else {
			return;
		};

		let _stop_listener = stop_rx.attach(mainloop.loop_(), {
			let mainloop = mainloop.clone();
			move |()| mainloop.quit()
		});

		let state = Rc::new(RefCell::new(LinkerState::default()));
		let _registry_listener = registry
			.add_listener_local()
			.global({
				let core = core.clone();
				let state = state.clone();
				let sink_ready = sink_ready.clone();
				let linked_count = linked_count.clone();
				let notify = notify.clone();
				move |global| {
					let Some(props) = global.props.as_ref() else {
						return;
					};
					let mut state = state.borrow_mut();

					if global.type_ == pw::types::ObjectType::Node {
						match props.get(*pw::keys::NODE_NAME) {
							Some(TEST_SINK) => {
								state.sink_node_id = Some(global.id);
								eprintln!("[test-pw-linker] sink node id={}", global.id);
							}
							Some(NODE_NAME) => {
								state.stream_node_id = Some(global.id);
								eprintln!(
									"[test-pw-linker] stream node id={} role={:?}",
									global.id,
									props.get(*pw::keys::MEDIA_ROLE)
								);
							}
							_ => {}
						}
					} else if global.type_ == pw::types::ObjectType::Port {
						let Some(node_id) = props
							.get(*pw::keys::NODE_ID)
							.and_then(|value| value.parse::<u32>().ok())
						else {
							return;
						};
						let Some(direction) = props.get(*pw::keys::PORT_DIRECTION) else {
							return;
						};
						let Some(channel) = props.get(*pw::keys::AUDIO_CHANNEL) else {
							return;
						};
						state.ports.push((
							global.id,
							node_id,
							direction.to_owned(),
							channel.to_owned(),
						));
					} else if global.type_ == pw::types::ObjectType::Link {
						eprintln!(
							"[test-pw-linker] link global id={} out={:?}:{:?} -> in={:?}:{:?}",
							global.id,
							props.get(*pw::keys::LINK_OUTPUT_NODE),
							props.get(*pw::keys::LINK_OUTPUT_PORT),
							props.get(*pw::keys::LINK_INPUT_NODE),
							props.get(*pw::keys::LINK_INPUT_PORT),
						);
					}

					if let Some(sink_id) = state.sink_node_id {
						let has_fl = state.ports.iter().any(|(_, node, dir, ch)| {
							*node == sink_id && dir == "in" && ch == "FL"
						});
						let has_fr = state.ports.iter().any(|(_, node, dir, ch)| {
							*node == sink_id && dir == "in" && ch == "FR"
						});
						if has_fl && has_fr && !sink_ready.swap(true, Ordering::AcqRel) {
							notify.notify_waiters();
						}
					}

					if let (Some(sink_id), Some(stream_id)) =
						(state.sink_node_id, state.stream_node_id)
					{
						for channel in ["FL", "FR"] {
							if state.linked_channels.contains(channel) {
								continue;
							}
							let out_port = state.ports.iter().find_map(|(port, node, dir, ch)| {
								(*node == stream_id && dir == "out" && ch == channel)
									.then_some(*port)
							});
							let in_port = state.ports.iter().find_map(|(port, node, dir, ch)| {
								(*node == sink_id && dir == "in" && ch == channel).then_some(*port)
							});
							if let (Some(out_port), Some(in_port)) = (out_port, in_port) {
								match core.create_object::<pw::link::Link>(
									"link-factory",
									&properties! {
										*pw::keys::LINK_OUTPUT_NODE => stream_id.to_string(),
										*pw::keys::LINK_OUTPUT_PORT => out_port.to_string(),
										*pw::keys::LINK_INPUT_NODE => sink_id.to_string(),
										*pw::keys::LINK_INPUT_PORT => in_port.to_string(),
									},
								) {
									Ok(link) => {
										eprintln!(
											"[test-pw-linker] created link {channel}: {stream_id}:{out_port} -> {sink_id}:{in_port}"
										);
										state.links.push(link);
										state.linked_channels.insert(channel.to_owned());
										linked_count.store(state.links.len(), Ordering::Release);
										notify.notify_waiters();
									}
									Err(error) => {
										eprintln!(
											"[test-pw-linker] failed to link {channel}: {error}"
										);
									}
								}
							}
						}
					}
				}
			})
			.register();

		mainloop.run();
	}

	async fn wait_until(
		notify: &tokio::sync::Notify, timeout: Duration, description: &str,
		mut ready: impl FnMut() -> bool,
	) -> Result<(), TestError> {
		tokio::time::timeout(timeout, async {
			loop {
				let notified = notify.notified();
				tokio::pin!(notified);
				notified.as_mut().enable();
				if ready() {
					return;
				}
				notified.await;
			}
		})
		.await
		.map_err(|_| format!("timed out waiting for {description}").into())
	}

	fn generate_stereo_sine(duration_ms: u32, rate: u32, freq_hz: f32) -> (Vec<u8>, PcmFormat) {
		let frames = (u64::from(rate) * u64::from(duration_ms) / 1000) as usize;
		let mut pcm = Vec::with_capacity(frames * 2 * SAMPLE_BYTES);
		for frame in 0..frames {
			let phase = 2.0 * std::f32::consts::PI * freq_hz * (frame as f32) / (rate as f32);
			let sample = (phase.sin() * 0.1).to_ne_bytes();
			pcm.extend_from_slice(&sample);
			pcm.extend_from_slice(&sample);
		}

		let format = PcmFormat {
			rate,
			positions: vec![spa_sys::SPA_AUDIO_CHANNEL_FL, spa_sys::SPA_AUDIO_CHANNEL_FR],
		};
		(pcm, format)
	}

	fn log_pw_dump_snapshot() {
		let Ok(output) = std::process::Command::new("pw-dump").output() else {
			return;
		};
		if !output.status.success() {
			return;
		}
		let Ok(objects) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) else {
			return;
		};
		for object in objects {
			let kind = object.get("type").and_then(serde_json::Value::as_str);
			let props = object.get("info").and_then(|info| info.get("props"));
			let node_name = props
				.and_then(|props| props.get("node.name"))
				.and_then(serde_json::Value::as_str);
			let is_relevant = matches!(kind, Some("PipeWire:Interface:Link"))
				|| matches!(node_name, Some(NODE_NAME | TEST_SINK));
			if is_relevant {
				eprintln!("[pw-dump] {object}");
			}
		}
	}

	#[test]
	#[ignore = "requires private PipeWire daemon"]
	fn plays_and_drains_on_private_pipewire() -> Result<(), TestError> {
		if std::env::var_os("SUBNIRI_PRIVATE_PIPEWIRE_CHILD").is_some() {
			return tokio::runtime::Builder::new_current_thread()
				.enable_all()
				.build()?
				.block_on(plays_and_drains_child());
		}

		let directory = create_private_dir("subniri-pw-sound")?;
		let client_conf_dir = directory.join("cfg/pipewire/client.conf.d");
		std::fs::create_dir_all(&client_conf_dir)?;
		std::fs::write(directory.join("pw.conf"), PRIVATE_PIPEWIRE_CONF)?;
		std::fs::write(
			client_conf_dir.join("10-auto-port.conf"),
			CLIENT_AUTO_PORT_CONF,
		)?;

		let log_path = directory.join("pipewire.log");
		let log_file = std::fs::File::create(&log_path)?;
		let log_err = log_file.try_clone()?;

		let mut fixture = PrivatePipeWire {
			directory: directory.clone(),
			daemon: None,
		};
		fixture.daemon = Some(
			std::process::Command::new("pipewire")
				.args(["-c"])
				.arg(directory.join("pw.conf"))
				.env("PIPEWIRE_RUNTIME_DIR", &directory)
				.env("XDG_RUNTIME_DIR", &directory)
				.stdout(std::process::Stdio::from(log_file))
				.stderr(std::process::Stdio::from(log_err))
				.spawn()?,
		);

		let deadline = Instant::now() + Duration::from_secs(3);
		loop {
			if std::os::unix::net::UnixStream::connect(directory.join(PRIVATE_REMOTE)).is_ok() {
				break;
			}
			if let Some(daemon) = &mut fixture.daemon
				&& let Some(status) = daemon.try_wait()?
			{
				let log = std::fs::read_to_string(&log_path).unwrap_or_default();
				return Err(format!("private PipeWire exited {status}: {log}").into());
			}
			if Instant::now() >= deadline {
				let log = std::fs::read_to_string(&log_path).unwrap_or_default();
				return Err(format!("private PipeWire socket did not become ready: {log}").into());
			}
			std::thread::sleep(Duration::from_millis(10));
		}

		let output = std::process::Command::new(std::env::current_exe()?)
			.args([
				"--exact",
				"sound::tests::plays_and_drains_on_private_pipewire",
				"--ignored",
				"--nocapture",
			])
			.env("SUBNIRI_PRIVATE_PIPEWIRE_CHILD", "1")
			.env("PIPEWIRE_RUNTIME_DIR", &directory)
			.env("XDG_RUNTIME_DIR", &directory)
			.env("PIPEWIRE_REMOTE", PRIVATE_REMOTE)
			.env("XDG_CONFIG_HOME", directory.join("cfg"))
			.output()?;

		let stdout = String::from_utf8_lossy(&output.stdout);
		let stderr = String::from_utf8_lossy(&output.stderr);
		eprint!("{stdout}{stderr}");
		if !output.status.success() {
			let log = std::fs::read_to_string(&log_path).unwrap_or_default();
			return Err(format!(
				"private PipeWire child failed ({}):\n{stdout}\n{stderr}\npipewire.log:\n{log}",
				output.status
			)
			.into());
		}

		Ok(())
	}

	async fn plays_and_drains_child() -> Result<(), TestError> {
		if std::env::var("PIPEWIRE_REMOTE").as_deref() != Ok(PRIVATE_REMOTE) {
			return Err("unexpected PIPEWIRE_REMOTE in private PipeWire child".into());
		}

		let linker = AutoLinker::spawn()?;
		wait_until(
			&linker.notify,
			Duration::from_secs(2),
			"null sink ports",
			|| linker.sink_ready.load(Ordering::Acquire),
		)
		.await?;

		let worker = TestWorker::spawn()?;
		let (pcm, format) = generate_stereo_sine(300, 48_000, 440.0);
		let deadline = Instant::now() + Duration::from_secs(5);

		worker.submit_pcm(pcm, format).await?;

		wait_until(
			&linker.notify,
			deadline.saturating_duration_since(Instant::now()),
			"stream ports to link to null sink",
			|| linker.linked_count.load(Ordering::Acquire) == 2,
		)
		.await?;
		log_pw_dump_snapshot();

		wait_until(
			&worker.stats.notify,
			deadline.saturating_duration_since(Instant::now()),
			"playback to drain and release permits",
			|| {
				worker.stats.drained.load(Ordering::Acquire) >= 1
					&& worker.stats.active_playbacks.load(Ordering::Acquire) == 0
					&& worker.permits.available_permits() == MAX_CONCURRENT
			},
		)
		.await?;

		assert_eq!(worker.stats.accepted.load(Ordering::Acquire), 1);
		assert_eq!(worker.stats.drained.load(Ordering::Acquire), 1);
		assert_eq!(worker.stats.stream_errors.load(Ordering::Acquire), 0);
		assert_eq!(worker.stats.stalled.load(Ordering::Acquire), 0);
		assert_eq!(worker.stats.timed_out.load(Ordering::Acquire), 0);
		assert_eq!(worker.stats.active_playbacks.load(Ordering::Acquire), 0);
		assert_eq!(worker.permits.available_permits(), MAX_CONCURRENT);
		assert_eq!(linker.linked_count.load(Ordering::Acquire), 2);

		worker.shutdown().await?;
		linker.stop();
		Ok(())
	}

	#[test]
	#[ignore = "spawns isolated child process with absent PipeWire socket"]
	fn absent_server_drops_sound_without_panic() -> Result<(), TestError> {
		if std::env::var_os("SUBNIRI_ABSENT_PIPEWIRE_CHILD").is_some() {
			return tokio::runtime::Builder::new_current_thread()
				.enable_all()
				.build()?
				.block_on(absent_server_child());
		}

		let runtime = TempRuntimeDir {
			directory: create_private_dir("subniri-pw-absent")?,
		};

		let output = std::process::Command::new(std::env::current_exe()?)
			.args([
				"--exact",
				"sound::tests::absent_server_drops_sound_without_panic",
				"--ignored",
				"--nocapture",
			])
			.env("SUBNIRI_ABSENT_PIPEWIRE_CHILD", "1")
			.env("PIPEWIRE_RUNTIME_DIR", &runtime.directory)
			.env("XDG_RUNTIME_DIR", &runtime.directory)
			.env("PIPEWIRE_REMOTE", ABSENT_REMOTE)
			.env("XDG_CONFIG_HOME", &runtime.directory)
			.output()?;

		let stdout = String::from_utf8_lossy(&output.stdout);
		let stderr = String::from_utf8_lossy(&output.stderr);
		eprint!("{stdout}{stderr}");
		assert!(
			output.status.success(),
			"absent PipeWire child failed ({}):\n{stdout}\n{stderr}",
			output.status
		);

		Ok(())
	}

	async fn absent_server_child() -> Result<(), TestError> {
		if std::env::var("PIPEWIRE_REMOTE").as_deref() != Ok(ABSENT_REMOTE) {
			return Err("unexpected PIPEWIRE_REMOTE in absent PipeWire child".into());
		}

		let worker = TestWorker::spawn()?;
		let (pcm, format) = generate_stereo_sine(200, 48_000, 440.0);

		worker.submit_pcm(pcm.clone(), format.clone()).await?;
		wait_until(
			&worker.stats.notify,
			Duration::from_secs(5),
			"absent server connect failure and permit release",
			|| {
				worker.stats.connect_failures.load(Ordering::Acquire) >= 1
					&& worker.stats.backoff_secs.load(Ordering::Acquire) >= 1
					&& worker.stats.active_playbacks.load(Ordering::Acquire) == 0
					&& worker.permits.available_permits() == MAX_CONCURRENT
			},
		)
		.await?;

		worker.submit_pcm(pcm, format).await?;
		wait_until(
			&worker.stats.notify,
			Duration::from_secs(2),
			"second sound dropped during reconnect backoff",
			|| {
				worker.stats.dropped_backoff.load(Ordering::Acquire) >= 1
					&& worker.permits.available_permits() == MAX_CONCURRENT
			},
		)
		.await?;

		assert_eq!(worker.stats.accepted.load(Ordering::Acquire), 0);
		assert_eq!(worker.stats.connect_failures.load(Ordering::Acquire), 1);
		assert!(worker.stats.backoff_secs.load(Ordering::Acquire) >= 1);
		assert_eq!(worker.stats.dropped_backoff.load(Ordering::Acquire), 1);
		assert_eq!(worker.stats.active_playbacks.load(Ordering::Acquire), 0);
		assert_eq!(worker.permits.available_permits(), MAX_CONCURRENT);

		worker.shutdown().await?;
		Ok(())
	}
}
