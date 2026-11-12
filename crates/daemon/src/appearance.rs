use config::{AppearanceMode, ConfigFile};
use daemon_common::{
	APPEARANCE_OBJECT_PATH, Appearance, AppearanceServer, AppearanceServerSignals, AppearanceState,
};
use tokio::sync::{RwLock, mpsc};
use zbus::{Connection, object_server::SignalEmitter};

use std::sync::Arc;

enum Command {
	Select(AppearanceMode),
	Retry,
	Configured,
}

type Persist = Arc<dyn Fn(AppearanceMode) -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
pub struct AppearanceService {
	state: Arc<RwLock<AppearanceState>>,
	commands: mpsc::UnboundedSender<Command>,
}

impl AppearanceService {
	pub async fn register(connection: Connection, mode: AppearanceMode) -> zbus::Result<Self> {
		Self::register_with_persistence(connection, mode, Arc::new(persist)).await
	}

	async fn register_with_persistence(
		connection: Connection, mode: AppearanceMode, persist: Persist,
	) -> zbus::Result<Self> {
		let (commands, receiver) = mpsc::unbounded_channel();
		let service = Self {
			state: Arc::new(RwLock::new(AppearanceState {
				theme: mode.as_str().into(),
				saving: false,
				error: String::new(),
				revision: 0,
			})),
			commands,
		};

		connection
			.object_server()
			.at(APPEARANCE_OBJECT_PATH, AppearanceServer(service.clone()))
			.await?;
		tokio::spawn(run(
			connection,
			service.state.clone(),
			receiver,
			mode,
			persist,
		));

		Ok(service)
	}

	pub fn reconfigure(&self) {
		let _ = self.commands.send(Command::Configured);
	}
}

impl Appearance for AppearanceService {
	async fn state(&self) -> AppearanceState {
		self.state.read().await.clone()
	}

	async fn select(&self, mode: &str) -> zbus::fdo::Result<()> {
		let mode = AppearanceMode::from_name(mode)
			.ok_or_else(|| zbus::fdo::Error::InvalidArgs("Expected light or dark".into()))?;

		self.commands
			.send(Command::Select(mode))
			.map_err(|_| zbus::fdo::Error::Failed("Appearance service stopped".into()))
	}

	async fn retry_saving(&self) -> zbus::fdo::Result<()> {
		self.commands
			.send(Command::Retry)
			.map_err(|_| zbus::fdo::Error::Failed("Appearance service stopped".into()))
	}
}

/// Only the actor changes these values: D-Bus calls, disk completions, and config
/// reloads cannot race each other or complete against an obsolete selection.
struct Session {
	effective: AppearanceMode,
	configured: AppearanceMode,
	override_active: bool,
}

impl Session {
	fn reconcile(&mut self, mode: AppearanceMode, override_active: bool) -> bool {
		let discarded = self.override_active && !override_active;
		let changed = mode != self.configured || discarded;
		self.configured = mode;
		self.override_active = override_active;

		if changed {
			self.effective = mode;
		}

		changed || mode == self.effective
	}
}

async fn run(
	connection: Connection, state: Arc<RwLock<AppearanceState>>,
	mut commands: mpsc::UnboundedReceiver<Command>, mode: AppearanceMode, persist: Persist,
) {
	let Ok(emitter) = SignalEmitter::new(&connection, APPEARANCE_OBJECT_PATH) else {
		return;
	};
	let mut session = Session {
		effective: mode,
		configured: mode,
		override_active: ConfigFile::override_active(),
	};
	let mut saving = None;
	let mut saving_theme = mode;
	let mut pending = false;
	let mut reload = false;

	loop {
		if saving.is_none() {
			if pending {
				pending = false;
				saving_theme = session.effective;
				let save = persist.clone();
				saving = Some(tokio::task::spawn_blocking(move || save(saving_theme)));
			} else if reload {
				reload = false;
				reload_configured(&mut session, &state, &emitter).await;
			}
		}

		tokio::select! {
			command = commands.recv() => {
				match command {
					Some(Command::Configured) => {
						// Re-read after our writes settle instead of applying stale watcher snapshots.
						reload = true;
						continue;
					}
					Some(Command::Select(theme)) => session.effective = theme,
					Some(Command::Retry) => {},
					None => return,
				}

				pending = true;
				publish(&state, &emitter, session.effective, true, String::new()).await;
			}
			result = save_completion(&mut saving) => {
				saving = None;
				let error = match result {
					Ok(Ok(())) => {
						session.configured = saving_theme;
						session.override_active = ConfigFile::override_active();
						String::new()
					}
					Ok(Err(error)) => error,
					Err(error) => error.to_string(),
				};

				if !error.is_empty() {
					log::warn!("Appearance changed but could not be saved: {error}");
				}
				// A stale completion cannot clear the newest selection's saving status.
				if !pending {
					publish(&state, &emitter, session.effective, false, error).await;
				}
			}
		}
	}
}

async fn save_completion(
	saving: &mut Option<tokio::task::JoinHandle<Result<(), String>>>,
) -> Result<Result<(), String>, tokio::task::JoinError> {
	match saving {
		Some(task) => task.await,
		None => std::future::pending().await,
	}
}

async fn reload_configured(
	session: &mut Session, state: &RwLock<AppearanceState>, emitter: &SignalEmitter<'_>,
) {
	let loaded = tokio::task::spawn_blocking(|| {
		ConfigFile::load()
			.map(|(_, config)| (config.appearance.theme, ConfigFile::override_active()))
			.map_err(|error| error.to_string())
	})
	.await;

	match loaded {
		Ok(Ok((theme, override_active))) if session.reconcile(theme, override_active) => {
			publish(state, emitter, session.effective, false, String::new()).await;
		}
		Ok(Err(error)) => log::warn!("Could not reload appearance: {error}"),
		Err(error) => log::warn!("Appearance reload task failed: {error}"),
		_ => {}
	}
}

fn persist(mode: AppearanceMode) -> Result<(), String> {
	ConfigFile::update(|config| config.appearance.theme = mode)
		.map(|_| ())
		.map_err(|error| error.to_string())
}

async fn publish(
	state: &RwLock<AppearanceState>, emitter: &SignalEmitter<'_>, mode: AppearanceMode,
	saving: bool, error: String,
) {
	let snapshot = {
		let mut current = state.write().await;
		current.theme = mode.as_str().into();
		current.saving = saving;
		current.error = error;
		current.revision += 1;
		current.clone()
	};

	if let Err(error) = emitter.changed(snapshot).await {
		log::warn!("Failed to announce appearance: {error}");
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use daemon_common::{AppearanceProxy, DEFAULT_BUS_NAME};
	use futures::StreamExt;
	use std::sync::atomic::{AtomicBool, Ordering};
	use std::time::Duration;

	async fn test_service(
		mode: AppearanceMode, persist: Persist,
	) -> (AppearanceService, Connection) {
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
		let service = AppearanceService::register_with_persistence(server.unwrap(), mode, persist)
			.await
			.unwrap();

		(service, client.unwrap())
	}

	async fn test_proxy(client: &Connection) -> AppearanceProxy<'_> {
		AppearanceProxy::builder(client)
			.destination(DEFAULT_BUS_NAME)
			.unwrap()
			.path(APPEARANCE_OBJECT_PATH)
			.unwrap()
			.build()
			.await
			.unwrap()
	}

	async fn next_state(
		stream: &mut (impl futures::Stream<Item = AppearanceState> + Unpin),
	) -> AppearanceState {
		tokio::time::timeout(Duration::from_secs(5), stream.next())
			.await
			.expect("appearance signal timed out")
			.expect("appearance signal stream closed")
	}

	#[tokio::test]
	async fn appearance_failed_save_is_shared_and_retry_persists_latest_theme() {
		let fail = Arc::new(AtomicBool::new(true));
		let stored = Arc::new(std::sync::Mutex::new(AppearanceMode::Light));
		let save = {
			let fail = fail.clone();
			let stored = stored.clone();
			Arc::new(move |theme| {
				if fail.load(Ordering::SeqCst) {
					return Err("read-only test configuration".to_owned());
				}
				*stored.lock().unwrap() = theme;
				Ok(())
			}) as Persist
		};
		let (_service, client) = test_service(AppearanceMode::Light, save.clone()).await;
		let proxy = test_proxy(&client).await;
		let mut changes = proxy
			.receive_changed()
			.await
			.unwrap()
			.map(|signal| signal.args().unwrap().state);

		proxy.select("dark").await.unwrap();
		assert!(next_state(&mut changes).await.saving);
		let unsaved = next_state(&mut changes).await;

		assert_eq!(unsaved.theme, "dark");
		assert!(!unsaved.saving);
		assert!(!unsaved.error.is_empty());
		assert_eq!(*stored.lock().unwrap(), AppearanceMode::Light);
		assert_eq!(proxy.state().await.unwrap(), unsaved);
		assert!(proxy.select("unknown").await.is_err());

		// A fresh service starts from persisted configuration, not the previous session's unsaved choice.
		let persisted = *stored.lock().unwrap();
		let (_restarted_service, restarted_client) = test_service(persisted, save).await;
		let restarted = test_proxy(&restarted_client).await.state().await.unwrap();
		assert_eq!(restarted.theme, "light");
		assert!(!restarted.saving);
		assert!(restarted.error.is_empty());
		assert_eq!(restarted.revision, 0);

		fail.store(false, Ordering::SeqCst);
		proxy.retry_saving().await.unwrap();
		assert!(next_state(&mut changes).await.saving);
		let saved = next_state(&mut changes).await;

		assert_eq!(saved.theme, "dark");
		assert!(!saved.saving);
		assert!(saved.error.is_empty());
		assert!(saved.revision > unsaved.revision);
		assert_eq!(*stored.lock().unwrap(), AppearanceMode::Dark);
	}

	#[tokio::test]
	async fn appearance_slow_save_does_not_block_selection_or_publish_stale_completion() {
		let (started, mut saves) = mpsc::unbounded_channel();
		let persist: Persist = Arc::new(move |theme| {
			let (release, gate) = tokio::sync::oneshot::channel();
			started.send((theme, release)).unwrap();
			gate.blocking_recv().expect("test dropped persistence gate")
		});
		let (_service, client) = test_service(AppearanceMode::Light, persist).await;
		let proxy = test_proxy(&client).await;
		let mut changes = proxy
			.receive_changed()
			.await
			.unwrap()
			.map(|signal| signal.args().unwrap().state);

		proxy.select("dark").await.unwrap();
		let first = next_state(&mut changes).await;
		let (theme, release_first) = tokio::time::timeout(Duration::from_secs(5), saves.recv())
			.await
			.unwrap()
			.unwrap();
		assert_eq!(theme, AppearanceMode::Dark);
		assert_eq!(first.theme, "dark");
		assert!(first.saving);

		// The first disk operation is still gated when the newer selection becomes observable.
		proxy.select("light").await.unwrap();
		let newer = next_state(&mut changes).await;
		assert_eq!(newer.theme, "light");
		assert!(newer.saving);
		assert!(newer.error.is_empty());
		assert!(newer.revision > first.revision);
		assert_eq!(proxy.state().await.unwrap(), newer);

		release_first.send(Ok(())).unwrap();
		let (theme, release_second) = tokio::time::timeout(Duration::from_secs(5), saves.recv())
			.await
			.unwrap()
			.unwrap();
		assert_eq!(theme, AppearanceMode::Light);
		// Starting the second save proves the actor handled the stale completion.
		assert_eq!(proxy.state().await.unwrap(), newer);

		release_second.send(Ok(())).unwrap();
		let saved = next_state(&mut changes).await;
		assert_eq!(saved.theme, "light");
		assert!(!saved.saving);
		assert!(saved.error.is_empty());
		assert_eq!(saved.revision, newer.revision + 1);
		assert_eq!(proxy.state().await.unwrap(), saved);
	}

	#[test]
	fn appearance_override_removal_discards_unsaved_selection_with_unchanged_configuration() {
		let mut session = Session {
			effective: AppearanceMode::Dark,
			configured: AppearanceMode::Light,
			override_active: true,
		};

		assert!(!session.reconcile(AppearanceMode::Light, true));
		assert_eq!(session.effective, AppearanceMode::Dark);

		assert!(session.reconcile(AppearanceMode::Light, false));
		assert_eq!(session.effective, AppearanceMode::Light);
		assert_eq!(session.configured, AppearanceMode::Light);
		assert!(!session.override_active);
	}

	#[test]
	fn appearance_unrelated_reload_preserves_unsaved_selection() {
		let mut session = Session {
			effective: AppearanceMode::Dark,
			configured: AppearanceMode::Light,
			override_active: true,
		};

		assert!(!session.reconcile(AppearanceMode::Light, true));
		assert_eq!(session.effective, AppearanceMode::Dark);

		assert!(session.reconcile(AppearanceMode::Dark, true));
		assert!(session.reconcile(AppearanceMode::Light, true));
		assert_eq!(session.effective, AppearanceMode::Light);
	}
}
