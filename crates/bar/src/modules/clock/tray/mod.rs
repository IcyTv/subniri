use std::collections::HashMap;

use iced::widget::text::Span;
use iced::widget::{column, container, image, rich_text, row, rule, scrollable, space, svg, text};
use iced::{Alignment, Color, Element, Length, Padding, Rectangle, Subscription};
use neo_widgets::phosphor_icon;
use neo_widgets::widgets::{NeoButtonStyle, NeoContentSurfaceStyle, NeoSurfaceStyle, neo_card};
use neo_widgets::{
	icons::{ResolvedIcon, try_resolve_icon_name},
	style::COLORS,
	widgets::neo_button,
};
use zbus::{names::OwnedBusName, zvariant::OwnedObjectPath};

use dbusmenu::{ChildrenDisplay, LayoutItemType, ToggleState, ToggleType};

use self::item::{Activation, TrayItem};

mod dbusmenu;
mod icon;
mod item;
mod watcher;

const ICON_SIZE: i32 = 24;
const MENU_ICON_SIZE: f32 = 18.0;

#[derive(Debug, Clone)]
pub enum PopupAction {
	Open(Rectangle),
	NativeContextMenu { service: String, bounds: Rectangle },
	CloseAll,
}

#[derive(Debug, Clone)]
pub enum Message {
	Registered(String, TrayItem),
	Removed(String),
	Pressed(String, Rectangle),
	ContextPressed(String, Rectangle),
	ContextMenuAt {
		service: String,
		x: i32,
		y: i32,
	},

	ShowDbusMenu {
		service: String,
		revision: u32,
		layout: dbusmenu::Layout,
	},
	ShowDbusSubmenu {
		service: String,
		parent_id: i32,
		revision: u32,
		layout: dbusmenu::Layout,
	},
	UpdateDbusMenuProperties {
		service: String,
		updated: dbusmenu::UpdatedProperties,
		removed: dbusmenu::RemovedProperties,
	},
	DbusMenuFailed {
		service: String,
	},
	MenuItemPressed {
		item_id: i32,
		depth: usize,
		bounds: Rectangle,
	},
}

#[derive(Debug)]
pub struct Tray {
	items: HashMap<String, TrayItem>,
	command_tx: async_channel::Sender<Command>,
	commands: watcher::CommandReceiver,

	open_dbusmenu: Option<OpenDbusMenu>,
}

#[derive(Debug)]
struct OpenDbusMenu {
	service: String,
	bus_name: OwnedBusName,
	path: OwnedObjectPath,
	revision: u32,
	layout: Option<dbusmenu::Layout>,
	open_path: Vec<i32>,
}

pub use watcher::Command;

impl Tray {
	pub fn new() -> Self {
		let (command_tx, commands) = watcher::command_channel();
		Self {
			items: HashMap::new(),
			command_tx,
			commands,
			open_dbusmenu: None,
		}
	}

	pub fn update(&mut self, event: Message) -> Option<PopupAction> {
		match event {
			Message::Registered(service, item) => {
				if !self.items.contains_key(&service) {
					log::info!(
						"Registered tray item {service}: id={}, title={}",
						item.id,
						item.title
					);
				}
				self.items.insert(service, item);
			}
			Message::Removed(service) => {
				log::info!("Removed tray item {service}");
				self.items.remove(&service);
				if self
					.open_dbusmenu
					.as_ref()
					.is_some_and(|menu| menu.service == service)
				{
					self.open_dbusmenu = None;
					return Some(PopupAction::CloseAll);
				}
			}
			Message::Pressed(service, bounds) => {
				log::info!("Pressed tray item {service}");
				let Some(item) = self.items.get(&service) else {
					log::warn!("Pressed tray item {service} not found");
					return None;
				};

				let opens_dbusmenu =
					matches!(item.activation, Activation::DbusMenu) && item.menu.is_some();
				if let Err(error) = match item.activation {
					Activation::Activate => self.send_command(Command::Activate {
						service,
						x: 0,
						y: 0,
					}),
					Activation::ContextMenu => self.send_command(Command::ContextMenu {
						service,
						x: 0,
						y: 0,
					}),
					Activation::DbusMenu if let Some(path) = item.menu.clone() => {
						let bus_name = item.address.bus_name.clone();
						self.open_dbusmenu = Some(OpenDbusMenu {
							service: service.clone(),
							bus_name: bus_name.clone(),
							path: path.clone(),
							revision: 0,
							layout: None,
							open_path: Vec::new(),
						});
						self.send_command(Command::ActivateWithDbusMenu {
							service,
							bus_name,
							path,
						})
					}
					_ => {
						log::warn!("Tray item {service} does not provide an activation mechanism");
						Ok(())
					}
				} {
					log::warn!("Failed to queue tray item action: {error}");
				} else if opens_dbusmenu {
					return Some(PopupAction::Open(bounds));
				}
			}
			Message::ContextPressed(service, bounds) => {
				let Some(item) = self.items.get(&service) else {
					return None;
				};
				let result = if let Some(path) = item.menu.clone() {
					let bus_name = item.address.bus_name.clone();
					self.open_dbusmenu = Some(OpenDbusMenu {
						service: service.clone(),
						bus_name: bus_name.clone(),
						path: path.clone(),
						revision: 0,
						layout: None,
						open_path: Vec::new(),
					});
					self.send_command(Command::ActivateWithDbusMenu {
						service,
						bus_name,
						path,
					})
				} else {
					return Some(PopupAction::NativeContextMenu { service, bounds });
				};
				if let Err(error) = result {
					log::warn!("Failed to queue tray context menu: {error}");
					self.open_dbusmenu = None;
					return None;
				}
				if self.open_dbusmenu.is_some() {
					return Some(PopupAction::Open(bounds));
				}
			}
			Message::ContextMenuAt { service, x, y } => {
				if let Err(error) = self.send_command(Command::ContextMenu { service, x, y }) {
					log::warn!("Failed to queue tray context menu: {error}");
				}
			}
			Message::ShowDbusMenu {
				service,
				revision,
				layout,
			} => {
				if let Some(menu) = &mut self.open_dbusmenu
					&& menu.service == service
				{
					menu.revision = revision;
					menu.layout = Some(layout);
				}
			}
			Message::ShowDbusSubmenu {
				service,
				parent_id,
				revision,
				layout,
			} => {
				if let Some(menu) = &mut self.open_dbusmenu
					&& menu.service == service
					&& revision >= menu.revision
				{
					menu.revision = revision;
					if let Some(root) = &mut menu.layout {
						if parent_id == 0 {
							*root = layout;
						} else {
							root.replace_subtree(layout);
						}
					}
				}
			}
			Message::UpdateDbusMenuProperties {
				service,
				updated,
				removed,
			} => {
				if let Some(menu) = &mut self.open_dbusmenu
					&& menu.service == service
					&& let Some(layout) = &mut menu.layout
				{
					layout.apply_property_updates(&updated, &removed);
				}
			}
			Message::DbusMenuFailed { service } => {
				if self
					.open_dbusmenu
					.as_ref()
					.is_some_and(|menu| menu.service == service)
				{
					self.open_dbusmenu = None;
					return Some(PopupAction::CloseAll);
				}
			}
			Message::MenuItemPressed {
				item_id,
				depth,
				bounds,
			} => {
				let Some(menu) = &mut self.open_dbusmenu else {
					return None;
				};
				let is_submenu = menu
					.layout
					.as_ref()
					.and_then(|layout| layout.find(item_id))
					.is_some_and(|item| {
						item.properties.children_display == ChildrenDisplay::Submenu
							|| !item.children.is_empty()
					});
				let command = if is_submenu {
					menu.open_path.truncate(depth);
					menu.open_path.push(item_id);
					Command::OpenDbusSubmenu {
						service: menu.service.clone(),
						bus_name: menu.bus_name.clone(),
						path: menu.path.clone(),
						item_id,
					}
				} else {
					Command::DbusMenuEvent {
						service: menu.service.clone(),
						bus_name: menu.bus_name.clone(),
						path: menu.path.clone(),
						item_id,
					}
				};
				if let Err(error) = self.send_command(command) {
					log::warn!("Failed to queue DBusMenu action: {error}");
					return None;
				}
				return Some(if is_submenu {
					PopupAction::Open(bounds)
				} else {
					PopupAction::CloseAll
				});
			}
		}

		None
	}

	pub fn subscription(&self) -> Subscription<Message> {
		Subscription::run_with(self.commands.clone(), watcher::events)
	}

	pub fn send_command(
		&self, command: Command,
	) -> Result<(), async_channel::TrySendError<Command>> {
		self.command_tx.try_send(command)
	}

	pub fn view(&self) -> Element<'_, Message> {
		row(self.items.iter().map(|(service, item)| {
			let size = Length::Fixed(ICON_SIZE as f32);
			let icon: Element<'_, Message> = match item.icon.resolve(ICON_SIZE as u32) {
				ResolvedIcon::Svg(handle) => svg(handle).width(size).height(size).into(),
				ResolvedIcon::Image(handle) => image(handle).width(size).height(size).into(),
			};
			let pressed_service = service.clone();
			let context_service = service.clone();
			neo_button(icon)
				.on_press_started_with_bounds(move |bounds| {
					Message::Pressed(pressed_service.clone(), bounds)
				})
				.on_context_menu_with_bounds(move |bounds| {
					Message::ContextPressed(context_service.clone(), bounds)
				})
				.background(COLORS.decorative.pink)
				.into()
		}))
		.spacing(4)
		.into()
	}

	pub fn view_context_menu(&self, depth: usize) -> Element<'_, Message> {
		let content = self
			.open_dbusmenu
			.as_ref()
			.and_then(|menu| {
				let layout = menu.layout.as_ref()?;
				if depth == 0 {
					Some(layout)
				} else {
					menu.open_path
						.get(depth - 1)
						.and_then(|id| layout.find(*id))
				}
			})
			.map_or_else(
				|| {
					container(text("Loading menu...").color(COLORS.text))
						.padding(12)
						.into()
				},
				|layout| menu(layout, depth),
			);

		neo_card(scrollable(content).height(Length::Fill))
			.padding(5)
			.radius(8.0)
			.background(COLORS.white)
			.width(Length::Fixed(286.0))
			.into()
	}
}

fn menu(layout: &dbusmenu::Layout, depth: usize) -> Element<'_, Message> {
	// Root is not a visible menu entry.
	column(
		layout
			.children
			.iter()
			.filter(|child| child.properties.visible)
			.map(|child| menu_item(child, depth)),
	)
	.spacing(2)
	.into()
}

fn menu_item(layout: &dbusmenu::Layout, depth: usize) -> Element<'_, Message> {
	if layout.properties.ty == LayoutItemType::Separator {
		return container(rule::horizontal(1).style(|_| rule::Style {
			color: COLORS.separator,
			radius: 0.0.into(),
			fill_mode: rule::FillMode::Full,
			snap: true,
		}))
		.padding(Padding::from([4, 8]))
		.into();
	}

	let properties = &layout.properties;
	let has_submenu =
		properties.children_display == ChildrenDisplay::Submenu || !layout.children.is_empty();
	let color = if properties.enabled {
		COLORS.text
	} else {
		Color {
			a: 0.42,
			..COLORS.text
		}
	};
	let shortcut = format_shortcut(&properties.shortcut);
	let content = row![
		toggle_indicator(properties, color),
		menu_icon(properties),
		rich_text(decode_mnemonics(&properties.label, color)).width(Length::Fill),
		text(shortcut).size(13).color(Color { a: 0.65, ..color }),
		if has_submenu {
			svg(phosphor_icon!("caret-right", "bold"))
				.width(14)
				.height(14)
		} else {
			svg(phosphor_icon!("caret-right", "bold"))
				.width(0)
				.height(14)
		},
	]
	.spacing(7)
	.align_y(Alignment::Center);
	let style = menu_button_style();
	let button = neo_button(content)
		.style(style)
		.width(Length::Fill)
		.height(36)
		.enabled(properties.enabled);

	if properties.enabled {
		button
			.on_press_started_with_bounds(move |bounds| Message::MenuItemPressed {
				item_id: layout.id,
				depth,
				bounds,
			})
			.into()
	} else {
		button.into()
	}
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

fn toggle_indicator(properties: &dbusmenu::LayoutProperties, color: Color) -> Element<'_, Message> {
	let icon = match (properties.toggle_type, properties.toggle_state) {
		(ToggleType::Checkmark, ToggleState::On) => Some(phosphor_icon!("check", "bold")),
		(ToggleType::Radio, ToggleState::On) => Some(phosphor_icon!("circle", "fill")),
		(ToggleType::Radio, _) => Some(phosphor_icon!("circle", "bold")),
		(_, ToggleState::Indeterminate) if properties.toggle_type != ToggleType::Empty => {
			Some(phosphor_icon!("minus", "bold"))
		}
		_ => None,
	};
	let indicator: Element<'_, Message> = icon.map_or_else(
		|| space().width(MENU_ICON_SIZE).height(MENU_ICON_SIZE).into(),
		|icon| {
			svg(icon)
				.width(MENU_ICON_SIZE)
				.height(MENU_ICON_SIZE)
				.style(move |_, _| svg::Style { color: Some(color) })
				.into()
		},
	);
	container(indicator).width(MENU_ICON_SIZE).into()
}

fn menu_icon(properties: &dbusmenu::LayoutProperties) -> Element<'_, Message> {
	let size = Length::Fixed(MENU_ICON_SIZE);
	if !properties.icon_name.is_empty()
		&& let Some(icon) = try_resolve_icon_name(&properties.icon_name, MENU_ICON_SIZE as u32, 1)
	{
		return match icon {
			ResolvedIcon::Svg(handle) => svg(handle).width(size).height(size).into(),
			ResolvedIcon::Image(handle) => image(handle).width(size).height(size).into(),
		};
	}
	if !properties.icon_data.is_empty() {
		return image(image::Handle::from_bytes(properties.icon_data.clone()))
			.width(size)
			.height(size)
			.into();
	}
	space().width(size).height(size).into()
}

fn format_shortcut(shortcuts: &[Vec<String>]) -> String {
	shortcuts.first().map_or_else(String::new, |shortcut| {
		shortcut
			.iter()
			.map(|part| match part.as_str() {
				"Control" => "Ctrl",
				other => other,
			})
			.collect::<Vec<_>>()
			.join("+")
	})
}

fn decode_mnemonics(label: &str, color: Color) -> Vec<Span<'_>> {
	let mut spans = Vec::new();
	let mut plain = String::new();
	let mut chars = label.chars().peekable();

	while let Some(c) = chars.next() {
		if c != '_' {
			plain.push(c);
			continue;
		}

		match chars.next() {
			// "__" means a literal underscore.
			Some('_') => {
				plain.push('_');
			}

			// "_" followed by a character marks that character mnemonic.
			Some(c) => {
				if !plain.is_empty() {
					spans.push(Span::new(std::mem::take(&mut plain)).color(color));
				}

				spans.push(Span::new(c.to_string()).underline(true).color(color));
			}

			// Be tolerant of a trailing "_".
			None => {
				plain.push('_');
			}
		}
	}

	if !plain.is_empty() {
		spans.push(Span::new(plain).color(color));
	}

	spans
}
