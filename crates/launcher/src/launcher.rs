use std::{
	collections::HashSet,
	env, fs,
	hash::Hash,
	process::Command,
	sync::Arc,
	time::{Duration, Instant},
};

use async_channel::Receiver;
use futures::{StreamExt, future::join_all};
use iced::{
	Alignment, Animation, Border, Color, Element, Font, Length, Subscription, Task, Theme, font,
	keyboard,
	theme::Style,
	widget::{column, container, image, row, space, svg, text, text::Rich, text_input},
	window::Id,
};
use iced_layershell::{
	reexport::{Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption},
	to_layer_message,
};
use neo_widgets::{
	icons::ResolvedIcon,
	phosphor_icon,
	style::COLORS,
	widgets::{neo_button, neo_card, neo_scrollable, spinner},
};

use crate::{
	dbus::DbusListener,
	providers::{
		Activation, ActivationKey, Candidate, CandidateId, PreviewModel, Provider, ProviderContext,
		ProviderEvent, ProviderId, ProviderStatus, Query, Revision, SessionHandle, SessionId,
		compare_candidates,
	},
	utils::follow_focus,
};

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub enum Message {
	Open,
	Close,
	Exit,
	ConnEstablished(Result<zbus::Connection, zbus::Error>),
	WindowOpened(Id),
	WindowFocused(Id),
	WindowUnfocused(Id),
	SearchChanged(String),
	Cycle(bool),
	Activate(ProviderId, CandidateId, ActivationKey),
	ActivateIndex(usize),
	Resumed,
	RestartAfterResume,
	Redraw(Instant),
	ProviderEvent(ProviderId, ProviderEvent),
	Iced(iced::Event),
	Noop,
}

pub struct Launcher {
	providers: Arc<[Arc<dyn Provider>]>,
	open: Option<Id>,
	conn: Option<zbus::Connection>,
	dbus_rx: Receiver<Message>,
	search: String,
	exit_on_close: bool,
	keep_open_on_focus_loss: bool,
	input_id: iced::widget::Id,
	scrollable_id: iced::widget::Id,
	loading: HashSet<ProviderId>,
	candidates: Vec<Candidate>,
	has_candidates: Animation<bool>,
	session: SessionId,
	revision: Revision,
}

impl Launcher {
	pub fn new(args: &crate::Args, providers: Arc<[Arc<dyn Provider>]>) -> (Self, Task<Message>) {
		let (tx, dbus_rx) = async_channel::unbounded();

		let dbus_conn_task = Task::future(async move {
			let res = DbusListener::connect(tx).await;

			Message::ConnEstablished(res)
		});
		let mut tasks = vec![dbus_conn_task];

		if args.open {
			tasks.push(Task::done(Message::Open));
		}

		(
			Self {
				providers,
				open: None,
				dbus_rx,
				conn: None,
				search: String::new(),
				exit_on_close: args.exit_on_close,
				keep_open_on_focus_loss: args.no_focus,
				input_id: iced::widget::Id::unique(),
				scrollable_id: iced::widget::Id::unique(),
				loading: HashSet::new(),
				candidates: Vec::new(),
				has_candidates: Animation::new(false).easing(iced::animation::Easing::EaseOutQuad),
				session: SessionId(0),
				revision: Revision(0),
			},
			Task::batch(tasks),
		)
	}

	pub fn namespace() -> String {
		String::from("avalaunch")
	}

	pub fn subscription(&self) -> Subscription<Message> {
		let keyboard = keyboard::listen().map(|e| match e {
			keyboard::Event::KeyPressed {
				key: keyboard::Key::Named(keyboard::key::Named::Escape),
				..
			} => Message::Close,
			keyboard::Event::KeyPressed {
				key: keyboard::Key::Named(keyboard::key::Named::ArrowUp),
				..
			} => Message::Cycle(true),
			keyboard::Event::KeyPressed {
				key: keyboard::Key::Named(keyboard::key::Named::ArrowDown),
				..
			} => Message::Cycle(false),
			keyboard::Event::KeyPressed {
				key: keyboard::Key::Named(keyboard::key::Named::Tab),
				modifiers,
				..
			} => Message::Cycle(modifiers.contains(keyboard::Modifiers::SHIFT)),
			_ => Message::Noop,
		});
		let event = iced::event::listen_with(|event, _status, window| match event {
			iced::Event::Window(iced::window::Event::Opened { .. }) => {
				Some(Message::WindowOpened(window))
			}
			iced::Event::Window(iced::window::Event::Focused) => {
				Some(Message::WindowFocused(window))
			}
			iced::Event::Window(iced::window::Event::Unfocused) => {
				Some(Message::WindowUnfocused(window))
			}
			_ => None,
		});
		let dbus = Subscription::run_with(HashableReceiver(self.dbus_rx.clone()), |rx| {
			let rx = rx.0.clone();
			async_stream::stream! {
				while let Ok(msg) = rx.recv().await {
					yield msg;
				}
			}
		});
		let resume = resume_events();
		let providers =
			Subscription::run_with(HashableProviders(self.providers.clone()), |providers| {
				let providers = providers.0.clone();
				async_stream::stream! {
					let receivers = join_all(providers
						.iter()
						.map(|p| async move {
							let pid = p.id();
							(pid, p.init(Arc::new(DummyCtx)).await)
						}))
						.await;
					let provider_streams = receivers
						.into_iter()
						.filter_map(|(pid, maybe_receiver)| {
							match maybe_receiver {
								Ok(receiver) => Some(receiver.map(move |ev| (pid, ev)).boxed()),
								Err(e) => {
									eprintln!("Provide {pid:?} failed to start: {e}");
									None
								}
							}
						})
						.collect::<futures::stream::SelectAll<_>>();
					let mut provider_streams = Box::pin(provider_streams);

					while let Some((provider_id, event)) = provider_streams.next().await {
						yield Message::ProviderEvent(provider_id, event);
					}
				}
			});
		let iced = iced::event::listen().map(Message::Iced);
		let frames = iced::window::frames().map(Message::Redraw);

		Subscription::batch([keyboard, event, dbus, resume, providers, iced, frames])
	}

	#[allow(clippy::needless_pass_by_value)]
	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Resumed => Task::future(async {
				tokio::time::sleep(Duration::from_millis(500)).await;
				Message::RestartAfterResume
			}),
			Message::RestartAfterResume => restart_launcher_process(),
			Message::Close if let Some(id) = self.open.take() => {
				self.search.clear();
				self.candidates.clear();
				self.has_candidates.go_mut(false, Instant::now());
				self.session.0 = self.session.0.wrapping_add(1);
				let close = Message::RemoveWindow(id);
				let exit_task = if self.exit_on_close {
					iced::exit()
				} else {
					Task::none()
				};
				Task::done(close).chain(exit_task)
			}
			Message::Open => {
				log::info!("Opening Launcher");

				let (id, new) = Self::new_layer();
				self.open = Some(id);
				Task::done(new)
			}
			Message::WindowOpened(id) if self.open == Some(id) => {
				log::debug!("Focusing input");
				iced::widget::operation::focus(self.input_id.clone())
			}
			Message::WindowFocused(id) if self.open == Some(id) => {
				log::debug!("Focusing input after window focus");
				iced::widget::operation::focus(self.input_id.clone())
			}
			Message::WindowUnfocused(id)
				if self.open == Some(id) && !self.keep_open_on_focus_loss =>
			{
				self.update(Message::Close)
			}
			Message::SearchChanged(search) => {
				self.search = search;
				self.revision.0 = self.revision.0.wrapping_add(1);

				let query = Query {
					raw: Arc::from(self.search.clone()),
					cursor: 0,
				};

				let session = self.session_handle();
				let ctx = Arc::new(DummyCtx);

				let providers = self.providers.clone();
				Task::future(async move {
					let update_futs = providers
						.iter()
						.map(|p| p.update_query(session, query.clone(), ctx.clone()));
					join_all(update_futs).await;
					Message::Noop
				})
			}
			Message::Cycle(upwards) => {
				if self.candidates.is_empty() {
					Task::none()
				} else if upwards {
					iced::widget::operation::focus_previous()
						.chain(follow_focus(self.scrollable_id.clone()))
				} else {
					iced::widget::operation::focus_next()
						.chain(follow_focus(self.scrollable_id.clone()))
				}
			}
			Message::ConnEstablished(Ok(conn)) => {
				self.conn = Some(conn);
				Task::none()
			}
			Message::ConnEstablished(Err(e)) => {
				log::error!("Error: {e}");

				iced::exit()
			}

			Message::Exit => iced::exit(),

			Message::ProviderEvent(pid, event) => self.update_provider_event(pid, event),

			Message::Activate(provider, cand_id, activation) => {
				let Some(provider) = self.providers.iter().find(|p| p.id() == provider) else {
					log::warn!("Provider not found");
					return Task::none();
				};
				let provider = provider.clone();

				let session = self.session_handle();

				Task::future(async move {
					match provider.activate(session, &cand_id, &activation).await {
						Ok(
							Activation::Noop | Activation::KeepOpen | Activation::SetResponse(_),
						) => Message::Noop,
						Ok(Activation::CloseLauncher | Activation::HideLauncher) => Message::Close,
						Ok(Activation::SetInput(inp)) => Message::SearchChanged(inp),
						Err(e) => {
							log::error!("Failed to activate: {e}");
							Message::Noop
						}
					}
				})
			}
			Message::ActivateIndex(index) => {
				let Some(cand) = self.candidates.get(index) else {
					return Task::none();
				};

				Task::done(Message::Activate(
					cand.provider,
					cand.id.clone(),
					cand.activation.clone(),
				))
			}

			Message::Iced(iced::Event::Window(iced::window::Event::RedrawRequested(now)))
			| Message::Redraw(now)
				if self.has_candidates.is_animating(now) =>
			{
				iced_runtime::task::effect(iced_runtime::Action::Window(
					iced::window::Action::RedrawAll,
				))
			}

			_ => Task::none(),
		}
	}

	fn new_layer() -> (Id, Message) {
		let id = iced::window::Id::unique();
		let new = Message::NewLayerShell {
			settings: NewLayerShellSettings {
				size: Some((0, 0)),
				layer: Layer::Overlay,
				anchor: Anchor::all(),
				exclusive_zone: None,
				keyboard_interactivity: KeyboardInteractivity::Exclusive,
				output_option: OutputOption::Active,
				namespace: Some(Self::namespace()),
				..Default::default()
			},
			id,
		};

		(id, new)
	}

	// TODO: Remove entries with wrong session? Do we then need i.e. CandidateRemove, etc? When do
	// we remove? Periodically? On event? On new session?
	fn update_provider_event(&mut self, pid: ProviderId, event: ProviderEvent) -> Task<Message> {
		match event {
			ProviderEvent::Status(ProviderStatus::Loading) => {
				self.loading.insert(pid);
				Task::none()
			}

			ProviderEvent::Status(ProviderStatus::Ready | ProviderStatus::Error(_))
			| ProviderEvent::Done => {
				self.loading.remove(&pid);
				Task::none()
			}
			ProviderEvent::CandidateUpsert(cand) => {
				if let Some(idx) = self.candidates.iter().position(|c| c.id == cand.id) {
					#[allow(clippy::indexing_slicing)]
					let candidate = &mut self.candidates[idx];

					*candidate = cand;
				} else {
					self.candidates.push(cand);
				}

				self.candidates.sort_by(compare_candidates);

				self.has_candidates.go_mut(true, Instant::now());

				Task::none()
			}
			ProviderEvent::CandidateRemove { id } => {
				self.candidates.retain(|c| c.id != id);

				if self.candidates.is_empty() {
					self.has_candidates.go_mut(false, Instant::now());
				}

				Task::none()
			}
			ProviderEvent::Reset => {
				self.candidates.retain(|c| c.provider != pid);

				if self.candidates.is_empty() {
					self.has_candidates.go_mut(false, Instant::now());
				}

				Task::none()
			}
			ev => {
				log::debug!("Provider event from {pid:?}: {ev:?}");
				Task::none()
			}
		}
	}

	pub fn view(&self, id: Id) -> Element<'_, Message> {
		if self.open != Some(id) {
			return "".into();
		}

		let input = self.search_row();

		let mut content = column![].height(Length::Fill);

		let end = self.candidates.len().min(10);

		for cand in self.candidates.get(..end).unwrap_or(&[]) {
			let mut display = row![].spacing(12);

			if let Some(icon) = &cand.icon {
				let icon: Element<'_, Message> = match icon {
					ResolvedIcon::Svg(handle) => {
						svg(handle.clone()).height(20).width(Length::Shrink).into()
					}
					ResolvedIcon::Image(handle) => {
						image(handle).height(20).width(Length::Shrink).into()
					}
				};
				display = display.push(icon);
			}

			let title_text: Element<'_, Message> = if let Some(title_spans) = &cand.title_spans {
				Rich::<'_, (), Message>::with_spans(title_spans)
					.font(Font {
						weight: font::Weight::Bold,
						..Default::default()
					})
					.color(COLORS.text)
					.into()
			} else {
				text(cand.title.as_ref())
					.color(COLORS.text)
					.weight(font::Weight::Bold)
					.into()
			};

			let mut title = column![title_text].spacing(4);

			if let Some(subtitle) = &cand.subtitle {
				title = title.push(
					Rich::<'_, (), Message>::with_spans(subtitle)
						.size(14)
						.color(COLORS.text.scale_alpha(0.8)),
				);
			}

			display = display.push(title);

			display = display.push(space::horizontal());

			if let Some(right_text) = &cand.right_text {
				display = display.push(right_text.as_ref());
			}

			let display = neo_button(display)
				.background(COLORS.body)
				.focusable(true)
				.focus_color(COLORS.decorative.pink)
				.on_press(Message::Activate(
					cand.provider,
					cand.id.clone(),
					cand.activation.clone(),
				))
				.id(format!("candidate:{}:{}", cand.provider.0, cand.id.0))
				.width(Length::Fill);

			content = content.push(display);
		}

		let content = neo_scrollable(content)
			.width(Length::Fill)
			.height(Length::Fill)
			.id(self.scrollable_id.clone());

		let content = column![input, content].height(Length::Fill);

		let height = self.has_candidates.interpolate(60.0, 480.0, Instant::now());
		// let height = 480.0;

		let content = neo_card(content).width(640).height(height).radius(8.0);

		container(content)
			.width(Length::Fill)
			.height(Length::Fill)
			.align_x(Alignment::Center)
			.align_y(Alignment::Center)
			.style(|_| container::Style {
				background: Some(iced::Background::Color(COLORS.black.scale_alpha(0.4))),
				..Default::default()
			})
			.into()
	}

	fn search_row(&self) -> Element<'_, Message> {
		let mut search_row = row![
			svg(phosphor_icon!("magnifying-glass"))
				.height(20)
				.width(Length::Shrink),
			self.search_box(),
		]
		.align_y(Alignment::Center)
		.spacing(10);

		if !self.loading.is_empty() {
			search_row = search_row.push(spinner().size(20.0));
		}

		search_row.into()
	}

	fn search_box(&self) -> Element<'_, Message> {
		text_input("Search", &self.search)
			.id(self.input_id.clone())
			.width(Length::Fill)
			.font(Font {
				weight: font::Weight::Bold,
				..Default::default()
			})
			.size(20)
			.style(|_, _| text_input::Style {
				background: iced::Background::Color(COLORS.white),
				border: Border::default().width(0),
				icon: COLORS.black,
				value: COLORS.text,
				selection: COLORS.decorative.pink,
				placeholder: COLORS.text.scale_alpha(0.7),
			})
			.on_input(Message::SearchChanged)
			.on_submit(Message::ActivateIndex(0))
			.into()
	}

	pub fn style(&self, _theme: &Theme) -> Style {
		let _ = self;

		Style {
			background_color: Color::TRANSPARENT,
			text_color: COLORS.text,
		}
	}

	fn session_handle(&self) -> SessionHandle {
		SessionHandle {
			session_id: self.session,
			revision: self.revision,
		}
	}
}

struct HashableReceiver(Receiver<Message>);

impl Hash for HashableReceiver {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		0xdead_beefu32.hash(state);
	}
}

struct HashableProviders(Arc<[Arc<dyn Provider>]>);

impl Hash for HashableProviders {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		0xdead_beefu32.hash(state);
	}
}

struct DummyCtx;

#[async_trait::async_trait]
impl ProviderContext for DummyCtx {
	async fn hide(&self) {}
	async fn close(&self) {}
	async fn set_input(&self, _input: String) {}
	async fn set_preview(&self, _preview: PreviewModel) {}
	async fn set_response(&self, _response: String) {}
}

fn resume_events() -> Subscription<Message> {
	if running_under_systemd_service() {
		return Subscription::none();
	}

	Subscription::run(|| {
		async_stream::stream! {
			let connection = match zbus::Connection::system().await {
				Ok(connection) => connection,
				Err(error) => {
					log::error!("Failed to connect to system bus for resume events: {error}");
					return;
				}
			};

			let proxy = match zbus::Proxy::new(
				&connection,
				"org.freedesktop.login1",
				"/org/freedesktop/login1",
				"org.freedesktop.login1.Manager",
			)
			.await
			{
				Ok(proxy) => proxy,
				Err(error) => {
					log::error!("Failed to connect to login manager for resume events: {error}");
					return;
				}
			};

			let mut sleep_signals = match proxy.receive_signal("PrepareForSleep").await {
				Ok(sleep_signals) => sleep_signals,
				Err(error) => {
					log::error!("Failed to listen for sleep preparation: {error}");
					return;
				}
			};

			while let Some(signal) = sleep_signals.next().await {
				match signal.body().deserialize::<bool>() {
					Ok(false) => yield Message::Resumed,
					Ok(true) => (),
					Err(error) => log::warn!("Failed to read sleep preparation signal: {error}"),
				}
			}
		}
	})
}

fn restart_launcher_process() -> Task<Message> {
	let exe = match env::current_exe() {
		Ok(exe) => exe,
		Err(error) => {
			log::error!("Failed to find current executable for resume restart: {error}");
			return Task::none();
		}
	};

	let mut command = Command::new(exe);
	command.args(env::args_os().skip(1));

	if let Ok(current_dir) = env::current_dir() {
		command.current_dir(current_dir);
	}

	match command.spawn() {
		Ok(_) => iced_runtime::exit(),
		Err(error) => {
			log::error!("Failed to restart launcher after resume: {error}");
			Task::none()
		}
	}
}

fn running_under_systemd_service() -> bool {
	fs::read_to_string("/proc/self/cgroup").is_ok_and(|cgroup| {
		cgroup.lines().any(|line| {
			let path = line.rsplit_once(':').map_or(line, |(_, path)| path);
			path.split('/').any(|part| part.ends_with(".service"))
		})
	})
}
