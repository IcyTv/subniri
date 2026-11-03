//! Isolated rendering probe. Run via scripts/popup-surface-stress.py, not on
//! the desktop's Wayland socket. No bar config, D-Bus or PipeWire connections.
use std::{collections::HashSet, time::Duration};

use iced::{
	Element, Subscription, Task,
	widget::{column, container, text},
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

#[allow(dead_code)]
#[path = "../src/modules/mod.rs"]
mod modules;

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
enum Message {
	Tick,
	Window(Id, window::Event),
	Clock(modules::ModuleMessage),
}

struct Probe {
	parent: Option<Id>,
	popup: Option<Id>,
	ticks: u32,
	cycles: u32,
	opened: u32,
	interval: Duration,
	clock: modules::Module,
	simple: bool,
	popup_ids: HashSet<Id>,
	animate: bool,
}

impl Probe {
	fn new() -> (Self, Task<Message>) {
		(
			Self {
				parent: None,
				popup: None,
				ticks: 0,
				opened: 0,
				cycles: std::env::var("SUBNIRI_STRESS_CYCLES")
					.ok()
					.and_then(|v| v.parse().ok())
					.unwrap_or(300),
				interval: Duration::from_millis(
					std::env::var("SUBNIRI_STRESS_INTERVAL_MS")
						.ok()
						.and_then(|v| v.parse().ok())
						.unwrap_or(8),
				),
				clock: modules::Module::clock(),
				simple: std::env::var("SUBNIRI_STRESS_CONTENT").as_deref() == Ok("simple"),
				popup_ids: HashSet::new(),
				animate: std::env::var_os("SUBNIRI_STRESS_NO_ANIMATION").is_none(),
			},
			Task::none(),
		)
	}

	fn subscription(&self) -> Subscription<Message> {
		Subscription::batch([
			iced::time::every(self.interval).map(|_| Message::Tick),
			window::events().map(|(id, event)| Message::Window(id, event)),
		])
	}

	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Clock(message) => return self.clock.update(message).map(Message::Clock),
			Message::Window(id, window::Event::Opened { .. }) => {
				if self.popup_ids.contains(&id) {
					self.opened += 1;
				} else if self.parent.is_none() {
					self.parent = Some(id);
				}
				log::info!("[DEBUG-surface-probe] opened id={id:?} tick={}", self.ticks);
			}
			Message::Window(id, window::Event::Closed) => {
				log::info!("[DEBUG-surface-probe] closed id={id:?} tick={}", self.ticks);
			}
			Message::Tick => {
				let Some(parent) = self.parent else {
					return Task::none();
				};
				self.ticks += 1;
				if self.ticks > self.cycles * 2 + 20 {
					if self.opened == 0 {
						eprintln!("STRESS_INCONCLUSIVE no popup opened");
						std::process::exit(2);
					}
					eprintln!(
						"STRESS_COMPLETE cycles={} opened={}",
						self.cycles, self.opened
					);
					return iced::exit();
				}
				if let Some(id) = self.popup.take() {
					log::info!(
						"[DEBUG-surface-probe] close-request id={id:?} tick={}",
						self.ticks
					);
					let _ = self.clock.update(modules::ModuleMessage::PopupClosed);
					return iced_runtime::task::effect(iced_runtime::Action::Window(
						iced_runtime::window::Action::Close(id),
					));
				}
				if self.ticks > self.cycles * 2 {
					return Task::none();
				}
				let id = Id::unique();
				self.popup = Some(id);
				self.popup_ids.insert(id);
				self.clock.set_popup_id(id);
				log::info!(
					"[DEBUG-surface-probe] create-request id={id:?} parent={parent:?} tick={}",
					self.ticks
				);
				return Task::done(Message::NewPopUp {
					id,
					settings: IcedNewPopupSettings::new(
						parent,
						PixelSize::px(480, 640),
						(800, 0),
						PixelSize::px(100, 60),
					)
					.anchor(PopupAnchor::Bottom)
					.gravity(PopupGravity::Bottom)
					.constraint_adjustment(
						PopupConstraintAdjustment::SlideX
							| PopupConstraintAdjustment::SlideY
							| PopupConstraintAdjustment::FlipY,
					),
				});
			}
			_ => {}
		}
		Task::none()
	}

	fn view(&self, id: Id) -> Element<'_, Message> {
		if !self.popup_ids.contains(&id) {
			container(text("Isolated popup surface stress probe")).into()
		} else {
			if !self.simple {
				if !self.animate {
					return self.clock.view_popup().map(Message::Clock);
				}
				return neo_widgets::widgets::grow(
					self.clock.view_popup().map(Message::Clock),
					Duration::from_millis(50),
					iced::animation::Easing::EaseOutQuart,
					neo_widgets::widgets::GrowFrom::TopCenter,
				)
				.into();
			}
			// Exercise text layout and redraws while popup geometry stays fixed.
			container(column![
				text("Clock popup surface probe").size(32),
				text(format!("tick {}", self.ticks)),
				text("November 2026\nMo Tu We Th Fr Sa Su\n 2  3  4  5  6  7  8")
			])
			.padding(20)
			.into()
		}
	}
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	if std::env::var_os("SUBNIRI_ISOLATED_SURFACE_PROBE").is_none() {
		return Err("use scripts/popup-surface-stress.py to isolate the compositor".into());
	}
	log::init!("surface_stress")?;
	daemon(
		Probe::new,
		|| "subniri-surface-probe".into(),
		Probe::update,
		Probe::view,
	)
	.theme(neo_widgets::style::neo_theme())
	.subscription(Probe::subscription)
	.layer_settings(LayerShellSettings {
		size: LayerSize::fill_width(60),
		exclusive_zone: 0,
		anchor: Anchor::Top | Anchor::Left | Anchor::Right,
		start_mode: if std::env::var_os("SUBNIRI_STRESS_ONE_OUTPUT").is_some() {
			StartMode::Active
		} else {
			StartMode::AllScreens
		},
		keyboard_interactivity: KeyboardInteractivity::None,
		..Default::default()
	})
	.run()?;
	Ok(())
}
