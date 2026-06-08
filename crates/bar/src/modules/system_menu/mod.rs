use std::time::Duration;

use config::{ConfigFile, SystemMenuWidgets};
use daemon::NightlightProxy;
use futures::{StreamExt as _, stream};
use iced::{
	Element, Font, Length, Subscription, Task,
	alignment::Vertical,
	font,
	widget::{column, grid, image, row, svg, text},
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

use crate::modules::{MODULE_HEIGHT, MODULE_RADIUS};

mod bluetooth;
mod wifi;

#[derive(Debug, Clone)]
pub enum Message {
	UptimeUpdated(jiff::Span),
	Wifi(wifi::Message),
	Bluetooth(bluetooth::Message),
	OpenPowerMenu,
	OpenSettings,

	NightlightEnabled(bool),
	BrightnessChanged(f64),
	TemperatureChanged(u32),

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
	nightlight_enabled: bool,
	temperature: u32,
	brightness: f64,
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
			nightlight_enabled: config.nightlight.enabled,
			brightness: config.nightlight.day.brightness,
			temperature: config.nightlight.day.temperature,
		}
	}

	pub fn init(&self) -> Task<Message> {
		Task::batch([
			self.wifi.init().map(Message::Wifi),
			self.bluetooth.init().map(Message::Bluetooth),
		])
	}

	pub fn subscription() -> Subscription<Message> {
		// TODO: Don't block...
		let nightlight = Subscription::run(|| futures::executor::block_on(nighlight_stream()));

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
			nightlight,
		])
	}

	pub fn update(&mut self, message: Message, config: &ConfigFile) -> Task<Message> {
		// FIXME: I absolutely hate this
		self.widgets.clone_from(&config.system_menu.widgets);

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
			Message::NightlightEnabled(enabled) => self.nightlight_enabled = enabled,
			Message::TemperatureChanged(temp) => self.temperature = temp,
			Message::BrightnessChanged(brightness) => self.brightness = brightness,
			_ => (),
		}

		Task::none()
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
				.padding(6),
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

		if self.nightlight_enabled {
			let temp = row![
				svg(phosphor_icon!("sun")).width(16),
				neo_slider(1000..=6500, self.temperature)
			]
			.align_y(Vertical::Center)
			.spacing(4);

			grid = grid.push(temp);

			let brightness = row![
				svg(phosphor_icon!("lightbulb")).width(16),
				neo_slider(0.0..=1.0, self.brightness),
			]
			.align_y(Vertical::Center)
			.spacing(4);

			grid = grid.push(brightness);
		}

		for widget in &self.widgets {
			let widget = match widget {
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
				SystemMenuWidgets::Nightlight => neo_toggle_button(
					phosphor_icon!("moon"),
					"Nightlight",
					"Off",
					false,
					Some(COLORS.white),
				),
			}
			.width(Length::Fill)
			.height(64);
			grid = grid.push(widget);
		}

		content = content.push(grid);

		neo_card(content)
			.width(480)
			.background(COLORS.decorative.blue)
			.radius(MODULE_RADIUS)
			.into()
	}
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

async fn nighlight_stream() -> stream::BoxStream<'static, Message> {
	let connection = match zbus::Connection::session().await {
		Ok(connection) => connection,
		Err(error) => {
			log::warn!("Failed to connect to session bus for nightlight signals: {error}");
			return stream::once(async { Message::Noop }).boxed();
		}
	};

	log::trace!("Connected");

	let proxy = match NightlightProxy::new(&connection).await {
		Ok(proxy) => proxy,
		Err(error) => {
			log::warn!("Failed to create nightlight proxy: {error}");
			return stream::once(async { Message::Noop }).boxed();
		}
	};

	log::trace!("got proxy");

	let enabled_changed = proxy.receive_enabled_changed();
	let brightness_changed = proxy.receive_brightness_changed();
	let temperature_changed = proxy.receive_temperature_changed();

	// TODO: Should we just be using receive_state_changed signal?
	let (enabled, brightness, temperature) =
		tokio::join!(enabled_changed, brightness_changed, temperature_changed,);

	log::trace!("Got property streams");

	let enabled = enabled.then(|enabled| async move {
		match enabled.get().await {
			Ok(value) => Message::NightlightEnabled(value),
			Err(error) => {
				log::warn!("Failed to read nightlight enabled value: {error}");
				Message::Noop
			}
		}
	});
	let brightness = brightness.then(|brightness| async move {
		match brightness.get().await {
			Ok(value) => Message::BrightnessChanged(value),
			Err(error) => {
				log::warn!("Failed to read nightlight brightness value: {error}");
				Message::Noop
			}
		}
	});
	let temperature = temperature.then(|temperature| async move {
		log::trace!("Temp changed");
		match temperature.get().await {
			Ok(value) => {
				log::trace!("Temp: {value:?}");
				Message::TemperatureChanged(value)
			}
			Err(error) => {
				log::warn!("Failed to read nightlight temperature value: {error}");
				Message::Noop
			}
		}
	});

	stream::select(stream::select(enabled, brightness), temperature).boxed()
}
