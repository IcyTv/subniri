//! Session-wide filesystem invalidation. Clients keep their own parsed entries
//! and image handles; only independent discovery revisions cross D-Bus.
use std::{
	collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
	fs,
	os::unix::fs::MetadataExt,
	path::{Component, Path, PathBuf},
	sync::Arc,
	time::{Duration, SystemTime},
};

use daemon_common::{
	DISCOVERY_OBJECT_PATH, Discovery, DiscoveryServer, DiscoveryServerSignals, DiscoveryState,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{RwLock, mpsc, watch};
use zbus::{Connection, object_server::SignalEmitter};

const DEBOUNCE: Duration = Duration::from_millis(300);
const RECONCILE: Duration = Duration::from_secs(300);

#[derive(Clone, Default)]
pub struct DiscoveryService(Arc<RwLock<DiscoveryState>>);

impl Discovery for DiscoveryService {
	async fn state(&self) -> DiscoveryState {
		*self.0.read().await
	}
}

impl DiscoveryService {
	pub async fn register(connection: Connection) -> zbus::Result<Self> {
		let service = Self::default();
		connection
			.object_server()
			.at(DISCOVERY_OBJECT_PATH, DiscoveryServer(service.clone()))
			.await?;
		Ok(service)
	}

	pub async fn run(self, connection: Connection, mut shutdown: watch::Receiver<bool>) {
		let (events, mut incoming) = mpsc::channel(1);
		let interests = Arc::new(std::sync::RwLock::new(Interests::default()));
		let callback_interests = interests.clone();
		let callback = move |event: notify::Result<notify::Event>| {
			// Access events include our own scans and must never trigger another scan.
			if event.as_ref().is_ok_and(|event| event.kind.is_access()) {
				return;
			}
			if let Ok(event) = &event {
				if !event.need_rescan()
					&& callback_interests.read().is_ok_and(|interests| {
						!event.paths.iter().any(|path| interests.contains(path))
					}) {
					return;
				}
			}
			let _ = events.try_send(());
		};
		let watcher = notify::recommended_watcher(callback);
		if let Err(error) = &watcher {
			log::warn!("Application/icon watcher unavailable; using reconciliation: {error}");
		}
		let mut monitor = Monitor {
			watcher: watcher.ok(),
			watched: BTreeMap::new(),
			snapshot: Snapshot::default(),
			interests,
		};
		monitor = tokio::task::spawn_blocking(move || {
			monitor.scan();
			monitor
		})
		.await
		.unwrap();
		let mut interval =
			tokio::time::interval_at(tokio::time::Instant::now() + RECONCILE, RECONCILE);
		interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
		let emitter = SignalEmitter::new(&connection, DISCOVERY_OBJECT_PATH).unwrap();
		// Clients can connect while the initial scan is still running. Publish a
		// baseline revision too, so their earlier snapshots cannot stay stale.
		self.publish(&emitter, true, true).await;

		loop {
			tokio::select! {
				_ = shutdown.changed() => return,
				_ = interval.tick() => {},
				Some(()) = incoming.recv() => {
					// Bound latency even during continuous package-manager writes.
					tokio::time::sleep(DEBOUNCE).await;
					while incoming.try_recv().is_ok() {}
				},
			}

			let result = tokio::task::spawn_blocking(move || {
				let changed = monitor.scan();
				(monitor, changed)
			})
			.await;
			let (next, (applications, icons)) = match result {
				Ok(result) => result,
				Err(error) => {
					log::error!("Discovery scan failed: {error}");
					return;
				}
			};
			monitor = next;
			if !applications && !icons {
				continue;
			}

			self.publish(&emitter, applications, icons).await;
		}
	}

	async fn publish(&self, emitter: &SignalEmitter<'_>, applications: bool, icons: bool) {
		let state = {
			let mut state = self.0.write().await;
			state.applications = state.applications.wrapping_add(u64::from(applications));
			state.icons = state.icons.wrapping_add(u64::from(icons));
			*state
		};
		if let Err(error) = emitter.changed(state).await {
			log::warn!("Cannot publish discovery changes: {error}");
		}
	}
}

struct Monitor {
	watcher: Option<RecommendedWatcher>,
	watched: BTreeMap<PathBuf, (u64, u64)>,
	snapshot: Snapshot,
	interests: Arc<std::sync::RwLock<Interests>>,
}

impl Monitor {
	fn scan(&mut self) -> (bool, bool) {
		let applications = freedesktop_desktop_entry::default_paths().collect::<Vec<_>>();
		let icons = icon::IconSearch::new().dirs;
		self.scan_roots(&applications, &icons)
	}

	fn scan_roots(&mut self, applications: &[PathBuf], icons: &[PathBuf]) -> (bool, bool) {
		let snapshot = Snapshot::read(applications, icons);
		if let Ok(mut interests) = self.interests.write() {
			*interests = snapshot.interests.clone();
		}
		if let Some(watcher) = &mut self.watcher {
			let desired: BTreeMap<_, _> = snapshot
				.watches
				.iter()
				.filter_map(|path| {
					let metadata = fs::metadata(path).ok()?;
					Some((path.clone(), (metadata.dev(), metadata.ino())))
				})
				.collect();
			for (path, identity) in &self.watched {
				if desired.get(path) != Some(identity) {
					let _ = watcher.unwatch(path);
				}
			}
			let mut watched = BTreeMap::new();
			for (path, identity) in desired {
				// A replacement at the same pathname has a new inode and needs a new watch.
				if self.watched.get(&path) == Some(&identity)
					|| watcher.watch(&path, RecursiveMode::NonRecursive).is_ok()
				{
					watched.insert(path, identity);
				}
			}
			self.watched = watched;
		}
		let changed = (
			self.snapshot.applications != snapshot.applications,
			self.snapshot.icons != snapshot.icons,
		);
		self.snapshot = snapshot;
		changed
	}
}

#[derive(Debug, PartialEq, Eq)]
struct Stamp {
	device: u64,
	inode: u64,
	length: u64,
	modified: Option<SystemTime>,
	changed: (i64, i64),
	target: Option<PathBuf>,
}

impl Stamp {
	fn read(path: &Path, metadata: &fs::Metadata) -> Self {
		Self {
			device: metadata.dev(),
			inode: metadata.ino(),
			length: metadata.len(),
			modified: metadata.modified().ok(),
			changed: (metadata.ctime(), metadata.ctime_nsec()),
			target: if metadata.is_symlink() {
				fs::read_link(path).ok()
			} else {
				None
			},
		}
	}
}

type Inventory = BTreeMap<PathBuf, Stamp>;

#[derive(Default)]
struct Snapshot {
	applications: Inventory,
	icons: Inventory,
	watches: BTreeSet<PathBuf>,
	interests: Interests,
}

#[derive(Clone, Default)]
struct Interests {
	exact: HashSet<PathBuf>,
	directories: HashSet<PathBuf>,
}

impl Interests {
	fn contains(&self, path: &Path) -> bool {
		self.exact.contains(path)
			|| self.directories.contains(path)
			|| path
				.parent()
				.is_some_and(|parent| self.directories.contains(parent))
	}
}

impl Snapshot {
	fn read(applications: &[PathBuf], icons: &[PathBuf]) -> Self {
		let mut snapshot = Self::default();
		collect(
			applications,
			&mut snapshot.applications,
			&mut snapshot.watches,
			&mut snapshot.interests,
		);
		collect(
			icons,
			&mut snapshot.icons,
			&mut snapshot.watches,
			&mut snapshot.interests,
		);
		snapshot
	}
}

fn collect(
	roots: &[PathBuf], inventory: &mut Inventory, watches: &mut BTreeSet<PathBuf>,
	interests: &mut Interests,
) {
	let mut visited = HashSet::new();
	let mut pending: Vec<_> = roots
		.iter()
		.filter_map(|path| resolve(path, inventory, watches, interests))
		.collect();
	while let Some(path) = pending.pop() {
		let Ok(metadata) = fs::symlink_metadata(&path) else {
			continue;
		};
		inventory.insert(path.clone(), Stamp::read(&path, &metadata));
		if metadata.is_symlink() {
			if let Some(target) = resolve(&path, inventory, watches, interests) {
				pending.push(target);
			}
			continue;
		}
		if !metadata.is_dir() || !visited.insert(path.clone()) {
			continue;
		}

		watches.insert(path.clone());
		interests.directories.insert(path.clone());
		if let Ok(entries) = fs::read_dir(path) {
			pending.extend(entries.filter_map(Result::ok).map(|entry| entry.path()));
		}
	}
}

/// Resolve each symlink hop, retaining watches on its parent. Watching only a
/// canonical applications directory misses Nix profile ancestor replacements.
fn resolve(
	path: &Path, inventory: &mut Inventory, watches: &mut BTreeSet<PathBuf>,
	interests: &mut Interests,
) -> Option<PathBuf> {
	let mut pending: VecDeque<_> = path
		.components()
		.map(|part| part.as_os_str().to_owned())
		.collect();
	let mut resolved = PathBuf::new();
	let mut links = 0;
	while let Some(part) = pending.pop_front() {
		if part == ".." {
			resolved.pop();
			continue;
		}
		if part == "." {
			continue;
		}
		resolved.push(part);
		interests.exact.insert(resolved.clone());
		let parent = resolved.parent().filter(|path| path.is_dir());
		if let Some(parent) = parent {
			watches.insert(parent.to_path_buf());
		}
		let Ok(metadata) = fs::symlink_metadata(&resolved) else {
			return None;
		};
		if metadata.is_symlink() {
			links += 1;
			if links > 40 {
				return None;
			}
			inventory.insert(resolved.clone(), Stamp::read(&resolved, &metadata));
			let target = fs::read_link(&resolved).ok()?;
			resolved.pop();
			if target.is_absolute() {
				resolved.clear();
			}
			for component in target.components().rev() {
				if component != Component::CurDir {
					pending.push_front(component.as_os_str().to_owned());
				}
			}
		}
	}
	Some(resolved)
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::{os::unix::fs::symlink, time::UNIX_EPOCH};

	fn fixture() -> PathBuf {
		let root = Path::new(env!("CARGO_MANIFEST_DIR"))
			.join("../../.opencode/app-discovery/tests")
			.join(format!(
				"daemon-{}-{}",
				std::process::id(),
				SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.unwrap()
					.as_nanos()
			));
		fs::create_dir_all(&root).unwrap();
		root.canonicalize().unwrap()
	}

	#[test]
	fn discovery_distinguishes_application_and_icon_edits_and_removals() {
		let root = fixture();
		let apps = root.join("applications");
		let icons = root.join("icons");
		fs::create_dir(&apps).unwrap();
		fs::create_dir(&icons).unwrap();
		let before = Snapshot::read(&[apps.clone()], &[icons.clone()]);
		fs::write(apps.join("app.desktop"), "Name=Original").unwrap();
		let installed = Snapshot::read(&[apps.clone()], &[icons.clone()]);
		assert_ne!(before.applications, installed.applications);
		assert_eq!(before.icons, installed.icons);

		fs::write(apps.join("app.desktop"), "Name=Changed application").unwrap();
		let edited = Snapshot::read(&[apps.clone()], &[icons.clone()]);
		assert_ne!(installed.applications, edited.applications);
		fs::write(icons.join("app.svg"), "<svg/>").unwrap();
		let themed = Snapshot::read(&[apps.clone()], &[icons.clone()]);
		assert_eq!(edited.applications, themed.applications);
		assert_ne!(edited.icons, themed.icons);

		fs::remove_file(apps.join("app.desktop")).unwrap();
		let removed = Snapshot::read(&[apps], &[icons]);
		assert_ne!(themed.applications, removed.applications);
		assert_eq!(themed.icons, removed.icons);
	}

	#[test]
	fn discovery_follows_profile_symlink_chains_and_ignores_unrelated_parent_events() {
		let root = fixture();
		for generation in ["old", "new"] {
			fs::create_dir_all(root.join(generation).join("share/applications")).unwrap();
			fs::write(
				root.join(generation).join("share/applications/app.desktop"),
				generation,
			)
			.unwrap();
		}
		symlink("old", root.join("generation")).unwrap();
		symlink("generation", root.join("profile")).unwrap();
		let apps = root.join("profile/share/applications");
		let before = Snapshot::read(&[apps.clone()], &[]);
		assert!(before.watches.contains(&root));
		assert!(before.interests.contains(&root.join("generation")));
		assert!(!before.interests.contains(&root.join("unrelated")));

		symlink("new", root.join("next")).unwrap();
		fs::rename(root.join("next"), root.join("generation")).unwrap();
		let switched = Snapshot::read(&[apps], &[]);
		assert_ne!(before.applications, switched.applications);
		assert!(
			switched
				.watches
				.contains(&root.join("new/share/applications"))
		);
		assert!(
			!switched
				.watches
				.contains(&root.join("old/share/applications"))
		);
	}

	#[test]
	fn discovery_handles_initially_missing_roots_and_symlink_cycles() {
		let root = fixture();
		let apps = root.join("missing/applications");
		let before = Snapshot::read(&[apps.clone()], &[]);
		assert!(before.watches.contains(&root));
		assert!(before.interests.contains(&root.join("missing")));
		assert!(before.applications.is_empty());

		fs::create_dir_all(&apps).unwrap();
		symlink(".", apps.join("cycle")).unwrap();
		fs::write(apps.join("app.desktop"), "Name=Available").unwrap();
		let available = Snapshot::read(&[apps], &[]);
		assert_ne!(before.applications, available.applications);
		assert!(
			available.applications.len() < 10,
			"cycles must not recurse indefinitely"
		);
	}

	#[test]
	fn discovery_reattaches_watches_after_directory_replacement() {
		let root = fixture();
		let apps = root.join("applications");
		fs::create_dir(&apps).unwrap();
		let (sender, incoming) = std::sync::mpsc::channel();
		let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
			if let Ok(event) = event {
				if !event.kind.is_access() {
					let _ = sender.send(event);
				}
			}
		})
		.unwrap();
		let mut monitor = Monitor {
			watcher: Some(watcher),
			watched: BTreeMap::new(),
			snapshot: Snapshot::default(),
			interests: Arc::new(std::sync::RwLock::new(Interests::default())),
		};
		monitor.scan_roots(&[apps.clone()], &[]);
		let identity = monitor.watched[&apps];
		fs::rename(&apps, root.join("old-applications")).unwrap();
		fs::create_dir(&apps).unwrap();
		assert_eq!(monitor.scan_roots(&[apps.clone()], &[]), (true, false));
		assert_ne!(monitor.watched[&apps], identity);

		let entry = apps.join("new.desktop");
		fs::write(&entry, "Name=New").unwrap();
		let deadline = std::time::Instant::now() + Duration::from_secs(5);
		loop {
			let event = incoming
				.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
				.unwrap();
			if event.paths.contains(&entry) {
				break;
			}
		}
	}

	#[tokio::test]
	async fn discovery_protocol_exposes_independent_revisions_and_change_signals() {
		use daemon_common::{DEFAULT_BUS_NAME, DiscoveryProxy};
		use futures::StreamExt;

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
		let server = server.unwrap();
		let client = client.unwrap();
		let service = DiscoveryService::register(server.clone()).await.unwrap();
		let proxy = DiscoveryProxy::builder(&client)
			.destination(DEFAULT_BUS_NAME)
			.unwrap()
			.path(DISCOVERY_OBJECT_PATH)
			.unwrap()
			.build()
			.await
			.unwrap();
		let mut changes = proxy.receive_changed().await.unwrap();
		assert_eq!(proxy.state().await.unwrap(), DiscoveryState::default());

		let emitter = SignalEmitter::new(&server, DISCOVERY_OBJECT_PATH).unwrap();
		for (applications, icons, state) in [
			(
				true,
				false,
				DiscoveryState {
					applications: 1,
					icons: 0,
				},
			),
			(
				false,
				true,
				DiscoveryState {
					applications: 1,
					icons: 1,
				},
			),
		] {
			service.publish(&emitter, applications, icons).await;
			let signal = tokio::time::timeout(Duration::from_secs(5), changes.next())
				.await
				.unwrap()
				.unwrap();
			assert_eq!(signal.args().unwrap().state, state);
			assert_eq!(proxy.state().await.unwrap(), state);
		}
	}
}
