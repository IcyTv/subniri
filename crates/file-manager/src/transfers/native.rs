//! Native offers transport references; the transfer engine still owns all I/O.
use super::{Operation, Request, portal};
use crate::{
	FileManager, Message as AppMessage,
	browser::drag::{self, Hit},
	context_menu::Target,
};
use iced::{Point, Task};
use iced_exwlshell::dnd::{Event, Listener, SourceData};
use std::{
	collections::{BTreeMap, HashSet},
	io::{self, Read},
	path::PathBuf,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	time::{Duration, Instant},
};

const PORTAL: &str = "application/vnd.portal.filetransfer";
const LEGACY_PORTAL: &str = "application/vnd.portal.files";
const URIS: &str = "text/uri-list";
const MAX_PAYLOAD: u64 = 4 * 1024 * 1024;

pub(crate) struct State {
	receiver: Option<async_channel::Receiver<Event>>,
	sender: async_channel::Sender<Event>,
	offers: BTreeMap<u64, Offer>,
	outgoing: BTreeMap<u64, Outgoing>,
	prepared: Option<Prepared>,
	preparing: Option<u64>,
	pub active_source: Option<u64>,
	current_hover: Option<u64>,
	pub job_offer: Option<u64>,
}

impl Default for State {
	fn default() -> Self {
		let (sender, receiver) = async_channel::bounded(128);
		Self {
			receiver: Some(receiver),
			sender,
			offers: BTreeMap::new(),
			outgoing: BTreeMap::new(),
			prepared: None,
			preparing: None,
			active_source: None,
			current_hover: None,
			job_offer: None,
		}
	}
}

struct Offer {
	mime: String,
	destination: Option<PathBuf>,
	dropped: bool,
	copy: bool,
	point: Point,
	cancelled: Arc<AtomicBool>,
	_receipt: Option<portal::Received>,
}

struct Outgoing {
	_guard: Target,
	export: Option<portal::Export>,
}

#[derive(Debug, Clone)]
pub(crate) struct Prepared {
	generation: u64,
	guard: Target,
	data: SourceData,
	export: Option<portal::Export>,
}

#[derive(Debug, Clone)]
pub(crate) struct Files {
	paths: Vec<PathBuf>,
	receipt: Option<portal::Received>,
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
	Event(Event),
	Prepared(u64, Result<Prepared, String>),
	Hover(u64, Point, Hit),
	Drop(u64, PathBuf, Hit),
	Files(u64, Result<Files, String>),
	Stopped(Result<(), String>),
	CancelPending,
}

impl From<Message> for AppMessage {
	fn from(message: Message) -> Self {
		Self::NativeDrag(message)
	}
}

impl FileManager {
	pub(crate) fn listen_native_drag(&mut self) -> Task<AppMessage> {
		let Some(receiver) = self.native_drag.receiver.take() else {
			return Task::none();
		};
		let sender = self.native_drag.sender.clone();
		let listener = Listener::new(move |event| {
			let motion = matches!(event, Event::Motion { .. });
			// Reserve queue capacity for drop/completion/cancellation lifetimes.
			if motion && sender.len() >= 32 {
				return true;
			}
			sender.try_send(event).is_ok() || motion
		});

		Task::batch([
			Task::done(AppMessage::DndListen {
				id: self.host,
				listener,
			}),
			Task::run(receiver, |event| Message::Event(event).into()),
		])
	}

	pub(crate) fn prepare_native_drag(
		&mut self, generation: u64, guard: Target,
	) -> Task<AppMessage> {
		if !self.drag.near_edge(self.host_size)
			|| self.native_drag.preparing == Some(generation)
			|| self.native_drag.prepared.is_some()
		{
			return Task::none();
		}
		self.native_drag.preparing = Some(generation);
		Task::perform(
			async move {
				let check = guard.clone();
				let (uris, files) = tokio::task::spawn_blocking(move || {
					if !check.available() {
						return Err("Drag sources changed".into());
					}
					Ok((uri_list(&check.paths())?, check.readable_files()))
				})
				.await
				.map_err(|error| error.to_string())
				.and_then(|result| result)?;
				let export = match match files {
					Ok(files) => portal::export(files).await,
					Err(error) => Err(error),
				} {
					Ok(export) => Some(export),
					Err(error) => {
						log::warn!(%error, "Portal drag export unavailable; offering local file URIs only");
						None
					}
				};
				let mut payloads = vec![(URIS.into(), Arc::new(uris))];
				if let Some(export) = &export {
					let token = Arc::new(export.payload());
					payloads.push((PORTAL.into(), token.clone()));
					payloads.push((LEGACY_PORTAL.into(), token));
				}
				let item_count = guard.paths().len();
				Ok(Prepared {
					generation,
					guard,
					data: SourceData {
						identifier: generation,
						item_count,
						payloads,
					},
					export,
				})
			},
			move |result| Message::Prepared(generation, result).into(),
		)
	}

	pub(crate) fn try_start_native_drag(&mut self) -> Task<AppMessage> {
		if !self.drag.active
			|| self.drag.external
			|| self.native_drag.active_source.is_some()
			|| self.menu.is_some()
			|| self.dialogs.blocked()
			|| self.transfers.blocked()
		{
			return Task::none();
		}
		if self.native_drag.prepared.is_none() {
			if let Some(guard) = self.drag.source_guard() {
				return self.prepare_native_drag(self.drag.generation, guard);
			}
		}
		if !self.drag.outside(self.host_size) {
			return Task::none();
		}
		let Some(prepared) = self.native_drag.prepared.take() else {
			return Task::none();
		};
		if prepared.generation != self.drag.generation {
			return stop_export(prepared.export);
		}
		let source = prepared.data.identifier;
		self.native_drag.active_source = Some(source);
		self.native_drag.outgoing.insert(
			source,
			Outgoing {
				_guard: prepared.guard,
				export: prepared.export,
			},
		);
		self.drag.end_external();
		self.drag.origin = None;
		Task::done(AppMessage::DndStart {
			id: self.host,
			data: prepared.data,
		})
	}

	pub(crate) fn abandon_native_preparation(&mut self) -> Task<AppMessage> {
		self.native_drag.preparing = None;
		stop_export(
			self.native_drag
				.prepared
				.take()
				.and_then(|prepared| prepared.export),
		)
	}

	pub(crate) fn finish_native_job(&mut self, success: bool) -> Task<AppMessage> {
		let Some(offer) = self.native_drag.job_offer.take() else {
			return Task::none();
		};
		self.native_drag.offers.remove(&offer);
		Task::done(AppMessage::DndFinish {
			id: self.host,
			offer,
			success,
		})
	}

	pub(crate) fn update_native_drag(&mut self, message: Message) -> Task<AppMessage> {
		match message {
			Message::Prepared(generation, result) => {
				if self.native_drag.preparing == Some(generation) {
					self.native_drag.preparing = None;
				}
				match result {
					Ok(prepared)
						if generation == self.drag.generation
							&& self.drag.active && !self.drag.external =>
					{
						self.native_drag.prepared = Some(prepared);
						return self.try_start_native_drag();
					}
					Ok(prepared) => return stop_export(prepared.export),
					Err(error) if generation == self.drag.generation && self.drag.active => {
						self.error = Some(format!("External drag unavailable: {error}"))
					}
					_ => {}
				}
			}
			Message::Event(Event::Enter {
				offer,
				x,
				y,
				mime_types,
				copy,
			}) => {
				let mime = preferred_mime(&mime_types);
				if self.transfers.busy
					|| self.native_drag.pending_count() >= 8
					|| !self.transfers.loaded
					|| self.dialogs.blocked()
					|| self.transfers.blocked()
					|| self.closing.is_some()
					|| mime.is_none()
				{
					return Task::done(AppMessage::DndAccept {
						id: self.host,
						offer,
						mime: None,
					});
				}
				self.native_drag.offers.insert(
					offer,
					Offer {
						mime: mime.unwrap_or_default(),
						destination: None,
						dropped: false,
						copy,
						point: Point::new(x as f32, y as f32),
						cancelled: Arc::new(AtomicBool::new(false)),
						_receipt: None,
					},
				);
				self.native_drag.current_hover = Some(offer);
				self.drag.begin_external(copy);
				let point = Point::new(x as f32, y as f32);
				return drag::widget::hit_test(point)
					.map(move |hit| Message::Hover(offer, point, hit).into());
			}
			Message::Event(Event::Motion { offer, x, y })
				if self.native_drag.current_hover == Some(offer) =>
			{
				let point = Point::new(x as f32, y as f32);
				if let Some(state) = self.native_drag.offers.get_mut(&offer) {
					state.point = point;
				}
				return drag::widget::hit_test(point)
					.map(move |hit| Message::Hover(offer, point, hit).into());
			}
			Message::Hover(offer, point, hit) if self.native_drag.current_hover == Some(offer) => {
				if let Some(state) = self.native_drag.offers.get_mut(&offer) {
					if point != state.point {
						return Task::none();
					}
					state.destination = hit.target.clone();
					self.drag.external_hit(point, hit);
					self.drag.valid &= state.copy;
					return Task::done(AppMessage::DndAccept {
						id: self.host,
						offer,
						mime: if self.drag.valid {
							Some(state.mime.clone())
						} else {
							None
						},
					});
				}
			}
			Message::Event(Event::Actions { offer, copy }) => {
				if let Some(state) = self.native_drag.offers.get_mut(&offer) {
					state.copy = copy;
					if self.native_drag.current_hover == Some(offer) {
						self.drag.external_copy = copy;
						if !copy {
							self.drag.valid = false;
							return Task::done(AppMessage::DndAccept {
								id: self.host,
								offer,
								mime: None,
							});
						}
						let point = state.point;
						return drag::widget::hit_test(point)
							.map(move |hit| Message::Hover(offer, point, hit).into());
					}
				}
			}
			Message::Event(Event::Drop { offer, x, y, copy }) => {
				if !copy
					|| self.transfers.busy
					|| self.dialogs.blocked()
					|| self.transfers.blocked()
				{
					return self.reject_native_offer(offer);
				}
				let Some(state) = self.native_drag.offers.get_mut(&offer) else {
					return self.reject_native_offer(offer);
				};
				state.dropped = true;
				let view = self.path.clone();
				return drag::widget::hit_test(Point::new(x as f32, y as f32))
					.map(move |hit| Message::Drop(offer, view.clone(), hit).into());
			}
			Message::Drop(offer, view, hit) => {
				if self.path != view
					|| self.transfers.busy
					|| self.transfers.blocked()
					|| self.dialogs.blocked()
					|| hit.target.is_none()
				{
					return self.reject_native_offer(offer);
				}
				let Some(state) = self.native_drag.offers.get_mut(&offer) else {
					return self.reject_native_offer(offer);
				};
				state.destination = hit.target;
				return Task::done(AppMessage::DndReceive {
					id: self.host,
					offer,
					mime: state.mime.clone(),
				});
			}
			Message::Event(Event::Payload {
				offer,
				mime,
				reader,
			}) => {
				if !self
					.native_drag
					.offers
					.get(&offer)
					.is_some_and(|state| state.dropped && state.mime == mime)
				{
					return self.reject_native_offer(offer);
				}
				let cancelled = self
					.native_drag
					.offers
					.get(&offer)
					.map(|state| state.cancelled.clone())
					.unwrap_or_else(|| Arc::new(AtomicBool::new(true)));
				return Task::perform(
					async move {
						let bytes = tokio::task::spawn_blocking(move || {
							read_payload(&reader, &cancelled, Duration::from_secs(10))
								.map_err(|error| error.to_string())
						})
						.await
						.map_err(|error| error.to_string())
						.and_then(|result| result)?;
						if mime == PORTAL || mime == LEGACY_PORTAL {
							let receipt = tokio::time::timeout(
								Duration::from_secs(10),
								portal::retrieve(bytes),
							)
							.await
							.map_err(|_| "Portal retrieval timed out".to_string())??;
							Ok(Files {
								paths: receipt.paths.clone(),
								receipt: Some(receipt),
							})
						} else {
							Ok(Files {
								paths: parse_uris(&bytes)?,
								receipt: None,
							})
						}
					},
					move |result| Message::Files(offer, result).into(),
				);
			}
			Message::Files(offer, result) => {
				if !self.native_drag.offers.contains_key(&offer) {
					return Task::none();
				}
				let files = match result {
					Ok(files) => files,
					Err(error) => {
						self.error = Some(format!("Cannot receive dragged files: {error}"));
						return self.reject_native_offer(offer);
					}
				};
				if self.transfers.busy || !self.transfers.loaded || self.closing.is_some() {
					return self.reject_native_offer(offer);
				}
				let Some(state) = self.native_drag.offers.get_mut(&offer) else {
					return self.reject_native_offer(offer);
				};
				let Some(destination) = state.destination.clone() else {
					return self.reject_native_offer(offer);
				};
				if files.paths.is_empty() {
					self.error = Some("The drag source supplied no local files".into());
					return self.reject_native_offer(offer);
				}
				state._receipt = files.receipt;
				self.native_drag.job_offer = Some(offer);
				self.drag.end_external();
				return self.update_transfers(super::Message::Start(Request {
					sources: files.paths,
					destination,
					operation: Operation::Copy,
					durability: self.transfers.policy,
					acknowledge_volatile: false,
				}));
			}
			Message::Event(Event::Leave { offer }) => {
				if self.native_drag.current_hover == Some(offer) {
					self.native_drag.current_hover = None;
					self.drag.end_external();
				}
				if self
					.native_drag
					.offers
					.get(&offer)
					.is_some_and(|state| !state.dropped)
				{
					self.native_drag.offers.remove(&offer);
				}
			}
			Message::Event(Event::OfferCancelled { offer }) => {
				if self.native_drag.job_offer == Some(offer) {
					self.transfers.cancel();
				} else {
					return self.reject_native_offer(offer);
				}
				if self.native_drag.current_hover == Some(offer) {
					self.native_drag.current_hover = None;
					self.drag.end_external();
				}
			}
			Message::Event(Event::SourceDropped { source }) => {
				if self.native_drag.active_source == Some(source) {
					self.native_drag.active_source = None;
				}
			}
			Message::Event(
				Event::SourceFinished { source } | Event::SourceCancelled { source },
			) => {
				if self.native_drag.active_source == Some(source) {
					self.native_drag.active_source = None;
				}
				if let Some(state) = self.native_drag.outgoing.remove(&source) {
					return stop_export(state.export);
				}
			}
			Message::Event(Event::SourceError { source, message }) => {
				self.error = Some(format!("Native drag failed: {message}"));
				if self.native_drag.active_source == Some(source) {
					self.native_drag.active_source = None;
				}
				if let Some(state) = self.native_drag.outgoing.remove(&source) {
					return stop_export(state.export);
				}
			}
			Message::Event(Event::OfferError { offer, message }) => {
				self.error = Some(format!("Native drop failed: {message}"));
				return self.reject_native_offer(offer);
			}
			Message::Stopped(Err(error)) => log::warn!(%error, "Portal drag cleanup failed"),
			Message::CancelPending => {
				let offers: Vec<_> = self
					.native_drag
					.offers
					.keys()
					.copied()
					.filter(|offer| Some(*offer) != self.native_drag.job_offer)
					.collect();
				return Task::batch(
					offers
						.into_iter()
						.map(|offer| self.reject_native_offer(offer)),
				);
			}
			_ => {}
		}

		Task::none()
	}

	fn reject_native_offer(&mut self, offer: u64) -> Task<AppMessage> {
		if self.native_drag.current_hover == Some(offer) {
			self.native_drag.current_hover = None;
			self.drag.end_external();
		}
		if let Some(state) = self.native_drag.offers.remove(&offer) {
			state.cancelled.store(true, Ordering::Release);
		}
		Task::done(AppMessage::DndFinish {
			id: self.host,
			offer,
			success: false,
		})
	}

	pub(crate) fn shutdown_native_drag(&mut self) -> Task<AppMessage> {
		let mut tasks = vec![self.abandon_native_preparation()];
		for (source, state) in std::mem::take(&mut self.native_drag.outgoing) {
			tasks.push(Task::done(AppMessage::DndCancel {
				id: self.host,
				source,
			}));
			tasks.push(stop_export(state.export));
		}
		self.native_drag.active_source = None;
		let offers: Vec<_> = self
			.native_drag
			.offers
			.keys()
			.copied()
			.filter(|offer| Some(*offer) != self.native_drag.job_offer)
			.collect();
		for offer in offers {
			tasks.push(self.reject_native_offer(offer));
		}

		Task::batch(tasks)
	}
}

impl State {
	pub(crate) fn pending_count(&self) -> usize {
		self.offers
			.iter()
			.filter(|(offer, state)| state.dropped && Some(**offer) != self.job_offer)
			.count()
	}
}

fn read_payload(
	reader: &std::fs::File, cancelled: &AtomicBool, timeout: Duration,
) -> io::Result<Vec<u8>> {
	use std::os::fd::AsRawFd;
	let mut reader = reader.try_clone()?;
	let descriptor = reader.as_raw_fd();
	let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
	if flags < 0 || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
	{
		return Err(io::Error::last_os_error());
	}
	let started = Instant::now();
	let mut bytes = Vec::new();
	let mut buffer = [0u8; 65536];
	loop {
		if cancelled.load(Ordering::Acquire) {
			return Err(io::Error::new(
				io::ErrorKind::Interrupted,
				"Native drop cancelled",
			));
		}
		if started.elapsed() >= timeout {
			return Err(io::Error::new(
				io::ErrorKind::TimedOut,
				"Drag MIME payload timed out",
			));
		}
		match reader.read(&mut buffer) {
			Ok(0) => return Ok(bytes),
			Ok(length) => {
				if bytes.len().saturating_add(length) as u64 > MAX_PAYLOAD {
					return Err(io::Error::other("Drag payload exceeds 4 MiB"));
				}
				bytes.extend_from_slice(&buffer[..length]);
			}
			Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
				let mut poll = libc::pollfd {
					fd: descriptor,
					events: libc::POLLIN,
					revents: 0,
				};
				let remaining = timeout
					.saturating_sub(started.elapsed())
					.as_millis()
					.min(100) as i32;
				if unsafe { libc::poll(&mut poll, 1, remaining) } < 0 {
					let error = io::Error::last_os_error();
					if error.kind() != io::ErrorKind::Interrupted {
						return Err(error);
					}
				}
			}
			Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
			Err(error) => return Err(error),
		}
	}
}

fn stop_export(export: Option<portal::Export>) -> Task<AppMessage> {
	export.map_or_else(Task::none, |export| {
		Task::perform(async move { export.stop().await }, |result| {
			Message::Stopped(result).into()
		})
	})
}

fn preferred_mime(mimes: &[String]) -> Option<String> {
	[PORTAL, LEGACY_PORTAL, URIS]
		.into_iter()
		.find(|preferred| mimes.iter().any(|mime| mime == preferred))
		.map(str::to_string)
}

fn uri_list(paths: &[PathBuf]) -> Result<Vec<u8>, String> {
	if paths.is_empty() || paths.len() > 10_000 {
		return Err("Drag selection must contain between 1 and 10000 entries".into());
	}
	let mut bytes = Vec::new();
	for path in paths {
		let uri = url::Url::from_file_path(path)
			.map_err(|_| "Drag source is not an absolute local path".to_string())?;
		bytes.extend_from_slice(uri.as_str().as_bytes());
		bytes.extend_from_slice(b"\r\n");
		if bytes.len() as u64 > MAX_PAYLOAD {
			return Err("Drag URI list exceeds 4 MiB".into());
		}
	}
	Ok(bytes)
}

fn parse_uris(bytes: &[u8]) -> Result<Vec<PathBuf>, String> {
	let text = std::str::from_utf8(bytes).map_err(|_| "Drag URI list is not UTF-8".to_string())?;
	let mut paths = Vec::new();
	let mut seen = HashSet::new();
	for line in text
		.lines()
		.map(str::trim)
		.filter(|line| !line.is_empty() && !line.starts_with('#'))
	{
		let uri = url::Url::parse(line).map_err(|error| error.to_string())?;
		if uri.scheme() != "file" || !matches!(uri.host_str(), None | Some("") | Some("localhost"))
		{
			return Err("Only local file URIs are supported; remote URLs are not downloads".into());
		}
		let path = uri
			.to_file_path()
			.map_err(|_| "Invalid local file URI".to_string())?;
		use std::os::unix::ffi::OsStrExt;
		if path.as_os_str().as_bytes().contains(&0) {
			return Err("NUL in a file URI".into());
		}
		if seen.insert(path.clone()) {
			paths.push(path);
		}
		if paths.len() > 10_000 {
			return Err("Drag contains more than 10000 entries".into());
		}
	}

	Ok(paths)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{Args, test_support::TestDir};

	fn fixture() -> (TestDir, FileManager) {
		let directory = TestDir::new();
		let (mut app, _) = FileManager::new(Args {
			path: directory.path().into(),
		});
		app.transfers.loaded = true;
		app.loading = false;

		(directory, app)
	}

	fn offer(destination: PathBuf, dropped: bool) -> Offer {
		Offer {
			mime: URIS.into(),
			destination: Some(destination),
			dropped,
			copy: true,
			point: Point::ORIGIN,
			cancelled: Arc::new(AtomicBool::new(false)),
			_receipt: None,
		}
	}

	#[test]
	fn late_payload_result_after_receipt_cancellation_cannot_start_a_job() {
		let (_directory, mut app) = fixture();
		let state = offer(app.path.clone(), true);
		let cancelled = state.cancelled.clone();
		app.native_drag.offers.insert(7, state);

		let _ = app.update_native_drag(Message::CancelPending);
		let _ = app.update_native_drag(Message::Files(
			7,
			Ok(Files {
				paths: vec![app.path.join("source")],
				receipt: None,
			}),
		));

		assert!(cancelled.load(Ordering::Acquire));
		assert!(app.native_drag.offers.is_empty());
		assert!(!app.transfers.busy);
		assert_eq!(app.native_drag.job_offer, None);
	}

	#[test]
	fn source_errors_do_not_reject_an_unrelated_offer_with_the_same_number() {
		let (_directory, mut app) = fixture();
		app.native_drag
			.offers
			.insert(7, offer(app.path.clone(), true));

		let _ = app.update_native_drag(Message::Event(Event::SourceError {
			source: 7,
			message: "source lost".into(),
		}));

		assert!(app.native_drag.offers.contains_key(&7));
		assert!(!app.transfers.busy);
	}

	#[test]
	fn completing_one_copy_does_not_destroy_a_later_hover_offer() {
		let (_directory, mut app) = fixture();
		app.native_drag
			.offers
			.insert(7, offer(app.path.clone(), true));
		app.native_drag
			.offers
			.insert(8, offer(app.path.clone(), false));
		app.native_drag.job_offer = Some(7);
		app.native_drag.current_hover = Some(8);

		let _ = app.finish_native_job(true);

		assert!(!app.native_drag.offers.contains_key(&7));
		assert!(app.native_drag.offers.contains_key(&8));
		assert_eq!(app.native_drag.current_hover, Some(8));
		assert_eq!(app.native_drag.job_offer, None);
	}

	#[test]
	fn uri_lists_are_local_deduplicated_and_byte_preserving() {
		use std::os::unix::ffi::OsStringExt;
		let paths = vec![
			PathBuf::from("/home/user/a b"),
			PathBuf::from(std::ffi::OsString::from_vec(
				b"/home/user/non-utf8-\xff".to_vec(),
			)),
		];
		assert_eq!(parse_uris(&uri_list(&paths).unwrap()).unwrap(), paths);
		assert_eq!(
			parse_uris(b"# comment\r\nfile:///tmp/file\r\nfile:///tmp/file\r\n")
				.unwrap()
				.len(),
			1
		);
		for uri in [
			b"https://example.org/file".as_slice(),
			b"file://remote/path",
			b"file:///tmp/%00file",
		] {
			assert!(parse_uris(uri).is_err());
		}
	}

	#[test]
	fn portal_mimes_take_precedence_without_requesting_tokens_on_hover() {
		assert_eq!(
			preferred_mime(&[URIS.into(), PORTAL.into()]),
			Some(PORTAL.into())
		);
		assert_eq!(
			preferred_mime(&[LEGACY_PORTAL.into()]),
			Some(LEGACY_PORTAL.into())
		);
		assert_eq!(preferred_mime(&["text/plain".into()]), None);
	}

	#[test]
	fn payload_reads_are_cancellable_timed_and_bounded() {
		use std::{
			fs::File,
			io::Write,
			os::{fd::OwnedFd, unix::net::UnixStream},
		};
		let (reader, _writer) = UnixStream::pair().unwrap();
		let descriptor: OwnedFd = reader.into();
		let reader = File::from(descriptor);
		assert_eq!(
			read_payload(&reader, &AtomicBool::new(true), Duration::from_secs(1))
				.unwrap_err()
				.kind(),
			io::ErrorKind::Interrupted
		);
		assert_eq!(
			read_payload(&reader, &AtomicBool::new(false), Duration::ZERO)
				.unwrap_err()
				.kind(),
			io::ErrorKind::TimedOut
		);

		let (reader, mut writer) = UnixStream::pair().unwrap();
		let descriptor: OwnedFd = reader.into();
		let reader = File::from(descriptor);
		let output =
			std::thread::spawn(move || writer.write_all(&vec![7; MAX_PAYLOAD as usize + 1]));
		let result = read_payload(&reader, &AtomicBool::new(false), Duration::from_secs(5));
		drop(reader);
		let _ = output.join().unwrap();
		assert!(result.unwrap_err().to_string().contains("4 MiB"));
	}

	#[test]
	fn payload_eof_returns_only_the_supplied_bytes() {
		use std::{
			fs::File,
			io::Write,
			os::{fd::OwnedFd, unix::net::UnixStream},
		};
		let (reader, mut writer) = UnixStream::pair().unwrap();
		writer.write_all(b"file:///tmp/source\r\n").unwrap();
		drop(writer);
		let descriptor: OwnedFd = reader.into();
		let reader = File::from(descriptor);

		assert_eq!(
			read_payload(&reader, &AtomicBool::new(false), Duration::from_secs(1)).unwrap(),
			b"file:///tmp/source\r\n"
		);
	}
}
