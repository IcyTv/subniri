use std::hash::Hash;

use async_channel::{Receiver, Sender};
use clap::Parser;
use iced::{Color, Element, Subscription, Task, Theme, keyboard, theme::Style, window::Id};
use iced_layershell::{
	daemon,
	reexport::{Anchor, KeyboardInteractivity, Layer},
	settings::{LayerShellSettings, StartMode},
	to_layer_message,
};
use neo_widgets::style::COLORS;
use zbus::interface;

#[derive(Parser, Clone)]
#[command(version, about)]
struct Args {}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	let _ = pretty_env_logger::try_init();

	let args = Args::parse();

	let (tx, rx) = async_channel::unbounded();

	let listener = DbusListener { tx };
	let _conn = zbus::connection::Builder::session()?
		.name("de.icytv.subniri.Launcher")?
		.serve_at("/de/icytv/subniri/Launcher", listener)?
		.build()
		.await?;

	let app = daemon(
		move || Launcher::new(args.clone(), rx.clone()),
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
		keyboard_interactivity: KeyboardInteractivity::Exclusive,
		..Default::default()
	});

	tokio::task::block_in_place(move || app.run().map_err(Into::into))
}

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
enum Message {
	Open,
	Close,
	Exit,
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
	dbus_rx: Receiver<Message>,
}

impl Launcher {
	fn new(_args: Args, dbus_rx: Receiver<Message>) -> Self {
		Self {
			open: None,
			dbus_rx,
		}
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
			_ => Message::Noop,
		});
		let event = iced::event::listen().map(|ev| match ev {
			iced::Event::Window(iced::window::Event::Unfocused) => Message::Close,
			_ => Message::Noop,
		});
		let dbus = Subscription::run_with(HashableReceiver(self.dbus_rx.clone()), |rx| {
			let rx = rx.0.clone();
			async_stream::stream! {
				while let Ok(msg) = rx.recv().await {
					yield msg;
				}
			}
		});

		Subscription::batch([keyboard, event, dbus])
	}

	#[allow(clippy::needless_pass_by_value)]
	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Close => {
				self.open = None;

				Task::none()
			}
			Message::Open => {
				log::info!("Opening Launcher");

				Task::none()
			}
			Message::Exit => iced::exit(),
			_ => Task::none(),
		}
	}

	fn view(&self, id: Id) -> Element<'_, Message> {
		if self.open != Some(id) {
			return "".into();
		}

		"".into()
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
