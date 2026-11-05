use std::time::{Duration, Instant};

use crate::modules::{MODULE_HEIGHT, MODULE_RADIUS};
use config::{ConfigFile, SystemMenuWidgets};
use daemon_common::NightlightProxy;
use iced::{
	Element, Font, Length, Rectangle, Size, Subscription, Task,
	alignment::Vertical,
	font,
	widget::{column, image, row, svg, text},
};
use jiff::{
	SpanRound, Unit,
	fmt::friendly::{Designator, Spacing, SpanPrinter},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, gif, gif::Frames, neo_button, neo_card},
};
use nix::unistd::{Uid, User};
use zbus::Connection;

mod bluetooth;
mod editor;
mod homeassistant;
mod layout;
mod nightlight;
mod spatial;
mod wifi;

const GUTTER_SIZE: f32 = 128.0;

#[derive(Debug, Clone)]
pub enum Message {
	HeaderMeasured(Size),
	UptimeUpdated(jiff::Span),

	Wifi(wifi::Message),
	Bluetooth(bluetooth::Message),
	Nightlight(nightlight::Message),
	Homeassistant(homeassistant::integration::Message),

	OpenPowerMenu,
	OpenSettings,

	ChangeBrightness(f64),
	ChangeTemperature(u32),

	EditingChanged(bool),
	OpenContextMenu(ContextMenu, usize, Rectangle),
	SaveLayout(config::SystemMenu),
	#[cfg(test)]
	RowsEdited(Vec<config::MenuRow>),
	TilesEdited(Vec<config::PlacedTile>, Vec<config::MenuRow>),
	ToggleWidget(usize),
	Dismiss,
	PreviewTiles(Option<Vec<config::PlacedTile>>),
	Frame(Instant),

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
						| nightlight::Message::SetBrightness(_)
						| nightlight::Message::SetTemperature(_)
				)
		)
	}
}

#[derive(Debug, Clone, Copy)]
pub enum ContextMenu {
	Nightlight(nightlight::ContextMenu),
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum Avatar {
	Gif(Frames),
	Image(image::Handle),
}

#[derive(Debug, Clone)]
pub struct SystemMenu {
	header_size: Size,
	layout: layout::Layout,
	username: String,
	avatar: Avatar,
	uptime: jiff::Span,

	wifi: Box<wifi::Wifi>,
	bluetooth: Box<bluetooth::Bluetooth>,
	nightlight: Box<nightlight::Nightlight>,
	homeassistant: homeassistant::integration::Integration,

	context_menus: Vec<ContextMenu>,
	preview: Option<Vec<config::PlacedTile>>,
	popup_id: Option<iced::window::Id>,
	maximum: Size,
	resize: neo_widgets::animation::geometry::PopupResize,
	edit_envelope: Option<Size>,
	edit_origin: iced::Vector,
	now: Instant,
}

impl SystemMenu {
	pub fn new(config: &ConfigFile) -> Self {
		let username = User::from_uid(Uid::effective())
			.ok()
			.flatten()
			.map_or_else(|| "<unknown>".to_string(), |u| u.name);

		let avatar = Avatar::Gif(
			Frames::from_bytes(include_bytes!("../../../../../assets/avatar.gif").as_slice())
				.expect("Embedded avatar to be a valid GIF"),
		);

		let uptime = uptime().unwrap_or_default();

		let layout = layout::Layout::new(config);
		let homeassistant =
			homeassistant::integration::Integration::new(layout.homeassistant_ids());
		for index in config.system_menu.resolve().displaced {
			log::warn!(
				"System menu tile {index} was pushed right by a tall tile; add an explicit gap to represent the reserved cells"
			);
		}
		let now = Instant::now();

		Self {
			header_size: Size::new(400.0, 78.0),
			layout,
			username,
			avatar,
			uptime,

			wifi: Box::new(wifi::Wifi::new()),
			bluetooth: Box::new(bluetooth::Bluetooth::new()),
			nightlight: Box::new(nightlight::Nightlight::new(config)),
			homeassistant,

			context_menus: Vec::new(),
			preview: None,
			popup_id: None,
			maximum: Size::new(1920.0, 1080.0),
			resize: neo_widgets::animation::geometry::PopupResize::new(Size::ZERO, now),
			edit_envelope: None,
			edit_origin: iced::Vector::ZERO,
			now,
		}
	}

	pub fn init(&self) -> Task<Message> {
		self.homeassistant.init().map(Message::Homeassistant)
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
			self.homeassistant
				.subscription()
				.map(Message::Homeassistant),
		];
		let now = Instant::now();
		let controls_animating = self.nightlight.enabled.is_animating(now)
			|| self.wifi.is_animating(now)
			|| self.bluetooth.is_animating(now)
			|| self
				.layout
				.selected()
				.iter()
				.filter_map(|widget| {
					if let SystemMenuWidgets::Homeassistant(id) = widget {
						self.homeassistant.get(id)
					} else {
						None
					}
				})
				.any(|entity| entity.is_animating(now));
		if self.popup_id.is_some() && (self.resize.needs_frame() || controls_animating) {
			subs.push(iced::window::frames().map(Message::Frame));
		}

		Subscription::batch(subs)
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::HeaderMeasured(size) => self.header_size = size,
			Message::TilesEdited(tiles, rows) => {
				self.preview = None;
				if self.layout.edit_tiles(tiles, rows) {
					self.retarget();
					return Task::done(Message::SaveLayout(self.layout.configuration()));
				}
			}
			Message::ToggleWidget(index) => {
				if self.layout.toggle(index) {
					self.retarget();
					return Task::done(Message::SaveLayout(self.layout.configuration()));
				}
			}
			Message::Frame(now) => {
				self.now = now;
				self.resize.tick(now);
			}
			Message::PreviewTiles(tiles) => {
				if self.layout.editing() {
					self.preview = tiles;
				}
			}
			#[cfg(test)]
			Message::RowsEdited(rows) => {
				self.preview = None;
				if self.layout.edit_rows(rows) {
					self.retarget();
					return Task::done(Message::SaveLayout(self.layout.configuration()));
				}
			}
			Message::SaveLayout(_) => {}
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

			Message::ChangeBrightness(brightness) => {
				return self
					.nightlight
					.update(nightlight::Message::SetBrightness(brightness))
					.map(Message::Nightlight);
			}
			Message::ChangeTemperature(temp) => {
				return self
					.nightlight
					.update(nightlight::Message::SetTemperature(temp))
					.map(Message::Nightlight);
			}

			Message::EditingChanged(editing) => {
				self.preview = None;
				if editing {
					self.layout.begin_edit(self.minimum_columns());
				} else if self.layout.finish_edit() {
					self.retarget();
					return Task::done(Message::SaveLayout(self.layout.configuration()));
				}
			}
			Message::Dismiss
			| Message::Noop
			| Message::OpenSettings
			| Message::OpenPowerMenu
			| Message::OpenContextMenu(_, _, _) => (),
		}
		self.retarget();

		Task::none()
	}

	pub fn sync_config(&mut self, config: &ConfigFile) -> Task<Message> {
		self.layout.reload(config);
		for index in config.system_menu.resolve().displaced {
			log::warn!(
				"System menu tile {index} needs an explicit gap to avoid implicit rightward displacement"
			);
		}
		self.retarget();
		self.homeassistant
			.reconfigure(self.layout.homeassistant_ids())
			.map(Message::Homeassistant)
	}

	pub fn popup_closed(&mut self) -> Task<Message> {
		let changed = self.layout.finish_edit();
		self.close_context_menus();
		self.preview = None;
		self.popup_id = None;
		self.edit_envelope = None;
		if changed {
			Task::done(Message::SaveLayout(self.layout.configuration()))
		} else {
			Task::none()
		}
	}

	pub fn popup_opened(&mut self, id: iced::window::Id, maximum: Size) {
		self.maximum = maximum;
		self.popup_id = Some(id);
		self.now = Instant::now();
		self.resize =
			neo_widgets::animation::geometry::PopupResize::new(self.target_size(), self.now);
	}

	pub fn popup_configured(&mut self, id: iced::window::Id, size: Size) {
		if self.popup_id == Some(id) {
			self.now = Instant::now();
			self.resize.configured(size, self.now);
		}
	}

	pub fn popup_presented(&mut self, id: iced::window::Id, content_updated_at: Instant) {
		if self.popup_id == Some(id) {
			self.resize.presented(content_updated_at);
		}
	}

	pub fn presentation_window(&self) -> Option<iced::window::Id> {
		self.popup_id
			.filter(|_| self.resize.awaiting_presentation())
	}

	pub fn popup_bounds(&mut self, maximum: Size) {
		self.maximum = maximum;
		self.retarget();
	}

	fn visible_tiles(&self) -> Vec<config::PlacedTile> {
		self.preview.clone().unwrap_or_else(|| self.layout.tiles())
	}

	fn grid_size(&self) -> Size {
		let rows = self.layout.edit_baseline_rows();
		let tiles = self.visible_tiles();
		let size = spatial::extent(&tiles, &rows);
		Size::new(
			size.width.max(spatial::span(self.minimum_columns())),
			size.height,
		)
	}

	fn target_size(&self) -> Size {
		let grid = self.grid_size();
		let inset = neo_widgets::widgets::NeoContentSurfaceStyle::default().inset();
		Size::new(
			(grid.width.max(spatial::span(self.minimum_columns())) + inset.width)
				.min(self.maximum.width),
			(grid.height + self.header_size.height + spatial::SPACING + inset.height)
				.min(self.maximum.height),
		)
	}

	fn minimum_columns(&self) -> u32 {
		let inset = neo_widgets::widgets::NeoContentSurfaceStyle::default().inset();
		((self.header_size.width - inset.width + spatial::SPACING) / spatial::PITCH)
			.ceil()
			.max(1.0) as u32
	}

	fn retarget(&mut self) {
		if self.popup_id.is_some() {
			self.now = Instant::now();
			let target = self.target_size();
			if self.layout.editing() {
				if self.edit_envelope.is_none() {
					self.edit_origin = iced::Vector::new(GUTTER_SIZE, GUTTER_SIZE);
				}
				let origin = self.gutter_origin();
				let requested = if self.preview.is_none() {
					Size::new(
						target.width + origin.x + GUTTER_SIZE,
						target.height + origin.y + GUTTER_SIZE,
					)
				} else {
					self.edit_envelope.unwrap_or(target)
				};
				let reserved = Size::new(
					requested.width.max(target.width + origin.x),
					requested.height.max(target.height + origin.y),
				);
				self.edit_envelope = Some(reserved);
				self.resize.request_reserved(target, reserved, self.now);
			} else {
				self.edit_envelope = None;
				self.resize.request(target, self.now);
			}
		}
	}

	fn gutter_origin(&self) -> iced::Vector {
		self.edit_origin
	}

	fn editor_maximum(&self) -> Size {
		let inset = neo_widgets::widgets::NeoContentSurfaceStyle::default().inset();
		let capacity = self.maximum;
		Size::new(
			(capacity.width - inset.width).max(1.0),
			(capacity.height - self.header_size.height - spatial::SPACING - inset.height).max(1.0),
		)
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
		let mut content = column![]
			.spacing(spatial::SPACING)
			.width(Length::Fill)
			.height(Length::Fill);

		content = content.push(neo_widgets::widgets::intrinsic_size(
			self.header(false),
			self.header(true),
			Message::HeaderMeasured,
		));

		let rows = self.layout.edit_baseline_rows();
		let baseline = self.layout.tiles();
		let enabled = self.layout.enabled();
		let visible = self.visible_tiles();
		let entries = visible
			.iter()
			.enumerate()
			.map(|(index, tile)| {
				let size = spatial::bounds(tile).size();
				(
					tile.tile.widget.clone(),
					Some(tile.clone()),
					if enabled.get(index) == Some(&false) {
						self.view_catalog_tile(&tile.tile.widget, size)
					} else {
						self.view_widget(&tile.tile.widget, size)
					},
				)
			})
			.collect::<Vec<_>>();
		let maximum = self.editor_maximum();
		let widgets = editor::grid(
			self.layout.editing(),
			&rows,
			&baseline,
			&enabled,
			entries,
			self.grid_size(),
			maximum,
			!self.resize.waiting(),
		);
		content = content.push(widgets);
		let card = neo_card(content)
			.width(Length::Fill)
			.height(Length::Fill)
			.background(COLORS.decorative.blue)
			.radius(MODULE_RADIUS);
		let target = self.target_size();
		let envelope = if self.popup_id.is_some() {
			self.resize.envelope
		} else {
			target
		};
		let visual = if self.popup_id.is_some() {
			self.resize.frame_visual(self.now)
		} else {
			target
		};
		let origin = if self.layout.editing() {
			self.gutter_origin()
		} else {
			iced::Vector::ZERO
		};
		if self.popup_id.is_some() {
			neo_widgets::widgets::popup_gutter(card, envelope, visual, origin, || Message::Dismiss)
		} else {
			neo_widgets::widgets::popup_surface(card, envelope, visual)
		}
	}

	fn header(&self, fill: bool) -> Element<'_, Message> {
		let width = if fill { Length::Fill } else { Length::Shrink };
		let user_card = row![
			neo_card(self.avatar())
				.padding(2)
				.background(COLORS.decorative.pink),
			column![
				text(&self.username)
					.size(16)
					.wrapping(iced::widget::text::Wrapping::None)
					.color(COLORS.text)
					.font(Font {
						weight: font::Weight::Bold,
						..Default::default()
					}),
				text(format!(
					"up {}",
					UPTIME_PRINTER.span_to_string(&self.uptime)
				))
				.color(COLORS.text)
				.wrapping(iced::widget::text::Wrapping::None)
				.size(12)
			]
			.width(width),
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
		.width(width)
		.spacing(8)
		.align_y(Vertical::Center);

		neo_card(user_card)
			.background(COLORS.decorative.pink)
			.width(width)
			.into()
	}

	fn avatar<'a>(&'a self) -> Element<'a, Message> {
		match &self.avatar {
			Avatar::Gif(frames) => gif(&frames).width(50.0).height(50.0).into(),
			Avatar::Image(handle) => image(handle.clone()).width(50.0).height(50.0).into(),
		}
	}

	fn view_catalog_tile(&self, widget: &SystemMenuWidgets, size: Size) -> Element<'_, Message> {
		let (icon, label) = match widget {
			SystemMenuWidgets::Wifi => (phosphor_icon!("wifi-high"), "Wi-Fi".to_owned()),
			SystemMenuWidgets::Bluetooth => (phosphor_icon!("bluetooth"), "Bluetooth".to_owned()),
			SystemMenuWidgets::Nightlight => (phosphor_icon!("moon"), "Nightlight".to_owned()),
			SystemMenuWidgets::Speaker => (phosphor_icon!("speaker-high"), "Speaker".to_owned()),
			SystemMenuWidgets::Microphone => {
				(phosphor_icon!("microphone"), "Microphone".to_owned())
			}
			SystemMenuWidgets::Vpn => (phosphor_icon!("shield-slash"), "VPN".to_owned()),
			SystemMenuWidgets::Homeassistant(id) => (
				phosphor_icon!("house"),
				self.homeassistant
					.get(id)
					.map_or_else(|| id.clone(), |entity| entity.title().to_owned()),
			),
		};
		neo_widgets::widgets::neo_tile::<Message>(
			icon,
			label,
			"",
			None,
			Some(COLORS.disabled_background),
			size,
		)
		.background(COLORS.disabled_background)
		.enabled(false)
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

	pub(crate) fn view_widget<'a>(
		&'a self, widget: &SystemMenuWidgets, size: Size,
	) -> Element<'a, Message> {
		let button = match widget {
			SystemMenuWidgets::Wifi => self.wifi.view_tile(size).map(Message::Wifi),
			SystemMenuWidgets::Bluetooth => self.bluetooth.view_tile(size).map(Message::Bluetooth),
			SystemMenuWidgets::Speaker => neo_widgets::widgets::neo_tile(
				phosphor_icon!("speaker-high"),
				"Default Sink",
				"-- %",
				Some(true),
				Some(COLORS.white),
				size,
			),
			SystemMenuWidgets::Microphone => neo_widgets::widgets::neo_tile(
				phosphor_icon!("microphone"),
				"Default Source",
				"-- %",
				Some(false),
				Some(COLORS.white),
				size,
			),
			SystemMenuWidgets::Vpn => neo_widgets::widgets::neo_tile(
				phosphor_icon!("shield-slash"),
				"VPN",
				"Disconnected",
				Some(false),
				Some(COLORS.white),
				size,
			),
			SystemMenuWidgets::Nightlight => self
				.nightlight
				.view_tile(size)
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
					neo_widgets::widgets::neo_tile(
						phosphor_icon!("house"),
						"Unknown Entity",
						id.clone(),
						Some(false),
						None,
						size,
					)
					.enabled(false)
				},
				|e| {
					let id = id.clone();
					e.view_tile(size).map(move |msg| {
						Message::Homeassistant(homeassistant::integration::Message::Entity(
							id.clone(),
							msg,
						))
					})
				},
			),
		};
		let view: Element<'_, Message> = if matches!(widget, SystemMenuWidgets::Nightlight)
			&& size.width >= spatial::span(4)
			&& size.height >= spatial::span(2)
		{
			// A separate header owns toggling; dragging sliders cannot trigger it.
			use neo_widgets::widgets::{NeoButtonStyle, NeoContentSurfaceStyle, NeoSurfaceStyle};
			let surface = NeoSurfaceStyle {
				background: iced::Color::TRANSPARENT,
				border_width: 0.0,
				shadow_width: 0.0,
				..Default::default()
			};
			let content_surface = NeoContentSurfaceStyle {
				surface,
				padding: 0.0.into(),
			};
			neo_button(self.nightlight.view_expanded(size))
				.width(Length::Fill)
				.height(Length::Fill)
				.style(NeoButtonStyle {
					surface: content_surface,
					hovered: surface,
					focused: content_surface,
					disabled_background: iced::Color::TRANSPARENT,
				})
				.on_context_menu_with_mouse_position(|position| {
					Message::OpenContextMenu(
						ContextMenu::Nightlight(nightlight::ContextMenu::Root),
						0,
						Rectangle::new(position, Size::new(1.0, 1.0)),
					)
				})
				.into()
		} else {
			button.width(Length::Fill).height(Length::Fill).into()
		};
		let label = match widget {
			SystemMenuWidgets::Homeassistant(id) => self
				.homeassistant
				.get(id)
				.map_or_else(|| id.clone(), |entity| entity.title().to_owned()),
			_ => format!("{widget:?}"),
		};
		iced::widget::tooltip(view, text(label), iced::widget::tooltip::Position::Bottom).into()
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

	#[tokio::test]
	async fn dismissal_persists_row_compaction_once_and_preserves_authored_gap()
	-> Result<(), test_bus::Error> {
		use config::{
			MenuEntry, MenuRow, MenuTile,
			SystemMenuWidgets::{Bluetooth, Wifi},
		};
		let mut config = config::ConfigFile::default();
		config.system_menu = config::SystemMenu::from_rows(vec![
			MenuRow(vec![
				MenuEntry::Widget(MenuTile::new(Wifi)),
				MenuEntry::Gap(1),
				MenuEntry::Widget(MenuTile::new(Bluetooth)),
			]),
			MenuRow::default(),
		]);
		let mut menu = SystemMenu::new(&config);
		let _ = menu.update(super::Message::EditingChanged(true));
		let _ = menu.update(super::Message::ToggleWidget(0));
		assert_eq!(menu.layout.configuration().resolve().tiles[0].column, 3);
		let message = test_bus::task_message(menu.popup_closed()).await?;
		let saved = menu.layout.configuration();
		assert!(matches!(message,super::Message::SaveLayout(layout) if layout == saved));
		assert_eq!(saved.resolve().tiles[0].column, 1);
		assert_eq!(saved.effective_rows().len(), 1);
		assert!(iced_runtime::task::into_stream(menu.popup_closed()).is_none());
		assert!(!menu.layout.editing());
		Ok(())
	}

	#[test]
	fn empty_menu_catalog_starts_at_first_row_and_fits_its_grid() {
		let mut menu = SystemMenu::new(&config::ConfigFile::default());
		let _ = menu.update(super::Message::EditingChanged(true));
		assert_eq!(menu.grid_size().height, super::spatial::span(2));
		assert_eq!(
			menu.grid_size().width,
			super::spatial::span(menu.minimum_columns())
		);
	}

	#[test]
	fn toggling_nightlight_never_changes_layout_and_off_controls_do_nothing() {
		use super::{Message, nightlight};
		let mut config = config::ConfigFile::default();
		config.system_menu =
			config::SystemMenu::from_rows(vec![config::MenuRow(vec![config::MenuEntry::Widget(
				config::MenuTile {
					widget: config::SystemMenuWidgets::Nightlight,
					width: 4,
					height: 2,
				},
			)])]);
		let mut menu = SystemMenu::new(&config);
		let before = menu.target_size();
		let brightness = menu.nightlight.brightness;
		assert_eq!(menu.update(Message::ChangeBrightness(0.4)).units(), 0);
		assert_eq!(menu.nightlight.brightness, brightness);
		let _ = menu.update(Message::Nightlight(nightlight::Message::Toggle));
		assert_eq!(menu.target_size(), before);
		let _ = menu.update(Message::Nightlight(nightlight::Message::Toggle));
		assert_eq!(menu.target_size(), before);
	}

	#[test]
	fn popup_growth_waits_for_its_own_configure_and_close_discards_old_acknowledgements() {
		use iced::{Size, window::Id};
		let mut config = config::ConfigFile::default();
		config.system_menu.widgets = vec![config::SystemMenuWidgets::Nightlight];
		let mut menu = SystemMenu::new(&config);
		let old = Id::unique();
		menu.popup_opened(old, Size::new(1000.0, 800.0));
		menu.popup_configured(old, menu.target_size());
		config.system_menu =
			config::SystemMenu::from_rows(vec![config::MenuRow(vec![config::MenuEntry::Widget(
				config::MenuTile {
					widget: config::SystemMenuWidgets::Nightlight,
					width: 8,
					height: 4,
				},
			)])]);
		let _ = menu.sync_config(&config);
		assert!(menu.resize.waiting());
		menu.popup_configured(Id::unique(), menu.target_size());
		assert!(menu.resize.waiting());
		menu.popup_configured(old, menu.target_size());
		assert!(!menu.resize.waiting());
		let _ = menu.popup_closed();
		let current = Id::unique();
		menu.popup_opened(current, Size::new(500.0, 400.0));
		let target = menu.target_size();
		assert!(target.width <= 500.0 && target.height <= 400.0);
		menu.popup_configured(old, Size::new(1.0, 1.0));
		assert_eq!(menu.popup_id, Some(current));
		assert_eq!(menu.resize.envelope, target);
	}

	#[test]
	fn config_reload_updates_available_homeassistant_widgets() {
		let mut config = config::ConfigFile::default();
		config.homeassistant.tracked_devices = vec!["light.old".into()];
		let mut menu = SystemMenu::new(&config);
		config.homeassistant.tracked_devices = vec!["light.new".into()];
		let _ = menu.sync_config(&config);
		assert!(
			menu.layout
				.available()
				.contains(&config::SystemMenuWidgets::Homeassistant(
					"light.new".into()
				))
		);
		assert!(
			!menu
				.layout
				.available()
				.contains(&config::SystemMenuWidgets::Homeassistant(
					"light.old".into()
				))
		);
	}

	#[test]
	fn config_reload_preserves_edits_and_refreshes_available_widget_catalog() {
		use config::SystemMenuWidgets::{Bluetooth, Homeassistant, Wifi};
		let mut config = config::ConfigFile::default();
		config.system_menu.widgets = vec![Wifi, Homeassistant("light.untracked".into())];
		let mut menu = SystemMenu::new(&config);
		assert!(
			menu.layout
				.available()
				.contains(&Homeassistant("light.untracked".into()))
		);
		let _ = menu.update(super::Message::EditingChanged(true));
		config.system_menu.widgets = vec![Bluetooth, Bluetooth];
		config.homeassistant.tracked_devices = vec!["light.new".into()];
		let _ = menu.sync_config(&config);
		assert_eq!(
			menu.layout.selected(),
			&[Wifi, Homeassistant("light.untracked".into())]
		);
		let _ = menu.popup_closed();
		let _ = menu.sync_config(&config);
		assert_eq!(menu.layout.selected(), &[Bluetooth, Bluetooth]);
		assert!(
			menu.layout
				.available()
				.contains(&Homeassistant("light.new".into()))
		);
		assert!(
			!menu
				.layout
				.available()
				.contains(&Homeassistant("light.untracked".into()))
		);
		config.system_menu.widgets.clear();
		let _ = menu.sync_config(&config);
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

		let previous_generation = menu.homeassistant.session().ok_or("missing replacement")?.0;
		config.homeassistant.tracked_devices = vec!["light.new".into()];
		let reconfigure = menu.sync_config(&config);
		assert_eq!(menu.homeassistant.len(), 0);
		let message = test_bus::task_message(reconfigure).await?;
		let _ = menu.update(message);
		assert!(menu.homeassistant.get("light.new").is_some());
		assert!(menu.homeassistant.get("light.test").is_none());
		assert!(menu.homeassistant.get("switch.test").is_none());
		assert_eq!(menu.homeassistant.len(), 1);
		assert_eq!(
			menu.sync_config(&config).units(),
			0,
			"unchanged configuration must not reconnect"
		);
		assert_eq!(
			menu.update(Message::Homeassistant(HaMessage::Disconnected(
				previous_generation
			)))
			.units(),
			0
		);
		assert!(menu.homeassistant.get("light.new").is_some());

		Ok(())
	}
}
