use std::time::Duration;

use crate::modules::{MODULE_HEIGHT, MODULE_RADIUS};
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

mod bluetooth;
mod editor;
mod homeassistant;
mod layout;
mod nightlight;
mod wifi;

#[derive(Debug, Clone)]
pub enum Message {
	UptimeUpdated(jiff::Span),

	Wifi(wifi::Message),
	Bluetooth(bluetooth::Message),
	Nightlight(nightlight::Message),
	Homeassistant(homeassistant::integration::Message),

	OpenPowerMenu,
	OpenSettings,

	BrightnessChanged(f64),
	TemperatureChanged(u32),

	ChangeBrightness(f64),
	ChangeTemperature(u32),

	EditingChanged(bool),
	OpenContextMenu(ContextMenu, usize, Rectangle),
	WidgetsEdited(Vec<SystemMenuWidgets>),
	SaveWidgets(Vec<SystemMenuWidgets>),

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
	layout: layout::Layout,
	username: String,
	avatar: image::Handle,
	uptime: jiff::Span,

	wifi: Box<wifi::Wifi>,
	bluetooth: Box<bluetooth::Bluetooth>,
	nightlight: Box<nightlight::Nightlight>,
	homeassistant: homeassistant::integration::Integration,

	context_menus: Vec<ContextMenu>,
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

		let layout = layout::Layout::new(config);
		let homeassistant = homeassistant::integration::Integration::new(
			layout
				.available()
				.iter()
				.filter_map(|widget| {
					if let SystemMenuWidgets::Homeassistant(id) = widget {
						Some(id.clone())
					} else {
						None
					}
				})
				.collect(),
		);

		Self {
			layout,
			username,
			avatar,
			uptime,

			wifi: Box::new(wifi::Wifi::new()),
			bluetooth: Box::new(bluetooth::Bluetooth::new()),
			nightlight: Box::new(nightlight::Nightlight::new(config)),
			homeassistant,

			context_menus: Vec::new(),
		}
	}

	pub fn init(&self) -> Task<Message> {
		self.homeassistant.init().map(Message::Homeassistant)
	}

	pub fn subscription(&self) -> Subscription<Message> {
		let subs = vec![
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
			self.homeassistant
				.subscription()
				.map(Message::Homeassistant),
		];

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
			Message::Homeassistant(message) => {
				return self
					.homeassistant
					.update(message)
					.map(Message::Homeassistant);
			}
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

			Message::EditingChanged(editing) => {
				self.layout.update(layout::Event::Editing(editing));
			}
			Message::WidgetsEdited(widgets) => {
				if let Some(widgets) = self.layout.update(layout::Event::Edited(widgets)) {
					return Task::done(Message::SaveWidgets(widgets));
				}
			}
			Message::SaveWidgets(_) => {}
			Message::Noop
			| Message::OpenSettings
			| Message::OpenPowerMenu
			| Message::OpenContextMenu(_, _, _) => (),
		}

		Task::none()
	}

	pub fn sync_config(&mut self, config: &ConfigFile) {
		self.layout
			.update(layout::Event::Reload(config.system_menu.widgets.clone()));
	}

	pub fn popup_closed(&mut self) {
		self.layout.update(layout::Event::Closed);
		self.close_context_menus();
	}

	pub fn close_context_menus(&mut self) {
		self.context_menus.clear();
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
				.background(if self.layout.editing() {
					COLORS.decorative.blue
				} else {
					COLORS.white
				})
				.on_press(Message::EditingChanged(!self.layout.editing())),
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
		if !self.layout.editing() && self.nightlight.enabled.value() {
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

		// The normal and edit layouts share precisely the same card visuals.
		let entries = self
			.layout
			.entries()
			.into_iter()
			.map(|widget| (widget.clone(), self.view_widget(widget)))
			.collect::<Vec<_>>();
		let widgets: Element<'_, Message> = if self.layout.editing() {
			editor::grid(self.layout.selected().len(), entries)
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
			SystemMenuWidgets::Homeassistant(id) => self.homeassistant.get(id).map_or_else(
				|| {
					neo_toggle_button(phosphor_icon!("house"), "Unknown Entity", id, false, None)
						.enabled(false)
				},
				|e| {
					e.view().map(|msg| {
						Message::Homeassistant(homeassistant::integration::Message::Entity(
							id.clone(),
							msg,
						))
					})
				},
			),
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

#[cfg(test)]
mod reliability_tests {
	use super::SystemMenu;
	use crate::modules::test_bus;

	#[test]
	fn config_reload_preserves_edits_and_keeps_existing_available_widget_catalog() {
		use config::SystemMenuWidgets::{Bluetooth, Homeassistant, Wifi};
		let mut config = config::ConfigFile::default();
		config.system_menu.widgets = vec![Wifi, Homeassistant("light.untracked".into())];
		let mut menu = SystemMenu::new(&config);
		assert!(
			menu.layout
				.available()
				.contains(&Homeassistant("light.untracked".into()))
		);
		let available = menu.layout.available().to_vec();
		let _ = menu.update(super::Message::EditingChanged(true));
		config.system_menu.widgets = vec![Bluetooth, Bluetooth];
		config.homeassistant.tracked_devices = vec!["light.new".into()];
		menu.sync_config(&config);
		assert_eq!(
			menu.layout.selected(),
			&[Wifi, Homeassistant("light.untracked".into())]
		);
		menu.popup_closed();
		menu.sync_config(&config);
		assert_eq!(menu.layout.selected(), &[Bluetooth, Bluetooth]);
		assert_eq!(menu.layout.available(), available);
		config.system_menu.widgets.clear();
		menu.sync_config(&config);
		assert!(menu.layout.selected().is_empty());
	}

	#[test]
	#[ignore = "private subprocess with a deliberately unavailable session bus"]
	fn absent_session_bus_does_not_crash_menu() -> Result<(), test_bus::Error> {
		test_bus::run_without_session("modules::system_menu::reliability_tests::absent_bus_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn absent_bus_child() -> Result<(), test_bus::Error> {
		test_bus::require_child()?;
		let mut menu = SystemMenu::new(&config::ConfigFile::default());
		let message = tokio::time::timeout(
			std::time::Duration::from_secs(2),
			test_bus::task_message(menu.init()),
		)
		.await??;
		let retry = menu.update(message);
		assert!(
			retry.units() > 0,
			"a transient session-bus failure must retry asynchronously"
		);
		Ok(())
	}

	#[test]
	#[ignore = "private session-bus subprocess"]
	fn shared_transport_recovers_and_rejects_stale_events() -> Result<(), test_bus::Error> {
		test_bus::run("modules::system_menu::reliability_tests::shared_transport_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn shared_transport_child() -> Result<(), test_bus::Error> {
		use super::Message;
		use super::homeassistant::integration::Message as HaMessage;
		use futures::StreamExt;

		test_bus::require_child()?;

		let mut config = config::ConfigFile::default();
		config.homeassistant.tracked_devices = vec!["switch.test".into(), "light.test".into()];

		let mut menu = SystemMenu::new(&config);
		let message = test_bus::task_message(menu.init()).await?;
		let _ = menu.update(message);
		assert_eq!(menu.homeassistant.len(), 2);

		let (old_generation, connection) = menu
			.homeassistant
			.session()
			.ok_or("missing shared connection")?;

		// Drive the real menu subscriptions against the isolated broker and
		// wait specifically for the shared transport's closure notification.
		let mut streams = futures::stream::select_all(
			iced::advanced::subscription::into_recipes(menu.subscription())
				.into_iter()
				.map(|recipe| recipe.stream(futures::stream::pending().boxed())),
		);
		(*connection).clone().close().await?;
		let disconnected = tokio::time::timeout(std::time::Duration::from_secs(2), async {
			loop {
				if let Some(message @ Message::Homeassistant(HaMessage::Disconnected(_))) =
					streams.next().await
				{
					break message;
				}
			}
		})
		.await?;

		drop(streams);
		let reconnect = menu.update(disconnected);
		assert_eq!(menu.homeassistant.len(), 0);

		let message = test_bus::task_message(reconnect).await?;
		let _ = menu.update(message);
		assert_eq!(menu.homeassistant.len(), 2);
		assert!(menu.homeassistant.session().is_some());

		let stale = menu.update(Message::Homeassistant(HaMessage::Disconnected(
			old_generation,
		)));
		assert_eq!(stale.units(), 0);
		assert!(menu.homeassistant.session().is_some());
		assert_eq!(menu.homeassistant.len(), 2);
		let stale = menu.update(Message::Homeassistant(HaMessage::Observed(
			old_generation,
			"light.test".into(),
			super::homeassistant::Message::CommandFinished(Ok(())),
		)));
		assert_eq!(stale.units(), 0);
		assert_eq!(menu.homeassistant.len(), 2);

		Ok(())
	}
}
