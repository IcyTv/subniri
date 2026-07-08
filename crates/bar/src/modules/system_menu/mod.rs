use std::time::{Duration, Instant};

use config::{ConfigFile, SystemMenuWidgets};
use daemon::NightlightProxy;
use iced::{
	Element, Font, Length, Padding, Rectangle, Subscription, Task,
	alignment::Vertical,
	font,
	widget::{column, container, grid, image, row, stack, svg, text},
};
use jiff::{
	SpanRound, Unit,
	fmt::friendly::{Designator, Spacing, SpanPrinter},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button, neo_card, neo_slider, neo_toggle_button},
};
use nix::unistd::{Uid, User};
use strum::VariantArray;
use zbus::Connection;

use crate::modules::{MODULE_HEIGHT, MODULE_RADIUS};

mod bluetooth;
mod nightlight;
mod wifi;

#[derive(Debug, Clone)]
pub enum Message {
	UptimeUpdated(jiff::Span),

	Wifi(wifi::Message),
	Bluetooth(bluetooth::Message),
	Nightlight(nightlight::Message),

	OpenPowerMenu,
	OpenSettings,

	NightlightEnabled(bool),
	BrightnessChanged(f64),
	TemperatureChanged(u32),

	ChangeBrightness(f64),
	ChangeTemperature(u32),

	EditingChanged(bool),
	OpenNightlightContextMenu(Rectangle),
	AddWidget(SystemMenuWidgets),
	RemoveWidget(SystemMenuWidgets),

	Noop,
}

#[derive(Debug, Clone)]
pub struct SystemMenu {
	widgets: Vec<SystemMenuWidgets>,
	username: String,
	avatar: image::Handle,
	uptime: jiff::Span,

	wifi: Box<wifi::Wifi>,
	bluetooth: Box<bluetooth::Bluetooth>,
	nightlight: Box<nightlight::Nightlight>,

	editing: bool,
}

impl SystemMenu {
	pub fn new(config: &ConfigFile) -> Self {
		let username = User::from_uid(Uid::effective())
			.ok()
			.flatten()
			.map_or_else(|| "<unknown>".to_string(), |u| u.name);
		let avatar = image::Handle::from_bytes(
			include_bytes!("../../../../../assets/avatar.gif").as_slice(),
		);
		let uptime = uptime().unwrap_or_default();

		Self {
			widgets: vec![],
			username,
			avatar,
			uptime,

			wifi: Box::new(wifi::Wifi::new()),
			bluetooth: Box::new(bluetooth::Bluetooth::new()),
			nightlight: Box::new(nightlight::Nightlight::new(config)),

			editing: false,
		}
	}

	pub fn init(&self) -> Task<Message> {
		Task::batch([
			self.wifi.init().map(Message::Wifi),
			self.bluetooth.init().map(Message::Bluetooth),
		])
	}

	pub fn subscription() -> Subscription<Message> {
		Subscription::batch([
			iced::time::repeat(
				|| async move {
					if let Ok(uptime) = uptime() {
						Message::UptimeUpdated(uptime)
					} else {
						Message::Noop
					}
				},
				Duration::from_mins(1),
			),
			wifi::Wifi::subscription().map(Message::Wifi),
			bluetooth::Bluetooth::subscription().map(Message::Bluetooth),
			nightlight::Nightlight::subscription().map(Message::Nightlight),
		])
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		if matches!(
			message,
			Message::NightlightEnabled(_)
				| Message::TemperatureChanged(_)
				| Message::BrightnessChanged(_)
		) {
			log::trace!("{message:?}");
		}

		match message {
			Message::UptimeUpdated(uptime) => self.uptime = uptime,
			Message::Wifi(message) => return self.wifi.update(message).map(Message::Wifi),
			Message::Bluetooth(message) => {
				return self.bluetooth.update(message).map(Message::Bluetooth);
			}
			Message::Nightlight(message) => {
				return self.nightlight.update(message).map(Message::Nightlight);
			}

			Message::NightlightEnabled(enabled) => {
				self.nightlight.enabled.go_mut(enabled, Instant::now())
			}
			Message::TemperatureChanged(temp) => self.nightlight.temperature = temp,
			Message::BrightnessChanged(brightness) => self.nightlight.brightness = brightness,

			Message::ChangeBrightness(brightness) => {
				// Optimistic Update
				let old_brightness = self.nightlight.brightness;
				self.nightlight.brightness = brightness;
				return Task::future(async move {
					match set_brightness(brightness).await {
						Ok(brightness) => Message::BrightnessChanged(brightness),
						Err(error) => {
							log::warn!("Failed to set nightlight brightness: {error}");
							Message::BrightnessChanged(old_brightness)
						}
					}
				});
			}
			Message::ChangeTemperature(temp) => {
				let old_temp = self.nightlight.temperature;
				self.nightlight.temperature = temp;
				return Task::future(async move {
					match set_temperature(temp).await {
						Ok(temp) => Message::TemperatureChanged(temp),
						Err(error) => {
							log::warn!("Failed to set nightlight temperature: {error}");
							Message::TemperatureChanged(old_temp)
						}
					}
				});
			}

			Message::EditingChanged(editing) => self.editing = editing,
			Message::AddWidget(widget) => self.widgets.push(widget),
			Message::RemoveWidget(widget) => self.widgets.retain(|w| *w != widget),
			_ => (),
		}

		Task::none()
	}

	pub fn sync_config(&mut self, config: &ConfigFile) {
		if !self.editing {
			self.sync_widgets(&config.system_menu.widgets);
		}
	}

	pub fn popup_closed(&mut self) {
		self.editing = false;
	}

	fn sync_widgets(&mut self, widgets: &[SystemMenuWidgets]) {
		if self.widgets.as_slice() != widgets {
			self.widgets.clear();
			self.widgets.extend_from_slice(widgets);
		}
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		let _ = self;
		neo_button(svg(phosphor_icon!("squares-four", "bold")).width(Length::Shrink))
			.width(48.0)
			.height(MODULE_HEIGHT)
			.background(COLORS.decorative.blue)
			.radius(MODULE_RADIUS)
	}

	#[allow(clippy::too_many_lines)]
	pub fn view_popup(&self) -> Element<'_, Message> {
		let mut content = column![].spacing(8);

		let user_card = row![
			neo_card(image(&self.avatar).width(50.0).height(50.0))
				.padding(2)
				.background(COLORS.decorative.pink),
			column![
				text(&self.username).color(COLORS.text).font(Font {
					weight: font::Weight::Bold,
					..Default::default()
				}),
				text(format!(
					"up {}",
					UPTIME_PRINTER.span_to_string(&self.uptime)
				))
				.color(COLORS.text)
				.size(12)
			]
			.width(Length::Fill),
			neo_button(svg(phosphor_icon!("power")))
				.width(32)
				.height(32)
				.padding(6)
				.on_press(Message::OpenPowerMenu),
			neo_button(svg(phosphor_icon!("gear")))
				.width(32)
				.height(32)
				.padding(6)
				.on_press(Message::OpenSettings),
			neo_button(svg(phosphor_icon!("pencil")))
				.width(32)
				.height(32)
				.padding(6)
				.background(if self.editing {
					COLORS.decorative.blue
				} else {
					COLORS.white
				})
				.on_press(Message::EditingChanged(!self.editing)),
		]
		.width(Length::Fill)
		.spacing(8)
		.align_y(Vertical::Center);

		content = content.push(
			neo_card(user_card)
				.background(COLORS.decorative.pink)
				.width(Length::Fill),
		);

		let mut grid = grid![].spacing(8).columns(2).height(Length::Shrink);

		if self.nightlight.enabled.value() {
			let temp = row![
				svg(phosphor_icon!("sun")).width(16),
				neo_slider(1000..=6500, self.nightlight.temperature)
					.on_change(Message::ChangeTemperature)
			]
			.align_y(Vertical::Center)
			.spacing(4);

			grid = grid.push(temp);

			let brightness = row![
				svg(phosphor_icon!("lightbulb")).width(16),
				neo_slider(0.2..=1.0, self.nightlight.brightness)
					.step(0.01)
					.on_change(Message::ChangeBrightness)
			]
			.align_y(Vertical::Center)
			.spacing(4);

			grid = grid.push(brightness);
		}

		if !self.editing {
			for widget in &self.widgets {
				let widget = self.view_widget(*widget);
				grid = grid.push(widget);
			}
		} else {
			let mut all = SystemMenuWidgets::VARIANTS.to_vec();

			// TODO: This can be done more efficiently...
			all.sort_by(|a, b| {
				let a_index = self.widgets.iter().position(|w| w == a);
				let b_index = self.widgets.iter().position(|w| w == b);

				if let Some(a_index) = a_index {
					if let Some(b_index) = b_index {
						a_index.cmp(&b_index)
					} else {
						std::cmp::Ordering::Less
					}
				} else if b_index.is_some() {
					std::cmp::Ordering::Greater
				} else {
					std::cmp::Ordering::Equal
				}
			});

			for widget in all {
				let is_addable = !self.widgets.contains(&widget);
				let widget = self.view_editable_widget(widget, is_addable);
				grid = grid.push(widget);
			}
		}

		content = content.push(grid);

		neo_card(content)
			.width(480)
			.background(COLORS.decorative.blue)
			.radius(MODULE_RADIUS)
			.into()
	}

	fn view_widget(&self, widget: SystemMenuWidgets) -> Element<'_, Message> {
		match widget {
			SystemMenuWidgets::Wifi => self.wifi.view().map(Message::Wifi),
			SystemMenuWidgets::Bluetooth => self.bluetooth.view().map(Message::Bluetooth),
			SystemMenuWidgets::Speaker => neo_toggle_button(
				phosphor_icon!("speaker-high"),
				"Default Sink",
				"-- %",
				true,
				Some(COLORS.white),
			),
			SystemMenuWidgets::Microphone => neo_toggle_button(
				phosphor_icon!("microphone"),
				"Default Source",
				"-- %",
				false,
				Some(COLORS.white),
			),
			SystemMenuWidgets::Vpn => neo_toggle_button(
				phosphor_icon!("shield-slash"),
				"VPN",
				"Disconnected",
				false,
				Some(COLORS.white),
			),
			SystemMenuWidgets::Nightlight => self
				.nightlight
				.view()
				.map(Message::Nightlight)
				.on_context_menu_with_bounds(Message::OpenNightlightContextMenu),
		}
		.width(Length::Fill)
		.height(64)
		.into()
	}

	fn view_editable_widget(
		&self, widget: SystemMenuWidgets, is_addable: bool,
	) -> Element<'_, Message> {
		// TODO: Disable the interaction on editing. Maybe I don't have to keep a state there, and
		// I can just use a capturing mouse_area?
		let content = container(self.view_widget(widget)).padding(Padding {
			top: 6.0,
			right: 6.0,
			bottom: 0.0,
			left: 0.0,
		});

		let badge = container(
			neo_button(if is_addable {
				svg(phosphor_icon!("plus"))
			} else {
				svg(phosphor_icon!("x"))
			})
			.padding(1)
			.shadow_width(2.0)
			.width(20)
			.height(20)
			.on_press(if is_addable {
				Message::AddWidget(widget)
			} else {
				Message::RemoveWidget(widget)
			}),
		)
		.align_right(Length::Fill)
		.align_top(Length::Fill);

		stack![content, badge].into()
	}
}

async fn set_brightness(brightness: f64) -> zbus::Result<f64> {
	let conn = Connection::session().await?;
	let proxy = NightlightProxy::new(&conn).await?;

	proxy.set_brightness(brightness).await?;

	Ok(brightness)
}

async fn set_temperature(temperature: u32) -> zbus::Result<u32> {
	let conn = Connection::session().await?;
	let proxy = NightlightProxy::new(&conn).await?;
	proxy.set_temperature(temperature).await?;
	Ok(temperature)
}

const UPTIME_PRINTER: SpanPrinter = SpanPrinter::new()
	.spacing(Spacing::BetweenUnitsAndDesignators)
	.designator(Designator::Verbose)
	.comma_after_designator(true);

fn uptime() -> Result<jiff::Span, Box<dyn std::error::Error>> {
	let info = nix::sys::sysinfo::sysinfo()?;
	let uptime = jiff::Span::try_from(info.uptime())?;
	let boot_time = jiff::Zoned::now() - uptime;
	let balanced = uptime.round(
		SpanRound::new()
			.largest(Unit::Year)
			.smallest(Unit::Minute)
			.relative(&boot_time),
	)?;

	let largest = if balanced.get_years() > 0 {
		Unit::Year
	} else if balanced.get_months() > 0 {
		Unit::Month
	} else if balanced.get_days() > 0 {
		Unit::Day
	} else if balanced.get_hours() > 0 {
		Unit::Hour
	} else {
		Unit::Minute
	};

	let smallest = match largest {
		Unit::Year => Unit::Day,   // Y M D
		Unit::Month => Unit::Hour, // M D H
		_ => Unit::Minute,         // D H M
	};

	Ok(uptime.round(
		SpanRound::new()
			.largest(largest)
			.smallest(smallest)
			.relative(&boot_time),
	)?)
}
