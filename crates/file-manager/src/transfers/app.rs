use super::{
	Choice, Conflict, Durability, Event, Operation, Outcome, Phase, Request, Store, model::Journal,
};
use crate::{FileManager, context_menu::Target};
use iced::{Task, window};
use std::{
	path::PathBuf,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
		mpsc,
	},
	time::Duration,
};

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;

#[path = "rate.rs"]
mod rate;

pub(crate) struct State {
	pub busy: bool,
	pub generation: u64,
	pub config_generation: u64,
	pub phase: Option<Phase>,
	pub bytes: u64,
	pub total_bytes: Option<u64>,
	pub started: Option<std::time::Instant>,
	pub progress_visible: bool,
	copy_rate: rate::Rate,
	pub policy: Durability,
	pub loaded: bool,
	pub history: Vec<Journal>,
	pub show_history: bool,
	pub latest: Option<Outcome>,
	pub dialog: Option<Dialog>,
	pub apply_remaining: bool,
	pub pending: Option<PreparedTransfer>,
	pending_generation: u64,
	preparing: Option<u64>,
	running_pending: Option<u64>,
	cancelled: Arc<AtomicBool>,
	response: Option<mpsc::Sender<Decision>>,
}

impl Default for State {
	fn default() -> Self {
		Self {
			busy: false,
			generation: 0,
			config_generation: 0,
			phase: None,
			bytes: 0,
			total_bytes: None,
			started: None,
			progress_visible: false,
			copy_rate: rate::Rate::default(),
			policy: Durability::Auto,
			loaded: false,
			history: Vec::new(),
			show_history: false,
			latest: None,
			dialog: None,
			apply_remaining: false,
			pending: None,
			pending_generation: 0,
			preparing: None,
			running_pending: None,
			cancelled: Arc::new(AtomicBool::new(false)),
			response: None,
		}
	}
}

impl State {
	pub(crate) fn eta(&self) -> Option<Duration> {
		if self.phase != Some(Phase::Copying) || self.bytes == 0 {
			return None;
		}
		self.copy_rate.eta(self.bytes, self.total_bytes?)
	}

	pub(crate) fn can_paste(&self) -> bool {
		self.pending.is_some() && self.preparing.is_none() && !self.busy && self.loaded
	}

	pub(crate) fn prepared_operation(&self, path: &std::path::Path) -> Option<Operation> {
		self.pending
			.as_ref()
			.filter(|pending| pending.paths.contains(path))
			.map(|pending| pending.operation)
	}

	pub fn owns(&self, id: window::Id) -> bool {
		self.dialog.as_ref().is_some_and(|dialog| dialog.id == id)
	}

	pub fn blocked(&self) -> bool {
		self.dialog.is_some()
	}

	pub fn cancel(&self) {
		self.cancelled.store(true, Ordering::Release);
		if let Some(response) = &self.response {
			let _ = response.send(Decision {
				choice: Choice::Cancel,
				remaining: false,
			});
		}
	}
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedTransfer {
	pub target: Target,
	pub operation: Operation,
	pub policy: Option<Durability>,
	pub generation: u64,
	paths: std::collections::HashSet<PathBuf>,
}

impl PreparedTransfer {
	fn request(&self, destination: PathBuf, global: Durability) -> Request {
		Request {
			sources: self.target.paths(),
			destination,
			operation: self.operation,
			durability: self.policy.unwrap_or(global),
			acknowledge_volatile: false,
		}
	}
}

pub(crate) struct Dialog {
	pub id: window::Id,
	pub open: bool,
	pub content: DialogContent,
}

pub(crate) enum DialogContent {
	Conflict(Conflict),
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
	Load,
	Loaded(u64, Result<Durability, String>),
	RefreshHistory,
	HistoryLoaded(Result<(Vec<Journal>, Vec<String>), String>),
	ShowHistory(bool),
	Prepare(Target, Operation, Option<Durability>),
	PrepareSelection(Operation),
	CapturedPrepare(u64, Operation, Result<Target, String>),
	Paste(PathBuf),
	PasteTarget(Target),
	CancelPrepared,
	Start(Request),
	StartGuarded(Request, Target),
	Cancel,
	DismissDialog,
	ApplyRemaining(bool),
	Choose(Choice),
	Undo(String),
	Notice(u64, Notice),
	ShowProgress(u64),
	WorkerExited(u64, Result<(), String>),
	Window(window::Id, window::Event),
	Key(window::Id, iced::keyboard::Event),
	Drag(window::Id),
}

#[derive(Debug, Clone)]
pub(crate) enum Notice {
	Event(Event),
	Done(Result<Outcome, String>),
}

struct Decision {
	choice: Choice,
	remaining: bool,
}

impl From<Message> for crate::Message {
	fn from(message: Message) -> Self {
		Self::Transfers(message)
	}
}

impl FileManager {
	pub(crate) fn update_transfers(&mut self, message: Message) -> Task<crate::Message> {
		match message {
			Message::Load => {
				self.transfers.config_generation += 1;
				let generation = self.transfers.config_generation;
				return Task::perform(
					async {
						tokio::task::spawn_blocking(|| {
							config::ConfigFile::load()
								.map(|(_, config)| from_config(config.fileberg.transfer_durability))
								.map_err(|error| error.to_string())
						})
						.await
						.map_err(|error| error.to_string())
						.and_then(|result| result)
					},
					move |result| Message::Loaded(generation, result).into(),
				);
			}
			Message::Loaded(generation, result)
				if generation == self.transfers.config_generation =>
			{
				match result {
					Ok(policy) => {
						self.transfers.policy = policy;
						self.transfers.loaded = true;
					}
					Err(error) => self.error = Some(format!("Transfer settings: {error}")),
				}
			}
			Message::RefreshHistory => {
				if self.transfers.busy {
					return Task::none();
				}
				return Task::perform(
					async {
						tokio::task::spawn_blocking(|| {
							let store = Store::open(
								Store::default_path().map_err(|error| error.to_string())?,
							)
							.map_err(|error| error.to_string())?;
							let warnings = super::retention::expire(
								&store,
								super::journal::now().map_err(|error| error.to_string())?,
								5 * 1024 * 1024 * 1024,
							)
							.map_err(|error| error.to_string())?;
							Ok((
								store.history().map_err(|error| error.to_string())?,
								warnings,
							))
						})
						.await
						.map_err(|error| error.to_string())
						.and_then(|result| result)
					},
					|result| Message::HistoryLoaded(result).into(),
				);
			}
			Message::HistoryLoaded(result) => match result {
				Ok((history, warnings)) => {
					self.transfers.history = history;
					if !warnings.is_empty() {
						self.error = Some(warnings.join("\n"));
					}
				}
				Err(error) => self.error = Some(format!("Transfer history: {error}")),
			},
			Message::ShowHistory(show) => {
				self.transfers.show_history = show;
				if show {
					return self.update_transfers(Message::RefreshHistory);
				}
			}
			Message::Prepare(target, operation, policy) => {
				if self.transfers.busy
					|| self.transfers.blocked()
					|| self.dialogs.blocked()
					|| self.closing.is_some()
				{
					return Task::none();
				}
				if !matches!(operation, Operation::Copy | Operation::Move) {
					return Task::none();
				}
				self.transfers.pending_generation += 1;
				self.transfers.preparing = None;
				let paths = target.paths().into_iter().collect();
				self.transfers.pending = Some(PreparedTransfer {
					target,
					operation,
					policy,
					generation: self.transfers.pending_generation,
					paths,
				});
				return self.update_file_drag(crate::browser::drag::Message::Cancel);
			}
			Message::PrepareSelection(operation) => {
				if self.transfers.busy
					|| self.transfers.blocked()
					|| self.dialogs.blocked()
					|| self.selection.paths.is_empty()
					|| self.transfers.show_history
				{
					return Task::none();
				}
				self.transfers.pending_generation += 1;
				let generation = self.transfers.pending_generation;
				self.transfers.preparing = Some(generation);
				let selected: Vec<_> = self
					.entries
					.iter()
					.filter(|entry| self.selection.paths.contains(&entry.path))
					.map(|entry| (entry.path.clone(), entry.is_dir))
					.collect();
				return Task::perform(
					async move {
						tokio::task::spawn_blocking(move || {
							let mut targets = selected
								.into_iter()
								.map(|(path, directory)| {
									Target::capture(
										&path,
										if directory {
											crate::context_menu::Context::Folder
										} else {
											crate::context_menu::Context::File
										},
									)
								})
								.collect::<Result<Vec<_>, _>>()?
								.into_iter();
							let mut target = targets
								.next()
								.ok_or_else(|| "No files selected".to_string())?;
							target.others = targets.collect();
							Ok(target)
						})
						.await
						.map_err(|error| error.to_string())
						.and_then(|result| result)
					},
					move |result| Message::CapturedPrepare(generation, operation, result).into(),
				);
			}
			Message::CapturedPrepare(generation, operation, result)
				if generation == self.transfers.pending_generation =>
			{
				self.transfers.preparing = None;
				match result {
					Ok(target) => {
						return self.update_transfers(Message::Prepare(target, operation, None));
					}
					Err(error) => {
						self.error = Some(format!("Cannot prepare selected files: {error}"))
					}
				}
			}
			Message::CancelPrepared => {
				if !self.transfers.busy {
					self.transfers.pending = None;
					self.transfers.preparing = None;
					self.transfers.pending_generation += 1;
				}
			}
			Message::ApplyRemaining(value) => self.transfers.apply_remaining = value,
			Message::Paste(destination) => return self.paste_prepared(destination, None),
			Message::PasteTarget(target) => {
				return self.paste_prepared(target.path.clone(), Some(target));
			}
			Message::Start(mut request) => {
				// Drag/drop never carries a hidden per-job policy override.
				request.durability = self.transfers.policy;
				return self.launch_transfer(Some(request), None, None, None);
			}
			Message::StartGuarded(mut request, guard) => {
				request.durability = self.transfers.policy;
				return self.launch_transfer(Some(request), Some(guard), None, None);
			}
			Message::Undo(id) => return self.launch_transfer(None, None, None, Some(id)),
			Message::Cancel => {
				self.transfers.cancel();
				self.transfers.phase = Some(Phase::RollingBack);
				return self.close_transfer_dialog();
			}
			Message::DismissDialog => {
				if self
					.transfers
					.dialog
					.as_ref()
					.is_some_and(|dialog| matches!(dialog.content, DialogContent::Conflict(_)))
				{
					self.transfers.cancel();
				}
				return self.close_transfer_dialog();
			}
			Message::Choose(choice) => {
				if !self.transfers.dialog.as_ref().is_some_and(|dialog| {
					dialog.open && matches!(dialog.content, DialogContent::Conflict(_))
				}) {
					return Task::none();
				}
				if let Some(response) = &self.transfers.response {
					let _ = response.send(Decision {
						choice,
						remaining: self.transfers.apply_remaining,
					});
				}
				return self.close_transfer_dialog();
			}
			Message::ShowProgress(generation)
				if generation == self.transfers.generation && self.transfers.busy =>
			{
				self.transfers.progress_visible = true;
			}
			Message::Notice(generation, notice) if generation == self.transfers.generation => {
				match notice {
					Notice::Event(Event::Progress {
						phase,
						bytes,
						total,
					}) => {
						if phase == Phase::Copying {
							self.transfers
								.copy_rate
								.observe(std::time::Instant::now(), bytes);
						} else {
							self.transfers.copy_rate.reset();
						}
						self.transfers.phase = Some(phase);
						self.transfers.bytes = bytes;
						self.transfers.total_bytes = total;
					}
					Notice::Event(Event::Warning(warning)) => self.error = Some(warning),
					Notice::Event(Event::Conflict(conflict)) => {
						self.transfers.copy_rate.reset();
						if self.closing.is_some() {
							self.transfers.cancel();
						} else {
							return self.open_transfer_dialog(DialogContent::Conflict(conflict));
						}
					}
					Notice::Done(result) => {
						let pending_job = self.transfers.running_pending.take();
						if result
							.as_ref()
							.is_ok_and(|outcome| outcome.status == super::Status::Complete)
							&& self.transfers.pending.as_ref().is_some_and(|pending| {
								Some(pending.generation) == pending_job
									&& pending.operation == Operation::Move
							}) {
							self.transfers.pending = None;
						}
						let native_finish = self.finish_native_job(
							result
								.as_ref()
								.is_ok_and(|outcome| outcome.status == super::Status::Complete),
						);
						self.transfers.busy = false;
						self.transfers.phase = None;
						self.transfers.response = None;
						match result {
							Ok(outcome) => {
								if !outcome.problems.is_empty() {
									self.error = Some(outcome.problems.join("\n"));
								}
								self.transfers.latest = Some(outcome);
							}
							Err(error) => self.error = Some(format!("Transfer failed: {error}")),
						}
						let close = self.close_transfer_dialog();
						if self.closing.is_some() {
							return Task::batch([close, native_finish])
								.chain(self.finish_pending_close());
						}
						return Task::batch([
							close,
							native_finish,
							self.load_directory(),
							self.update_transfers(Message::RefreshHistory),
						]);
					}
				}
			}
			Message::WorkerExited(generation, Err(error))
				if generation == self.transfers.generation =>
			{
				self.transfers.busy = false;
				self.transfers.running_pending = None;
				self.transfers.response = None;
				self.error = Some(format!(
					"Transfer worker stopped; recovery journal retained: {error}"
				));
				let close = self.close_transfer_dialog();
				let native_finish = self.finish_native_job(false);
				return if self.closing.is_some() {
					Task::batch([close, native_finish]).chain(self.finish_pending_close())
				} else {
					Task::batch([
						close,
						native_finish,
						self.update_transfers(Message::RefreshHistory),
					])
				};
			}
			Message::Window(id, event) if self.transfers.owns(id) => match event {
				window::Event::Opened { .. } => {
					if let Some(dialog) = &mut self.transfers.dialog {
						dialog.open = true;
						return iced::widget::operation::focus("transfer-cancel");
					}
				}
				window::Event::CloseRequested | window::Event::Closed => {
					return self.update_transfers(Message::DismissDialog);
				}
				_ => {}
			},
			Message::Key(
				id,
				iced::keyboard::Event::KeyPressed {
					key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
					..
				},
			) if self.transfers.owns(id) => return self.update_transfers(Message::DismissDialog),
			Message::Key(
				id,
				iced::keyboard::Event::KeyPressed {
					key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab),
					modifiers,
					..
				},
			) if self.transfers.owns(id)
				&& !modifiers.control()
				&& !modifiers.alt()
				&& !modifiers.logo() =>
			{
				let mut ids = Vec::new();
				if let Some(dialog) = &self.transfers.dialog {
					match &dialog.content {
						DialogContent::Conflict(conflict) => {
							if conflict.reason != super::model::ConflictReason::Entry {
								ids.extend([
									super::view::choice_id(Choice::Cancel),
									super::view::choice_id(Choice::ApproveRecovery),
								]);
							} else {
								if conflict.directories {
									ids.push(super::view::choice_id(Choice::Merge));
								}
								ids.extend([
									super::view::choice_id(Choice::Replace),
									super::view::choice_id(Choice::ReplaceCentral),
									super::view::choice_id(Choice::Skip),
									super::view::choice_id(Choice::KeepBoth),
									super::view::choice_id(Choice::Cancel),
									"transfer-remaining",
								]);
							}
						}
					}
				}
				return iced::advanced::widget::operate(CycleFocus {
					ids: ids
						.into_iter()
						.map(iced::advanced::widget::Id::from)
						.collect(),
					current: None,
					backwards: modifiers.shift(),
				});
			}
			Message::Drag(id) if self.transfers.owns(id) => return window::drag(id),
			_ => {}
		}

		if self.closing.is_some() {
			self.finish_pending_close()
		} else {
			Task::none()
		}
	}

	fn paste_prepared(
		&mut self, destination: PathBuf, destination_guard: Option<Target>,
	) -> Task<crate::Message> {
		if !self.transfers.can_paste()
			|| self.transfers.blocked()
			|| self.dialogs.blocked()
			|| self.closing.is_some()
		{
			return Task::none();
		}
		let Some(pending) = self.transfers.pending.clone() else {
			return Task::none();
		};
		let request = pending.request(destination, self.transfers.policy);
		let task =
			self.launch_transfer(Some(request), Some(pending.target), destination_guard, None);
		if self.transfers.busy {
			self.transfers.running_pending = Some(pending.generation);
		}
		task
	}

	fn open_transfer_dialog(&mut self, content: DialogContent) -> Task<crate::Message> {
		let close = self.menu_lifecycle(neo_widgets::popup::Event::CloseAll);
		let id = window::Id::unique();
		self.typeahead.clear();
		self.modifiers = iced::keyboard::Modifiers::empty();
		self.selection.end_range();
		self.transfers.apply_remaining = false;
		self.transfers.dialog = Some(Dialog {
			id,
			open: false,
			content,
		});
		close.chain(Task::done(crate::Message::NewBaseWindow {
			id,
			settings: iced_exwlshell::actions::IcedXdgWindowSettings {
				parent: Some(self.host),
				size: Some(iced_exwlshell::reexport::PixelSize::px(660, 400)),
				client_side_decorations: true,
			},
		}))
	}

	pub(crate) fn close_transfer_dialog(&mut self) -> Task<crate::Message> {
		self.transfers
			.dialog
			.take()
			.map_or_else(Task::none, |dialog| {
				let focus = self
					.entries
					.iter()
					.find(|entry| Some(&entry.path) == self.selected.as_ref())
					.map(|entry| entry.id.clone());
				window::close(dialog.id)
					.chain(focus.map_or_else(Task::none, iced::widget::operation::focus))
			})
	}

	fn launch_transfer(
		&mut self, request: Option<Request>, guard: Option<Target>,
		destination_guard: Option<Target>, undo: Option<String>,
	) -> Task<crate::Message> {
		if self.transfers.busy || !self.transfers.loaded || self.closing.is_some() {
			return Task::none();
		}
		self.transfers.generation += 1;
		let generation = self.transfers.generation;
		self.transfers.busy = true;
		self.transfers.latest = None;
		self.transfers.bytes = 0;
		self.transfers.total_bytes = None;
		self.transfers.started = Some(std::time::Instant::now());
		self.transfers.progress_visible = false;
		self.transfers.copy_rate.reset();
		self.transfers.running_pending = None;
		self.transfers.phase = Some(if undo.is_some() {
			Phase::RollingBack
		} else {
			Phase::Preparing
		});
		self.transfers.cancelled = Arc::new(AtomicBool::new(false));
		let cancelled = self.transfers.cancelled.clone();
		let (response, answers) = mpsc::channel::<Decision>();
		self.transfers.response = Some(response);
		let (sender, receiver) = async_channel::bounded(32);
		let producer = Task::perform(
			async move {
				tokio::task::spawn_blocking(move || {
					let result = (|| {
						let mut request = request;
						let resolved_destination = request
							.as_mut()
							.map(|request| {
								request.destination = std::fs::canonicalize(&request.destination)
									.map_err(|error| error.to_string())?;
								Target::capture(
									&request.destination,
									crate::context_menu::Context::Folder,
								)
							})
							.transpose()?;
						let store = Store::open_wait(
							Store::default_path().map_err(|error| error.to_string())?,
							&cancelled,
						)
						.map_err(|error| error.to_string())?;
						validate_launch_targets(
							guard.as_ref(),
							destination_guard.as_ref(),
							resolved_destination.as_ref(),
						)?;
						let mut last_progress = None;
						let report = |event| {
							if matches!(
								event,
								Event::Progress {
									phase: Phase::Copying,
									..
								}
							) {
								let now = std::time::Instant::now();
								if last_progress.is_none_or(|at| {
									now.duration_since(at) >= Duration::from_millis(100)
								}) {
									if sender.try_send(Notice::Event(event)).is_ok() {
										last_progress = Some(now);
									}
								}
							} else {
								last_progress = None;
								let _ = sender.send_blocking(Notice::Event(event));
							}
						};
						if let Some(id) = undo {
							return super::undo(&store, &id, report)
								.map_err(|error| error.to_string());
						}
						let mut remaining: Option<(bool, Choice)> = None;
						super::execute(
							&store,
							request.ok_or_else(|| "Missing transfer request".to_string())?,
							&cancelled,
							|conflict| {
								let entry_conflict =
									conflict.reason == super::model::ConflictReason::Entry;
								if let Some((directories, choice)) = remaining {
									if entry_conflict && directories == conflict.directories {
										return choice;
									}
								}
								let directories = conflict.directories;
								if sender
									.send_blocking(Notice::Event(Event::Conflict(conflict)))
									.is_err()
								{
									return Choice::Cancel;
								}
								loop {
									if cancelled.load(Ordering::Acquire) {
										return Choice::Cancel;
									}
									match answers.recv_timeout(Duration::from_millis(100)) {
										Ok(decision) => {
											if entry_conflict
												&& decision.remaining && decision.choice
												!= Choice::ApproveRecovery
											{
												remaining = Some((directories, decision.choice));
											}
											return decision.choice;
										}
										Err(mpsc::RecvTimeoutError::Timeout) => {}
										Err(_) => return Choice::Cancel,
									}
								}
							},
							report,
						)
						.map_err(|error| error.to_string())
					})();
					let _ = sender.send_blocking(Notice::Done(result));
				})
				.await
				.map_err(|error| error.to_string())
			},
			move |result| Message::WorkerExited(generation, result).into(),
		);

		Task::batch([
			producer,
			Task::perform(
				async { tokio::time::sleep(Duration::from_millis(400)).await },
				move |_| Message::ShowProgress(generation).into(),
			),
			Task::run(receiver, move |notice| {
				Message::Notice(generation, notice).into()
			}),
		])
	}
}

struct CycleFocus {
	ids: Vec<iced::advanced::widget::Id>,
	current: Option<usize>,
	backwards: bool,
}

impl iced::advanced::widget::Operation<crate::Message> for CycleFocus {
	fn traverse(
		&mut self,
		visit: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation<crate::Message>),
	) {
		visit(self);
	}

	fn focusable(
		&mut self, id: Option<&iced::advanced::widget::Id>, _: iced::Rectangle,
		state: &mut dyn iced::advanced::widget::operation::Focusable,
	) {
		if state.is_focused() {
			self.current = self
				.ids
				.iter()
				.position(|candidate| Some(candidate) == id)
				.or(self.current);
		}
	}

	fn finish(&self) -> iced::advanced::widget::operation::Outcome<crate::Message> {
		if self.ids.is_empty() {
			return iced::advanced::widget::operation::Outcome::None;
		}
		let next = match (self.current, self.backwards) {
			(Some(index), true) => (index + self.ids.len() - 1) % self.ids.len(),
			(Some(index), false) => (index + 1) % self.ids.len(),
			(None, true) => self.ids.len() - 1,
			(None, false) => 0,
		};
		self.ids
			.get(next)
			.map_or(iced::advanced::widget::operation::Outcome::None, |id| {
				iced::advanced::widget::operation::Outcome::Chain(Box::new(
					iced::advanced::widget::operation::focusable::focus::<crate::Message>(
						id.clone(),
					),
				))
			})
	}
}

fn from_config(policy: config::FilebergDurability) -> Durability {
	match policy {
		config::FilebergDurability::Auto => Durability::Auto,
		config::FilebergDurability::Durable => Durability::Durable,
		config::FilebergDurability::Buffered => Durability::Buffered,
	}
}

fn validate_launch_targets(
	source: Option<&Target>, destination: Option<&Target>, resolved: Option<&Target>,
) -> Result<(), String> {
	if source.is_some_and(|target| !target.available()) {
		return Err("Prepared source entries changed before Paste could start".into());
	}
	if destination
		.into_iter()
		.chain(resolved)
		.any(|target| !target.available())
	{
		return Err(
			"Paste destination changed while waiting to start; no files were transferred".into(),
		);
	}
	Ok(())
}
