use std::{future::Future, time::Duration};

use config::ConfigFile;
use iced::{Color, Element, Rectangle, Subscription, Task, widget::text, window::Id};
use neo_widgets::{
	style::COLORS,
	widgets::{neo_button, neo_card, spinner},
};

mod bluetooth;
mod clock;
mod media_controls;
mod network;
mod system_menu;
mod taskbar;
mod volume;

#[cfg(test)]
mod test_bus;

#[derive(Debug)]
pub enum Module {
	SystemMenu(system_menu::SystemMenu),
	Network(ServiceState<network::Network>),
	Bluetooth(ServiceState<bluetooth::Bluetooth>),
	Clock(clock::Clock),
	Volume(Result<volume::Volume, String>),
	MediaControls(media_controls::MediaControls),
	Taskbar(Option<taskbar::Taskbar>),
}

#[derive(Debug, Clone)]
pub enum ModuleMessage {
	ConfigUpdated(ConfigFile),
	SaveSystemMenuWidgets(Vec<config::SystemMenuWidgets>),
	PopupClosed,
	Pressed(ModuleKind, Rectangle),
	OpenPopup(ModuleKind, Rectangle),
	OpenContextMenu(Rectangle),
	OpenTrayContextMenu { service: String, bounds: Rectangle },
	InvokeTrayContextMenu { service: String, x: i32, y: i32 },
	CloseContextMenus,
	ClosePopup,

	OpenPowerMenu,
	OpenSettings,

	Clock(clock::Message),
	MediaControls(media_controls::Message),
	SystemMenu(system_menu::Message),

	NetworkInitialized(Result<network::Network, String>),
	Network(network::Message),
	NetworkEvent(utilities::Hashable<nmrs::NetworkManager>, network::Message),

	BluetoothInitialized(Result<bluetooth::Bluetooth, String>),
	Bluetooth(bluetooth::Message),

	Volume(volume::Message),

	TaskbarInitialized(Result<taskbar::Taskbar, String>),
	Taskbar(taskbar::Message),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
	SystemMenu,
	MediaControls,
	Volume,
	Network,
	Bluetooth,
	Clock,
}

const MODULE_HEIGHT: f32 = 48.0;
const MODULE_RADIUS: f32 = 8.0;
const ICON_HEIGHT: f32 = 20.0;

const INITIALIZATION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum ServiceState<T> {
	Connecting,
	Ready(T),
	Unavailable { error: String, attempt: u32 },
}

impl<T> ServiceState<T> {
	fn failed(&mut self, error: String) -> Duration {
		let attempt = match self {
			Self::Unavailable { attempt, .. } => attempt.saturating_add(1),
			_ => 0,
		};
		*self = Self::Unavailable { error, attempt };
		Duration::from_secs((1_u64 << attempt.min(5)).min(30))
	}
}

async fn initialize<T>(
	delay: Duration, connect: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
	tokio::time::sleep(delay).await;
	tokio::time::timeout(INITIALIZATION_TIMEOUT, connect)
		.await
		.map_err(|_| "Service initialization timed out".to_string())?
}

impl Module {
	pub fn clock() -> Self {
		Self::Clock(clock::Clock::new())
	}

	pub fn bluetooth() -> Self {
		Self::Bluetooth(ServiceState::Connecting)
	}

	pub fn volume() -> Self {
		Self::Volume(volume::Volume::new())
	}

	pub fn media_controls() -> Self {
		Self::MediaControls(media_controls::MediaControls::new())
	}

	pub fn taskbar() -> Self {
		Self::Taskbar(None)
	}

	pub fn system_menu(config: &ConfigFile) -> Self {
		Self::SystemMenu(system_menu::SystemMenu::new(config))
	}

	pub fn network() -> Self {
		Self::Network(ServiceState::Connecting)
	}

	pub fn set_popup_id(&mut self, id: Id) {
		let _ = (self, id);
	}

	pub fn init_task(&self) -> Task<ModuleMessage> {
		match self {
			Self::Bluetooth(ServiceState::Connecting) => Task::perform(
				initialize(Duration::ZERO, bluetooth::Bluetooth::new()),
				ModuleMessage::BluetoothInitialized,
			),
			Self::Taskbar(None) => {
				Task::perform(taskbar::Taskbar::new(), ModuleMessage::TaskbarInitialized)
			}
			Self::Network(ServiceState::Connecting) => Task::perform(
				initialize(Duration::ZERO, network::Network::new()),
				ModuleMessage::NetworkInitialized,
			),
			Self::SystemMenu(menu) => menu.init().map(ModuleMessage::SystemMenu),
			_ => Task::none(),
		}
	}

	pub fn update(&mut self, message: ModuleMessage) -> Task<ModuleMessage> {
		match (self, message) {
			(Self::SystemMenu(menu), ModuleMessage::ConfigUpdated(config)) => {
				menu.sync_config(&config);
			}
			(Self::SystemMenu(menu), ModuleMessage::PopupClosed) => {
				menu.popup_closed();
			}
			(
				Self::SystemMenu(_),
				ModuleMessage::SystemMenu(system_menu::Message::OpenPowerMenu),
			) => {
				return Task::done(ModuleMessage::OpenPowerMenu);
			}
			(
				Self::SystemMenu(_),
				ModuleMessage::SystemMenu(system_menu::Message::OpenSettings),
			) => return Task::done(ModuleMessage::OpenSettings),
			(
				Self::SystemMenu(menu),
				ModuleMessage::SystemMenu(system_menu::Message::OpenContextMenu(
					context_menu,
					depth,
					bounds,
				)),
			) => {
				menu.open_context_menu(context_menu, depth);
				return Task::done(ModuleMessage::OpenContextMenu(bounds));
			}
			(Self::Bluetooth(bluetooth), ModuleMessage::BluetoothInitialized(result)) => {
				match result {
					Ok(initialized) => *bluetooth = ServiceState::Ready(initialized),
					Err(error) => {
						log::warn!("Failed to initialize Bluetooth module: {error}");
						let delay = bluetooth.failed(error);
						return Task::perform(
							initialize(delay, bluetooth::Bluetooth::new()),
							ModuleMessage::BluetoothInitialized,
						);
					}
				}
			}
			(Self::Taskbar(taskbar), ModuleMessage::TaskbarInitialized(result)) => match result {
				Ok(initialized) => *taskbar = Some(initialized),
				Err(error) => log::warn!("Failed to initialize Niri taskbar: {error}"),
			},
			(
				Self::Bluetooth(ServiceState::Ready(bluetooth)),
				ModuleMessage::Bluetooth(message),
			) => {
				bluetooth.update(message);
			}
			(Self::Volume(Ok(volume)), ModuleMessage::Volume(message)) => {
				return volume.update(message).map(ModuleMessage::Volume);
			}
			(Self::Network(network), ModuleMessage::NetworkInitialized(Ok(initialized))) => {
				*network = ServiceState::Ready(initialized);
			}
			(Self::Network(network), ModuleMessage::NetworkInitialized(Err(error))) => {
				log::warn!("Failed to initialize Network module: {error}");
				let delay = network.failed(error);
				return Task::perform(
					initialize(delay, network::Network::new()),
					ModuleMessage::NetworkInitialized,
				);
			}
			(Self::Network(ServiceState::Ready(network)), ModuleMessage::Network(message)) => {
				log::trace!("Network module update: {:?}", message);
				let connection = network.connection();
				return network
					.update(message)
					.map(move |message| ModuleMessage::NetworkEvent(connection.clone(), message));
			}
			(Self::Network(state), ModuleMessage::NetworkEvent(connection, message)) => {
				let ServiceState::Ready(network) = state else {
					return Task::none();
				};
				if network.connection().as_ptr() != connection.as_ptr() {
					return Task::none();
				}
				if let network::Message::Disconnected(error) = message {
					log::warn!("Network module disconnected: {error}");
					let delay = state.failed(error);
					return Task::perform(
						async move {
							let _ = connection.dbus_connection().clone().close().await;
							initialize(delay, network::Network::new()).await
						},
						ModuleMessage::NetworkInitialized,
					);
				}
				return network
					.update(message)
					.map(move |message| ModuleMessage::NetworkEvent(connection.clone(), message));
			}
			(Self::Clock(clock), ModuleMessage::Clock(message)) => {
				if let Some(action) = clock.update(message) {
					return Task::done(match action {
						clock::PopupAction::Open(bounds) => ModuleMessage::OpenContextMenu(bounds),
						clock::PopupAction::NativeContextMenu { service, bounds } => {
							ModuleMessage::OpenTrayContextMenu { service, bounds }
						}
						clock::PopupAction::CloseAll => ModuleMessage::CloseContextMenus,
					});
				}
			}
			(Self::Clock(clock), ModuleMessage::InvokeTrayContextMenu { service, x, y }) => {
				clock.open_tray_context_menu(service, x, y);
			}
			(Self::Clock(clock), ModuleMessage::PopupClosed) => clock.on_popup_closed(),
			(
				Self::MediaControls(_),
				ModuleMessage::MediaControls(media_controls::Message::ClosePopup),
			) => {
				return Task::done(ModuleMessage::ClosePopup);
			}
			(Self::MediaControls(controls), ModuleMessage::MediaControls(message)) => {
				return controls.update(message).map(ModuleMessage::MediaControls);
			}
			(Self::Taskbar(Some(taskbar)), ModuleMessage::Taskbar(message)) => {
				taskbar.update(message);
			}
			(Self::SystemMenu(menu), ModuleMessage::SystemMenu(message)) => {
				let previous_widgets = matches!(message, system_menu::Message::WidgetsEdited(_))
					.then(|| menu.widgets().to_vec());
				let closes_context_menus = message.closes_context_menus();
				if closes_context_menus {
					menu.close_context_menus();
				}
				let update = menu.update(message).map(ModuleMessage::SystemMenu);
				let update = if previous_widgets.is_some_and(|previous| previous != menu.widgets())
				{
					Task::batch([
						update,
						Task::done(ModuleMessage::SaveSystemMenuWidgets(
							menu.widgets().to_vec(),
						)),
					])
				} else {
					update
				};
				return if closes_context_menus {
					Task::batch([update, Task::done(ModuleMessage::CloseContextMenus)])
				} else {
					update
				};
			}
			(_, ModuleMessage::Pressed(kind, bounds)) => {
				return Task::done(ModuleMessage::OpenPopup(kind, bounds));
			}
			_ => {}
		}

		Task::none()
	}

	pub fn subscription(&self) -> Subscription<ModuleMessage> {
		match self {
			Self::Bluetooth(ServiceState::Ready(bluetooth)) => {
				bluetooth.subscription().map(ModuleMessage::Bluetooth)
			}
			Self::Volume(Ok(volume)) => volume.subscription().map(ModuleMessage::Volume),
			Self::Clock(clock) => clock.subscription().map(ModuleMessage::Clock),
			Self::MediaControls(controls) => {
				controls.subscription().map(ModuleMessage::MediaControls)
			}
			Self::Taskbar(Some(_)) => taskbar::Taskbar::subscription().map(ModuleMessage::Taskbar),
			Self::SystemMenu(menu) => menu.subscription().map(ModuleMessage::SystemMenu),
			Self::Network(ServiceState::Ready(network)) => {
				let connection = network.connection();
				network
					.subscription()
					.with(connection)
					.map(|(connection, message)| ModuleMessage::NetworkEvent(connection, message))
			}
			_ => Subscription::none(),
		}
	}

	pub fn view(&self, output_name: Option<&str>) -> Element<'_, ModuleMessage> {
		match self {
			Self::SystemMenu(menu) => menu
				.view()
				.map(ModuleMessage::SystemMenu)
				.on_press_with_bounds(|bounds| {
					ModuleMessage::Pressed(ModuleKind::SystemMenu, bounds)
				})
				.into(),
			Self::MediaControls(controls) => controls
				.view()
				.map(ModuleMessage::MediaControls)
				.on_press_with_bounds(|bounds| {
					ModuleMessage::Pressed(ModuleKind::MediaControls, bounds)
				})
				.into(),
			Self::Volume(Ok(volume)) => volume
				.view()
				.map(ModuleMessage::Volume)
				.on_press_with_bounds(|bounds| ModuleMessage::Pressed(ModuleKind::Volume, bounds))
				.into(),
			// TODO
			Self::Volume(Err(_e)) => loading(COLORS.decorative.yellow),
			Self::Network(ServiceState::Connecting) => loading(COLORS.decorative.green),
			Self::Network(ServiceState::Unavailable { error, .. }) => {
				unavailable("Network unavailable", error, COLORS.decorative.green)
			}
			Self::Network(ServiceState::Ready(network)) => network
				.view()
				.map(ModuleMessage::Network)
				.on_press_with_bounds(|bounds| ModuleMessage::Pressed(ModuleKind::Network, bounds))
				.into(),
			Self::Bluetooth(ServiceState::Ready(bluetooth)) => bluetooth
				.view()
				.map(ModuleMessage::Bluetooth)
				.on_press_with_bounds(|bounds| {
					ModuleMessage::Pressed(ModuleKind::Bluetooth, bounds)
				})
				.into(),
			Self::Bluetooth(ServiceState::Connecting) => loading(COLORS.decorative.blue),
			Self::Bluetooth(ServiceState::Unavailable { error, .. }) => {
				unavailable("Bluetooth unavailable", error, COLORS.decorative.blue)
			}
			Self::Clock(clock) => clock
				.view()
				.map(ModuleMessage::Clock)
				.on_press_with_bounds(|bounds| ModuleMessage::Pressed(ModuleKind::Clock, bounds))
				.into(),
			Self::Taskbar(None) => loading(COLORS.white),
			Self::Taskbar(Some(taskbar)) => taskbar.view(output_name).map(ModuleMessage::Taskbar),
		}
	}

	pub fn view_popup(&self) -> Element<'_, ModuleMessage> {
		match self {
			Self::MediaControls(controls) => {
				controls.view_popup().map(ModuleMessage::MediaControls)
			}
			Self::SystemMenu(menu) => menu.view_popup().map(ModuleMessage::SystemMenu),
			Self::Volume(Ok(vol)) => vol.view_popup().map(ModuleMessage::Volume),
			Self::Bluetooth(ServiceState::Ready(bt)) => {
				bt.view_popup().map(ModuleMessage::Bluetooth)
			}
			Self::Clock(clock) => clock.view_popup().map(ModuleMessage::Clock),
			Self::Network(ServiceState::Ready(network)) => {
				network.view_popup().map(ModuleMessage::Network)
			}
			_ => neo_card(text("No popup for module").color(COLORS.text))
				.background(COLORS.feedback.danger)
				.into(),
		}
	}

	pub fn view_context_menu(&self, depth: usize) -> Element<'_, ModuleMessage> {
		match self {
			Self::Clock(clock) => clock.view_context_menu(depth).map(ModuleMessage::Clock),
			Self::SystemMenu(menu) => menu.view_context_menu(depth).map(ModuleMessage::SystemMenu),
			_ => neo_card(text("Unimplemented context menu").color(COLORS.text))
				.padding(8)
				.background(COLORS.white)
				.into(),
		}
	}
}

pub fn loading<'a>(background: Color) -> Element<'a, ModuleMessage> {
	neo_button(spinner().bar_color(COLORS.black))
		.height(MODULE_HEIGHT)
		.width(MODULE_HEIGHT)
		.background(background)
		.radius(MODULE_RADIUS)
		.into()
}

fn unavailable<'a>(
	label: &'a str, error: &'a str, background: Color,
) -> Element<'a, ModuleMessage> {
	iced::widget::tooltip(
		neo_button(text(label))
			.height(MODULE_HEIGHT)
			.background(background)
			.radius(MODULE_RADIUS),
		text(format!("{error}\nRetrying automatically")),
		iced::widget::tooltip::Position::Bottom,
	)
	.into()
}

#[cfg(test)]
mod tests {
	use super::{Module, ModuleMessage, ServiceState};

	#[test]
	fn network_startup_failure_schedules_recovery() {
		let mut module = Module::network();
		let task = module.update(ModuleMessage::NetworkInitialized(Err("unavailable".into())));
		assert!(
			task.units() > 0,
			"network initialization failure must retry"
		);
		assert!(matches!(
			module,
			Module::Network(ServiceState::Unavailable { .. })
		));
		assert_eq!(module.subscription().units(), 0);
	}

	#[test]
	fn bluetooth_startup_failure_schedules_recovery() {
		let mut module = Module::bluetooth();
		let task = module.update(ModuleMessage::BluetoothInitialized(Err(
			"unavailable".into()
		)));
		assert!(
			task.units() > 0,
			"Bluetooth initialization failure must retry"
		);
		assert!(matches!(
			module,
			Module::Bluetooth(ServiceState::Unavailable { .. })
		));
		assert_eq!(module.subscription().units(), 0);
	}

	#[test]
	fn retries_are_capped_and_reset_after_success() {
		let mut state = ServiceState::<()>::Connecting;
		for seconds in [1, 2, 4, 8, 16, 30, 30] {
			assert_eq!(state.failed("unavailable".into()).as_secs(), seconds);
		}
		state = ServiceState::Ready(());
		assert_eq!(state.failed("disconnected".into()).as_secs(), 1);
	}

	#[tokio::test(start_paused = true)]
	async fn nonreplying_initialization_times_out() {
		let result =
			super::initialize::<()>(std::time::Duration::ZERO, std::future::pending()).await;
		assert!(result.is_err());
	}

	#[tokio::test(start_paused = true)]
	async fn retry_delay_precedes_connection_attempt() {
		let start = tokio::time::Instant::now();
		let result = super::initialize(std::time::Duration::from_secs(4), async {
			assert!(start.elapsed() >= std::time::Duration::from_secs(4));
			Ok(())
		})
		.await;
		assert!(result.is_ok());
	}
}
