//! Production tile rendering and input verification. No device tasks/subscriptions.
use iced::{
	Element, Event, Length, Point, Rectangle, Size, Subscription, Task, Vector,
	advanced::{
		Layout, Shell, Widget, layout, mouse, renderer,
		widget::{Tree, tree},
	},
	widget::{column, container, row, text},
	window::{self, Id},
};
use iced_exwlshell::{
	daemon,
	reexport::{Anchor, KeyboardInteractivity, LayerSize},
	settings::{LayerShellSettings, StartMode},
	to_layer_message,
};
use neo_widgets::{phosphor_icon, style::COLORS, widgets::neo_tile};
use serde_json::json;
use std::time::{Duration, Instant};

#[allow(dead_code)]
#[path = "../src/modules/mod.rs"]
mod modules;

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
enum Message {
	Ready,
	Presented(Instant),
	Audit,
	Exit,
	Tile(modules::ModuleMessage),
}
struct Gallery {
	on: modules::Module,
	off: modules::Module,
	started: Instant,
	audit: bool,
	ready_at: Option<Instant>,
	announced: bool,
	shell_events: iced_wayland_subscriber::shell::ShellReceiver,
}
fn later(message: Message, ms: u64) -> Task<Message> {
	Task::future(async move {
		tokio::time::sleep(Duration::from_millis(ms)).await;
		message
	})
}
impl Gallery {
	fn new(shell_events: iced_wayland_subscriber::shell::ShellReceiver) -> (Self, Task<Message>) {
		let mut config = config::ConfigFile::default();
		config.nightlight.enabled = true;
		let on = modules::Module::system_menu(&config);
		config.nightlight.enabled = false;
		let off = modules::Module::system_menu(&config);
		(
			Self {
				on,
				off,
				started: Instant::now(),
				audit: false,
				ready_at: None,
				announced: false,
				shell_events,
			},
			Task::batch([later(Message::Ready, 600), later(Message::Exit, 15000)]),
		)
	}
	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Ready => self.ready_at = Some(Instant::now()),
			Message::Presented(stamp)
				if !self.announced && self.ready_at.is_some_and(|ready| stamp >= ready) =>
			{
				self.announced = true;
				eprintln!("GALLERY_READY");
				return Task::batch([later(Message::Audit, 700), later(Message::Exit, 1600)]);
			}
			Message::Audit => self.audit = true,
			Message::Exit => return iced::exit(),
			Message::Tile(message) => eprintln!("GALLERY_UNEXPECTED {message:?}"),
			_ => {}
		}
		Task::none()
	}
	fn tile<'a>(
		&'a self, name: &'static str, width: u32, height: u32, enabled: bool,
	) -> Element<'a, Message> {
		let size = Size::new(width as f32 * 72.0 - 8.0, height as f32 * 72.0 - 8.0);
		let modules::Module::SystemMenu(menu) = (if enabled { &self.on } else { &self.off }) else {
			unreachable!()
		};
		let child = menu
			.view_widget(&config::SystemMenuWidgets::Nightlight, size)
			.map(modules::ModuleMessage::SystemMenu);
		let audited = Element::new(AuditTile {
			child,
			name,
			size,
			enabled,
			expanded: width >= 4 && height >= 2,
			audit: self.audit,
			started: self.started,
		});
		column![text(name).size(14), audited.map(Message::Tile)]
			.spacing(6)
			.into()
	}
	fn view(&self, _: Id) -> Element<'_, Message> {
		let action_size = Size::new(136.0, 136.0);
		let action = neo_tile(
			phosphor_icon!("play"),
			"Run script",
			"No artificial switch",
			None,
			Some(COLORS.decorative.pink),
			action_size,
		)
		.on_press(modules::ModuleMessage::ClosePopup);
		let action = Element::new(AuditTile {
			child: action.into(),
			name: "Action 2×2",
			size: action_size,
			enabled: true,
			expanded: false,
			audit: self.audit,
			started: self.started,
		});
		let long_size = Size::new(280.0, 64.0);
		let long = neo_tile(
			phosphor_icon!("house"),
			"An intentionally very long entity title which must not cover its switch",
			"A long subtitle must also stop before the switch",
			Some(true),
			Some(COLORS.decorative.green),
			long_size,
		)
		.on_press(modules::ModuleMessage::ClosePopup);
		let long = Element::new(AuditTile {
			child: long.into(),
			name: "Long title",
			size: long_size,
			enabled: true,
			expanded: false,
			audit: self.audit,
			started: self.started,
		});
		container(
			column![
				text("Production responsive quick tiles — no backend connections").size(20),
				row![
					self.tile("Square 2×2", 2, 2, true),
					self.tile("Portrait 2×3", 2, 3, true),
					self.tile("Default 2×1", 2, 1, true),
					self.tile("Landscape 3×1", 3, 1, true),
					self.tile("Wide 4×1", 4, 1, true),
					column![text("Action 2×2").size(14), action.map(Message::Tile)].spacing(6)
				]
				.spacing(12),
				row![
					self.tile("Nightlight on 4×2", 4, 2, true),
					self.tile("Nightlight off 4×2", 4, 2, false),
					self.tile("Borderline 3×2", 3, 2, true)
				]
				.spacing(12),
				row![
					self.tile("Nightlight on 6×3", 6, 3, true),
					self.tile("Nightlight off 6×3", 6, 3, false),
					column![text("Long title 4×1").size(14), long.map(Message::Tile)].spacing(6),
					self.tile("1×1", 1, 1, true)
				]
				.spacing(12)
			]
			.spacing(16),
		)
		.padding(12)
		.width(Length::Fill)
		.height(Length::Fill)
		.style(|_| container::Style {
			background: Some(COLORS.decorative.blue.into()),
			..Default::default()
		})
		.into()
	}
}

struct AuditTile<'a> {
	child: Element<'a, modules::ModuleMessage>,
	name: &'static str,
	size: Size,
	enabled: bool,
	expanded: bool,
	audit: bool,
	started: Instant,
}
#[derive(Default)]
struct AuditState {
	checked: bool,
}
type Paragraph = <iced::Renderer as iced::advanced::text::Renderer>::Paragraph;
fn paragraphs(tree: &Tree, output: &mut Vec<serde_json::Value>) {
	use iced::advanced::text::paragraph::{Paragraph as _, Plain};
	if tree.tag == tree::Tag::of::<Plain<Paragraph>>() {
		let p = tree.state.downcast_ref::<Plain<Paragraph>>();
		output.push(json!({"text":p.content(),"font_px":p.raw().size().0,"paragraph_bounds":[p.raw().bounds().width,p.raw().bounds().height],"min_bounds":[p.raw().min_bounds().width,p.raw().min_bounds().height],"ellipsis":format!("{:?}",p.raw().ellipsis())}));
	}
	for child in &tree.children {
		paragraphs(child, output);
	}
}
#[derive(Default)]
struct Labels(Vec<(String, Rectangle)>);
impl iced::advanced::widget::Operation for Labels {
	fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation)) {
		operate(self);
	}
	fn text(&mut self, _: Option<&iced::advanced::widget::Id>, bounds: Rectangle, content: &str) {
		self.0.push((content.to_owned(), bounds));
	}
}
impl Widget<modules::ModuleMessage, iced::Theme, iced::Renderer> for AuditTile<'_> {
	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<AuditState>()
	}
	fn state(&self) -> tree::State {
		tree::State::new(AuditState::default())
	}
	fn diff(&mut self, tree: &mut Tree) {
		tree.diff_children(std::slice::from_mut(&mut self.child));
	}
	fn size(&self) -> Size<Length> {
		Size::new(
			Length::Fixed(self.size.width),
			Length::Fixed(self.size.height),
		)
	}
	fn layout(
		&mut self, tree: &mut Tree, renderer: &iced::Renderer, _: &layout::Limits,
	) -> layout::Node {
		layout::Node::with_children(
			self.size,
			vec![self.child.as_widget_mut().layout(
				&mut tree.children[0],
				renderer,
				&layout::Limits::new(self.size, self.size),
			)],
		)
	}
	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &iced::Renderer, shell: &mut Shell<'_, modules::ModuleMessage>,
		viewport: &Rectangle,
	) {
		let child_layout = layout.children().next().expect("tile layout");
		self.child.as_widget_mut().update(
			&mut tree.children[0],
			event,
			child_layout,
			cursor,
			renderer,
			shell,
			viewport,
		);
		if self.audit
			&& !tree.state.downcast_ref::<AuditState>().checked
			&& matches!(event, Event::Window(window::Event::RedrawRequested(_)))
		{
			tree.state.downcast_mut::<AuditState>().checked = true;
			let mut texts = Vec::new();
			paragraphs(&tree.children[0], &mut texts);
			eprintln!(
				"GALLERY {}",
				json!({"kind":"metrics","name":self.name,"size":[self.size.width,self.size.height],"texts":texts,"elapsed_ms":self.started.elapsed().as_millis()})
			);
			let mut labels = Labels::default();
			self.child.as_widget_mut().operate(
				&mut tree.children[0],
				child_layout,
				renderer,
				&mut labels,
			);
			let mut checks = vec![(
				"header",
				child_layout.position() + Vector::new(28.0, 28.0),
				mouse::Button::Left,
			)];
			if self.expanded {
				for label in ["Temperature", "Brightness"] {
					let bounds = labels
						.0
						.iter()
						.find(|(text, _)| {
							text == if label == "Temperature" {
								"6500 K"
							} else {
								"100 %"
							}
						})
						.expect("production slider label")
						.1;
					checks.push((
						label,
						Point::new(bounds.x + 30.0, bounds.y + bounds.height + 12.0),
						mouse::Button::Left,
					));
				}
				checks.push((
					"body",
					child_layout.position() + Vector::new(self.size.width / 2.0, 43.0),
					mouse::Button::Left,
				));
			}
			checks.push((
				"context",
				child_layout.position() + Vector::new(28.0, 28.0),
				mouse::Button::Right,
			));
			for (name, point, button) in checks {
				let mut bus = iced::advanced::shell::Bus::new();
				let drag = matches!(name, "Temperature" | "Brightness");
				let end = if drag {
					point + Vector::new(20.0, 0.0)
				} else {
					point
				};
				let mut events = vec![(Event::Mouse(mouse::Event::ButtonPressed(button)), point)];
				if drag {
					events.push((
						Event::Mouse(mouse::Event::CursorMoved { position: end }),
						end,
					));
				}
				events.push((Event::Mouse(mouse::Event::ButtonReleased(button)), end));
				for (event, position) in events {
					let mut test_shell = Shell::new(
						&window::Headless,
						iced::advanced::shell::Waker::noop(),
						&mut bus,
					);
					self.child.as_widget_mut().update(
						&mut tree.children[0],
						&event,
						child_layout,
						mouse::Cursor::Available(position),
						renderer,
						&mut test_shell,
						viewport,
					);
				}
				let messages = bus
					.into_iter()
					.map(|message| format!("{message:?}"))
					.collect::<Vec<_>>();
				eprintln!(
					"GALLERY {}",
					json!({"kind":"input","name":self.name,"control":name,"enabled":self.enabled,"drag":drag,"messages":messages})
				);
			}
		}
	}
	fn draw(
		&self, tree: &Tree, renderer: &mut iced::Renderer, theme: &iced::Theme,
		style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		self.child.as_widget().draw(
			&tree.children[0],
			renderer,
			theme,
			style,
			layout.children().next().expect("tile layout"),
			cursor,
			viewport,
		);
	}
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
	let runtime = std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR")?);
	if !runtime.starts_with("/tmp/opencode")
		|| std::fs::read_to_string(runtime.join("tile-gallery.marker"))?
			!= "isolated production tile gallery"
	{
		return Err("launch with scripts/test-tile-gallery.py".into());
	}
	let (shell_broadcast, shell_events) = iced_wayland_subscriber::shell::channel();
	daemon(
		move || Gallery::new(shell_events.clone()),
		|| "subniri-tile-gallery".into(),
		Gallery::update,
		Gallery::view,
	)
	.theme(neo_widgets::style::neo_theme())
	.settings(iced_exwlshell::Settings {
		shell_broadcast,
		..Default::default()
	})
	.subscription(|gallery: &Gallery| {
		if gallery.announced {
			Subscription::none()
		} else {
			gallery.shell_events.listen().filter_map(|event| {
				if let iced_wayland_subscriber::shell::ShellEvent::Presented {
					content_updated_at,
					..
				} = event
				{
					Some(Message::Presented(content_updated_at))
				} else {
					None
				}
			})
		}
	})
	.layer_settings(LayerShellSettings {
		size: LayerSize::FILL,
		anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
		start_mode: StartMode::Active,
		keyboard_interactivity: KeyboardInteractivity::None,
		exclusive_zone: 0,
		..Default::default()
	})
	.run()?;
	Ok(())
}
