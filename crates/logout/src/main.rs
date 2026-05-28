use clap::{Parser, Subcommand};
use futures::StreamExt;
use iced::{
	Alignment::Center,
	Background, Border, Color, Element, Font, Length, Padding, Subscription, Task, Theme, keyboard,
	mouse,
	theme::Style,
	widget::{Column, column, container, mouse_area, row, space, svg, text},
};
use iced_layershell::{
	application,
	reexport::{Anchor, KeyboardInteractivity, Layer, core::font},
	settings::{LayerShellSettings, StartMode},
	to_layer_message,
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button, neo_card},
};
use zbus::Connection;

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

fn main() -> Result<(), iced_layershell::Error> {
	let _ = pretty_env_logger::try_init();

	let args = Args::parse();

	let app = application(
		move || Logout::new(args.clone()),
		Logout::namespace,
		Logout::update,
		Logout::view,
	)
	.style(Logout::style)
	.subscription(Logout::subscription)
	.layer_settings(LayerShellSettings {
		size: Some((0, 0)),
		anchor: Anchor::all(),
		start_mode: StartMode::Active,
		layer: Layer::Overlay,
		keyboard_interactivity: KeyboardInteractivity::Exclusive,
		..Default::default()
	});

	app.run()
}

#[to_layer_message]
#[derive(Debug, Clone)]
enum Message {
	Exit,
	PowerCapabilitiesLoaded(PowerCapabilities),
	ActionFinished(ActionResult),
	CycleEntry(bool),
	SelectEntry(usize),
	ActivateCurrentEntry,
	SelectAndActivate(usize),
	Noop,

	IcedEvent(iced::event::Event),
}

struct Logout {
	args: Args,
	selected_entry: usize,
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
			args,
			selected_entry: 0,
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
			} if ('1'..='6').contains(&c.chars().next().unwrap_or_default()) => {
				let index: u8 = c.parse().unwrap();
				Message::SelectEntry(index as usize - 1)
			}
			keyboard::Event::KeyPressed {
				key: keyboard::Key::Named(keyboard::key::Named::Enter),
				..
			} => Message::ActivateCurrentEntry,
			_ => Message::Noop,
		});
		let event = iced::event::listen().map(Message::IcedEvent);

		Subscription::batch([keyboard, event])
	}

	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Exit => {
				if matches!(self.state, LogoutState::Choosing) {
					return iced::exit();
				}
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
				if !matches!(self.state, LogoutState::Choosing) {
					return Task::none();
				}

				self.select_next_entry(backwards);
			}
			Message::SelectEntry(entry) => {
				if !matches!(self.state, LogoutState::Choosing) {
					return Task::none();
				}

				if self.is_entry_enabled(entry) {
					self.selected_entry = entry;
				}
			}
			Message::IcedEvent(iced::event::Event::Window(iced::window::Event::Unfocused)) => {
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
		if let LogoutState::Transition { label } = self.state {
			return self.transition(label);
		}

		let content = match self.args.command.unwrap_or_default() {
			Command::Session => self.session(),
			Command::Power => self.power(0),
			Command::All => column![self.session(), self.power(2),].spacing(12),
		};

		let content = column![text("Choose an action.").color(COLORS.white), content,].spacing(12);

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
					background: Some(Background::Color(COLORS.black)),
					..Default::default()
				}),
			space().width(8),
			mouse_area(svg(phosphor_icon!("x")).width(Length::Shrink).height(24))
				.on_press(Message::Exit)
				.interaction(mouse::Interaction::Pointer),
		];

		let header = container(header)
			.padding(12)
			.width(Length::Fill)
			.style(|_| container::Style {
				background: Some(Background::Color(COLORS.body)),
				border: Border {
					width: 2.0,
					color: COLORS.border,
					..Default::default()
				},
				..Default::default()
			});

		let content = neo_card(column![header, content].spacing(18))
			.padding(Padding {
				left: 0.0,
				right: 0.0,
				top: 0.0,
				bottom: 20.0,
			})
			.background(COLORS.black.mix(COLORS.white, 0.1))
			.width(480);

		let content = mouse_area(content).on_press(Message::Noop);

		mouse_area(
			container(content)
				.style(|_| container::Style {
					background: Some(Background::Color(COLORS.black.scale_alpha(0.4))),
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

	fn session(&self) -> Column<'_, Message> {
		column![
			action_button(phosphor_icon!("lock"), "LOCK", 0).background(
				if self.selected_entry == 0 {
					COLORS.decorative.yellow
				} else {
					COLORS.white
				}
			),
			action_button(phosphor_icon!("sign-out"), "LOGOUT", 1).background(
				if self.selected_entry == 1 {
					COLORS.decorative.green
				} else {
					COLORS.white
				}
			),
		]
		.spacing(12)
	}

	fn power(&self, offset: usize) -> Column<'_, Message> {
		column![
			action_button(phosphor_icon!("power"), "POWER OFF", offset)
				.background(if self.selected_entry == offset {
					COLORS.feedback.danger
				} else {
					COLORS.white
				})
				.enabled(self.power_capabilities.power_off.enabled()),
			action_button(
				phosphor_icon!("arrow-counter-clockwise"),
				"REBOOT",
				offset + 1
			)
			.background(if self.selected_entry == offset + 1 {
				COLORS.feedback.warning
			} else {
				COLORS.white
			})
			.enabled(self.power_capabilities.reboot.enabled()),
			action_button(phosphor_icon!("pause-circle"), "HIBERNATE", offset + 2)
				.background(if self.selected_entry == offset + 2 {
					COLORS.decorative.yellow
				} else {
					COLORS.white
				})
				.enabled(self.power_capabilities.hibernate.enabled()),
			action_button(phosphor_icon!("moon"), "SLEEP", offset + 3)
				.background(if self.selected_entry == offset + 3 {
					COLORS.decorative.blue
				} else {
					COLORS.white
				})
				.enabled(self.power_capabilities.suspend.enabled()),
		]
		.spacing(12)
	}

	fn transition(&self, label: &'static str) -> Element<'_, Message> {
		let content = neo_card(
			column![
				text(label).font(Font {
					weight: font::Weight::Bold,
					..Default::default()
				}),
				text("Please wait.").color(COLORS.white),
			]
			.spacing(12),
		)
		.background(COLORS.black.mix(COLORS.white, 0.1))
		.width(480);

		container(content)
			.style(|_| container::Style {
				background: Some(Background::Color(COLORS.black.scale_alpha(0.4))),
				..Default::default()
			})
			.padding(0)
			.width(Length::Fill)
			.height(Length::Fill)
			.align_x(Center)
			.align_y(Center)
			.into()
	}

	// fn session(&self) -> Element<'_, Message> {
	// 	neo_card(
	// 		column![
	// 			text("Choose an action.").color(COLORS.white),
	// 			action_button(phosphor_icon!("lock"), "LOCK", 1).background(
	// 				if self.selected_entry == 0 {
	// 					COLORS.decorative.yellow
	// 				} else {
	// 					COLORS.white
	// 				}
	// 			),
	// 			action_button(phosphor_icon!("sign-out"), "LOGOUT", 2).background(
	// 				if self.selected_entry == 1 {
	// 					COLORS.decorative.orange
	// 				} else {
	// 					COLORS.white
	// 				}
	// 			)
	// 		]
	// 		.spacing(12),
	// 	)
	// 	.background(COLORS.black.mix(COLORS.white, 0.1))
	// 	.width(480)
	// 	.into()
	// }

	fn style(&self, _theme: &Theme) -> Style {
		Style {
			background_color: Color::TRANSPARENT,
			text_color: Color::BLACK,
		}
	}

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

fn action_button(icon: svg::Handle, label: &str, index: usize) -> NeoButton<'_, Message> {
	neo_button(
		row![
			container(svg(icon).width(Length::Shrink),).padding(12),
			space().width(6),
			container("")
				.height(Length::Fill)
				.width(2)
				.style(|_| container::Style {
					background: Some(Background::Color(COLORS.black)),
					..Default::default()
				}),
			space().width(12),
			text(label)
				.font(Font {
					weight: font::Weight::Bold,
					..Default::default()
				})
				.color(COLORS.text),
			space::horizontal(),
			text(format!("{:>02}", index + 1))
		]
		.align_y(Center),
	)
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
