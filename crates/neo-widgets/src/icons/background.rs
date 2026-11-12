//! Nonblocking view-time lookups. Subscribe to [`updates`] to rebuild when
//! results arrive. Pending and missing icons use the caller's normal fallback.
use super::{ApplicationIconResolver, DiscoveryChange, ResolvedIcon, default_icon};
use std::{
	collections::HashMap,
	path::{Path, PathBuf},
	sync::{Arc, Mutex, OnceLock, TryLockError},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Source {
	Name(String, Option<PathBuf>),
	AppId(String),
	Window(String, Option<i32>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Request {
	source: Source,
	size: u32,
	scale: u32,
}

enum Entry {
	Pending,
	Ready(Option<ResolvedIcon>),
}

#[derive(Default)]
struct Cache {
	entries: HashMap<Request, Entry>,
	applications: u64,
	icons: u64,
}

impl std::ops::Deref for Cache {
	type Target = HashMap<Request, Entry>;

	fn deref(&self) -> &Self::Target {
		&self.entries
	}
}

impl std::ops::DerefMut for Cache {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.entries
	}
}

impl Cache {
	fn revision(&self, request: &Request) -> (u64, u64) {
		let applications = if matches!(request.source, Source::Name(..)) {
			0
		} else {
			self.applications
		};
		(applications, self.icons)
	}
}

struct Service {
	entries: Arc<Mutex<Cache>>,
	requests: async_channel::Sender<((u64, u64), Request)>,
	changed: async_channel::Receiver<()>,
	wake: async_channel::Sender<()>,
}

impl Service {
	fn new() -> Self {
		Self::start(|| {
			// Discovery, including desktop-entry scanning, must run on this thread.
			let mut resolver = ApplicationIconResolver::new();
			move |request: &Request| {
				let icon = match &request.source {
					Source::Name(name, path) => {
						if let Some(path) = path {
							resolver.add_icon_dir(path);
						}
						resolver.get_icon_by_name(name, request.size, request.scale)
					}
					Source::AppId(id) => {
						resolver.get_icon_by_app_id(id, request.size, request.scale)
					}
					Source::Window(id, pid) => {
						resolver.get_icon_by_window_identity(id, *pid, request.size, request.scale)
					}
				};
				(resolver.generation, icon.map(ResolvedIcon::from_icon_file))
			}
		})
	}

	fn start<F>(initialize: impl FnOnce() -> F + Send + 'static) -> Self
	where
		F: FnMut(&Request) -> (u64, Option<ResolvedIcon>) + Send + 'static,
	{
		let entries = Arc::new(Mutex::new(Cache::default()));
		// Bound outstanding work; duplicate requests never consume queue slots.
		let (requests, incoming) = async_channel::bounded::<((u64, u64), Request)>(128);
		let (wake, changed) = async_channel::bounded(1);
		let worker_entries = entries.clone();
		let worker_wake = wake.clone();
		if let Err(error) = std::thread::Builder::new()
			.name("icon-lookup".into())
			.spawn(move || {
				let mut lookup = initialize();
				let mut generation = 0;
				while let Ok((revision, request)) = incoming.recv_blocking() {
					// Never hold the shared result lock while accessing the filesystem.
					let (current_generation, result) = lookup(&request);
					let Ok(mut entries) = worker_entries.lock() else {
						break;
					};
					if entries.revision(&request) != revision {
						continue;
					}
					if generation != current_generation {
						// Queued requests will resolve against the new generation. Old
						// ready entries must not hide newly available icons (or misses).
						entries.retain(|_, entry| matches!(entry, Entry::Pending));
						generation = current_generation;
					}
					entries.insert(request, Entry::Ready(result));
					drop(entries);
					// One notification covers all completions until the next UI update.
					let _ = worker_wake.try_send(());
				}
			}) {
			log::error!("Cannot start icon lookup worker: {error}");
		}
		Self {
			entries,
			requests,
			changed,
			wake,
		}
	}

	fn lookup(&self, request: Request) -> Option<ResolvedIcon> {
		// Even publishing a large invalidation must not make a view wait.
		let mut entries = match self.entries.try_lock() {
			Ok(entries) => entries,
			Err(TryLockError::WouldBlock) => {
				let _ = self.wake.try_send(());
				return None;
			}
			Err(TryLockError::Poisoned(_)) => return None,
		};
		match entries.get(&request) {
			Some(Entry::Ready(icon)) => return icon.clone(),
			Some(Entry::Pending) => return None,
			None => {}
		}
		entries.insert(request.clone(), Entry::Pending);
		let revision = entries.revision(&request);
		if self.requests.try_send((revision, request.clone())).is_err() {
			// A queued completion will wake the UI to retry a full queue. A dead
			// worker simply leaves the normal fallback, without a redraw loop.
			entries.remove(&request);
		}
		None
	}

	fn invalidate(&self, change: DiscoveryChange) {
		let Ok(mut entries) = self.entries.lock() else {
			return;
		};
		if change.applications {
			entries.applications = entries.applications.wrapping_add(1);
		}
		if change.icons {
			entries.icons = entries.icons.wrapping_add(1);
		}
		entries.retain(|request, _| {
			!change.icons && (!change.applications || matches!(request.source, Source::Name(..)))
		});
		drop(entries);
		let _ = self.wake.try_send(());
	}
}

pub(super) fn invalidate(change: DiscoveryChange) {
	service().invalidate(change);
}

fn service() -> &'static Service {
	static SERVICE: OnceLock<Service> = OnceLock::new();
	SERVICE.get_or_init(Service::new)
}

/// A coalesced completion/invalidation notification; no per-frame polling.
pub fn updates() -> iced::Subscription<()> {
	iced::Subscription::run(|| {
		iced::futures::stream::unfold(service().changed.clone(), |receiver| async move {
			receiver.recv().await.ok().map(|()| ((), receiver))
		})
	})
}

pub fn resolve_icon(app_id: impl AsRef<str>, size: u32, scale: u32) -> ResolvedIcon {
	service()
		.lookup(Request {
			source: Source::AppId(app_id.as_ref().into()),
			size,
			scale,
		})
		.unwrap_or_else(default_icon)
}

pub fn try_resolve_icon_name(name: impl AsRef<str>, size: u32, scale: u32) -> Option<ResolvedIcon> {
	service().lookup(Request {
		source: Source::Name(name.as_ref().into(), None),
		size,
		scale,
	})
}

pub fn try_resolve_icon_name_in_path(
	name: impl AsRef<str>, path: impl AsRef<Path>, size: u32, scale: u32,
) -> Option<ResolvedIcon> {
	service().lookup(Request {
		source: Source::Name(name.as_ref().into(), Some(path.as_ref().into())),
		size,
		scale,
	})
}

pub fn resolve_from_window(window: &niri_ipc::Window, size: u32, scale: u32) -> ResolvedIcon {
	window
		.app_id
		.as_ref()
		.and_then(|id| {
			service().lookup(Request {
				source: Source::Window(id.clone(), window.pid),
				size,
				scale,
			})
		})
		.unwrap_or_else(default_icon)
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::{
		atomic::{AtomicUsize, Ordering},
		mpsc,
	};
	use std::time::Duration;

	fn name(value: &str) -> Request {
		Request {
			source: Source::Name(value.into(), None),
			size: 24,
			scale: 1,
		}
	}

	fn nonblocking(service: &Arc<Service>, request: Request) -> Option<ResolvedIcon> {
		let service = service.clone();
		let (tx, rx) = mpsc::channel();
		std::thread::spawn(move || tx.send(service.lookup(request)).unwrap());
		rx.recv_timeout(Duration::from_secs(1))
			.expect("UI waited for icon worker")
	}

	#[test]
	fn application_invalidation_preserves_named_results_and_rejects_in_flight_app_results() {
		let (started, busy) = mpsc::channel();
		let (release, gate) = mpsc::channel();
		let service = Service::start(move || {
			move |request: &Request| {
				if matches!(request.source, Source::AppId(_)) {
					started.send(()).unwrap();
					gate.recv().unwrap();
				}
				(0, Some(default_icon()))
			}
		});
		let named = name("retained");
		service.lookup(named.clone());
		service.changed.recv_blocking().unwrap();
		let app = Request {
			source: Source::AppId("changed".into()),
			size: 24,
			scale: 1,
		};
		service.lookup(app.clone());
		busy.recv_timeout(Duration::from_secs(1)).unwrap();

		service.invalidate(DiscoveryChange {
			applications: true,
			icons: false,
		});
		assert!(service.lookup(named.clone()).is_some());
		release.send(()).unwrap();
		// A later named request is a FIFO barrier behind the obsolete app lookup.
		service.lookup(name("barrier"));
		loop {
			service.changed.recv_blocking().unwrap();
			let entries = service.entries.lock().unwrap();
			if matches!(entries.get(&name("barrier")), Some(Entry::Ready(_))) {
				break;
			}
		}
		assert!(!service.entries.lock().unwrap().contains_key(&app));

		service.invalidate(DiscoveryChange {
			applications: false,
			icons: true,
		});
		assert!(service.entries.lock().unwrap().is_empty());
	}

	#[test]
	fn cold_initialization_does_not_block_and_pending_requests_are_deduplicated() {
		let (started, initialized) = mpsc::channel();
		let (release, gate) = mpsc::channel();
		let calls = Arc::new(AtomicUsize::new(0));
		let count = calls.clone();
		let service = Arc::new(Service::start(move || {
			started.send(()).unwrap();
			gate.recv().unwrap();
			move |_: &Request| {
				count.fetch_add(1, Ordering::SeqCst);
				(0, Some(default_icon()))
			}
		}));
		initialized.recv_timeout(Duration::from_secs(1)).unwrap();
		for _ in 0..10 {
			assert!(nonblocking(&service, name("spotify")).is_none());
		}
		assert_eq!(service.requests.len(), 1);
		release.send(()).unwrap();
		service.changed.recv_blocking().unwrap();
		assert!(nonblocking(&service, name("spotify")).is_some());
		assert_eq!(calls.load(Ordering::SeqCst), 1);
		assert!(
			service.changed.try_recv().is_err(),
			"idle cache hits must not redraw"
		);
	}

	#[test]
	fn slow_missing_lookup_does_not_block_and_completion_wakes_an_idle_ui() {
		let (started, busy) = mpsc::channel();
		let (release, gate) = mpsc::channel();
		let calls = Arc::new(AtomicUsize::new(0));
		let count = calls.clone();
		let service = Arc::new(Service::start(move || {
			move |_: &Request| {
				count.fetch_add(1, Ordering::SeqCst);
				started.send(()).unwrap();
				gate.recv().unwrap();
				(0, None)
			}
		}));
		assert!(nonblocking(&service, name("sonarr-missing")).is_none());
		busy.recv_timeout(Duration::from_secs(1)).unwrap();
		assert!(nonblocking(&service, name("sonarr-missing")).is_none());
		release.send(()).unwrap();
		service.changed.recv_blocking().unwrap();
		assert!(nonblocking(&service, name("sonarr-missing")).is_none());
		assert_eq!(calls.load(Ordering::SeqCst), 1, "misses must stay cached");
		assert!(service.requests.is_empty());
	}

	#[test]
	fn publishing_results_never_blocks_a_view_on_the_cache_lock() {
		let service = Arc::new(Service::start(|| |_: &Request| (0, None)));
		let guard = service.entries.lock().unwrap();
		assert!(nonblocking(&service, name("spotify")).is_none());
		assert!(
			service.changed.try_recv().is_ok(),
			"contention needs a retry wakeup"
		);
		drop(guard);
	}

	#[test]
	fn generation_changes_invalidate_ready_hits_and_misses_without_losing_queued_requests() {
		let service = Service::start(|| {
			|request: &Request| {
				if request.source == Source::Name("new-theme".into(), None) {
					(1, Some(default_icon()))
				} else if request.source == Source::Name("old-hit".into(), None) {
					(0, Some(default_icon()))
				} else {
					(0, None)
				}
			}
		});
		service.lookup(name("old-missing"));
		service.changed.recv_blocking().unwrap();
		service.lookup(name("old-hit"));
		service.changed.recv_blocking().unwrap();
		service.lookup(name("new-theme"));
		service.changed.recv_blocking().unwrap();
		let entries = service.entries.lock().unwrap();
		assert!(!entries.contains_key(&name("old-missing")));
		assert!(!entries.contains_key(&name("old-hit")));
		assert!(matches!(
			entries.get(&name("new-theme")),
			Some(Entry::Ready(Some(_)))
		));
	}

	#[test]
	fn changed_or_removed_items_cannot_receive_an_old_requests_icon() {
		let (started, busy) = mpsc::channel();
		let (release, gate) = mpsc::channel();
		let service = Service::start(move || {
			move |_: &Request| {
				started.send(()).unwrap();
				gate.recv().unwrap();
				(0, Some(default_icon()))
			}
		});
		service.lookup(name("old-item"));
		busy.recv_timeout(Duration::from_secs(1)).unwrap();
		service.lookup(name("replacement-item"));
		release.send(()).unwrap();
		service.changed.recv_blocking().unwrap();
		busy.recv_timeout(Duration::from_secs(1)).unwrap();
		// An old completion only warms its own key. It never assigns a result
		// to a module or window that may since have changed/disappeared.
		assert!(service.lookup(name("replacement-item")).is_none());
		release.send(()).unwrap();
		service.changed.recv_blocking().unwrap();
		assert!(service.lookup(name("replacement-item")).is_some());
	}

	#[test]
	fn queue_overflow_never_blocks_or_leaves_an_unqueued_request_pending() {
		let (started, busy) = mpsc::channel();
		let (release, gate) = mpsc::channel();
		let service = Arc::new(Service::start(move || {
			move |_: &Request| {
				let _ = started.send(());
				let _ = gate.recv();
				(0, None)
			}
		}));
		service.lookup(name("first"));
		busy.recv_timeout(Duration::from_secs(1)).unwrap();
		for i in 0..128 {
			service.lookup(name(&format!("queued-{i}")));
		}
		assert_eq!(service.requests.len(), 128);
		assert!(nonblocking(&service, name("overflow")).is_none());
		assert!(
			!service
				.entries
				.lock()
				.unwrap()
				.contains_key(&name("overflow"))
		);
		release.send(()).unwrap();
		service.changed.recv_blocking().unwrap();
		busy.recv_timeout(Duration::from_secs(1)).unwrap();
		assert!(nonblocking(&service, name("overflow")).is_none());
		assert!(matches!(
			service.entries.lock().unwrap().get(&name("overflow")),
			Some(Entry::Pending)
		));
		// Disconnecting the test gate lets the remaining bounded queue drain.
		drop(release);
	}
}
