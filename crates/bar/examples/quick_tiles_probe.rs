//! Isolated production menu/gutter input probe. Never launches device backends.
use iced::{
	Element, Size, Subscription, Task,
	widget::text,
	window::{self, Id},
};
use iced_exwlshell::{
	actions::IcedNewPopupSettings,
	daemon,
	reexport::{
		Anchor, KeyboardInteractivity, LayerSize, PixelSize, PopupAnchor,
		PopupConstraintAdjustment, PopupGravity,
	},
	settings::{LayerShellSettings, StartMode},
	to_layer_message,
};
use neo_widgets::widgets::neo_button;
use std::time::Duration;
#[allow(dead_code)]
#[path = "../src/modules/mod.rs"]
mod modules;
use modules::system_menu;
#[path = "support/opening_audit.rs"]
mod opening_audit;

#[to_layer_message(multi)]
#[derive(Clone, Debug)]
enum Message {
	Window(Id, window::Event),
	Shell(iced_wayland_subscriber::shell::ShellEvent),
	Menu(system_menu::Message),
	Clock(modules::ModuleMessage),
	Open,
	Exit,
	Frame(std::time::Instant),
}
struct Probe {
	module: system_menu::SystemMenu,
	clock: Option<modules::Module>,
	parent: Option<Id>,
	popup: Option<Id>,
	receiver: iced_wayland_subscriber::shell::ShellReceiver,
}
impl Probe {
	fn new(receiver: iced_wayland_subscriber::shell::ShellReceiver) -> (Self, Task<Message>) {
		let mut config = config::ConfigFile::default();
		config.system_menu =
			config::SystemMenu::from_widgets(vec![config::SystemMenuWidgets::Nightlight]);
		if std::env::var("PROBE_FIXED").as_deref() == Ok("1") {
			config.system_menu =
				config::SystemMenu::from_widgets(config::SystemMenuWidgets::VARIANTS.to_vec());
		}
		if std::env::var("PROBE_CYCLES").as_deref() == Ok("1") {
			let mut rows = config.system_menu.effective_rows();
			rows.push(config::MenuRow::default());
			config.system_menu = config::SystemMenu::from_rows(rows);
		}
		(
			Self {
				module: system_menu::SystemMenu::new(&config),
				clock: (std::env::var("PROBE_CLOCK").as_deref() == Ok("1"))
					.then(modules::Module::clock),
				parent: None,
				popup: None,
				receiver,
			},
			Task::future(async {
				tokio::time::sleep(Duration::from_secs(30)).await;
				Message::Exit
			}),
		)
	}
	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Clock(message) => {
				if let Some(clock) = &mut self.clock {
					return clock.update(message).map(Message::Clock);
				}
			}
			Message::Frame(now) => {
				let _ = self.module.update(system_menu::Message::Frame(now));
			}
			Message::Window(id, window::Event::Opened { size, .. }) if self.parent.is_none() => {
				self.parent = Some(id);
				eprintln!("MENU_READY {size:?}");
			}
			Message::Open if self.popup.is_none() => {
				let bottom = std::env::var("PROBE_EDGE").as_deref() == Ok("bottom-right");
				let id = Id::unique();
				self.popup = Some(id);
				self.module.popup_opened(id, Size::new(1280.0, 740.0));
				return Task::done(Message::NewPopUp {
					id,
					settings: IcedNewPopupSettings::new(
						self.parent.expect("parent"),
						PixelSize::px(480, 640),
						if bottom { (1278, 4) } else { (320, 56) },
						PixelSize::px(1, 1),
					)
					.anchor(if bottom {
						PopupAnchor::TopRight
					} else {
						PopupAnchor::Bottom
					})
					.gravity(if bottom {
						PopupGravity::TopLeft
					} else {
						PopupGravity::Bottom
					})
					.constraint_adjustment(
						PopupConstraintAdjustment::SlideX
							| PopupConstraintAdjustment::SlideY
							| PopupConstraintAdjustment::FlipY,
					),
				});
			}
			Message::Window(
				id,
				window::Event::Opened { size, .. } | window::Event::Resized(size),
			) if self.popup == Some(id) => {
				self.module.popup_configured(id, size);
				eprintln!("MENU_CONFIGURE {} {}", size.width, size.height);
			}
			Message::Window(id, window::Event::Focused | window::Event::Unfocused) => {
				eprintln!("MENU_FOCUS {id:?}")
			}
			Message::Shell(iced_wayland_subscriber::shell::ShellEvent::Presented {
				window,
				content_updated_at,
			}) => self.module.popup_presented(window, content_updated_at),
			Message::Menu(system_menu::Message::Dismiss) => {
				if let Some(id) = self.popup.take() {
					eprintln!("MENU_DISMISSED");
					let _ = self.module.popup_closed();
					return iced_runtime::task::effect(iced_runtime::Action::Window(
						iced_runtime::window::Action::Close(id),
					));
				}
			}
			Message::Menu(message) => {
				eprintln!("MENU_INPUT {message:?}");
				return self.module.update(message).map(Message::Menu);
			}
			Message::Window(_, window::Event::RedrawRequested(now)) => {
				let _ = self.module.update(system_menu::Message::Frame(now));
			}
			Message::Exit => return iced::exit(),
			_ => {}
		}
		Task::none()
	}
	fn view(&self, id: Id) -> Element<'_, Message> {
		if self.popup == Some(id) {
			let (view, name) = self.clock.as_ref().map_or_else(
				|| (self.module.view_popup().map(Message::Menu), "system-menu"),
				|clock| (clock.view_popup().map(Message::Clock), "clock"),
			);
			let view = neo_widgets::widgets::popup_grow(view).into();
			if std::env::var("PROBE_OPEN").as_deref() == Ok("1") {
				opening_audit::audit(view, name)
			} else {
				view
			}
		} else {
			neo_button(text("Open menu"))
				.width(140)
				.height(56)
				.on_press(Message::Open)
				.into()
		}
	}
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
	let runtime = std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR")?);
	if !runtime.starts_with("/tmp/opencode")
		|| std::fs::read_to_string(runtime.join("quick-tiles.marker"))?
			!= "isolated quick tiles input"
	{
		return Err("launch with scripts/test-quick-tiles.py".into());
	}
	let (broadcast, receiver) = iced_wayland_subscriber::shell::channel();
	daemon(
		move || Probe::new(receiver.clone()),
		|| "quick-tiles-probe".into(),
		Probe::update,
		Probe::view,
	)
	.theme(neo_widgets::style::neo_theme())
	.style(|_: &Probe, _: &iced::Theme| iced::theme::Style {
		background_color: iced::Color::TRANSPARENT,
		text_color: neo_widgets::style::COLORS.text,
	})
	.settings(iced_exwlshell::Settings {
		shell_broadcast: broadcast,
		..Default::default()
	})
	.subscription(|probe: &Probe| {
		Subscription::batch([
			window::events().map(|(id, event)| Message::Window(id, event)),
			probe.receiver.listen().map(Message::Shell),
			window::frames().map(Message::Frame),
		])
	})
	.layer_settings(LayerShellSettings {
		size: LayerSize::fill_width(60),
		anchor: (if std::env::var("PROBE_EDGE").as_deref() == Ok("bottom-right") {
			Anchor::Bottom
		} else {
			Anchor::Top
		}) | Anchor::Left
			| Anchor::Right,
		start_mode: StartMode::Active,
		keyboard_interactivity: KeyboardInteractivity::OnDemand,
		exclusive_zone: 60,
		..Default::default()
	})
	.run()?;
	Ok(())
}
