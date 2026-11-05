//! Real-renderer, real-Wayland test of the production popup sizing utilities.
//! Launch only with scripts/test-popup-resize.py, never against the desktop socket.
use iced::advanced::Renderer as _;
use iced::{
	Element, Length, Rectangle, Size, Subscription, Task,
	advanced::{Layout, Widget, layout, mouse, renderer, widget::Tree},
	widget::{container, text},
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
use neo_widgets::{
	animation::geometry::PopupResize,
	style::COLORS,
	widgets::{neo_card, popup_surface},
};
use serde_json::{Value, json};
use std::{
	path::PathBuf,
	time::{Duration, Instant},
};

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
enum Message {
	Shell(iced_wayland_subscriber::shell::ShellEvent),
	Window(Id, window::Event),
	Frame(Instant),
	Stage(u8),
	Exit,
	Watchdog,
}

struct Probe {
	shell_events: iced_wayland_subscriber::shell::ShellReceiver,
	parent: Option<Id>,
	popup: Option<Id>,
	resize: PopupResize,
	started: Instant,
	now: Instant,
	stage: u8,
	app_scale: f32,
	screen: Size,
	bottom_right: bool,
	started_sequence: bool,
}

fn emit(mut value: Value, start: Instant) {
	value["ms"] = json!(start.elapsed().as_secs_f64() * 1000.0);
	value["unix_ns"] = json!(
		std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_nanos()
	);
	eprintln!("PROBE {value}");
}
fn later(message: Message, milliseconds: u64) -> Task<Message> {
	Task::future(async move {
		tokio::time::sleep(Duration::from_millis(milliseconds)).await;
		message
	})
}

impl Probe {
	fn new(shell_events: iced_wayland_subscriber::shell::ShellReceiver) -> (Self, Task<Message>) {
		let started = Instant::now();
		let number = |key: &str, fallback| {
			std::env::var(key)
				.ok()
				.and_then(|value| value.parse().ok())
				.unwrap_or(fallback)
		};
		let app_scale = number("PROBE_APP_SCALE", 1.0);
		let screen = Size::new(
			number("PROBE_SCREEN_WIDTH", 1024.0),
			number("PROBE_SCREEN_HEIGHT", 640.0),
		);
		let mut probe = Self {
			shell_events,
			parent: None,
			popup: None,
			resize: PopupResize::new(Size::ZERO, started),
			started,
			now: started,
			stage: 0,
			app_scale,
			screen,
			bottom_right: std::env::var("PROBE_EDGE").as_deref() == Ok("bottom-right"),
			started_sequence: false,
		};
		probe.resize = PopupResize::new(probe.dimensions(0), started);
		(probe, later(Message::Watchdog, 15000))
	}
	fn dimensions(&self, stage: u8) -> Size {
		let (width, height) = match stage {
			1 => (0.70, 0.60),
			3 => (0.88, 0.76),
			4 => (0.44, 0.36),
			5 => (0.78, 0.64),
			_ => (0.30, 0.25),
		};
		Size::new(
			(self.screen.width / self.app_scale * width).floor(),
			((self.screen.height - 60.0) / self.app_scale * height).floor(),
		)
	}
	fn snapshot(&self, kind: &str) -> Value {
		json!({"kind":kind,"stage":self.stage,"target":[self.resize.target().width,self.resize.target().height],"visual":[self.resize.frame_visual(self.now).width,self.resize.frame_visual(self.now).height],"envelope":[self.resize.envelope.width,self.resize.envelope.height],"waiting":self.resize.waiting(),"app_scale":self.app_scale})
	}
	fn subscription(&self) -> Subscription<Message> {
		let mut subscriptions =
			vec![window::events().map(|(id, event)| Message::Window(id, event))];
		if self.resize.awaiting_presentation() {
			subscriptions.push(self.shell_events.listen().filter_map(|event| {
				matches!(
					event,
					iced_wayland_subscriber::shell::ShellEvent::Presented { .. }
				)
				.then_some(Message::Shell(event))
			}));
		}
		if self.popup.is_some() && self.resize.needs_frame() {
			subscriptions.push(window::frames().map(Message::Frame));
		}
		Subscription::batch(subscriptions)
	}
	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Shell(iced_wayland_subscriber::shell::ShellEvent::Presented {
				window,
				content_updated_at,
			}) if Some(window) == self.popup => {
				let before = self.resize.envelope;
				self.resize.presented(content_updated_at);
				emit(
					self.snapshot(if before != self.resize.envelope {
						"release"
					} else {
						"presented"
					}),
					self.started,
				);
			}
			Message::Window(id, window::Event::Opened { size, .. }) if self.parent.is_none() => {
				self.parent = Some(id);
				let popup = Id::unique();
				self.popup = Some(popup);
				let initial = self.resize.envelope;
				emit(
					json!({"kind":"start","screen":[self.screen.width,self.screen.height],"parent":[size.width,size.height],"edge":if self.bottom_right { "bottom-right" } else { "top-left" },"app_scale":self.app_scale}),
					self.started,
				);
				return Task::done(Message::NewPopUp {
					id: popup,
					settings: IcedNewPopupSettings::new(
						id,
						PixelSize::px(
							(initial.width * self.app_scale).round() as u32,
							(initial.height * self.app_scale).round() as u32,
						),
						(
							if self.bottom_right {
								size.width as i32 - 8
							} else {
								8
							},
							56,
						),
						PixelSize::px(1, 1),
					)
					.anchor(if self.bottom_right {
						PopupAnchor::BottomRight
					} else {
						PopupAnchor::BottomLeft
					})
					.gravity(if self.bottom_right {
						PopupGravity::BottomRight
					} else {
						PopupGravity::BottomLeft
					})
					.constraint_adjustment(
						PopupConstraintAdjustment::SlideX
							| PopupConstraintAdjustment::SlideY
							| PopupConstraintAdjustment::FlipX
							| PopupConstraintAdjustment::FlipY,
					),
				});
			}
			Message::Window(
				id,
				window::Event::Opened { size, .. } | window::Event::Resized(size),
			) if Some(id) == self.popup => {
				self.now = Instant::now();
				self.resize.configured(
					Size::new(size.width / self.app_scale, size.height / self.app_scale),
					self.now,
				);
				let mut record = self.snapshot("configure");
				record["raw"] = json!([size.width, size.height]);
				emit(record, self.started);
				if !self.started_sequence {
					self.started_sequence = true;
					return later(Message::Stage(1), 350);
				}
			}
			Message::Frame(now) => {
				self.now = now;
				let before = self.resize.envelope;
				self.resize.tick(now);
				emit(
					self.snapshot(if before != self.resize.envelope {
						"release"
					} else {
						"frame"
					}),
					self.started,
				);
			}
			Message::Stage(stage) => {
				// Stable stages must settle without another timed application message
				// supplying missing redraws. Stages 3/4 deliberately interrupt motion.
				if matches!(stage, 2 | 3 | 6 | 7) && !self.resize.is_settled() {
					emit(self.snapshot("FAIL_unsettled"), self.started);
					std::process::exit(2);
				}
				if stage == 7 {
					emit(self.snapshot("done"), self.started);
					return later(Message::Exit, 1200);
				}
				self.stage = stage;
				self.now = Instant::now();
				let before = self.resize.frame_visual(self.now);
				self.resize.request(self.dimensions(stage), self.now);
				let mut record = self.snapshot("request");
				record["before"] = json!([before.width, before.height]);
				emit(record, self.started);
				if stage == 3 {
					// Reverse in the same model update: no configure can arrive
					// between these requests, so pending-growth reversal is guaranteed.
					return self.update(Message::Stage(4));
				}
				return later(
					Message::Stage(stage + 1),
					match stage {
						4 => 55,
						_ => 700,
					},
				);
			}
			Message::Exit => return iced::exit(),
			Message::Watchdog => {
				emit(self.snapshot("FAIL_watchdog"), self.started);
				std::process::exit(3);
			}
			_ => {}
		}
		Task::none()
	}
	fn view(&self, id: Id) -> Element<'_, Message> {
		if Some(id) != self.popup {
			return container(text("Isolated popup resize probe")).into();
		}
		let paint = Paint {
			record: self.snapshot("draw"),
			started: self.started,
		};
		popup_surface(
			neo_card(Element::new(paint))
				.width(Length::Fill)
				.height(Length::Fill)
				.background(COLORS.decorative.blue),
			self.resize.envelope,
			self.resize.frame_visual(self.now),
		)
	}
}

struct Paint {
	record: Value,
	started: Instant,
}
impl Widget<Message, iced::Theme, iced::Renderer> for Paint {
	fn size(&self) -> Size<Length> {
		Size::new(Length::Fill, Length::Fill)
	}
	fn layout(
		&mut self, _: &mut Tree, _: &iced::Renderer, limits: &layout::Limits,
	) -> layout::Node {
		layout::Node::new(limits.max())
	}
	fn draw(
		&self, _: &Tree, renderer: &mut iced::Renderer, _: &iced::Theme, _: &renderer::Style,
		layout: Layout<'_>, _: mouse::Cursor, viewport: &Rectangle,
	) {
		let bounds = layout.bounds();
		let mut record = self.record.clone();
		record["bounds"] = json!([bounds.x, bounds.y, bounds.width, bounds.height]);
		record["clip"] = json!([viewport.x, viewport.y, viewport.width, viewport.height]);
		emit(record, self.started);
		for fraction in [0.0, 0.33, 0.66] {
			let rect = Rectangle::new(
				iced::Point::new(bounds.x, bounds.y + bounds.height * fraction),
				Size::new(bounds.width, (bounds.height / 3.0 - 4.0).max(1.0)),
			);
			renderer.fill_quad(
				renderer::Quad {
					bounds: rect,
					border: iced::Border {
						color: COLORS.black,
						width: 2.0,
						radius: 3.0.into(),
					},
					..Default::default()
				},
				COLORS.decorative.green90,
			);
		}
	}
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let runtime = PathBuf::from(std::env::var("XDG_RUNTIME_DIR")?);
	if std::env::var_os("SUBNIRI_ISOLATED_RESIZE_PROBE").as_deref() != Some(runtime.as_os_str())
		|| std::fs::read_to_string(runtime.join("resize-probe.marker"))?
			!= "isolated popup resize probe"
		|| !runtime.starts_with("/tmp/opencode")
	{
		return Err("launch with scripts/test-popup-resize.py on its private compositor".into());
	}
	let bottom = std::env::var("PROBE_EDGE").as_deref() == Ok("bottom-right");
	let (shell_broadcast, shell_events) = iced_wayland_subscriber::shell::channel();
	daemon(
		move || Probe::new(shell_events.clone()),
		|| "subniri-isolated-resize-probe".into(),
		Probe::update,
		Probe::view,
	)
	.style(|_: &Probe, _: &iced::Theme| iced::theme::Style {
		background_color: iced::Color::TRANSPARENT,
		text_color: COLORS.text,
	})
	.theme(neo_widgets::style::neo_theme())
	.subscription(Probe::subscription)
	.scale_factor(|probe: &Probe, _| probe.app_scale)
	.settings(iced_exwlshell::Settings {
		shell_broadcast,
		..Default::default()
	})
	.layer_settings(LayerShellSettings {
		size: LayerSize::fill_width(60),
		exclusive_zone: 0,
		anchor: if bottom { Anchor::Bottom } else { Anchor::Top } | Anchor::Left | Anchor::Right,
		start_mode: StartMode::Active,
		keyboard_interactivity: KeyboardInteractivity::None,
		..Default::default()
	})
	.run()?;
	Ok(())
}
