use std::{
	collections::HashSet,
	hash::Hash,
	sync::{Arc, LazyLock},
	time::Instant,
};

use async_channel::{Receiver, Sender};
use clap::Parser;
use futures::{StreamExt, future::join_all};
use iced::{
	Alignment, Animation, Border, Color, Element, Font, Length, Subscription, Task, Theme, font,
	keyboard,
	theme::Style,
	widget::{column, container, row, scrollable, space, svg, text, text_input},
	window::Id,
};
use iced_layershell::{
	daemon,
	reexport::{Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption},
	settings::{LayerShellSettings, StartMode},
	to_layer_message,
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{neo_button, neo_card, spinner},
};
use zbus::interface;

use crate::{
	providers::{
		Activation, ActivationKey, Candidate, CandidateId, PreviewModel, Provider, ProviderContext,
		ProviderEvent, ProviderId, ProviderStatus, Query, Revision, SessionHandle, SessionId,
		applications::ApplicationProvider, calculator::CalcProvider, compare_candidates,
	},
	utils::follow_focus,
};

mod providers;
mod utils;

#[derive(Parser, Clone)]
#[command(version, about)]
struct Args {
	/// Open the launcher on startup
	#[clap(long)]
	open: bool,
	/// Exit the launcher on close
	#[clap(long)]
	exit_on_close: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let _ = pretty_env_logger::try_init();

	let args = Args::parse();

	let app = daemon(
		move || Launcher::new(&args),
		Launcher::namespace,
		Launcher::update,
		Launcher::view,
	)
	.style(Launcher::style)
	.subscription(Launcher::subscription)
	.layer_settings(LayerShellSettings {
		size: None,
		anchor: Anchor::all(),
		start_mode: StartMode::Background,
		layer: Layer::Overlay,
		keyboard_interactivity: KeyboardInteractivity::None,
		..Default::default()
	});

	tokio::task::block_in_place(move || app.run().map_err(Into::into))
}

static PROVIDERS: LazyLock<Arc<[Arc<dyn Provider>]>> = LazyLock::new(|| {
	Arc::<[Arc<dyn Provider>]>::from([
		Arc::new(CalcProvider::new()) as _,
		Arc::new(ApplicationProvider::new()) as _,
	])
});

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
enum Message {
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
	Redraw(Instant),
	ProviderEvent(ProviderId, ProviderEvent),
	Iced(iced::Event),
	Noop,
}

struct HashableReceiver(Receiver<Message>);

impl Hash for HashableReceiver {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		0xdead_beefu32.hash(state);
	}
}

struct Launcher {
	open: Option<Id>,
	conn: Option<zbus::Connection>,
	dbus_rx: Receiver<Message>,
	search: String,
	exit_on_close: bool,
	input_id: iced::widget::Id,
	scrollable_id: iced::widget::Id,
	loading: HashSet<ProviderId>,
	candidates: Vec<Candidate>,
	has_candidates: Animation<bool>,
}

impl Launcher {
	fn new(args: &Args) -> (Self, Task<Message>) {
		let (tx, dbus_rx) = async_channel::unbounded();

		let dbus_conn_task = Task::future(async move {
			let listener = DbusListener { tx };

			let res = zbus::connection::Builder::session()
				.and_then(|s| s.name("de.icytv.subniri.Launcher"))
				.and_then(|s| s.serve_at("/de/icytv/subniri/Launcher", listener))
				.map(zbus::connection::Builder::build);

			let res = match res {
				Ok(s) => s.await,
				Err(e) => Err(e),
			};

			Message::ConnEstablished(res)
		});
		let mut tasks = vec![dbus_conn_task];

		if args.open {
			tasks.push(Task::done(Message::Open));
		}

		(
			Self {
				open: None,
				dbus_rx,
				conn: None,
				search: String::new(),
				exit_on_close: args.exit_on_close,
				input_id: iced::widget::Id::unique(),
				scrollable_id: iced::widget::Id::unique(),
				loading: HashSet::new(),
				candidates: Vec::new(),
				has_candidates: Animation::new(false).easing(iced::animation::Easing::EaseOutQuad),
			},
			Task::batch(tasks),
		)
	}

	fn namespace() -> String {
		String::from("avalaunch")
	}

	fn subscription(&self) -> Subscription<Message> {
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
		let providers = Subscription::run(|| {
			async_stream::stream! {
				let receivers = join_all(PROVIDERS
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

		Subscription::batch([keyboard, event, dbus, providers, iced, frames])
	}

	#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Close if let Some(id) = self.open.take() => {
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
			Message::WindowUnfocused(id) if self.open == Some(id) => self.update(Message::Close),
			Message::SearchChanged(search) => {
				self.search = search;

				let query = Query {
					raw: Arc::from(self.search.clone()),
					cursor: 0,
				};

				let session = SessionHandle {
					session_id: SessionId(0),
					revision: Revision(1),
				};
				let ctx = Arc::new(DummyCtx);

				Task::future(async move {
					let update_futs = PROVIDERS
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

			Message::ProviderEvent(pid, ProviderEvent::Status(ProviderStatus::Loading)) => {
				self.loading.insert(pid);
				Task::none()
			}
			Message::ProviderEvent(
				pid,
				ProviderEvent::Status(ProviderStatus::Ready | ProviderStatus::Error(_))
				| ProviderEvent::Done,
			) => {
				self.loading.remove(&pid);
				Task::none()
			}
			Message::ProviderEvent(_pid, ProviderEvent::CandidateUpsert(cand)) => {
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
			Message::ProviderEvent(_pid, ProviderEvent::CandidateRemove { id }) => {
				self.candidates.retain(|c| c.id != id);

				if self.candidates.is_empty() {
					self.has_candidates.go_mut(false, Instant::now());
				}

				Task::none()
			}
			Message::ProviderEvent(pid, ProviderEvent::Reset) => {
				self.candidates.retain(|c| c.provider != pid);

				if self.candidates.is_empty() {
					self.has_candidates.go_mut(false, Instant::now());
				}

				Task::none()
			}
			Message::ProviderEvent(pid, ev) => {
				log::debug!("Provider event from {pid:?}: {ev:?}");
				Task::none()
			}

			Message::Activate(provider, cand_id, activation) => {
				let Some(provider) = PROVIDERS.iter().find(|p| p.id() == provider) else {
					log::warn!("Provider not found");
					return Task::none();
				};

				let session = SessionHandle {
					session_id: SessionId(0),
					revision: Revision(1),
				};

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

	fn view(&self, id: Id) -> Element<'_, Message> {
		if self.open != Some(id) {
			return "".into();
		}

		let input = text_input("Search", &self.search)
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
			.on_input(Message::SearchChanged);

		let mut input = row![
			svg(phosphor_icon!("magnifying-glass"))
				.height(20)
				.width(Length::Shrink),
			input,
		]
		.align_y(Alignment::Center)
		.spacing(10);

		if !self.loading.is_empty() {
			input = input.push(spinner().size(20.0));
		}

		let mut content = column![input].height(Length::Fill);

		let end = self.candidates.len().min(10);

		for cand in self.candidates.get(..end).unwrap_or(&[]) {
			let mut display = row![].spacing(12);

			if let Some(icon) = &cand.icon {
				display = display.push(svg(icon.clone()).height(20).width(Length::Shrink));
			}

			let mut title = column![
				text(cand.title.as_ref())
					.color(COLORS.text)
					.weight(font::Weight::Bold),
			]
			.spacing(4);

			if let Some(subtitle) = &cand.subtitle {
				title = title.push(
					text(subtitle.as_ref())
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

		let content = scrollable(content)
			.width(Length::Fill)
			.height(Length::Fill)
			.id(self.scrollable_id.clone());

		let height = self.has_candidates.interpolate(60.0, 480.0, Instant::now());

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

	fn style(&self, _theme: &Theme) -> Style {
		let _ = self;

		Style {
			background_color: Color::TRANSPARENT,
			text_color: COLORS.text,
		}
	}
}

struct DbusListener {
	tx: Sender<Message>,
}

impl DbusListener {
	async fn send(&self, msg: Message) -> zbus::fdo::Result<()> {
		self.tx
			.send(msg)
			.await
			.map_err(|e| zbus::fdo::Error::Failed(format!("{e}")))
	}
}

#[interface(name = "de.icytv.subniri.Launcher")]
impl DbusListener {
	async fn open(&self) -> zbus::fdo::Result<()> {
		self.send(Message::Open).await
	}

	async fn close(&self) -> zbus::fdo::Result<()> {
		self.send(Message::Close).await
	}

	async fn exit(&self) -> zbus::fdo::Result<()> {
		self.send(Message::Exit).await
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
