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
	IconsUpdated,
}
struct Probe {
	module: system_menu::SystemMenu,
	clock: Option<modules::Module>,
	parent: Option<Id>,
	parent_size: Size,
	popup: Option<Id>,
	contexts: Vec<Id>,
	receiver: iced_wayland_subscriber::shell::ShellReceiver,
}
impl Probe {
	fn new(receiver: iced_wayland_subscriber::shell::ShellReceiver) -> (Self, Task<Message>) {
		let mut config = config::ConfigFile::default();
		if std::env::var("PROBE_CONTEXTS").as_deref() == Ok("1") {
			config.nightlight.enabled = true;
		}
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
				parent_size: Size::new(1280.0, 60.0),
				popup: None,
				contexts: Vec::new(),
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
				self.parent_size = size;
				eprintln!("MENU_READY {size:?}");
			}
			Message::Open if self.popup.is_none() => {
				let bottom = std::env::var("PROBE_EDGE").as_deref() == Ok("bottom-right");
				let id = Id::unique();
				self.popup = Some(id);
				let output_scale = std::env::var("PROBE_OUTPUT_SCALE")
					.ok()
					.and_then(|v| v.parse::<f32>().ok())
					.unwrap_or(1.0);
				self.module.popup_opened(
					id,
					Size::new(self.parent_size.width, 800.0 / output_scale - 60.0),
				);
				return Task::done(Message::NewPopUp {
					id,
					settings: IcedNewPopupSettings::new(
						self.parent.expect("parent"),
						PixelSize::px(480, 640),
						if bottom {
							(self.parent_size.width as i32 - 2, 4)
						} else {
							(
								std::env::var("PROBE_ANCHOR_X")
									.ok()
									.and_then(|x| x.parse().ok())
									.unwrap_or(320),
								56,
							)
						},
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
			Message::Open => {
				if let Some(id) = self.popup.take() {
					eprintln!("MENU_DISMISSED");
					let _ = self.module.popup_closed();
					return iced_runtime::task::effect(iced_runtime::Action::Window(
						iced_runtime::window::Action::Close(id),
					));
				}
			}
			Message::Window(id, window::Event::Closed) if self.popup == Some(id) => {
				self.popup = None;
				self.contexts.clear();
				let _ = self.module.popup_closed();
				eprintln!("MENU_DISMISSED");
			}
			Message::Window(id, window::Event::CloseRequested)
				if self.popup == Some(id) || self.contexts.contains(&id) =>
			{
				return iced_runtime::task::effect(iced_runtime::Action::Window(
					iced_runtime::window::Action::Close(id),
				));
			}
			Message::Window(id, window::Event::CloseRequested) if self.parent == Some(id) => {
				if let Some(popup) = self.contexts.last().copied().or(self.popup) {
					return iced_runtime::task::effect(iced_runtime::Action::Window(
						iced_runtime::window::Action::Close(popup),
					));
				}
			}
			Message::Window(id, window::Event::Closed) if self.contexts.contains(&id) => {
				let depth = self
					.contexts
					.iter()
					.position(|candidate| *candidate == id)
					.expect("context");
				self.contexts.truncate(depth);
			}
			Message::Menu(system_menu::Message::OpenContextMenu(menu, depth, bounds)) => {
				let parent = if depth == 0 {
					self.popup
				} else {
					self.contexts.get(depth - 1).copied()
				};
				if let Some(parent) = parent {
					self.module.open_context_menu(menu, depth);
					self.contexts.truncate(depth);
					let id = Id::unique();
					self.contexts.push(id);
					eprintln!("CONTEXT_OPEN {depth}");
					return Task::done(Message::NewPopUp {
						id,
						settings: IcedNewPopupSettings::new(
							parent,
							PixelSize::px(300, 320),
							(bounds.x.round() as i32, bounds.y.round() as i32),
							PixelSize::px(
								bounds.width.round().max(1.0) as u32,
								bounds.height.round().max(1.0) as u32,
							),
						)
						.anchor(if depth == 0 {
							PopupAnchor::Bottom
						} else {
							PopupAnchor::Right
						})
						.gravity(if depth == 0 {
							PopupGravity::Bottom
						} else {
							PopupGravity::Right
						})
						.constraint_adjustment(
							PopupConstraintAdjustment::SlideX
								| PopupConstraintAdjustment::SlideY
								| PopupConstraintAdjustment::FlipX
								| PopupConstraintAdjustment::FlipY,
						),
					});
				}
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
				if std::env::var("PROBE_CONTEXTS").as_deref() == Ok("1")
					&& matches!(message, system_menu::Message::Nightlight(_))
				{
					// Exercise real widget dispatch without executing device tasks.
					return Task::none();
				}
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
		if let Some(depth) = self.contexts.iter().position(|candidate| *candidate == id) {
			self.module.view_context_menu(depth).map(Message::Menu)
		} else if self.popup == Some(id) {
			let (view, name) = self.clock.as_ref().map_or_else(
				|| (self.module.view_popup().map(Message::Menu), "system-menu"),
				|clock| (clock.view_popup().map(Message::Clock), "clock"),
			);
			let view = if let Ok(ms) = std::env::var("PROBE_ENTER_MS")
				&& std::env::var("PROBE_VISUALS").as_deref() != Ok("1")
			{
				neo_widgets::widgets::grow(
					view,
					Duration::from_millis(ms.parse().expect("enter duration")),
					iced::animation::Easing::EaseOutQuart,
					neo_widgets::widgets::GrowFrom::TopCenter,
				)
				.into()
			} else {
				neo_widgets::widgets::popup_grow(view).into()
			};
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
	if std::env::var("SUBNIRI_LOG_BACKEND").as_deref() == Ok("console") {
		log::try_init("iced_exwlshell::multi_window=debug")?;
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
		popup_transitions: probe_transitions(),
		..Default::default()
	})
	.subscription(|probe: &Probe| {
		let clock = probe.clock.is_some() || std::env::var("PROBE_QUIET").as_deref() == Ok("1");
		let mut subscriptions = vec![
			neo_widgets::icons::background::updates().map(|()| Message::IconsUpdated),
			window::events()
				.with(clock)
				.filter_map(|(clock, (id, event))| {
					if clock && matches!(event, window::Event::RedrawRequested(_)) {
						None
					} else {
						Some(Message::Window(id, event))
					}
				}),
			iced::event::listen_with(|event, status, id| match event {
				iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
					key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
					..
				}) if status == iced::event::Status::Ignored => {
					Some(Message::Window(id, window::Event::CloseRequested))
				}
				_ => None,
			}),
			probe
				.receiver
				.listen()
				.with(clock)
				.filter_map(|(clock, event)| {
					if clock
						&& matches!(
							event,
							iced_wayland_subscriber::shell::ShellEvent::Presented { .. }
						) {
						None
					} else {
						Some(Message::Shell(event))
					}
				}),
		];
		// The production clock has no per-frame model subscription. Do not hide
		// missing backend animation wakeups by driving it with the menu's frames.
		if !clock {
			subscriptions.push(window::frames().map(Message::Frame));
		}
		Subscription::batch(subscriptions)
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

fn probe_transitions() -> Option<iced_exwlshell::popup_animation::Profile> {
	use iced_exwlshell::popup_animation::{Pose, Profile};
	if std::env::var("PROBE_VISUALS").as_deref() != Ok("1") {
		return None;
	}
	let mut profile = Profile::default();
	if let Ok(ms) = std::env::var("PROBE_ENTER_MS") {
		profile.enter.duration = Duration::from_millis(ms.parse().expect("enter duration"));
	}
	if let Ok(ms) = std::env::var("PROBE_CLOSE_MS") {
		profile.exit.duration = Duration::from_millis(ms.parse().expect("close duration"));
	}
	if matches!(
		std::env::var("PROBE_POSE").as_deref(),
		Ok("fixed" | "fixed-clip-alpha")
	) {
		profile.initial = |bounds| Pose {
			transformation: iced::Transformation::translate(12.0, 40.0)
				* iced::Transformation::translate(bounds.center_x(), bounds.y)
				* iced::Transformation::scale(0.8)
				* iced::Transformation::translate(-bounds.center_x(), -bounds.y),
			..Pose::identity(bounds)
		};
		profile.enter.duration = Duration::ZERO;
		profile.enter.sample = |_, from, _| from;
		if std::env::var("PROBE_POSE").as_deref() == Ok("fixed-clip-alpha") {
			profile.initial = |bounds| {
				let transformation = iced::Transformation::translate(12.0, 40.0)
					* iced::Transformation::translate(bounds.center_x(), bounds.y)
					* iced::Transformation::scale(0.8)
					* iced::Transformation::translate(-bounds.center_x(), -bounds.y);
				Pose {
					transformation,
					opacity: 0.6,
					clip: iced::Rectangle {
						x: bounds.center_x(),
						width: bounds.width / 2.0,
						..bounds
					},
				}
			};
		}
	}
	Some(profile)
}
