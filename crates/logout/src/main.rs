use clap::{Parser, Subcommand};
use futures::StreamExt;
use iced::{
	Alignment::Center,
	Background, Border, Color, Element, Font, Length, Padding, Subscription, Task, Theme, keyboard,
	mouse,
	theme::Style,
	widget::{Column, column, container, mouse_area, row, space, svg, text},
};
use iced_exwlshell::{
	layershell::application,
	reexport::{Anchor, KeyboardInteractivity, Layer, LayerSize, core::font},
	settings::{LayerShellSettings, StartMode},
	to_layer_message,
};
use neo_widgets::{
	appearance::{Appearance, Event as AppearanceEvent},
	phosphor_icon,
	style::Colors,
	widgets::{NeoButton, neo_button, neo_card},
};
use zbus::Connection;

fn shortcut_entry(character: &str) -> Option<usize> {
	match character.as_bytes() {
		[digit @ b'1'..=b'6'] => Some(usize::from(*digit - b'1')),
		_ => None,
	}
}

#[cfg(test)]
mod shortcut_tests {
	use super::*;

	#[test]
	fn actions_are_neutral_until_hover_or_keyboard_navigation() {
		let (mut logout, _) = Logout::new(Args::parse_from(["iceout"]));
		assert!(!logout.action_highlighted(0));

		let _ = logout.update(Message::HoverEntry(Some(0)));
		assert!(logout.action_highlighted(0));
		let _ = logout.update(Message::HoverEntry(None));
		assert!(!logout.action_highlighted(0));

		let _ = logout.update(Message::CycleEntry(false));
		assert!(logout.action_highlighted(logout.selected_entry));
		let _ = logout.update(Message::HoverEntry(Some(0)));
		assert!(logout.action_highlighted(0));
		assert!(!logout.keyboard_focus);
	}

	#[test]
	fn character_payload_must_be_exactly_one_shortcut_digit() {
		for invalid in ["", "1x", "1234", "1\u{301}", "0", "7", "12", "１"] {
			assert_eq!(super::shortcut_entry(invalid), None, "{invalid:?}");
		}
		for (digit, index) in [("1", 0), ("2", 1), ("3", 2), ("4", 3), ("5", 4), ("6", 5)] {
			assert_eq!(super::shortcut_entry(digit), Some(index));
		}
	}
}

#[derive(Debug, Clone, Parser)]
#[command(version, about)]
struct Args {
	#[command(subcommand)]
	command: Option<Command>,
}

#[derive(Debug, Clone, Copy, Default, Subcommand)]
enum Command {
	Session,
	Power,
	#[default]
	All,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	log::init!("logout", "iceout")?;

	let args = Args::parse();

	let app = application(
		move || Logout::new(args.clone()),
		Logout::namespace,
		Logout::update,
		Logout::view,
	)
	.theme(|logout: &Logout| logout.appearance.colors().iced_theme())
	.style(Logout::style)
	.subscription(Logout::subscription)
	.layer_settings(LayerShellSettings {
		size: LayerSize::FILL,
		anchor: Anchor::all(),
		start_mode: StartMode::Active,
		layer: Layer::Overlay,
		keyboard_interactivity: KeyboardInteractivity::Exclusive,
		..Default::default()
	});

	Ok(app.run()?)
}

#[to_layer_message]
#[derive(Debug, Clone)]
enum Message {
	Appearance(AppearanceEvent),
	Exit,
	PowerCapabilitiesLoaded(PowerCapabilities),
	ActionFinished(ActionResult),
	CycleEntry(bool),
	SelectEntry(usize),
	ActivateCurrentEntry,
	SelectAndActivate(usize),
	HoverEntry(Option<usize>),
	Noop,

	IcedEvent(iced::event::Event),
}

struct Logout {
	appearance: Appearance,
	args: Args,
	selected_entry: usize,
	keyboard_focus: bool,
	hovered_entry: Option<usize>,
	power_capabilities: PowerCapabilities,
	state: LogoutState,
}

#[derive(Debug, Clone, Copy)]
enum LogoutState {
	Choosing,
	Transition { label: &'static str },
}

#[derive(Debug, Clone, Copy)]
struct ActionResult {
	exit: bool,
	failed_label: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
enum LogoutAction {
	Lock,
	SignOut,
	PowerOff,
	Reboot,
	Hibernate,
	Suspend,
}

impl LogoutAction {
	fn label(self) -> &'static str {
		match self {
			Self::Lock => "Locking...",
			Self::SignOut => "Signing out...",
			Self::PowerOff => "Shutting down...",
			Self::Reboot => "Restarting...",
			Self::Hibernate => "Hibernating...",
			Self::Suspend => "Suspending...",
		}
	}

	fn failed_label(self) -> &'static str {
		match self {
			Self::Lock => "lock",
			Self::SignOut => "sign out",
			Self::PowerOff => "power off",
			Self::Reboot => "reboot",
			Self::Hibernate => "hibernate",
			Self::Suspend => "suspend",
		}
	}
}

#[derive(Debug, Clone, Copy, Default)]
struct PowerCapabilities {
	power_off: PowerCapability,
	reboot: PowerCapability,
	hibernate: PowerCapability,
	suspend: PowerCapability,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PowerCapability {
	#[default]
	Disabled,
	Allowed,
	Challenge,
}

impl PowerCapability {
	fn enabled(self) -> bool {
		self != Self::Disabled
	}

	fn interactive(self) -> bool {
		self == Self::Challenge
	}
}

impl Logout {
	fn new(args: Args) -> (Self, Task<Message>) {
		let logout = Logout {
			appearance: Appearance::default(),
			args,
			selected_entry: 0,
			keyboard_focus: false,
			hovered_entry: None,
			power_capabilities: PowerCapabilities::default(),
			state: LogoutState::Choosing,
		};
		(logout, Self::load_power_capabilities())
	}

	fn namespace() -> String {
		String::from("iceout")
	}

	fn subscription(&self) -> Subscription<Message> {
		let keyboard = keyboard::listen().map(|e| match e {
			keyboard::Event::KeyPressed {
				key: keyboard::Key::Named(keyboard::key::Named::Escape),
				..
			} => Message::Exit,
			keyboard::Event::KeyPressed {
				key: keyboard::Key::Named(keyboard::key::Named::Tab),
				modifiers,
				..
			} => Message::CycleEntry(modifiers.shift()),
			keyboard::Event::KeyPressed {
				key: keyboard::Key::Named(keyboard::key::Named::ArrowDown),
				..
			} => Message::CycleEntry(false),
			keyboard::Event::KeyPressed {
				key: keyboard::Key::Named(keyboard::key::Named::ArrowUp),
				..
			} => Message::CycleEntry(true),
			keyboard::Event::KeyPressed {
				key: keyboard::Key::Character(c),
				..
			} => shortcut_entry(&c).map_or(Message::Noop, Message::SelectEntry),
			keyboard::Event::KeyPressed {
				key: keyboard::Key::Named(keyboard::key::Named::Enter),
				..
			} => Message::ActivateCurrentEntry,
			_ => Message::Noop,
		});
		let event = iced::event::listen().map(Message::IcedEvent);

		Subscription::batch([
			keyboard,
			event,
			self.appearance.subscription().map(Message::Appearance),
		])
	}

	#[allow(clippy::needless_pass_by_value)]
	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Appearance(event) => {
				// The layer-shell backend redraws on messages, including appearance frames.
				self.appearance.update(event);
			}
			Message::PowerCapabilitiesLoaded(capabilities) => {
				self.power_capabilities = capabilities;
				if !self.is_entry_enabled(self.selected_entry) {
					self.select_next_entry(false);
				}
			}
			Message::ActionFinished(result) => {
				if let Some(action) = result.failed_label {
					log::error!("Failed to {action}");
					self.state = LogoutState::Choosing;
				} else if result.exit {
					return iced::exit();
				}
			}
			Message::CycleEntry(backwards) => {
				self.keyboard_focus = true;
				self.hovered_entry = None;
				if !matches!(self.state, LogoutState::Choosing) {
					return Task::none();
				}

				self.select_next_entry(backwards);
			}
			Message::SelectEntry(entry) => {
				self.keyboard_focus = true;
				self.hovered_entry = None;
				if !matches!(self.state, LogoutState::Choosing) {
					return Task::none();
				}

				if self.is_entry_enabled(entry) {
					self.selected_entry = entry;
				}
			}
			Message::HoverEntry(entry) => {
				self.hovered_entry = entry;
				self.keyboard_focus = false;
			}
			Message::IcedEvent(iced::event::Event::Window(iced::window::Event::Unfocused))
			| Message::Exit => {
				if matches!(self.state, LogoutState::Choosing) {
					return iced::exit();
				}
			}
			Message::ActivateCurrentEntry => {
				if !matches!(self.state, LogoutState::Choosing) {
					return Task::none();
				}

				if !self.is_entry_enabled(self.selected_entry) {
					return Task::none();
				}

				let action = self.selected_action();
				return self.perform_action(action);
			}
			Message::SelectAndActivate(index) => {
				if !matches!(self.state, LogoutState::Choosing) {
					return Task::none();
				}

				if !self.is_entry_enabled(index) {
					return Task::none();
				}

				self.selected_entry = index;
				return Task::done(Message::ActivateCurrentEntry);
			}
			_ => (),
		}

		Task::none()
	}

	fn load_power_capabilities() -> Task<Message> {
		Task::future(async move {
			let connection = match Connection::system().await {
				Ok(connection) => connection,
				Err(e) => {
					log::error!("Failed to connect to system bus: {e}");
					return Message::PowerCapabilitiesLoaded(PowerCapabilities::default());
				}
			};

			let login_manager = match LoginManagerProxy::builder(&connection).build().await {
				Ok(login_manager) => login_manager,
				Err(e) => {
					log::error!("Failed to connect to login manager: {e}");
					return Message::PowerCapabilitiesLoaded(PowerCapabilities::default());
				}
			};

			let capabilities = PowerCapabilities {
				power_off: power_capability("power off", login_manager.can_power_off().await),
				reboot: power_capability("reboot", login_manager.can_reboot().await),
				hibernate: power_capability("hibernate", login_manager.can_hibernate().await),
				suspend: power_capability("suspend", login_manager.can_suspend().await),
			};

			Message::PowerCapabilitiesLoaded(capabilities)
		})
	}

	fn entry_count(&self) -> usize {
		match self.args.command.unwrap_or_default() {
			Command::Session => 2,
			Command::Power => 4,
			Command::All => 6,
		}
	}

	fn select_next_entry(&mut self, backwards: bool) {
		let count = self.entry_count();
		let add = if backwards { count - 1 } else { 1 };

		for _ in 0..count {
			self.selected_entry = (self.selected_entry + add) % count;
			if self.is_entry_enabled(self.selected_entry) {
				return;
			}
		}
	}

	fn is_entry_enabled(&self, index: usize) -> bool {
		match (self.args.command.unwrap_or_default(), index) {
			(Command::Session | Command::All, 0 | 1) => true,
			(Command::Power, 0) | (Command::All, 2) => self.power_capabilities.power_off.enabled(),
			(Command::Power, 1) | (Command::All, 3) => self.power_capabilities.reboot.enabled(),
			(Command::Power, 2) | (Command::All, 4) => self.power_capabilities.hibernate.enabled(),
			(Command::Power, 3) | (Command::All, 5) => self.power_capabilities.suspend.enabled(),
			_ => false,
		}
	}

	fn selected_action(&self) -> LogoutAction {
		match (self.args.command.unwrap_or_default(), self.selected_entry) {
			(Command::Session | Command::All, 0) => LogoutAction::Lock,
			(Command::Session | Command::All, 1) => LogoutAction::SignOut,
			(Command::Power, 0) | (Command::All, 2) => LogoutAction::PowerOff,
			(Command::Power, 1) | (Command::All, 3) => LogoutAction::Reboot,
			(Command::Power, 2) | (Command::All, 4) => LogoutAction::Hibernate,
			(Command::Power, 3) | (Command::All, 5) => LogoutAction::Suspend,
			_ => unreachable!(),
		}
	}

	fn perform_action(&mut self, action: LogoutAction) -> Task<Message> {
		if matches!(action, LogoutAction::Lock) {
			return self.lock();
		}

		self.state = LogoutState::Transition {
			label: action.label(),
		};

		match action {
			LogoutAction::Lock => unreachable!(),
			LogoutAction::SignOut => self.sign_out(),
			LogoutAction::PowerOff => self.power_off(),
			LogoutAction::Reboot => self.reboot(),
			LogoutAction::Hibernate => self.hibernate(),
			LogoutAction::Suspend => self.suspend(),
		}
	}

	fn view(&self) -> Element<'_, Message> {
		let colors = self.appearance.colors();

		if let LogoutState::Transition { label } = self.state {
			return self.transition(label, colors);
		}

		let content = match self.args.command.unwrap_or_default() {
			Command::Session => self.session(colors),
			Command::Power => self.power(0, colors),
			Command::All => column![self.session(colors), self.power(2, colors),].spacing(12),
		};

		let content = column![text("Choose an action.").color(colors.text), content,].spacing(12);

		let content = container(content).padding(Padding {
			left: 24.0,
			right: 24.0,
			..Default::default()
		});

		let header = match self.args.command.unwrap_or_default() {
			Command::Session => "LOGOUT",
			Command::Power | Command::All => "POWER OFF",
		};

		let header = row![
			text(header).font(Font {
				weight: font::Weight::Bold,
				..Default::default()
			}),
			space::horizontal(),
			container("")
				.height(Length::Fill)
				.width(2)
				.style(|_| container::Style {
					background: Some(Background::Color(Color::BLACK)),
					..Default::default()
				}),
			space().width(8),
			mouse_area(
				svg(phosphor_icon!("x"))
					.width(Length::Shrink)
					.height(24)
					.style(move |_, _| svg::Style {
						color: Some(colors.text)
					})
			)
			.on_press(Message::Exit)
			.interaction(mouse::Interaction::Pointer),
		];

		let header = container(header)
			.padding(12)
			.width(Length::Fill)
			.style(move |_| container::Style {
				background: Some(Background::Color(colors.body)),
				border: Border {
					width: 2.0,
					color: Color::BLACK,
					..Default::default()
				},
				..Default::default()
			});

		let content = neo_card(column![header, content].spacing(18))
			.with_colors(colors)
			.padding(Padding {
				left: 0.0,
				right: 0.0,
				top: 0.0,
				bottom: 20.0,
			})
			.background(colors.surface)
			.width(480);

		let content = mouse_area(content).on_press(Message::Noop);

		mouse_area(
			container(content)
				.style(|_| container::Style {
					background: Some(Background::Color(Color::BLACK.scale_alpha(0.4))),
					..Default::default()
				})
				.padding(0)
				.width(Length::Fill)
				.height(Length::Fill)
				.align_x(Center)
				.align_y(Center),
		)
		.on_press(Message::Exit)
		.into()
	}

	fn session(&self, colors: Colors) -> Column<'_, Message> {
		column![
			self.action(
				phosphor_icon!("lock"),
				"LOCK",
				0,
				colors,
				colors.decorative.yellow
			),
			self.action(
				phosphor_icon!("sign-out"),
				"LOGOUT",
				1,
				colors,
				colors.decorative.green
			),
		]
		.spacing(12)
	}

	fn power(&self, offset: usize, colors: Colors) -> Column<'_, Message> {
		column![
			self.action(
				phosphor_icon!("power"),
				"POWER OFF",
				offset,
				colors,
				colors.feedback.danger
			),
			self.action(
				phosphor_icon!("arrow-counter-clockwise"),
				"REBOOT",
				offset + 1,
				colors,
				colors.feedback.warning
			),
			self.action(
				phosphor_icon!("pause-circle"),
				"HIBERNATE",
				offset + 2,
				colors,
				colors.decorative.yellow
			),
			self.action(
				phosphor_icon!("moon"),
				"SLEEP",
				offset + 3,
				colors,
				colors.decorative.blue
			),
		]
		.spacing(12)
	}

	fn action<'a>(
		&self, icon: svg::Handle, label: &'a str, index: usize, colors: Colors, accent: Color,
	) -> Element<'a, Message> {
		let enabled = self.is_entry_enabled(index);
		let highlighted = self.action_highlighted(index);
		let button = action_button(icon, label, index, colors, highlighted)
			.background(if highlighted { accent } else { colors.surface })
			.enabled(enabled);

		iced::widget::mouse_area(button)
			.on_enter(Message::HoverEntry(Some(index)))
			.on_exit(Message::HoverEntry(None))
			.into()
	}

	fn action_highlighted(&self, index: usize) -> bool {
		self.is_entry_enabled(index)
			&& (self.hovered_entry == Some(index)
				|| (self.keyboard_focus && self.selected_entry == index))
	}

	#[allow(clippy::unused_self)]
	fn transition(&self, label: &'static str, colors: Colors) -> Element<'_, Message> {
		let content = neo_card(
			column![
				text(label).font(Font {
					weight: font::Weight::Bold,
					..Default::default()
				}),
				text("Please wait.").color(colors.text),
			]
			.spacing(12),
		)
		.with_colors(colors)
		.background(colors.surface)
		.width(480);

		container(content)
			.style(|_| container::Style {
				background: Some(Background::Color(Color::BLACK.scale_alpha(0.4))),
				..Default::default()
			})
			.padding(0)
			.width(Length::Fill)
			.height(Length::Fill)
			.align_x(Center)
			.align_y(Center)
			.into()
	}

	fn style(&self, _theme: &Theme) -> Style {
		Style {
			background_color: Color::TRANSPARENT,
			text_color: self.appearance.colors().text,
		}
	}

	#[allow(clippy::unused_self)]
	fn lock(&self) -> Task<Message> {
		Task::future(async move {
			let action = LogoutAction::Lock;
			let connection = match Connection::system().await {
				Ok(connection) => connection,
				Err(e) => {
					log::error!("Failed to connect to system bus: {e}");
					return action_failed(action);
				}
			};

			let login_manager = match LoginManagerProxy::builder(&connection).build().await {
				Ok(login_manager) => login_manager,
				Err(e) => {
					log::error!("Failed to connect to login manager: {e}");
					return action_failed(action);
				}
			};

			if let Err(e) = login_manager.lock_session("auto".to_string()).await {
				log::error!("Failed to lock: {e}");
				return action_failed(action);
			}

			action_finished(true)
		})
	}

	#[allow(clippy::unused_self)]
	fn sign_out(&self) -> Task<Message> {
		Task::future(async move {
			let action = LogoutAction::SignOut;
			let connection = match Connection::system().await {
				Ok(connection) => connection,
				Err(e) => {
					log::error!("Failed to connect to system bus: {e}");
					return action_failed(action);
				}
			};

			let login_manager = match LoginManagerProxy::builder(&connection).build().await {
				Ok(login_manager) => login_manager,
				Err(e) => {
					log::error!("Failed to connect to login manager: {e}");
					return action_failed(action);
				}
			};

			let uid = nix::unistd::Uid::current();

			if let Err(e) = login_manager.terminate_user(uid.as_raw()).await {
				log::error!("Failed to sign out: {e}");
				return action_failed(action);
			}

			action_finished(false)
		})
	}

	fn power_off(&self) -> Task<Message> {
		let interactive = self.power_capabilities.power_off.interactive();

		Task::future(async move {
			let action = LogoutAction::PowerOff;
			let connection = match Connection::system().await {
				Ok(connection) => connection,
				Err(e) => {
					log::error!("Failed to connect to system bus: {e}");
					return action_failed(action);
				}
			};

			let login_manager = match LoginManagerProxy::builder(&connection).build().await {
				Ok(login_manager) => login_manager,
				Err(e) => {
					log::error!("Failed to connect to login manager: {e}");
					return action_failed(action);
				}
			};

			if let Err(e) = login_manager.power_off(interactive).await {
				log::error!("Failed to power off: {e}");
				return action_failed(action);
			}

			action_finished(false)
		})
	}

	fn reboot(&self) -> Task<Message> {
		let interactive = self.power_capabilities.reboot.interactive();

		Task::future(async move {
			let action = LogoutAction::Reboot;
			let connection = match Connection::system().await {
				Ok(connection) => connection,
				Err(e) => {
					log::error!("Failed to connect to system bus: {e}");
					return action_failed(action);
				}
			};

			let login_manager = match LoginManagerProxy::builder(&connection).build().await {
				Ok(login_manager) => login_manager,
				Err(e) => {
					log::error!("Failed to connect to login manager: {e}");
					return action_failed(action);
				}
			};

			if let Err(e) = login_manager.reboot(interactive).await {
				log::error!("Failed to reboot: {e}");
				return action_failed(action);
			}

			action_finished(false)
		})
	}

	fn suspend(&self) -> Task<Message> {
		let interactive = self.power_capabilities.suspend.interactive();

		Task::future(async move {
			let action = LogoutAction::Suspend;
			let connection = match Connection::system().await {
				Ok(connection) => connection,
				Err(e) => {
					log::error!("Failed to connect to system bus: {e}");
					return action_failed(action);
				}
			};

			let login_manager = match LoginManagerProxy::builder(&connection).build().await {
				Ok(login_manager) => login_manager,
				Err(e) => {
					log::error!("Failed to connect to login manager: {e}");
					return action_failed(action);
				}
			};

			let mut sleep_signals = match login_manager
				.inner()
				.receive_signal("PrepareForSleep")
				.await
			{
				Ok(sleep_signals) => sleep_signals,
				Err(e) => {
					log::error!("Failed to listen for sleep preparation: {e}");
					return action_failed(action);
				}
			};

			if let Err(e) = login_manager.suspend(interactive).await {
				log::error!("Failed to suspend: {e}");
				return action_failed(action);
			}

			wait_for_sleep(action, &mut sleep_signals).await
		})
	}

	fn hibernate(&self) -> Task<Message> {
		let interactive = self.power_capabilities.hibernate.interactive();

		Task::future(async move {
			let action = LogoutAction::Hibernate;
			let connection = match Connection::system().await {
				Ok(connection) => connection,
				Err(e) => {
					log::error!("Failed to connect to system bus: {e}");
					return action_failed(action);
				}
			};

			let login_manager = match LoginManagerProxy::builder(&connection).build().await {
				Ok(login_manager) => login_manager,
				Err(e) => {
					log::error!("Failed to connect to login manager: {e}");
					return action_failed(action);
				}
			};

			let mut sleep_signals = match login_manager
				.inner()
				.receive_signal("PrepareForSleep")
				.await
			{
				Ok(sleep_signals) => sleep_signals,
				Err(e) => {
					log::error!("Failed to listen for sleep preparation: {e}");
					return action_failed(action);
				}
			};

			if let Err(e) = login_manager.hibernate(interactive).await {
				log::error!("Failed to hibernate: {e}");
				return action_failed(action);
			}

			wait_for_sleep(action, &mut sleep_signals).await
		})
	}
}

fn action_button(
	icon: svg::Handle, label: &str, index: usize, colors: Colors, selected: bool,
) -> NeoButton<'_, Message> {
	let foreground = if selected {
		colors.on_accent
	} else {
		colors.text
	};

	neo_button(
		row![
			container(
				svg(icon)
					.width(Length::Shrink)
					.style(move |_, _| svg::Style {
						color: Some(foreground)
					}),
			)
			.padding(12),
			space().width(6),
			container("")
				.height(Length::Fill)
				.width(2)
				.style(|_| container::Style {
					background: Some(Background::Color(Color::BLACK)),
					..Default::default()
				}),
			space().width(12),
			text(label)
				.font(Font {
					weight: font::Weight::Bold,
					..Default::default()
				})
				.color(foreground),
			space::horizontal(),
			text(format!("{:>02}", index + 1)).color(foreground)
		]
		.align_y(Center),
	)
	.with_colors(colors)
	.on_press(Message::SelectAndActivate(index))
	.height(72)
	.width(Length::Fill)
}

fn action_finished(exit: bool) -> Message {
	Message::ActionFinished(ActionResult {
		exit,
		failed_label: None,
	})
}

fn action_failed(action: LogoutAction) -> Message {
	Message::ActionFinished(ActionResult {
		exit: false,
		failed_label: Some(action.failed_label()),
	})
}

async fn wait_for_sleep(
	action: LogoutAction, sleep_signals: &mut zbus::proxy::SignalStream<'_>,
) -> Message {
	while let Some(signal) = sleep_signals.next().await {
		match signal.body().deserialize::<bool>() {
			Ok(true) => return action_finished(true),
			Ok(false) => (),
			Err(e) => {
				log::error!("Failed to read sleep preparation signal: {e}");
				return action_failed(action);
			}
		}
	}

	log::error!("Sleep preparation signal stream ended unexpectedly");
	action_failed(action)
}

fn power_capability(action: &str, result: zbus::Result<CanDoOperation>) -> PowerCapability {
	match result {
		Ok(CanDoOperation::Yes) => PowerCapability::Allowed,
		Ok(CanDoOperation::Challenge) => PowerCapability::Challenge,
		Ok(_) => PowerCapability::Disabled,
		Err(e) => {
			log::error!("Failed to check if system can {action}: {e}");
			PowerCapability::Disabled
		}
	}
}

#[zbus::proxy(
	interface = "org.freedesktop.login1.Manager",
	default_service = "org.freedesktop.login1",
	default_path = "/org/freedesktop/login1"
)]
trait LoginManager {
	#[zbus(name = "LockSession")]
	fn lock_session(&self, id: String) -> zbus::Result<()>;

	#[zbus(name = "TerminateSession")]
	fn terminate_session(&self, id: String) -> zbus::Result<()>;

	#[zbus(name = "TerminateUser")]
	fn terminate_user(&self, id: u32) -> zbus::Result<()>;

	#[zbus(name = "CanPowerOff")]
	fn can_power_off(&self) -> zbus::Result<CanDoOperation>;
	#[zbus(name = "PowerOff")]
	fn power_off(&self, interactive: bool) -> zbus::Result<()>;

	#[zbus(name = "CanReboot")]
	fn can_reboot(&self) -> zbus::Result<CanDoOperation>;
	#[zbus(name = "Reboot")]
	fn reboot(&self, interactive: bool) -> zbus::Result<()>;

	#[zbus(name = "CanSuspend")]
	fn can_suspend(&self) -> zbus::Result<CanDoOperation>;
	#[zbus(name = "Suspend")]
	fn suspend(&self, interactive: bool) -> zbus::Result<()>;

	#[zbus(name = "CanHibernate")]
	fn can_hibernate(&self) -> zbus::Result<CanDoOperation>;
	#[zbus(name = "Hibernate")]
	fn hibernate(&self, interactive: bool) -> zbus::Result<()>;

	#[zbus(name = "CanHybridSleep")]
	fn can_hybrid_sleep(&self) -> zbus::Result<CanDoOperation>;
	#[zbus(name = "HybridSleep")]
	fn hybrid_sleep(&self, interactive: bool) -> zbus::Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize, zvariant::Type)]
#[zvariant(signature = "s")]
enum CanDoOperation {
	#[serde(rename = "na")]
	#[zbus(name = "na")]
	NotAvailable,
	#[serde(rename = "yes")]
	#[zbus(name = "yes")]
	Yes,
	#[serde(rename = "no")]
	#[zbus(name = "no")]
	No,
	#[serde(rename = "challenge")]
	#[zbus(name = "challenge")]
	Challenge,
}
