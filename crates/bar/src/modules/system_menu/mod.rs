use std::{collections::HashMap, time::Duration};

use config::{ConfigFile, SystemMenuWidgets};
use daemon_common::NightlightProxy;
use iced::{
	Element, Font, Length, Rectangle, Subscription, Task,
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
use zbus::Connection;

use self::homeassistant::HomeassistantEntity;
use crate::modules::{MODULE_HEIGHT, MODULE_RADIUS};

mod bluetooth;
mod editor;
mod homeassistant;
mod nightlight;
mod wifi;

#[derive(Debug, Clone)]
pub enum Message {
	UptimeUpdated(jiff::Span),

	Wifi(wifi::Message),
	Bluetooth(bluetooth::Message),
	Nightlight(nightlight::Message),
	HomeassistantEntity(String, homeassistant::Message),

	OpenPowerMenu,
	OpenSettings,

	BrightnessChanged(f64),
	TemperatureChanged(u32),

	ChangeBrightness(f64),
	ChangeTemperature(u32),

	EditingChanged(bool),
	OpenContextMenu(ContextMenu, usize, Rectangle),
	WidgetsEdited(Vec<SystemMenuWidgets>),

	Noop,
}

impl Message {
	pub fn closes_context_menus(&self) -> bool {
		matches!(
			self,
			Self::EditingChanged(true)
				| Self::Nightlight(
					nightlight::Message::Toggle
						| nightlight::Message::Suspend(_)
						| nightlight::Message::SetPreset(_)
				)
		)
	}
}

#[derive(Debug, Clone, Copy)]
pub enum ContextMenu {
	Nightlight(nightlight::ContextMenu),
}

#[derive(Debug, Clone)]
pub struct SystemMenu {
	widgets: Vec<SystemMenuWidgets>,
	username: String,
	avatar: image::Handle,
	uptime: jiff::Span,

	all_widgets: Vec<SystemMenuWidgets>,

	wifi: Box<wifi::Wifi>,
	bluetooth: Box<bluetooth::Bluetooth>,
	nightlight: Box<nightlight::Nightlight>,
	homeassistant_entities: HashMap<String, HomeassistantEntity>,

	context_menus: Vec<ContextMenu>,

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

		let mut all_widgets = SystemMenuWidgets::VARIANTS.to_vec();

		let all_ha_devices = config
			.homeassistant
			.tracked_devices
			.iter()
			.cloned()
			.map(SystemMenuWidgets::Homeassistant)
			.collect::<Vec<_>>();
		log::debug!("All HA devices: {all_ha_devices:?}");

		// FIXME: Don't block!!!
		let connection =
			futures::executor::block_on(async { zbus::Connection::session().await }).unwrap();

		let homeassistant_entities = all_ha_devices
			.iter()
			.filter_map(|v| match v {
				SystemMenuWidgets::Homeassistant(id) => Some(id),
				_ => None,
			})
			.map(|id| {
				(
					id.clone(),
					HomeassistantEntity::new(connection.clone(), id.clone()),
				)
			})
			.collect();

		all_widgets.extend(all_ha_devices);
		for widget in &config.system_menu.widgets {
			if !all_widgets.contains(widget) {
				all_widgets.push(widget.clone());
			}
		}

		log::debug!("All widgets: {all_widgets:?}");

		let widgets = config.system_menu.widgets.clone();

		Self {
			widgets,
			username,
			avatar,
			uptime,

			all_widgets,

			wifi: Box::new(wifi::Wifi::new()),
			bluetooth: Box::new(bluetooth::Bluetooth::new()),
			nightlight: Box::new(nightlight::Nightlight::new(config)),
			homeassistant_entities,

			context_menus: Vec::new(),

			editing: false,
		}
	}

	pub fn init(&self) -> Task<Message> {
		Task::batch([
			self.wifi.init().map(Message::Wifi),
			self.bluetooth.init().map(Message::Bluetooth),
		])
	}

	pub fn subscription(&self) -> Subscription<Message> {
		let mut subs = vec![
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
		];

		for (id, entity) in &self.homeassistant_entities {
			subs.push(
				entity
					.subscription()
					.with(id.clone())
					.map(move |(id, msg)| Message::HomeassistantEntity(id, msg)),
			);
		}

		Subscription::batch(subs)
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		if matches!(
			message,
			Message::TemperatureChanged(_) | Message::BrightnessChanged(_)
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
			Message::WidgetsEdited(widgets) => {
				if self.editing {
					self.widgets = widgets;
				}
			}
			Message::HomeassistantEntity(id, message)
				if let Some(entity) = self.homeassistant_entities.get_mut(&id) =>
			{
				return entity
					.update(message)
					.map(move |message| Message::HomeassistantEntity(id.clone(), message));
			}
			Message::HomeassistantEntity(id, _) => {
				log::warn!("Tried to address non-existant homeassistant entity: '{id}'");
			}
			Message::Noop
			| Message::OpenSettings
			| Message::OpenPowerMenu
			| Message::OpenContextMenu(_, _, _) => (),
		}

		Task::none()
	}

	pub fn sync_config(&mut self, config: &ConfigFile) {
		if !self.editing {
			self.sync_widgets(&config.system_menu.widgets);
		}
	}

	pub(super) fn widgets(&self) -> &[SystemMenuWidgets] {
		&self.widgets
	}

	pub fn popup_closed(&mut self) {
		self.editing = false;
		self.close_context_menus();
	}

	pub fn close_context_menus(&mut self) {
		self.context_menus.clear();
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

		// Transient device controls are not part of the editable widget layout.
		if !self.editing && self.nightlight.enabled.value() {
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

		let mut ordered = self.widgets.iter().collect::<Vec<_>>();
		if self.editing {
			ordered.extend(
				self.all_widgets
					.iter()
					.filter(|widget| !self.widgets.contains(widget)),
			);
		}
		// The normal and edit layouts share precisely the same card visuals.
		let entries = ordered
			.into_iter()
			.map(|widget| (widget.clone(), self.view_widget(widget)))
			.collect::<Vec<_>>();
		let widgets: Element<'_, Message> = if self.editing {
			editor::grid(self.widgets.len(), entries)
		} else {
			for (_, view) in entries {
				grid = grid.push(view);
			}
			grid.into()
		};

		content = content.push(widgets);

		neo_card(content)
			.width(480)
			.height(Length::Shrink)
			.background(COLORS.decorative.blue)
			.radius(MODULE_RADIUS)
			.into()
	}

	pub fn view_context_menu(&self, depth: usize) -> Element<'_, Message> {
		match self.context_menus.get(depth).copied() {
			Some(ContextMenu::Nightlight(menu)) => {
				self.nightlight
					.view_context_menu(menu)
					.map(move |message| match message {
						nightlight::Message::OpenContextMenu(menu, bounds) => {
							Message::OpenContextMenu(
								ContextMenu::Nightlight(menu),
								depth + 1,
								bounds,
							)
						}
						message => Message::Nightlight(message),
					})
			}
			_ => neo_card(text("No context menu").color(COLORS.text))
				.padding(8)
				.background(COLORS.white)
				.into(),
		}
	}

	pub fn open_context_menu(&mut self, menu: ContextMenu, depth: usize) {
		if depth > self.context_menus.len() {
			return;
		}
		self.context_menus.truncate(depth);
		self.context_menus.push(menu);
	}

	fn view_widget<'a>(&'a self, widget: &'a SystemMenuWidgets) -> Element<'a, Message> {
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
				.on_context_menu_with_mouse_position(|position| {
					Message::OpenContextMenu(
						ContextMenu::Nightlight(nightlight::ContextMenu::Root),
						0,
						Rectangle::new(position, iced::Size::new(1.0, 1.0)),
					)
				}),
			SystemMenuWidgets::Homeassistant(id) => {
				self.homeassistant_entities.get(id).map_or_else(
					|| {
						neo_toggle_button(
							phosphor_icon!("house"),
							"Unknown Entity",
							id,
							false,
							None,
						)
						.enabled(false)
					},
					|e| {
						e.view()
							.map(|msg| Message::HomeassistantEntity(id.clone(), msg))
					},
				)
			}
		}
		.width(Length::Fill)
		.height(64)
		.into()
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
