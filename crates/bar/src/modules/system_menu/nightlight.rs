use std::time::Instant;

use config::ConfigFile;
use daemon::{NightlightPreset, NightlightProxy};
use futures::StreamExt;
use iced::{
	Alignment, Animation, Element, Length, Padding, Rectangle, Subscription, Task,
	widget::{column, row, space, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{
		NeoButton, NeoButtonStyle, NeoContentSurfaceStyle, NeoSurfaceStyle, neo_button, neo_card,
		neo_toggle_button,
	},
};

#[derive(Debug, Clone, Copy)]
pub enum Message {
	EnabledChanged(bool),
	PresetChanged(NightlightPreset),
	TemperatureChanged(u32),
	BrightnessChanged(f64),

	Toggle,
	Suspend(u64),
	SetPreset(NightlightPreset),
	OpenContextMenu(ContextMenu, Rectangle),

	Noop,
}

#[derive(Debug, Clone, Copy)]
pub enum ContextMenu {
	Root,
	Suspend,
	Preset,
}

#[derive(Debug, Clone)]
pub struct Nightlight {
	pub enabled: Animation<bool>,
	pub preset: NightlightPreset,
	pub temperature: u32,
	pub brightness: f64,
}

impl Nightlight {
	pub fn new(config: &ConfigFile) -> Self {
		Self {
			enabled: Animation::new(config.nightlight.enabled).very_quick(),
			preset: NightlightPreset::Day,
			temperature: config.nightlight.day.temperature,
			brightness: config.nightlight.day.brightness,
		}
	}

	pub fn subscription() -> Subscription<Message> {
		Subscription::run(|| {
			futures::stream::once(async move {
				let connection = match zbus::Connection::session().await {
					Ok(connection) => connection,
					Err(e) => {
						log::warn!("Failed to connect to D-Bus session bus: {}", e);
						return futures::stream::once(async { Message::Noop }).boxed();
					}
				};
				let proxy = match NightlightProxy::new(&connection).await {
					Ok(proxy) => proxy,
					Err(e) => {
						log::warn!("Failed to create nightlight proxy: {}", e);
						return futures::stream::once(async { Message::Noop }).boxed();
					}
				};

				let enabled_changed =
					proxy
						.receive_enabled_changed()
						.await
						.then(|enabled| async move {
							match enabled.get().await {
								Ok(value) => Message::EnabledChanged(value),
								Err(e) => {
									log::warn!("Failed to receive enabled_changed signal: {}", e);
									Message::Noop
								}
							}
						});
				let preset_changed =
					proxy
						.receive_preset_changed()
						.await
						.then(|preset| async move {
							match preset.get().await {
								Ok(value) => Message::PresetChanged(
									NightlightPreset::parse(&value)
										.unwrap_or(NightlightPreset::Day),
								),
								Err(e) => {
									log::warn!("Failed to receive preset_changed signal: {}", e);
									Message::Noop
								}
							}
						});
				let temperature_changed =
					proxy
						.receive_temperature_changed()
						.await
						.then(|temperature| async move {
							match temperature.get().await {
								Ok(value) => Message::TemperatureChanged(value),
								Err(e) => {
									log::warn!(
										"Failed to receive temperature_changed signal: {}",
										e
									);
									Message::Noop
								}
							}
						});
				let brightness_changed =
					proxy
						.receive_brightness_changed()
						.await
						.then(|brightness| async move {
							match brightness.get().await {
								Ok(value) => Message::BrightnessChanged(value),
								Err(e) => {
									log::warn!(
										"Failed to receive brightness_changed signal: {}",
										e
									);
									Message::Noop
								}
							}
						});

				futures::stream::select_all(vec![
					enabled_changed.boxed(),
					preset_changed.boxed(),
					temperature_changed.boxed(),
					brightness_changed.boxed(),
				])
				.boxed()
			})
			.flatten()
		})
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::EnabledChanged(enabled) => self.enabled.go_mut(enabled, Instant::now()),
			Message::PresetChanged(preset) => self.preset = preset,
			Message::TemperatureChanged(temperature) => self.temperature = temperature,
			Message::BrightnessChanged(brightness) => self.brightness = brightness,
			Message::Toggle => {
				let old_enabled = self.enabled.value();
				let enabled = !old_enabled;
				self.enabled.go_mut(enabled, Instant::now());
				return Task::future(async move {
					match set_enabled(enabled).await {
						Ok(()) => Message::Noop,
						Err(error) => {
							log::warn!("Failed to set nightlight enabled state: {error}");
							Message::EnabledChanged(old_enabled)
						}
					}
				});
			}
			Message::Suspend(duration_secs) => {
				return Task::future(async move {
					if let Err(error) = suspend(duration_secs).await {
						log::warn!("Failed to suspend nightlight: {error}");
					}
					Message::Noop
				});
			}
			Message::SetPreset(preset) => {
				return Task::future(async move {
					if let Err(error) = set_preset(preset).await {
						log::warn!("Failed to set nightlight preset: {error}");
					}
					Message::Noop
				});
			}
			_ => (),
		}

		Task::none()
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		let icon_color =
			self.enabled
				.interpolate(COLORS.white, COLORS.decorative.green, Instant::now());
		let background =
			self.enabled
				.interpolate(COLORS.white, COLORS.decorative.green90, Instant::now());

		neo_toggle_button(
			phosphor_icon!("moon"),
			"Nightlight",
			self.subtitle(),
			self.enabled.value(),
			Some(icon_color),
		)
		.background(background)
		.on_press(Message::Toggle)
		.width(Length::Fill)
		.height(64)
	}

	pub fn view_context_menu(&self, menu: ContextMenu) -> Element<'_, Message> {
		let content = match menu {
			ContextMenu::Root => column![
				menu_action("Enabled", self.enabled.value(), Message::Toggle),
				menu_submenu("Suspend", ContextMenu::Suspend),
				menu_submenu("Preset", ContextMenu::Preset),
			],
			ContextMenu::Suspend => column![
				menu_action("15 minutes", false, Message::Suspend(15 * 60)),
				menu_action("30 minutes", false, Message::Suspend(30 * 60)),
				menu_action("1 hour", false, Message::Suspend(60 * 60)),
				menu_action("2 hours", false, Message::Suspend(2 * 60 * 60)),
			],
			ContextMenu::Preset => column![
				menu_preset("Day", NightlightPreset::Day, self.preset),
				menu_preset("Night", NightlightPreset::Night, self.preset),
			],
		};

		neo_card(content.spacing(2))
			.padding(5)
			.radius(8.0)
			.background(COLORS.white)
			.width(180.0)
			.into()
	}

	fn subtitle(&self) -> String {
		match self.preset {
			NightlightPreset::Day | NightlightPreset::Night => self.preset.as_str().to_string(),
			NightlightPreset::Custom => format!("{:.02} - {} K", self.brightness, self.temperature),
		}
	}
}

fn menu_action(label: &str, checked: bool, message: Message) -> Element<'_, Message> {
	let indicator = if checked {
		svg(phosphor_icon!("check", "bold")).width(16).height(16)
	} else {
		svg(phosphor_icon!("check", "bold")).width(0).height(16)
	};

	neo_button(
		row![indicator, text(label).width(Length::Fill)]
			.spacing(7)
			.align_y(Alignment::Center),
	)
	.style(menu_button_style())
	.width(Length::Fill)
	.height(36)
	.on_press(message)
	.into()
}

fn menu_submenu(label: &str, menu: ContextMenu) -> Element<'_, Message> {
	neo_button(
		row![
			space().width(16).height(16),
			text(label).width(Length::Fill),
			svg(phosphor_icon!("caret-right", "bold"))
				.width(14)
				.height(14),
		]
		.spacing(7)
		.align_y(Alignment::Center),
	)
	.style(menu_button_style())
	.width(Length::Fill)
	.height(36)
	.on_press_started_with_bounds(move |bounds| Message::OpenContextMenu(menu, bounds))
	.into()
}

fn menu_preset(
	label: &str, preset: NightlightPreset, selected: NightlightPreset,
) -> Element<'_, Message> {
	let icon = if preset == selected {
		phosphor_icon!("circle", "fill")
	} else {
		phosphor_icon!("circle", "bold")
	};
	neo_button(
		row![
			svg(icon).width(16).height(16),
			text(label).width(Length::Fill),
		]
		.spacing(7)
		.align_y(Alignment::Center),
	)
	.style(menu_button_style())
	.width(Length::Fill)
	.height(36)
	.on_press(Message::SetPreset(preset))
	.into()
}

async fn set_enabled(enabled: bool) -> zbus::Result<()> {
	let connection = zbus::Connection::session().await?;
	let proxy = NightlightProxy::new(&connection).await?;
	proxy.set_enabled(enabled).await
}

async fn suspend(duration_secs: u64) -> zbus::Result<()> {
	let connection = zbus::Connection::session().await?;
	let proxy = NightlightProxy::new(&connection).await?;
	proxy.suspend(duration_secs).await
}

async fn set_preset(preset: NightlightPreset) -> zbus::Result<()> {
	let connection = zbus::Connection::session().await?;
	let proxy = NightlightProxy::new(&connection).await?;
	proxy.set_preset(preset.as_str()).await
}

fn menu_button_style() -> NeoButtonStyle {
	let surface = NeoSurfaceStyle {
		background: COLORS.white,
		border_width: 0.0,
		shadow_width: 0.0,
		radius: 4.0,
		..Default::default()
	};
	NeoButtonStyle {
		surface: NeoContentSurfaceStyle {
			surface,
			padding: Padding::from([6, 8]),
		},
		hovered: NeoSurfaceStyle {
			background: COLORS.decorative.pink90,
			..surface
		},
		focused: NeoContentSurfaceStyle {
			surface: NeoSurfaceStyle {
				background: COLORS.decorative.pink90,
				..surface
			},
			padding: Padding::from([6, 8]),
		},
		disabled_background: COLORS.white,
	}
}
