use config::ConfigFile;
use iced::{Color, Element, Rectangle, Subscription, Task, widget::text, window::Id};
use neo_widgets::{
	style::Colors,
	widgets::{neo_button, neo_card, spinner},
};

mod bluetooth;
mod clock;
mod connection;
mod media_controls;
mod network;
pub(crate) mod system_menu;
mod taskbar;
mod volume;

#[cfg(test)]
mod test_bus;

#[cfg(test)]
mod layout_tests;

#[derive(Debug)]
pub enum Module {
	SystemMenu(system_menu::SystemMenu),
	Network(network::integration::Integration),
	Bluetooth(bluetooth::integration::Integration),
	Clock(clock::Clock),
	Volume(Result<volume::Volume, String>),
	MediaControls(media_controls::MediaControls),
	Taskbar(Option<taskbar::Taskbar>),
}

#[derive(Debug, Clone)]
pub enum ModuleMessage {
	ConfigUpdated(ConfigFile),
	AppearanceChanged {
		theme: config::AppearanceMode,
		connected: bool,
	},
	SelectTheme(config::AppearanceMode),
	SaveSystemMenuLayout(config::SystemMenu),
	PopupClosed,
	Pressed(ModuleKind, Rectangle),
	OpenPopup(ModuleKind, Rectangle),
	OpenContextMenu(Rectangle),
	OpenTrayContextMenu {
		service: String,
		bounds: Rectangle,
	},
	InvokeTrayContextMenu {
		service: String,
		x: i32,
		y: i32,
	},
	CloseContextMenus,
	ClosePopup,

	OpenPowerMenu,
	OpenSettings,

	Clock(clock::Message),
	MediaControls(media_controls::Message),
	SystemMenu(system_menu::Message),

	Network(network::integration::Message),

	Bluetooth(bluetooth::integration::Message),

	Volume(volume::Message),

	TaskbarInitialized(Result<taskbar::Taskbar, String>),
	Taskbar(taskbar::Message),
}

impl ModuleMessage {
	fn from_system_menu(message: system_menu::Message) -> Self {
		match message {
			system_menu::Message::SaveLayout(layout) => Self::SaveSystemMenuLayout(layout),
			system_menu::Message::SelectTheme(theme) => Self::SelectTheme(theme),
			message => Self::SystemMenu(message),
		}
	}
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

/// Center the font's visible glyphs, rather than only its line box, in bar faces.
fn module_label<'a, Message: 'a>(
	label: iced::widget::Text<'a>,
) -> iced::widget::Container<'a, Message> {
	iced::widget::container(label).padding(iced::Padding {
		top: 2.0,
		..iced::Padding::ZERO
	})
}

impl Module {
	pub fn clock() -> Self {
		Self::Clock(clock::Clock::new())
	}

	pub fn bluetooth() -> Self {
		Self::Bluetooth(bluetooth::integration::Integration::new())
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
		Self::Network(network::integration::Integration::new())
	}

	pub fn set_popup_id(&mut self, id: Id) {
		if let Self::SystemMenu(menu) = self {
			menu.popup_opened(id, iced::Size::new(1920.0, 1080.0));
		}
	}

	pub fn popup_opened(&mut self, id: Id, maximum: iced::Size) {
		if let Self::SystemMenu(menu) = self {
			menu.popup_opened(id, maximum);
		} else {
			self.set_popup_id(id);
		}
	}

	pub fn popup_configured(&mut self, id: Id, size: iced::Size) {
		if let Self::SystemMenu(menu) = self {
			menu.popup_configured(id, size);
		}
	}

	pub fn popup_presented(&mut self, id: Id, content_updated_at: std::time::Instant) {
		if let Self::SystemMenu(menu) = self {
			menu.popup_presented(id, content_updated_at);
		}
	}

	pub fn presentation_window(&self) -> Option<Id> {
		if let Self::SystemMenu(menu) = self {
			menu.presentation_window()
		} else {
			None
		}
	}

	pub fn popup_bounds(&mut self, maximum: iced::Size) {
		if let Self::SystemMenu(menu) = self {
			menu.popup_bounds(maximum);
		}
	}

	pub fn init_task(&self) -> Task<ModuleMessage> {
		match self {
			Self::Bluetooth(bluetooth) => bluetooth.init().map(ModuleMessage::Bluetooth),
			Self::Taskbar(None) => {
				Task::perform(taskbar::Taskbar::new(), ModuleMessage::TaskbarInitialized)
			}
			Self::Network(network) => network.init().map(ModuleMessage::Network),
			Self::SystemMenu(menu) => menu.init().map(ModuleMessage::from_system_menu),
			_ => Task::none(),
		}
	}

	pub fn update(&mut self, message: ModuleMessage) -> Task<ModuleMessage> {
		match (self, message) {
			(Self::SystemMenu(menu), ModuleMessage::AppearanceChanged { theme, connected }) => {
				menu.appearance_changed(theme, connected);
			}
			(Self::SystemMenu(menu), ModuleMessage::ConfigUpdated(config)) => {
				return menu
					.sync_config(&config)
					.map(ModuleMessage::from_system_menu);
			}
			(Self::SystemMenu(menu), ModuleMessage::PopupClosed) => {
				return menu.popup_closed().map(ModuleMessage::from_system_menu);
			}
			(
				Self::SystemMenu(_),
				ModuleMessage::SystemMenu(system_menu::Message::SelectTheme(theme)),
			) => {
				return Task::done(ModuleMessage::SelectTheme(theme));
			}
			(Self::SystemMenu(_), ModuleMessage::SystemMenu(system_menu::Message::Dismiss)) => {
				return Task::done(ModuleMessage::ClosePopup);
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
				menu.open_context_menu(context_menu, depth, bounds);
				return Task::done(ModuleMessage::OpenContextMenu(bounds));
			}
			(Self::Taskbar(taskbar), ModuleMessage::TaskbarInitialized(result)) => match result {
				Ok(initialized) => *taskbar = Some(initialized),
				Err(error) => log::warn!("Failed to initialize Niri taskbar: {error}"),
			},
			(
				Self::Bluetooth(_),
				ModuleMessage::Bluetooth(bluetooth::integration::Message::OpenPopup(bounds)),
			) => {
				return Task::done(ModuleMessage::OpenPopup(ModuleKind::Bluetooth, bounds));
			}
			(Self::Bluetooth(bluetooth), ModuleMessage::Bluetooth(message)) => {
				return bluetooth.update(message).map(ModuleMessage::Bluetooth);
			}
			(Self::Volume(Ok(volume)), ModuleMessage::Volume(message)) => {
				return volume.update(message).map(ModuleMessage::Volume);
			}
			(
				Self::Network(_),
				ModuleMessage::Network(network::integration::Message::OpenPopup(bounds)),
			) => {
				return Task::done(ModuleMessage::OpenPopup(ModuleKind::Network, bounds));
			}
			(Self::Network(network), ModuleMessage::Network(message)) => {
				return network.update(message).map(ModuleMessage::Network);
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
				let closes_context_menus = message.closes_context_menus();
				if closes_context_menus {
					menu.close_context_menus();
				}

				let update = menu.update(message).map(ModuleMessage::from_system_menu);

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
			Self::Bluetooth(bluetooth) => bluetooth.subscription().map(ModuleMessage::Bluetooth),
			Self::Volume(Ok(volume)) => volume.subscription().map(ModuleMessage::Volume),
			Self::Clock(clock) => clock.subscription().map(ModuleMessage::Clock),
			Self::MediaControls(controls) => {
				controls.subscription().map(ModuleMessage::MediaControls)
			}
			Self::Taskbar(Some(_)) => taskbar::Taskbar::subscription().map(ModuleMessage::Taskbar),
			Self::SystemMenu(menu) => menu.subscription().map(ModuleMessage::from_system_menu),
			Self::Network(network) => network.subscription().map(ModuleMessage::Network),
			_ => Subscription::none(),
		}
	}

	pub fn view(&self, output_name: Option<&str>, colors: Colors) -> Element<'_, ModuleMessage> {
		match self {
			Self::SystemMenu(menu) => menu
				.view(colors)
				.map(ModuleMessage::SystemMenu)
				.on_press_with_bounds(|bounds| {
					ModuleMessage::Pressed(ModuleKind::SystemMenu, bounds)
				})
				.into(),
			Self::MediaControls(controls) => controls
				.view(colors)
				.map(ModuleMessage::MediaControls)
				.on_press_with_bounds(|bounds| {
					ModuleMessage::Pressed(ModuleKind::MediaControls, bounds)
				})
				.into(),
			Self::Volume(Ok(volume)) => volume
				.view(colors)
				.map(ModuleMessage::Volume)
				.on_press_with_bounds(|bounds| ModuleMessage::Pressed(ModuleKind::Volume, bounds))
				.into(),
			// TODO
			Self::Volume(Err(_e)) => loading(colors.decorative.yellow, colors),
			Self::Network(network) => network.view(colors).map(ModuleMessage::Network),
			Self::Bluetooth(bluetooth) => bluetooth.view(colors).map(ModuleMessage::Bluetooth),
			Self::Clock(clock) => clock
				.view(colors)
				.map(ModuleMessage::Clock)
				.on_press_with_bounds(|bounds| ModuleMessage::Pressed(ModuleKind::Clock, bounds))
				.into(),
			Self::Taskbar(None) => loading(colors.surface, colors),
			Self::Taskbar(Some(taskbar)) => taskbar
				.view(output_name, colors)
				.map(ModuleMessage::Taskbar),
		}
	}

	pub fn view_popup(&self, colors: Colors) -> Element<'_, ModuleMessage> {
		match self {
			Self::MediaControls(controls) => controls
				.view_popup(colors)
				.map(ModuleMessage::MediaControls),
			Self::SystemMenu(menu) => menu.view_popup(colors).map(ModuleMessage::SystemMenu),
			Self::Volume(Ok(vol)) => vol.view_popup(colors).map(ModuleMessage::Volume),
			Self::Bluetooth(bt) => bt.view_popup(colors).map(ModuleMessage::Bluetooth),
			Self::Clock(clock) => clock.view_popup(colors).map(ModuleMessage::Clock),
			Self::Network(network) => network.view_popup(colors).map(ModuleMessage::Network),
			_ => neo_card(text("No popup for module").color(colors.on_accent))
				.with_colors(colors)
				.background(colors.feedback.danger)
				.into(),
		}
	}

	pub fn view_context_menu(&self, depth: usize, colors: Colors) -> Element<'_, ModuleMessage> {
		match self {
			Self::Clock(clock) => clock
				.view_context_menu(depth, colors)
				.map(ModuleMessage::Clock),
			Self::SystemMenu(menu) => menu
				.view_context_menu(depth, colors)
				.map(ModuleMessage::SystemMenu),
			_ => neo_card(text("Unimplemented context menu").color(colors.text))
				.with_colors(colors)
				.padding(8)
				.background(colors.surface)
				.into(),
		}
	}
}

pub fn loading<'a, Message: Clone + 'a>(background: Color, colors: Colors) -> Element<'a, Message> {
	neo_button(spinner().bar_color(colors.text_on(background)))
		.with_colors(colors)
		.height(MODULE_HEIGHT)
		.width(MODULE_HEIGHT)
		.background(background)
		.radius(MODULE_RADIUS)
		.into()
}

fn unavailable<'a, Message: Clone + 'a>(
	label: &'a str, error: &'a str, background: Color, colors: Colors,
) -> Element<'a, Message> {
	iced::widget::tooltip(
		neo_button(module_label(text(label).color(colors.on_accent)))
			.with_colors(colors)
			.height(MODULE_HEIGHT)
			.background(background)
			.radius(MODULE_RADIUS),
		text(format!("{error}\nRetrying automatically")).color(colors.text),
		iced::widget::tooltip::Position::Bottom,
	)
	.into()
}

fn connection_popup<'a, Message: 'a>(
	label: &'a str, detail: &'a str, colors: Colors,
) -> Element<'a, Message> {
	neo_card(
		iced::widget::column![
			text(label)
				.size(20)
				.color(colors.text)
				.weight(iced::font::Weight::Bold),
			text(detail).size(14).color(colors.secondary),
		]
		.spacing(16),
	)
	.with_colors(colors)
	.width(320)
	.radius(MODULE_RADIUS)
	.background(colors.body)
	.into()
}

#[cfg(test)]
mod tests {
	use super::connection::{State as ServiceState, initialize};
	use super::{Module, ModuleMessage};

	#[test]
	fn bar_module_heights_match_in_both_themes_and_service_states() {
		for colors in [neo_widgets::style::LIGHT, neo_widgets::style::DARK] {
			let modules = [
				Module::system_menu(&config::ConfigFile::default()),
				Module::clock(),
				Module::media_controls(),
				Module::network(),
				Module::bluetooth(),
				Module::Volume(Err("Unavailable".into())),
				Module::Taskbar(None),
			];

			for module in modules {
				assert_eq!(
					module.view(None, colors).as_widget().size().height,
					iced::Length::Fixed(super::MODULE_HEIGHT)
				);
			}
			let unavailable: iced::Element<'_, ()> =
				super::unavailable("Unavailable", "Offline", colors.decorative.blue, colors);
			assert_eq!(
				unavailable.as_widget().size().height,
				iced::Length::Fixed(super::MODULE_HEIGHT)
			);
		}
	}

	#[tokio::test]
	#[ignore = "requires a headless graphics renderer"]
	async fn bar_module_layouts_match_visible_faces() -> Result<(), super::layout_tests::Error> {
		let mut renderer = super::layout_tests::renderer().await?;
		for (theme, colors) in [
			("light", neo_widgets::style::LIGHT),
			("dark", neo_widgets::style::DARK),
		] {
			for (name, module) in [
				("menu", Module::system_menu(&config::ConfigFile::default())),
				("clock", Module::clock()),
				("media", Module::media_controls()),
				("network-loading", Module::network()),
				("bluetooth-loading", Module::bluetooth()),
				("audio-unavailable", Module::Volume(Err("Offline".into()))),
			] {
				let snapshot = super::layout_tests::capture(
					module.view(None, colors),
					&mut renderer,
					colors,
					&format!("bar-{name}-{theme}"),
				)?;
				assert_eq!(
					snapshot.nodes.first().ok_or("missing root")?.height,
					super::MODULE_HEIGHT
				);
				for bounds in snapshot.texts.values() {
					// The face excludes the 4px shadow. The font's optical correction
					// moves its line-box center 1px below the face's geometric center.
					assert!((bounds.center_y() - 23.0).abs() < 0.01);
				}
			}
		}
		Ok(())
	}

	#[tokio::test]
	async fn theme_selection_is_forwarded_to_parent() -> Result<(), super::test_bus::Error> {
		let mut module = Module::system_menu(&config::ConfigFile::default());
		let theme = config::AppearanceMode::Dark;

		let task = module.update(ModuleMessage::SystemMenu(
			super::system_menu::Message::SelectTheme(theme),
		));
		let message = super::test_bus::task_message(task).await?;

		assert!(matches!(message, ModuleMessage::SelectTheme(selected) if selected == theme));
		Ok(())
	}

	#[test]
	fn emitted_theme_selection_is_forwarded_to_parent() {
		let theme = config::AppearanceMode::Dark;

		let message =
			ModuleMessage::from_system_menu(super::system_menu::Message::SelectTheme(theme));

		assert!(matches!(message, ModuleMessage::SelectTheme(selected) if selected == theme));
	}

	#[tokio::test]
	async fn layout_changes_save_only_accepted_changed_edits() -> Result<(), super::test_bus::Error>
	{
		use super::system_menu::Message;
		use config::SystemMenuWidgets::{Bluetooth, Wifi};
		let mut config = config::ConfigFile::default();
		config.system_menu.widgets = vec![Wifi, Bluetooth];
		let mut module = Module::system_menu(&config);
		let rows = |widget| config::SystemMenu::from_widgets(vec![widget]).effective_rows();
		let edit = || ModuleMessage::SystemMenu(Message::RowsEdited(rows(Bluetooth)));
		assert_eq!(module.update(edit()).units(), 0);
		let _ = module.update(ModuleMessage::SystemMenu(Message::EditingChanged(true)));
		let task = module.update(edit());
		let message = super::test_bus::task_message(task).await?;
		assert!(
			matches!(message, ModuleMessage::SaveSystemMenuLayout(layout) if layout.effective_rows() == rows(Bluetooth))
		);
		assert_eq!(module.update(edit()).units(), 0);
		let _ = module.update(ModuleMessage::PopupClosed);
		assert_eq!(
			module
				.update(ModuleMessage::SystemMenu(Message::RowsEdited(rows(Wifi))))
				.units(),
			0
		);
		Ok(())
	}

	#[test]
	fn network_startup_failure_schedules_recovery() {
		let mut module = Module::network();
		let task = module.update(ModuleMessage::Network(
			super::network::integration::Message::Initialized(Err("unavailable".into())),
		));
		assert!(
			task.units() > 0,
			"network initialization failure must retry"
		);
		assert!(matches!(module, Module::Network(_)));
		assert_eq!(module.subscription().units(), 0);
	}

	#[test]
	fn bluetooth_startup_failure_schedules_recovery() {
		let mut module = Module::bluetooth();
		let task = module.update(ModuleMessage::Bluetooth(
			super::bluetooth::integration::Message::Initialized(Err("unavailable".into())),
		));
		assert!(
			task.units() > 0,
			"Bluetooth initialization failure must retry"
		);
		assert!(matches!(module, Module::Bluetooth(_)));
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
		let result = initialize::<()>(std::time::Duration::ZERO, std::future::pending()).await;
		assert!(result.is_err());
	}

	#[tokio::test(start_paused = true)]
	async fn retry_delay_precedes_connection_attempt() {
		let start = tokio::time::Instant::now();
		let result = initialize(std::time::Duration::from_secs(4), async {
			assert!(start.elapsed() >= std::time::Duration::from_secs(4));
			Ok(())
		})
		.await;
		assert!(result.is_ok());
	}
}
