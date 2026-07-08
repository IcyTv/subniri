use std::time::Instant;

use config::ConfigFile;
use daemon::{NightlightPreset, NightlightProxy};
use futures::StreamExt;
use iced::{Animation, Length, Subscription, Task};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_toggle_button},
};

#[derive(Debug, Clone)]
pub enum Message {
	EnabledChanged(bool),
	PresetChanged(NightlightPreset),
	TemperatureChanged(u32),
	BrightnessChanged(f64),

	Toggle,

	Noop,
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

	fn subtitle(&self) -> String {
		match self.preset {
			NightlightPreset::Day | NightlightPreset::Night => self.preset.as_str().to_string(),
			NightlightPreset::Custom => format!("{:.02} - {} K", self.brightness, self.temperature),
		}
	}
}
