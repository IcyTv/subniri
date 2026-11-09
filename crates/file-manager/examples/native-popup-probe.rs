//! Phase-1 gate: normal application parent, shared ancestry, native input grabs.
use iced::{
	Element, Subscription, Task, Theme, event, keyboard,
	widget::{button, column, container, mouse_area, text, text_input},
	window,
};
use iced_exwlshell::{
	actions::{IcedNewPopupSettings, IcedXdgWindowSettings},
	reexport::PixelSize,
	settings::{LayerShellSettings, StartMode},
	to_exwlshell_message,
};
use neo_widgets::popup::{self, Effect, Event, Popups};

#[to_exwlshell_message]
#[derive(Debug, Clone)]
enum Message {
	Open(window::Id, iced::Point),
	Child(window::Id),
	Input(String),
	Event(window::Id, iced::Event),
	Lifecycle(Event<()>),
	ParentAction,
}

struct Probe {
	host: window::Id,
	popups: Popups<()>,
	query: String,
	clicks: usize,
	host_size: iced::Size,
}

fn close(id: window::Id) -> Task<Message> {
	iced_runtime::task::effect(iced_runtime::Action::Window(
		iced_runtime::window::Action::Close(id),
	))
}

impl Probe {
	fn new() -> (Self, Task<Message>) {
		let host = window::Id::unique();
		(
			Self {
				host,
				popups: Popups::default(),
				query: String::new(),
				clicks: 0,
				host_size: iced::Size::new(700.0, 600.0),
			},
			Task::done(Message::NewBaseWindow {
				id: host,
				settings: IcedXdgWindowSettings {
					parent: None,
					size: Some(PixelSize::px(700, 600)),
					client_side_decorations: true,
				},
			}),
		)
	}
	fn lifecycle(&mut self, event: Event<()>) -> Task<Message> {
		let change = self.popups.update(event);
		change
			.effects
			.into_iter()
			.fold(Task::none(), |task, e| match e {
				Effect::Close(id) => task.chain(close(id)),
				Effect::NotifyClosed(()) => {
					println!("PROBE dismissed");
					task
				}
			})
	}
	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Open(parent, position) => {
				let id = window::Id::unique();
				let closing = self.lifecycle(Event::Open {
					id,
					parent: Some(parent),
					owner: (),
				});
				self.query.clear();
				println!("PROBE root parent={parent:?} position={position:?}");
				closing
					.chain(Task::done(Message::NewPopUp {
						id,
						settings: IcedNewPopupSettings::at_position(
							parent,
							PixelSize::px(300, 220),
							(position.x as i32, position.y as i32),
						),
					}))
					.chain(iced::widget::operation::focus("probe-search"))
			}
			Message::Child(parent) => {
				let id = window::Id::unique();
				let change = self.popups.update(Event::Context {
					id,
					parent,
					owner: (),
				});
				if change.opened.is_none() {
					return Task::none();
				}
				let closing = change
					.effects
					.into_iter()
					.fold(Task::none(), |t, e| match e {
						Effect::Close(id) => t.chain(close(id)),
						_ => t,
					});
				println!("PROBE child");
				closing.chain(Task::done(Message::NewPopUp {
					id,
					settings: IcedNewPopupSettings::at_position(
						parent,
						PixelSize::px(220, 120),
						(290, 90),
					),
				}))
			}
			Message::Input(value) => {
				println!("PROBE input={value}");
				self.query = value;
				Task::none()
			}
			Message::ParentAction => {
				self.clicks += 1;
				println!("PROBE parent-click={}", self.clicks);
				Task::none()
			}
			Message::Lifecycle(event) => self.lifecycle(event),
			Message::Event(id, event) => {
				if matches!(
					event,
					iced::Event::Keyboard(_) | iced::Event::Window(window::Event::Focused)
				) {
					println!("PROBE event {id:?} {event:?}");
				}
				match event {
					iced::Event::Window(
						window::Event::Opened { size, .. } | window::Event::Resized(size),
					) if id == self.host => {
						self.host_size = size;
						Task::none()
					}
					iced::Event::Window(window::Event::Opened { .. }) if id != self.host => {
						iced::widget::operation::focus("probe-search")
					}
					iced::Event::Window(window::Event::Closed) if id == self.host => iced::exit(),
					iced::Event::Window(window::Event::Closed) => self.lifecycle(Event::Closed(id)),
					iced::Event::Window(window::Event::CloseRequested) => {
						if id == self.host {
							close(id)
						} else {
							self.lifecycle(Event::CloseAll)
						}
					}
					iced::Event::Keyboard(keyboard::Event::KeyPressed {
						key, modifiers, ..
					}) => match key.as_ref() {
						keyboard::Key::Named(keyboard::key::Named::Escape) => {
							self.lifecycle(Event::CloseAll)
						}
						keyboard::Key::Named(keyboard::key::Named::F10)
							if modifiers.shift() && id == self.host =>
						{
							self.update(Message::Open(
								id,
								iced::Point::new(
									self.host_size.width - 24.0,
									self.host_size.height - 24.0,
								),
							))
						}
						_ => Task::none(),
					},
					_ => Task::none(),
				}
			}
			_ => Task::none(),
		}
	}
	fn view(&self, id: window::Id) -> Element<'_, Message> {
		if id == self.host {
			return mouse_area(
				container(
					column![
						text("Native popup probe — right-click or Shift+F10"),
						button("Parent action").on_press(Message::ParentAction),
						text(format!("Parent clicks: {}", self.clicks))
					]
					.spacing(20),
				)
				.padding(20)
				.width(iced::Fill)
				.height(iced::Fill),
			)
			.on_right_press(Message::Open(id, iced::Point::new(200.0, 120.0)))
			.into();
		}
		container(
			column![
				text("Native context menu"),
				text_input("Search", &self.query)
					.id("probe-search")
					.on_input(Message::Input),
				button("Submenu").on_press(Message::Child(id)),
				text("Escape closes the entire menu")
			]
			.spacing(12),
		)
		.padding(12)
		.width(300)
		.height(220)
		.style(iced::widget::container::bordered_box)
		.into()
	}
	fn subscription(&self) -> Subscription<Message> {
		Subscription::batch([
			event::listen_with(|event, _, id| {
				(!matches!(
					event,
					iced::Event::Window(window::Event::RedrawRequested(_))
				))
				.then_some(Message::Event(id, event))
			}),
			popup::parent_clicks(self.popups.context_children()).map(Message::Lifecycle),
		])
	}
}

fn main() -> iced_exwlshell::Result {
	iced_exwlshell::daemon(
		Probe::new,
		|| "fileberg-popup-probe".to_owned(),
		Probe::update,
		Probe::view,
	)
	.theme(Theme::Light)
	.subscription(Probe::subscription)
	.layer_settings(LayerShellSettings {
		start_mode: StartMode::Background,
		..Default::default()
	})
	.run()
}
