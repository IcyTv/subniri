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

#[derive(Debug)]
pub enum Module {
	SystemMenu(system_menu::SystemMenu),
	Network,
	Bluetooth(Option<bluetooth::Bluetooth>),
	Clock(clock::Clock),
	Volume(Option<volume::Volume>),
	MediaControls(media_controls::MediaControls),
	Taskbar(Option<taskbar::Taskbar>),
}

#[derive(Debug, Clone)]
pub enum ModuleMessage {
	Pressed(ModuleKind, Rectangle),
	OpenPopup(ModuleKind, Rectangle),

	OpenPowerMenu,
	OpenSettings,

	Clock(clock::Message),
	Network(network::Message),
	MediaControls(media_controls::Message),
	SystemMenu(system_menu::Message),

	BluetoothInitialized(Result<bluetooth::Bluetooth, String>),
	Bluetooth(bluetooth::Message),

	VolumeInitialized(Result<volume::Volume, String>),
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

impl Module {
	pub fn clock() -> Self {
		Self::Clock(clock::Clock::new())
	}

	pub fn bluetooth() -> Self {
		Self::Bluetooth(None)
	}

	pub fn volume() -> Self {
		Self::Volume(None)
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

	pub fn set_popup_id(&mut self, id: Id) {
		let _ = id;
	}

	pub fn init_task(&self) -> Task<ModuleMessage> {
		match self {
			Self::Bluetooth(None) => Task::perform(
				bluetooth::Bluetooth::new(),
				ModuleMessage::BluetoothInitialized,
			),
			Self::Volume(None) => {
				Task::perform(volume::Volume::new(), ModuleMessage::VolumeInitialized)
			}
			Self::Taskbar(None) => {
				Task::perform(taskbar::Taskbar::new(), ModuleMessage::TaskbarInitialized)
			}
			Self::SystemMenu(menu) => menu.init().map(ModuleMessage::SystemMenu),
			_ => Task::none(),
		}
	}

	pub fn update(&mut self, message: ModuleMessage, config: &ConfigFile) -> Task<ModuleMessage> {
		match (self, message) {
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
			(Self::Bluetooth(bluetooth), ModuleMessage::BluetoothInitialized(result)) => {
				match result {
					Ok(initialized) => *bluetooth = Some(initialized),
					Err(error) => log::warn!("Failed to initialize Bluetooth module: {error}"),
				}
			}
			(Self::Volume(volume), ModuleMessage::VolumeInitialized(result)) => match result {
				Ok(initialized) => *volume = Some(initialized),
				Err(error) => log::warn!("Failed to initialize Volme module: {error}"),
			},
			(Self::Taskbar(taskbar), ModuleMessage::TaskbarInitialized(result)) => match result {
				Ok(initialized) => *taskbar = Some(initialized),
				Err(error) => log::warn!("Failed to initialize Niri taskbar: {error}"),
			},
			(Self::Bluetooth(Some(bluetooth)), ModuleMessage::Bluetooth(message)) => {
				bluetooth.update(message)
			}
			(Self::Volume(Some(volume)), ModuleMessage::Volume(message)) => {
				return volume.update(message).map(ModuleMessage::Volume);
			}
			(Self::Clock(clock), ModuleMessage::Clock(message)) => clock.update(message),
			(Self::MediaControls(controls), ModuleMessage::MediaControls(message)) => {
				return controls.update(message).map(ModuleMessage::MediaControls);
			}
			(Self::Taskbar(Some(taskbar)), ModuleMessage::Taskbar(message)) => {
				taskbar.update(message)
			}
			(Self::SystemMenu(menu), ModuleMessage::SystemMenu(message)) => {
				return menu.update(message, config).map(ModuleMessage::SystemMenu);
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
			Self::Bluetooth(Some(bluetooth)) => {
				bluetooth.subscription().map(ModuleMessage::Bluetooth)
			}
			Self::Volume(Some(volume)) => volume.subscription().map(ModuleMessage::Volume),
			Self::Clock(clock) => clock.subscription().map(ModuleMessage::Clock),
			Self::MediaControls(controls) => {
				controls.subscription().map(ModuleMessage::MediaControls)
			}
			Self::Taskbar(Some(taskbar)) => taskbar.subscription().map(ModuleMessage::Taskbar),
			Self::SystemMenu(menu) => menu.subscription().map(ModuleMessage::SystemMenu),
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
			Self::Volume(Some(volume)) => volume
				.view()
				.map(ModuleMessage::Volume)
				.on_press_with_bounds(|bounds| ModuleMessage::Pressed(ModuleKind::Volume, bounds))
				.into(),
			Self::Volume(None) => loading(COLORS.decorative.yellow),
			Self::Network => network::network()
				.map(ModuleMessage::Network)
				.on_press_with_bounds(|bounds| ModuleMessage::Pressed(ModuleKind::Network, bounds))
				.into(),
			Self::Bluetooth(Some(bluetooth)) => bluetooth
				.view()
				.map(ModuleMessage::Bluetooth)
				.on_press_with_bounds(|bounds| {
					ModuleMessage::Pressed(ModuleKind::Bluetooth, bounds)
				})
				.into(),
			Self::Bluetooth(None) => loading(COLORS.decorative.blue),
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
			Self::Volume(Some(vol)) => vol.view_popup().map(ModuleMessage::Volume),
			Self::Bluetooth(Some(bt)) => bt.view_popup().map(ModuleMessage::Bluetooth),
			_ => neo_card(text("No popup for module").color(COLORS.text))
				.background(COLORS.feedback.danger)
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
