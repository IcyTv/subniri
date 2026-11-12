use std::time::Instant;

use config::ConfigFile;
use daemon_common::{NightlightPreset, NightlightProxy};
use futures::StreamExt;
use iced::{
	Alignment, Animation, Element, Length, Padding, Rectangle, Subscription, Task,
	widget::{column, row, space, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::Colors,
	widgets::{
		NeoButton, NeoButtonStyle, NeoContentSurfaceStyle, NeoSurfaceStyle, neo_button, neo_card,
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
	SetBrightness(f64),
	SetTemperature(u32),
	BrightnessWritten(bool),
	TemperatureWritten(bool),
	OpenContextMenu(ContextMenu, Rectangle),

	Noop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenu {
	Root,
	Suspend,
	Preset,
	Brightness,
	Temperature,
}

#[derive(Clone, Copy, Debug)]
pub struct PointerPlacement {
	right: bool,
	y: f32,
}

impl PointerPlacement {
	pub fn from_geometry(anchor: Rectangle, popup: Rectangle) -> Self {
		Self {
			right: popup.center_x() < anchor.center_x(),
			y: anchor.center_y() - popup.y,
		}
	}
}

#[derive(Debug, Clone)]
pub struct Nightlight {
	pub enabled: Animation<bool>,
	pub preset: NightlightPreset,
	pub temperature: u32,
	pub brightness: f64,
	brightness_write: utilities::LatestWrite<f64>,
	temperature_write: utilities::LatestWrite<u32>,
}

impl Nightlight {
	pub fn new(config: &ConfigFile) -> Self {
		Self {
			enabled: Animation::new(config.nightlight.enabled).very_quick(),
			preset: NightlightPreset::Day,
			temperature: config.nightlight.day.temperature,
			brightness: config.nightlight.day.brightness,
			brightness_write: utilities::LatestWrite::new(config.nightlight.day.brightness),
			temperature_write: utilities::LatestWrite::new(config.nightlight.day.temperature),
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
			Message::TemperatureChanged(temperature) => {
				self.temperature_write.observe(temperature);
				self.temperature = self.temperature_write.value();
			}
			Message::BrightnessChanged(brightness) => {
				self.brightness_write.observe(brightness);
				self.brightness = self.brightness_write.value();
			}
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
			Message::SetBrightness(value) => {
				if !self.enabled.value() {
					return Task::none();
				}
				let next = self.brightness_write.request(value);
				self.brightness = self.brightness_write.value();
				return next.map_or_else(Task::none, write_brightness);
			}
			Message::SetTemperature(value) => {
				if !self.enabled.value() {
					return Task::none();
				}
				let next = self.temperature_write.request(value);
				self.temperature = self.temperature_write.value();
				return next.map_or_else(Task::none, write_temperature);
			}
			Message::BrightnessWritten(success) => {
				let next = self.brightness_write.complete(success);
				self.brightness = self.brightness_write.value();
				return next.map_or_else(Task::none, write_brightness);
			}
			Message::TemperatureWritten(success) => {
				let next = self.temperature_write.complete(success);
				self.temperature = self.temperature_write.value();
				return next.map_or_else(Task::none, write_temperature);
			}
			_ => (),
		}

		Task::none()
	}

	pub fn view_tile(&self, size: iced::Size, colors: Colors) -> NeoButton<'_, Message> {
		let now = Instant::now();

		neo_widgets::widgets::neo_tile_with_colors(
			colors,
			phosphor_icon!("moon"),
			"Nightlight",
			self.subtitle(),
			Some(self.enabled.value()),
			Some(
				self.enabled
					.interpolate(colors.surface, colors.decorative.green, now),
			),
			size,
		)
		.on_press(Message::Toggle)
	}

	pub fn view_expanded(&self, size: iced::Size, colors: Colors) -> Element<'_, super::Message> {
		use neo_widgets::widgets::{neo_slider, neo_toggle};

		let inset = NeoContentSurfaceStyle::with_colors(colors).inset();
		let active = self.enabled.value();
		let surface = NeoSurfaceStyle {
			background: colors.surface,
			border_width: 0.0,
			shadow_width: 0.0,
			..NeoSurfaceStyle::with_colors(colors)
		};
		let plain = NeoContentSurfaceStyle {
			surface,
			padding: 0.0.into(),
		};

		let header = neo_button(
			row![
				svg(phosphor_icon!("moon"))
					.width(24)
					.height(24)
					.style(move |_, _| svg::Style {
						color: Some(colors.text)
					}),
				text("Nightlight")
					.size(16)
					.color(colors.text)
					.width(Length::Fill),
				neo_toggle::<super::Message>()
					.with_colors(colors)
					.toggled(active)
					.width(32)
					.height(16)
			]
			.spacing(8)
			.align_y(Alignment::Center),
		)
		.with_colors(colors)
		.style(NeoButtonStyle {
			surface: plain,
			focused: plain,
			hovered: surface,
			disabled_background: colors.surface,
		})
		.on_press(super::Message::Nightlight(Message::Toggle))
		.width(Length::Fill)
		.height(28);

		let temperature = column![
			text("Temperature").size(12).color(colors.text),
			text(format!("{} K", self.temperature))
				.size(16)
				.color(colors.text),
			neo_slider(1000..=6500, self.temperature)
				.with_colors(colors)
				.on_change_live(super::Message::ChangeTemperature)
				.enabled(active)
		]
		.spacing(4)
		.width(Length::Fill);
		let brightness = column![
			text("Brightness").size(12).color(colors.text),
			text(format!("{} %", (self.brightness * 100.0).round() as u32))
				.size(16)
				.color(colors.text),
			neo_slider(0.2..=1.0, self.brightness)
				.with_colors(colors)
				.step(0.01)
				.on_change_live(super::Message::ChangeBrightness)
				.enabled(active)
		]
		.spacing(4)
		.width(Length::Fill);

		neo_card(neo_widgets::widgets::scale_to_fit(
			column![
				header,
				iced::widget::rule::horizontal(2).style(move |_| iced::widget::rule::Style {
					color: colors.separator,
					radius: 0.0.into(),
					fill_mode: iced::widget::rule::FillMode::Full,
					snap: false,
				}),
				row![temperature, brightness].spacing(16)
			]
			.spacing(6),
			iced::Size::new(
				(size.width - inset.width).max(1.0),
				(size.height - inset.height).max(1.0),
			),
		))
		.with_colors(colors)
		.background(colors.surface)
		.width(Length::Fill)
		.height(Length::Fill)
		.padding(8)
		.into()
	}

	pub fn view_context_menu(
		&self, menu: ContextMenu, child: Option<ContextMenu>, pointer: Option<PointerPlacement>,
		colors: Colors,
	) -> Element<'_, Message> {
		let content = match menu {
			ContextMenu::Root => column![
				menu_action("Enabled", self.enabled.value(), Message::Toggle, colors),
				menu_submenu("Suspend", ContextMenu::Suspend, child, colors),
				menu_submenu("Preset", ContextMenu::Preset, child, colors),
				menu_submenu("Brightness", ContextMenu::Brightness, child, colors)
					.enabled(self.enabled.value()),
				menu_submenu("Temperature", ContextMenu::Temperature, child, colors)
					.enabled(self.enabled.value()),
			],
			ContextMenu::Suspend => column![
				menu_action("15 minutes", false, Message::Suspend(15 * 60), colors),
				menu_action("30 minutes", false, Message::Suspend(30 * 60), colors),
				menu_action("1 hour", false, Message::Suspend(60 * 60), colors),
				menu_action("2 hours", false, Message::Suspend(2 * 60 * 60), colors),
			],
			ContextMenu::Preset => column![
				menu_preset("Day", NightlightPreset::Day, self.preset, colors),
				menu_preset("Night", NightlightPreset::Night, self.preset, colors),
			],
			ContextMenu::Brightness => {
				let mut content = column![];
				for percent in [20, 40, 60, 80, 100] {
					let value = f64::from(percent) / 100.0;
					content = content.push(
						menu_action(
							&format!("{percent}%"),
							(self.brightness - value).abs() < f64::EPSILON,
							Message::SetBrightness(value),
							colors,
						)
						.enabled(self.enabled.value()),
					);
				}
				content
			}
			ContextMenu::Temperature => {
				let mut content = column![];
				for value in [2000, 3000, 4000, 5000, 6500] {
					content = content.push(
						menu_action(
							&format!("{value} K"),
							self.temperature == value,
							Message::SetTemperature(value),
							colors,
						)
						.enabled(self.enabled.value()),
					);
				}
				content
			}
		};

		let card = neo_card(content.spacing(2))
			.with_colors(colors)
			.padding(5)
			.radius(8.0)
			.background(colors.surface)
			.width(220.0);
		if menu == ContextMenu::Root {
			return card.into();
		}

		let rows = match menu {
			ContextMenu::Suspend => 4,
			ContextMenu::Preset => 2,
			_ => 5,
		};
		let height = rows as f32 * 36.0 + (rows - 1) as f32 * 2.0 + 14.0;
		let right = pointer.is_some_and(|pointer| pointer.right);
		let y = pointer
			.map_or(height / 2.0, |pointer| pointer.y)
			.clamp(16.0, height - 16.0);
		let path = if right {
			"M0 1 L11 12 L0 23"
		} else {
			"M12 1 L1 12 L12 23"
		};
		let [r, g, b, _] = colors.surface.into_rgba8();
		let artwork = format!(
			"<svg xmlns='http://www.w3.org/2000/svg' width='12' height='24'><path d='{path}' fill='rgb({r},{g},{b})' stroke='black' stroke-width='2'/></svg>"
		);
		let tip: Element<'_, Message> = if pointer.is_some() {
			svg(svg::Handle::from_memory(artwork.into_bytes()))
				.width(12)
				.height(24)
				.into()
		} else {
			space().width(12).height(24).into()
		};
		let gutter = column![space().height(y - 12.0), tip]
			.width(12)
			.height(height);

		if right {
			row![card, gutter].height(height).into()
		} else {
			row![gutter, card].height(height).into()
		}
	}

	fn subtitle(&self) -> String {
		match self.preset {
			NightlightPreset::Day | NightlightPreset::Night => self.preset.as_str().to_string(),
			NightlightPreset::Custom => format!("{:.02} - {} K", self.brightness, self.temperature),
		}
	}
}

fn write_brightness(value: f64) -> Task<Message> {
	Task::future(async move {
		match super::set_brightness(value).await {
			Ok(_) => Message::BrightnessWritten(true),
			Err(error) => {
				log::warn!("Failed to set nightlight brightness: {error}");
				Message::BrightnessWritten(false)
			}
		}
	})
}

fn write_temperature(value: u32) -> Task<Message> {
	Task::future(async move {
		match super::set_temperature(value).await {
			Ok(_) => Message::TemperatureWritten(true),
			Err(error) => {
				log::warn!("Failed to set nightlight temperature: {error}");
				Message::TemperatureWritten(false)
			}
		}
	})
}

fn menu_action(
	label: &str, checked: bool, message: Message, colors: Colors,
) -> NeoButton<'static, Message> {
	let background = if checked {
		colors.decorative.green
	} else {
		colors.surface
	};
	let foreground = colors.text_on(background);
	let icon = match &message {
		Message::Toggle => phosphor_icon!("power", "bold"),
		Message::Suspend(_) => phosphor_icon!("timer", "bold"),
		Message::SetBrightness(_) => phosphor_icon!("sun", "bold"),
		Message::SetTemperature(_) => phosphor_icon!("thermometer", "bold"),
		_ => phosphor_icon!("moon", "bold"),
	};
	let indicator = svg(icon)
		.width(16)
		.height(16)
		.style(move |_, _| svg::Style {
			color: Some(foreground),
		});

	neo_button(
		row![
			indicator,
			text(label.to_owned()).color(foreground).width(Length::Fill)
		]
		.spacing(7)
		.align_y(Alignment::Center),
	)
	.with_colors(colors)
	.style(menu_button_style(colors))
	.background(background)
	.width(Length::Fill)
	.height(36)
	.on_press(message)
}

fn menu_submenu(
	label: &str, menu: ContextMenu, child: Option<ContextMenu>, colors: Colors,
) -> NeoButton<'static, Message> {
	let icon = match menu {
		ContextMenu::Suspend => phosphor_icon!("timer", "bold"),
		ContextMenu::Preset => phosphor_icon!("sun-horizon", "bold"),
		ContextMenu::Brightness => phosphor_icon!("sun", "bold"),
		ContextMenu::Temperature => phosphor_icon!("thermometer", "bold"),
		ContextMenu::Root => phosphor_icon!("moon", "bold"),
	};

	neo_button(
		row![
			svg(icon)
				.width(16)
				.height(16)
				.style(move |_, _| svg::Style {
					color: Some(colors.text)
				}),
			text(label.to_owned())
				.color(colors.text)
				.width(Length::Fill),
			svg(phosphor_icon!("caret-right", "bold"))
				.width(14)
				.height(14)
				.style(move |_, _| svg::Style {
					color: Some(colors.text)
				}),
		]
		.spacing(7)
		.align_y(Alignment::Center),
	)
	.with_colors(colors)
	.style(menu_button_style(colors))
	.background(if child == Some(menu) {
		colors.raised
	} else {
		colors.surface
	})
	.width(Length::Fill)
	.height(36)
	.on_press_started_with_bounds(move |bounds| Message::OpenContextMenu(menu, bounds))
}

fn menu_preset(
	label: &str, preset: NightlightPreset, selected: NightlightPreset, colors: Colors,
) -> Element<'_, Message> {
	let icon = if preset == NightlightPreset::Day {
		phosphor_icon!("sun", "bold")
	} else {
		phosphor_icon!("moon", "bold")
	};

	neo_button(
		row![
			svg(icon)
				.width(16)
				.height(16)
				.style(move |_, _| svg::Style {
					color: Some(colors.text)
				}),
			text(label).color(colors.text).width(Length::Fill),
		]
		.spacing(7)
		.align_y(Alignment::Center),
	)
	.with_colors(colors)
	.style(menu_button_style(colors))
	.background(if preset == selected {
		colors.raised
	} else {
		colors.surface
	})
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

async fn suspend(duration_secs: u64) -> zbus::fdo::Result<()> {
	let connection = zbus::Connection::session().await?;
	let proxy = NightlightProxy::new(&connection).await?;
	proxy.suspend(duration_secs).await
}

async fn set_preset(preset: NightlightPreset) -> zbus::Result<()> {
	let connection = zbus::Connection::session().await?;
	let proxy = NightlightProxy::new(&connection).await?;
	proxy.set_preset(preset.as_str()).await
}

fn menu_button_style(colors: Colors) -> NeoButtonStyle {
	let surface = NeoSurfaceStyle {
		background: colors.surface,
		border_width: 0.0,
		shadow_width: 0.0,
		radius: 4.0,
		..NeoSurfaceStyle::with_colors(colors)
	};

	NeoButtonStyle {
		surface: NeoContentSurfaceStyle {
			surface,
			padding: Padding::from([6, 8]),
		},
		hovered: NeoSurfaceStyle {
			background: colors.raised,
			..surface
		},
		focused: NeoContentSurfaceStyle {
			surface: NeoSurfaceStyle {
				background: colors.raised,
				..surface
			},
			padding: Padding::from([6, 8]),
		},
		disabled_background: colors.surface,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn submenu_pointer_tracks_compositor_flip_and_vertical_slide() {
		let anchor = Rectangle {
			x: 10.0,
			y: 38.0,
			width: 200.0,
			height: 36.0,
		};
		let right_popup = Rectangle {
			x: 210.0,
			y: 20.0,
			width: 232.0,
			height: 164.0,
		};

		let pointer = PointerPlacement::from_geometry(anchor, right_popup);
		assert!(!pointer.right);
		assert_eq!(pointer.y, 36.0);

		let flipped = PointerPlacement::from_geometry(
			anchor,
			Rectangle {
				x: -222.0,
				y: -20.0,
				..right_popup
			},
		);
		assert!(flipped.right);
		assert_eq!(flipped.y, 76.0);
	}

	#[test]
	fn live_values_coalesce_and_old_signals_do_not_undo_the_preview() {
		let mut light = Nightlight::new(&ConfigFile::default());
		let _ = light.update(Message::EnabledChanged(true));
		assert!(light.update(Message::SetBrightness(0.4)).units() > 0);
		assert_eq!(light.update(Message::SetBrightness(0.6)).units(), 0);
		assert_eq!(light.update(Message::SetBrightness(0.8)).units(), 0);
		let _ = light.update(Message::BrightnessChanged(0.4));
		assert_eq!(light.brightness, 0.8);
		assert!(light.update(Message::BrightnessWritten(true)).units() > 0);
		assert_eq!(light.brightness, 0.8);
		assert_eq!(light.update(Message::BrightnessWritten(false)).units(), 0);
		assert_eq!(light.brightness, 0.4);
	}
}
