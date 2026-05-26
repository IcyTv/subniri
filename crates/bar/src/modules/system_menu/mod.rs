use std::time::Duration;

use config::{ConfigFile, SystemMenuWidgets};
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
use nix::unistd::{Uid, User};

use crate::{
	modules::{MODULE_HEIGHT, MODULE_RADIUS},
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button, neo_card, neo_toggle_button},
};

mod wifi;

#[derive(Debug, Clone)]
pub enum Message {
	UptimeUpdated(jiff::Span),
	Wifi(wifi::Message),
	Noop,
}

#[derive(Debug, Clone)]
pub struct SystemMenu {
	widgets: Vec<SystemMenuWidgets>,
	username: String,
	avatar: image::Handle,
	uptime: jiff::Span,
	wifi: wifi::Wifi,
}

impl SystemMenu {
	pub fn new() -> Self {
		let username = User::from_uid(Uid::effective())
			.ok()
			.flatten()
			.map(|u| u.name)
			.unwrap_or_else(|| "<unknown>".to_string());
		let avatar = image::Handle::from_bytes(
			include_bytes!("../../../../../assets/avatar.gif").as_slice(),
		);
		let uptime = uptime().unwrap_or_default();

		Self {
			widgets: vec![],
			username,
			avatar,
			uptime,
			wifi: wifi::Wifi::new(),
		}
	}

	pub fn init(&self) -> Task<Message> {
		self.wifi.init().map(Message::Wifi)
	}

	pub fn subscription(&self) -> Subscription<Message> {
		Subscription::batch([
			iced::time::repeat(
				|| async move {
					if let Ok(uptime) = uptime() {
						Message::UptimeUpdated(uptime)
					} else {
						Message::Noop
					}
				},
				Duration::from_secs(60),
			),
			self.wifi.subscription().map(Message::Wifi),
		])
	}

	pub fn update(&mut self, message: Message, config: &ConfigFile) -> Task<Message> {
		// FIXME: I absolutely hate this
		self.widgets = config.system_menu.widgets.clone();

		match message {
			Message::UptimeUpdated(uptime) => self.uptime = uptime,
			Message::Wifi(message) => return self.wifi.update(message).map(Message::Wifi),
			_ => (),
		}

		Task::none()
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		neo_button(svg(phosphor_icon!("squares-four", "bold")).width(Length::Shrink))
			.width(48.0)
			.height(MODULE_HEIGHT)
			.background(COLORS.decorative.blue)
			.radius(MODULE_RADIUS)
			// .on_press_with_bounds(|bounds| ModuleMessage::Pressed(ModuleKind::SystemMenu, bounds))
			.into()
	}

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
			neo_button(svg(phosphor_icon!("lock")))
				.width(32)
				.height(32)
				.padding(6),
			neo_button(svg(phosphor_icon!("power")))
				.width(32)
				.height(32)
				.padding(6),
			neo_button(svg(phosphor_icon!("gear")))
				.width(32)
				.height(32)
				.padding(6),
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

		for widget in &self.widgets {
			let widget = match widget {
				SystemMenuWidgets::Wifi => self.wifi.view().map(Message::Wifi),
				SystemMenuWidgets::Bluetooth => neo_toggle_button(
					phosphor_icon!("bluetooth"),
					"Bluetooth",
					"0 connected",
					false,
					Some(COLORS.white),
				),
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
		Unit::Year => Unit::Day,    // Y M D
		Unit::Month => Unit::Hour,  // M D H
		Unit::Day => Unit::Minute,  // D H M
		Unit::Hour => Unit::Minute, // H M
		_ => Unit::Minute,
	};

	Ok(uptime.round(
		SpanRound::new()
			.largest(largest)
			.smallest(smallest)
			.relative(&boot_time),
	)?)
}
