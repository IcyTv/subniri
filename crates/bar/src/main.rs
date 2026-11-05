use config::ConfigFile;
use iced::alignment::Vertical;
use std::{collections::HashMap, env, fs, path::PathBuf, process::Command, time::Duration};

use futures::StreamExt;
use iced::Length;
use iced::widget::{container, row, stack, text};
use iced::window::Id;
use iced::{Color, Element, Subscription, Task, Theme};
use iced_exwlshell::actions::IcedNewPopupSettings;
use iced_exwlshell::reexport::{
	Anchor, KeyboardInteractivity, LayerSize, PixelSize, PopupAnchor, PopupConstraintAdjustment,
	PopupGravity,
};
use iced_exwlshell::settings::{LayerShellSettings, StartMode};
use iced_exwlshell::{Settings, daemon, to_layer_message};
use iced_wayland_subscriber::shell::{ShellEvent, ShellReceiver};
use neo_widgets::{
	style::{COLORS, neo_theme},
	widgets::{neo_card, popup_grow},
};
use niri_ipc::{Reply, Request, Response, socket::SOCKET_PATH_ENV};
use tokio::{
	io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
	net::UnixStream,
};
use wayland_client::Connection;

use crate::modules::{Module, ModuleKind, ModuleMessage};

mod modules;
mod popups;
mod widget_config;

// mod clock;
// mod icons;
// mod mpris;
fn main() -> Result<(), Box<dyn std::error::Error>> {
	log::init!("bar", "polarbar")?;

	let connection = Connection::connect_to_env()?;

	let (doc, config) = ConfigFile::load()?;

	let (shell_broadcast, shell_events) = iced_wayland_subscriber::shell::channel();

	let app = daemon(
		{
			let conn = connection.clone();
			// FIXME: Don't clone
			let doc = doc.clone();
			let config = config.clone();
			let events = shell_events.clone();
			move || Bar::new(&conn, doc.clone(), config.clone(), events.clone())
		},
		Bar::namespace,
		Bar::update,
		Bar::view,
	)
	.theme(neo_theme())
	.style(Bar::style)
	.scale_factor(Bar::scale_factor)
	.subscription(Bar::subscription)
	.settings(Settings {
		with_connection: Some(connection.into()),
		default_text_size: 18.into(),
		shell_broadcast,
		popup_transitions: Some(iced_exwlshell::popup_animation::Profile::default()),
		..Default::default()
	})
	.layer_settings(LayerShellSettings {
		size: LayerSize::fill_width(BASE_BAR_HEIGHT),
		exclusive_zone: BASE_BAR_HEIGHT.cast_signed(),
		anchor: Anchor::Top | Anchor::Left | Anchor::Right,
		start_mode: StartMode::AllScreens,
		keyboard_interactivity: KeyboardInteractivity::OnDemand,
		..Default::default()
	});

	Ok(app.run()?)
}

const BASE_BAR_HEIGHT: u32 = 60;

fn scale_for_screen(height: u32) -> f32 {
	const BASE_SCREEN_HEIGHT: f32 = 1440.0;
	const SCREEN_SCALE_EXPONENT: f32 = 0.75;
	if height == 0 {
		return 1.0;
	}
	let linear_scale = height as f32 / BASE_SCREEN_HEIGHT;
	linear_scale.powf(SCREEN_SCALE_EXPONENT).clamp(0.7, 1.25)
}

struct Bar {
	left: Vec<Module>,
	center: Vec<Module>,
	right: Vec<Module>,

	popups: popups::Popups,
	layer_heights: HashMap<Id, u32>,
	window_scales: HashMap<Id, f32>,
	window_output_names: HashMap<Id, String>,
	output_sizes: HashMap<Id, iced::Size>,
	shell_events: ShellReceiver,
	config_doc: kdl::KdlDocument,
	config_file: ConfigFile,
	widget_config: widget_config::Writer,
}

impl Bar {
	fn new(
		connection: &Connection, config_doc: kdl::KdlDocument, config_file: ConfigFile,
		shell_events: ShellReceiver,
	) -> (Self, Task<BarMessage>) {
		let _ = connection;

		let bar = Self {
			left: vec![Module::system_menu(&config_file), Module::taskbar()],
			center: vec![Module::media_controls()],
			right: vec![
				Module::volume(),
				Module::network(),
				Module::bluetooth(),
				Module::clock(),
			],
			popups: popups::Popups::default(),
			layer_heights: HashMap::new(),
			window_scales: HashMap::new(),
			window_output_names: HashMap::new(),
			output_sizes: HashMap::new(),
			shell_events,
			config_doc,
			config_file,
			widget_config: widget_config::Writer::default(),
		};

		let tasks = Task::batch([
			Self::module_init_tasks(Section::Left, &bar.left),
			Self::module_init_tasks(Section::Center, &bar.center),
			Self::module_init_tasks(Section::Right, &bar.right),
		]);

		(bar, tasks)
	}

	fn namespace() -> String {
		String::from("polarbar-daemon")
	}

	#[allow(clippy::too_many_lines)]
	fn update(&mut self, message: BarMessage) -> Task<BarMessage> {
		match message {
			BarMessage::ConfigUpdated => {
				let (doc, mut config) = match ConfigFile::load() {
					Ok(config) => config,
					Err(error) => {
						log::warn!("Error loading config: {error}");
						return Task::none();
					}
				};

				if let Some(layout) = self.widget_config.latest_layout() {
					config.system_menu = layout.clone();
				}
				self.config_doc = doc;
				self.config_file = config.clone();

				self.broadcast_module_message(ModuleMessage::ConfigUpdated(config))
			}
			BarMessage::Module(_, _, _, ModuleMessage::SaveSystemMenuLayout(layout)) => {
				self.config_file.system_menu = layout.clone();
				self.widget_config.queue_layout(layout)
			}
			BarMessage::SystemMenuWidgetsSaved(result) => {
				if let Err(error) = result {
					log::warn!("Failed to save system-menu layout: {error}");
				}
				self.widget_config.completed()
			}
			BarMessage::Resumed => Task::future(async {
				tokio::time::sleep(Duration::from_millis(500)).await;
				BarMessage::RestartAfterResume
			}),
			BarMessage::RestartAfterResume => restart_bar_process(),
			BarMessage::WindowEvent(id, event) => match event {
				iced::window::Event::Opened { size, .. } | iced::window::Event::Resized(size) => {
					let scale = self.scale_factor(id);
					if let Some(route) = self.popups.route(id).filter(|route| route.depth.is_none())
					{
						if let Some(module) = self.module_mut(route.section, route.index) {
							module.popup_configured(
								id,
								iced::Size::new(size.width / scale, size.height / scale),
							);
						}
					}
					self.sync_layer_scale(id)
				}
				iced::window::Event::Rescaled(_) => self.sync_layer_scale(id),
				iced::window::Event::Closed => self.window_closed(id),
				iced::window::Event::CloseRequested if self.popups.route(id).is_some() => {
					iced_runtime::task::effect(iced_runtime::Action::Window(
						iced_runtime::window::Action::Close(id),
					))
				}
				iced::window::Event::CloseRequested if self.layer_heights.contains_key(&id) => {
					// Layer-shell compositors may keep keyboard focus on the bar
					// while an owned popup holds the grab.
					self.popups.topmost().map_or_else(Task::none, |popup| {
						iced_runtime::task::effect(iced_runtime::Action::Window(
							iced_runtime::window::Action::Close(popup),
						))
					})
				}
				iced::window::Event::Unfocused => self.popup_unfocused(id),
				_ => Task::none(),
			},
			BarMessage::WindowClosed(id) => self.window_closed(id),
			BarMessage::CloseContextDescendants {
				parent,
				expected_child,
			} => self.close_context_descendants(parent, expected_child),
			BarMessage::Module(_, _, _, ModuleMessage::CloseContextMenus) => {
				self.popup_event(popups::Event::CloseContexts)
			}
			BarMessage::Module(
				source_id,
				section,
				index,
				ModuleMessage::OpenPopup(kind, bounds),
			) => {
				let id = Id::unique();
				let change = self
					.popups
					.update(popups::Event::Toggle { id, section, index });
				let opening = change.opened.is_some();
				let close_module_popup = self.apply_popup_change(change);

				if !opening {
					let change_kbd = if let Some(parent_id) = source_id {
						Task::done(BarMessage::KeyboardInteractivityChange {
							id: parent_id,
							keyboard_interactivity: KeyboardInteractivity::OnDemand,
						})
					} else {
						Task::none()
					};
					return Task::batch([close_module_popup, change_kbd]);
				}

				let scale = source_id.map_or(1.0, |id| self.scale_factor(id));
				self.window_scales.insert(id, scale);
				if let Some(size) = source_id.and_then(|id| self.output_sizes.get(&id)).copied() {
					self.output_sizes.insert(id, size);
				}

				let anchor_x = bounds.x.round() as i32;
				let anchor_y = bounds.y.round() as i32;
				let anchor_w = bounds.width.round() as u32;
				let anchor_h = bounds.height.round() as u32;

				let popup_settings = if let Some(parent_id) = source_id {
					IcedNewPopupSettings::new(
						parent_id,
						PixelSize::px(480, 640),
						(anchor_x, anchor_y),
						PixelSize::px(anchor_w.max(1), anchor_h.max(1)),
					)
				} else {
					IcedNewPopupSettings::on_current_surface(
						PixelSize::px(480, 640),
						(anchor_x, anchor_y),
						PixelSize::px(anchor_w.max(1), anchor_h.max(1)),
					)
				}
				.anchor(PopupAnchor::Bottom)
				.gravity(PopupGravity::Bottom)
				.constraint_adjustment(
					PopupConstraintAdjustment::SlideX
						| PopupConstraintAdjustment::SlideY
						| PopupConstraintAdjustment::FlipY,
				);

				let change_kbd = if let Some(parent_id) = source_id {
					Task::done(BarMessage::KeyboardInteractivityChange {
						id: parent_id,
						keyboard_interactivity: KeyboardInteractivity::OnDemand,
					})
				} else {
					Task::none()
				};

				Task::batch([close_module_popup, change_kbd])
					.chain(Task::done(BarMessage::SetPopupId(section, index, kind, id)))
					.chain(Task::done(BarMessage::NewPopUp {
						settings: popup_settings,
						id,
					}))
			}
			BarMessage::Module(
				source_id,
				section,
				index,
				ModuleMessage::OpenContextMenu(bounds),
			) => {
				let Some(source_id) = source_id else {
					return Task::none();
				};
				let id = Id::unique();
				let change = self.popups.update(popups::Event::Context {
					id,
					parent: source_id,
					section,
					index,
				});
				let Some(route) = change.opened else {
					log::debug!("Ignoring context popup request from stale parent {source_id:?}");
					return Task::none();
				};

				let retained = route.depth.unwrap_or(0);
				let scale = self.scale_factor(source_id);
				self.window_scales.insert(id, scale);
				let close_descendants = self.apply_popup_change(change);

				let anchor_x = bounds.x.round() as i32;
				let anchor_y = bounds.y.round() as i32;
				let anchor_w = bounds.width.round() as u32;
				let anchor_h = bounds.height.round() as u32;

				let popup_settings = IcedNewPopupSettings::new(
					source_id,
					PixelSize::px(300, 320),
					(anchor_x, anchor_y),
					PixelSize::px(anchor_w.max(1), anchor_h.max(1)),
				)
				.anchor(if retained == 0 {
					PopupAnchor::Bottom
				} else {
					PopupAnchor::Right
				})
				.gravity(if retained == 0 {
					PopupGravity::Bottom
				} else {
					PopupGravity::Right
				})
				.constraint_adjustment(
					PopupConstraintAdjustment::SlideX
						| PopupConstraintAdjustment::SlideY
						| PopupConstraintAdjustment::FlipX
						| PopupConstraintAdjustment::FlipY,
				);

				close_descendants.chain(Task::done(BarMessage::NewPopUp {
					settings: popup_settings,
					id,
				}))
			}
			BarMessage::Module(
				source_id,
				section,
				index,
				ModuleMessage::OpenTrayContextMenu { service, bounds },
			) => {
				let output_name = source_id
					.and_then(|id| self.window_output_names.get(&id))
					.cloned();
				Task::perform(
					tray_context_menu_position(output_name, bounds),
					move |(x, y)| {
						BarMessage::Module(
							None,
							section,
							index,
							ModuleMessage::InvokeTrayContextMenu { service, x, y },
						)
					},
				)
			}
			BarMessage::Module(
				_,
				_,
				_,
				msg @ (ModuleMessage::OpenSettings | ModuleMessage::OpenPowerMenu),
			) => {
				let close_popup = self.close_open_popup();
				let close_popups = close_popup;

				match msg {
					ModuleMessage::OpenPowerMenu => close_popups.chain(Task::future(async {
						// std::thread::sleep(Duration::from_millis(100));
						open_power_menu();
						BarMessage::Noop
					})),
					ModuleMessage::OpenSettings => close_popups.chain(Task::future(async {
						open_settings();
						BarMessage::Noop
					})),
					_ => unreachable!(),
				}
			}
			BarMessage::Module(_, _, _, ModuleMessage::ClosePopup) => self.close_open_popup(),
			BarMessage::ShellEvent(ShellEvent::Presented {
				window,
				content_updated_at,
			}) => {
				if let Some(route) = self
					.popups
					.route(window)
					.filter(|route| route.depth.is_none())
				{
					if let Some(module) = self.module_mut(route.section, route.index) {
						module.popup_presented(window, content_updated_at);
					}
				}
				Task::none()
			}
			BarMessage::ShellEvent(ShellEvent::WindowOutputChanged { window, output }) => {
				if let Some(info) = output {
					if let Some(name) = info.name {
						self.window_output_names.insert(window, name);
					}
					if let Some((width, height)) = info.logical_size {
						self.output_sizes.insert(
							window,
							iced::Size::new(width.max(1) as f32, height.max(1) as f32),
						);
						let scale = scale_for_screen(height.max(0).cast_unsigned());
						self.window_scales.insert(window, scale);
						if let Some(route) = self
							.popups
							.route(window)
							.filter(|route| route.depth.is_none())
						{
							let maximum = iced::Size::new(
								width.max(1) as f32 / scale,
								(height.max(1) as f32 - BASE_BAR_HEIGHT as f32 * scale).max(1.0)
									/ scale,
							);
							if let Some(module) = self.module_mut(route.section, route.index) {
								module.popup_bounds(maximum);
							}
						}
						return self.sync_layer_scale(window);
					}
				}
				Task::none()
			}
			BarMessage::SetPopupId(section, index, _kind, id) => {
				let scale = self.scale_factor(id);
				let output = self
					.output_sizes
					.get(&id)
					.copied()
					.unwrap_or(iced::Size::new(1920.0, 1080.0));
				let maximum = iced::Size::new(
					output.width / scale,
					(output.height - BASE_BAR_HEIGHT as f32 * scale).max(1.0) / scale,
				);
				if let Some(module) = self.module_mut(section, index) {
					module.popup_opened(id, maximum);
					Task::none()
				} else {
					Task::done(BarMessage::RemoveWindow(id))
				}
			}
			BarMessage::Module(source_id, section, index, message) => {
				if let Some(module) = self.module_mut(section, index) {
					return module
						.update(message)
						.map(move |msg| BarMessage::Module(source_id, section, index, msg));
				}

				Task::none()
			}
			_ => Task::none(),
		}
	}

	fn window_closed(&mut self, id: Id) -> Task<BarMessage> {
		self.layer_heights.remove(&id);
		self.window_scales.remove(&id);
		self.window_output_names.remove(&id);
		self.output_sizes.remove(&id);
		self.popup_event(popups::Event::Closed(id))
	}

	fn popup_unfocused(&mut self, id: Id) -> Task<BarMessage> {
		self.popup_event(popups::Event::Unfocused(id))
	}

	fn close_context_descendants(&mut self, parent: Id, expected_child: Id) -> Task<BarMessage> {
		self.popup_event(popups::Event::CloseDescendants {
			parent,
			expected_child,
		})
	}

	fn popup_event(&mut self, event: popups::Event) -> Task<BarMessage> {
		let change = self.popups.update(event);
		self.apply_popup_change(change)
	}

	fn close_open_popup(&mut self) -> Task<BarMessage> {
		self.popup_event(popups::Event::CloseAll)
	}

	fn apply_popup_change(&mut self, change: popups::Change) -> Task<BarMessage> {
		change
			.effects
			.into_iter()
			.fold(Task::none(), |tasks, effect| {
				let task = match effect {
					popups::Effect::Close(id) => {
						self.window_scales.remove(&id);
						self.layer_heights.remove(&id);
						self.window_output_names.remove(&id);
						self.output_sizes.remove(&id);
						iced_runtime::task::effect(iced_runtime::Action::Window(
							iced_runtime::window::Action::Close(id),
						))
					}
					popups::Effect::NotifyClosed(section, index) => self
						.module_mut(section, index)
						.map_or_else(Task::none, |module| {
							module
								.update(ModuleMessage::PopupClosed)
								.map(move |message| {
									BarMessage::Module(None, section, index, message)
								})
						}),
				};
				tasks.chain(task)
			})
	}

	fn sync_layer_scale(&mut self, id: Id) -> Task<BarMessage> {
		if self.popups.route(id).is_some() {
			return Task::none();
		}

		let height = Self::bar_height_for_scale(self.scale_factor(id));
		if self.layer_heights.insert(id, height) == Some(height) {
			return Task::none();
		}

		Task::batch([
			Task::done(BarMessage::LayoutChange {
				id,
				anchor: Anchor::Top | Anchor::Left | Anchor::Right,
				size: LayerSize::fill_width(height),
			}),
			Task::done(BarMessage::ExclusiveZoneChange {
				id,
				zone_size: height.cast_signed(),
			}),
		])
	}

	fn scale_factor(&self, id: Id) -> f32 {
		self.window_scales.get(&id).copied().unwrap_or(1.0)
	}

	#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
	fn bar_height_for_scale(scale: f32) -> u32 {
		(BASE_BAR_HEIGHT as f32 * scale).round() as u32
	}

	fn view(&self, id: iced::window::Id) -> Element<'_, BarMessage> {
		if let Some(popup) = self.popups.route(id).filter(|route| route.depth.is_some()) {
			let depth = popup.depth.unwrap_or(0);
			if let Some(module) = self.module(popup.section, popup.index) {
				module.view_context_menu(depth).map(move |message| {
					BarMessage::Module(Some(id), popup.section, popup.index, message)
				})
			} else {
				neo_card(text("Context menu unavailable").color(COLORS.text))
					.padding(8)
					.background(COLORS.white)
					.into()
			}
		} else if let Some(popups::Route { section, index, .. }) = self.popups.route(id) {
			let content: Element<'_, BarMessage> = if let Some(module) = self.module(section, index)
			{
				module
					.view_popup()
					.map(move |message| BarMessage::Module(Some(id), section, index, message))
			} else {
				neo_card(text("Something went wrong").color(COLORS.text))
					.background(COLORS.feedback.danger90)
					.into()
			};

			popup_grow(content).into()
		} else {
			let output_name = self.window_output_names.get(&id).map(String::as_str);

			stack![
				container(self.section(id, Section::Left, &self.left, output_name))
					.align_left(Length::Fill)
					.align_y(Vertical::Center)
					.height(Length::Fill)
					.padding([4, 16]),
				container(self.section(id, Section::Center, &self.center, output_name))
					.center_x(Length::Fill)
					.align_y(Vertical::Center)
					.height(Length::Fill)
					.padding([4, 16]),
				container(self.section(id, Section::Right, &self.right, output_name))
					.align_right(Length::Fill)
					.align_y(Vertical::Center)
					.height(Length::Fill)
					.padding([4, 16]),
			]
			.height(Length::Fill)
			.into()
		}
	}

	fn section<'a>(
		&self, id: Id, section: Section, modules: &'a [Module], output_name: Option<&str>,
	) -> Element<'a, BarMessage> {
		let _ = self;
		modules
			.iter()
			.enumerate()
			.fold(row![], |row, (index, module)| {
				row.push(
					module
						.view(output_name)
						.map(move |message| BarMessage::Module(Some(id), section, index, message)),
				)
			})
			.spacing(10.)
			.align_y(iced::Alignment::Center)
			.into()
	}

	fn module_mut(&mut self, section: Section, index: usize) -> Option<&mut Module> {
		match section {
			Section::Left => self.left.get_mut(index),
			Section::Center => self.center.get_mut(index),
			Section::Right => self.right.get_mut(index),
		}
	}

	fn module(&self, section: Section, index: usize) -> Option<&Module> {
		match section {
			Section::Left => self.left.get(index),
			Section::Center => self.center.get(index),
			Section::Right => self.right.get(index),
		}
	}

	fn broadcast_module_message(&mut self, message: ModuleMessage) -> Task<BarMessage> {
		let mut tasks = Vec::new();
		Self::update_modules(Section::Left, &mut self.left, &message, &mut tasks);
		Self::update_modules(Section::Center, &mut self.center, &message, &mut tasks);
		Self::update_modules(Section::Right, &mut self.right, &message, &mut tasks);
		Task::batch(tasks)
	}

	fn update_modules(
		section: Section, modules: &mut [Module], message: &ModuleMessage,
		tasks: &mut Vec<Task<BarMessage>>,
	) {
		for (index, module) in modules.iter_mut().enumerate() {
			tasks.push(
				module
					.update(message.clone())
					.map(move |msg| BarMessage::Module(None, section, index, msg)),
			);
		}
	}

	fn style(&self, _theme: &Theme) -> iced::theme::Style {
		let _ = self;
		iced::theme::Style {
			// background_color: Color::from_rgba(1.0, 0.0, 0.0, 0.5),
			background_color: Color::TRANSPARENT,
			text_color: COLORS.text,
		}
	}

	fn subscription(&self) -> Subscription<BarMessage> {
		let config_watch = Subscription::run(|| {
			ConfigFile::watch().map_or_else(
				|error| {
					log::warn!("Error watching config file: {error}");
					futures::stream::once(async { BarMessage::Noop }).boxed()
				},
				|watch| {
					watch
						.map(|res| match res {
							Ok(()) => BarMessage::ConfigUpdated,
							Err(error) => {
								log::warn!("Invalid config file: {error}");
								BarMessage::Noop
							}
						})
						.boxed()
				},
			)
		});

		let window_events = iced::event::listen_with(|event, status, window| match event {
			iced::Event::Window(iced::window::Event::RedrawRequested(_)) => None,
			iced::Event::Window(window_event) => {
				Some(BarMessage::WindowEvent(window, window_event))
			}
			iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
				key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
				..
			}) if status == iced::event::Status::Ignored => Some(BarMessage::WindowEvent(
				window,
				iced::window::Event::CloseRequested,
			)),
			_ => None,
		});
		let context_children = self.popups.context_children();
		let context_parent_clicks = iced::event::listen_with(|event, _status, window| {
			matches!(
				event,
				iced::Event::Mouse(iced::mouse::Event::ButtonPressed(_))
			)
			.then_some(window)
		})
		.with(context_children)
		.map(|(context_children, window)| {
			if let Some((parent, expected_child)) = context_children
				.into_iter()
				.find(|(parent, _)| *parent == window)
			{
				BarMessage::CloseContextDescendants {
					parent,
					expected_child,
				}
			} else {
				BarMessage::Noop
			}
		});

		let mut subscriptions = vec![
			neo_widgets::icons::background::updates().map(|()| BarMessage::Noop),
			self.shell_events.listen().filter_map(|event| {
				if matches!(event, ShellEvent::Presented { .. }) {
					None
				} else {
					Some(BarMessage::ShellEvent(event))
				}
			}),
			resume_events(),
			config_watch,
			window_events,
			context_parent_clicks,
		];
		// Only presentation waiters should turn feedback into model updates;
		// forwarding every presented frame would create a perpetual redraw loop.
		let presentation_windows = self
			.left
			.iter()
			.chain(&self.center)
			.chain(&self.right)
			.filter_map(Module::presentation_window)
			.collect::<Vec<_>>();
		if !presentation_windows.is_empty() {
			subscriptions.push(
				self.shell_events
					.listen()
					.with(presentation_windows)
					.filter_map(|(windows, event)| {
						if matches!(&event, ShellEvent::Presented { window, .. } if windows.contains(window))
						{
							Some(BarMessage::ShellEvent(event))
						} else {
							None
						}
					}),
			);
		}

		subscriptions.extend(Self::module_subscriptions(Section::Left, &self.left));
		subscriptions.extend(Self::module_subscriptions(Section::Center, &self.center));
		subscriptions.extend(Self::module_subscriptions(Section::Right, &self.right));

		Subscription::batch(subscriptions)
	}

	fn module_subscriptions(
		section: Section, modules: &[Module],
	) -> impl Iterator<Item = Subscription<BarMessage>> + '_ {
		modules.iter().enumerate().map(move |(index, module)| {
			module
				.subscription()
				.with((section, index))
				.map(|((section, index), message)| {
					BarMessage::Module(None, section, index, message)
				})
		})
	}

	fn module_init_tasks(section: Section, modules: &[Module]) -> Task<BarMessage> {
		Task::batch(modules.iter().enumerate().map(move |(index, module)| {
			module
				.init_task()
				.map(move |message| BarMessage::Module(None, section, index, message))
		}))
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Section {
	Left,
	Center,
	Right,
}

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
enum BarMessage {
	ShellEvent(ShellEvent),
	WindowEvent(Id, iced::window::Event),
	WindowClosed(Id),
	CloseContextDescendants { parent: Id, expected_child: Id },
	ConfigUpdated,
	SystemMenuWidgetsSaved(Result<(), String>),
	Resumed,
	RestartAfterResume,
	Module(Option<Id>, Section, usize, ModuleMessage),
	SetPopupId(Section, usize, ModuleKind, Id),
	Noop,
}

fn resume_events() -> Subscription<BarMessage> {
	if running_under_systemd_service() {
		return Subscription::none();
	}

	Subscription::run(|| {
		async_stream::stream! {
			let connection = match zbus::Connection::system().await {
				Ok(connection) => connection,
				Err(error) => {
					log::error!("Failed to connect to system bus for resume events: {error}");
					return;
				}
			};

			let proxy = match zbus::Proxy::new(
				&connection,
				"org.freedesktop.login1",
				"/org/freedesktop/login1",
				"org.freedesktop.login1.Manager",
			)
			.await
			{
				Ok(proxy) => proxy,
				Err(error) => {
					log::error!("Failed to connect to login manager for resume events: {error}");
					return;
				}
			};

			let mut sleep_signals = match proxy.receive_signal("PrepareForSleep").await {
				Ok(sleep_signals) => sleep_signals,
				Err(error) => {
					log::error!("Failed to listen for sleep preparation: {error}");
					return;
				}
			};

			while let Some(signal) = sleep_signals.next().await {
				match signal.body().deserialize::<bool>() {
					Ok(false) => yield BarMessage::Resumed,
					Ok(true) => (),
					Err(error) => log::warn!("Failed to read sleep preparation signal: {error}"),
				}
			}
		}
	})
}

fn restart_bar_process() -> Task<BarMessage> {
	let exe = match env::current_exe() {
		Ok(exe) => exe,
		Err(error) => {
			log::error!("Failed to find current executable for resume restart: {error}");
			return Task::none();
		}
	};

	let mut command = Command::new(exe);
	command.args(env::args_os().skip(1));

	if let Ok(current_dir) = env::current_dir() {
		command.current_dir(current_dir);
	}

	match command.spawn() {
		Ok(_) => iced_runtime::exit(),
		Err(error) => {
			log::error!("Failed to restart bar after resume: {error}");
			Task::none()
		}
	}
}

fn running_under_systemd_service() -> bool {
	fs::read_to_string("/proc/self/cgroup").is_ok_and(|cgroup| {
		cgroup.lines().any(|line| {
			let path = line.rsplit_once(':').map_or(line, |(_, path)| path);
			path.split('/').any(|part| part.ends_with(".service"))
		})
	})
}

async fn tray_context_menu_position(
	output_name: Option<String>, bounds: iced::Rectangle,
) -> (i32, i32) {
	let local = tray_context_menu_point((0, 0), bounds);
	let Some(output_name) = output_name else {
		return local;
	};

	match niri_output_origin(&output_name).await {
		Ok(origin) => tray_context_menu_point(origin, bounds),
		Err(error) => {
			log::warn!("Failed to locate output {output_name} for tray context menu: {error}");
			local
		}
	}
}

fn tray_context_menu_point(origin: (i32, i32), bounds: iced::Rectangle) -> (i32, i32) {
	(
		origin.0 + (bounds.x + bounds.width / 2.0).round() as i32,
		origin.1 + (bounds.y + bounds.height).round() as i32,
	)
}

async fn niri_output_origin(output_name: &str) -> Result<(i32, i32), String> {
	let socket_path = env::var_os(SOCKET_PATH_ENV).ok_or("NIRI_SOCKET is not set")?;
	let mut stream = BufReader::new(
		UnixStream::connect(socket_path)
			.await
			.map_err(|error| error.to_string())?,
	);
	let mut request =
		serde_json::to_string(&Request::Outputs).map_err(|error| error.to_string())?;
	request.push('\n');
	stream
		.write_all(request.as_bytes())
		.await
		.map_err(|error| error.to_string())?;

	let mut response = String::new();
	stream
		.read_line(&mut response)
		.await
		.map_err(|error| error.to_string())?;
	let reply: Reply = serde_json::from_str(&response).map_err(|error| error.to_string())?;
	let response = reply.map_err(|error| error.to_string())?;
	let Response::Outputs(outputs) = response else {
		return Err(format!("unexpected niri response: {response:?}"));
	};
	let output = outputs
		.get(output_name)
		.ok_or_else(|| format!("output not found: {output_name}"))?;
	let logical = output
		.logical
		.as_ref()
		.ok_or_else(|| format!("output has no logical position: {output_name}"))?;
	Ok((logical.x, logical.y))
}

fn open_power_menu() {
	let Some(iceout) = iceout_bin() else {
		log::error!("Failed to find iceout executable");
		return;
	};

	if let Err(e) = Command::new(&iceout).spawn() {
		log::error!("Failed to launch iceout at '{}': {e}", iceout.display());
	}
}

fn iceout_bin() -> Option<PathBuf> {
	option_env!("SUBNIRI_ICEOUT_BIN")
		.map(PathBuf::from)
		.or_else(|| {
			std::env::current_exe()
				.ok()?
				.parent()
				.map(|p| p.join("iceout"))
		})
}

fn open_settings() {
	let Some(snowconf) = snowconf_bin() else {
		log::error!("Failed to find snowconf executable");
		return;
	};

	if let Err(e) = Command::new(&snowconf).spawn() {
		log::error!("Failed to launch snowconf at '{}': {e}", snowconf.display());
	}
}

fn snowconf_bin() -> Option<PathBuf> {
	option_env!("SUBNIRI_SNOWCONF_BIN")
		.map(PathBuf::from)
		.or_else(|| {
			std::env::current_exe()
				.ok()?
				.parent()
				.map(|p| p.join("snowconf"))
		})
}

#[cfg(test)]
mod tests {
	use iced::Rectangle;

	use super::tray_context_menu_point;
	use super::*;

	pub(super) fn empty_bar() -> Bar {
		Bar {
			left: Vec::new(),
			center: Vec::new(),
			right: Vec::new(),
			popups: popups::Popups::default(),
			layer_heights: HashMap::new(),
			window_scales: HashMap::new(),
			window_output_names: HashMap::new(),
			output_sizes: HashMap::new(),
			shell_events: iced_wayland_subscriber::shell::channel().1,
			config_doc: kdl::KdlDocument::default(),
			config_file: ConfigFile::default(),
			widget_config: widget_config::Writer::default(),
		}
	}

	#[test]
	fn system_menu_and_clock_use_the_standard_whole_popup_grow_path() {
		let standard: Element<'_, BarMessage> = popup_grow(text("popup")).into();
		let tag = standard.as_widget().tag();
		for module in [Module::system_menu(&ConfigFile::default()), Module::clock()] {
			let mut bar = empty_bar();
			bar.left.push(module);
			let id = Id::unique();
			bar.popups.update(popups::Event::Toggle {
				id,
				section: Section::Left,
				index: 0,
			});
			assert_eq!(
				bar.view(id).as_widget().tag(),
				tag,
				"root popup must not bypass the common grow wrapper"
			);
		}
	}

	#[tokio::test]
	async fn failed_layout_writes_keep_newer_edits_and_request_final_reload()
	-> Result<(), Box<dyn std::error::Error>> {
		use config::SystemMenuWidgets::{Bluetooth, Wifi};
		let mut bar = empty_bar();
		// Drop write tasks without running them; never touch the user's config.
		let _ = bar.update(BarMessage::Module(
			None,
			Section::Left,
			0,
			ModuleMessage::SaveSystemMenuLayout(config::SystemMenu::from_widgets(vec![Wifi])),
		));
		let _ = bar.update(BarMessage::Module(
			None,
			Section::Left,
			0,
			ModuleMessage::SaveSystemMenuLayout(config::SystemMenu::from_widgets(vec![Bluetooth])),
		));
		assert_eq!(
			bar.widget_config.latest_layout(),
			Some(&config::SystemMenu::from_widgets(vec![Bluetooth]))
		);
		let next = bar.update(BarMessage::SystemMenuWidgetsSaved(Err(
			"first write failed".into(),
		)));
		assert!(next.units() > 0);
		drop(next);
		assert_eq!(
			bar.widget_config.latest_layout(),
			Some(&config::SystemMenu::from_widgets(vec![Bluetooth]))
		);
		let final_task = bar.update(BarMessage::SystemMenuWidgetsSaved(Err(
			"second write failed".into(),
		)));
		let mut actions = iced_runtime::task::into_stream(final_task).ok_or("missing reload")?;
		assert!(matches!(
			actions.next().await,
			Some(iced_runtime::Action::Output(BarMessage::ConfigUpdated))
		));
		assert!(bar.widget_config.latest_layout().is_none());
		assert_eq!(bar.config_file.system_menu.widgets, vec![Bluetooth]);
		Ok(())
	}

	async fn opened_id(task: Task<BarMessage>) -> Result<Id, Box<dyn std::error::Error>> {
		let mut actions = iced_runtime::task::into_stream(task).ok_or("missing task")?;
		while let Some(action) = actions.next().await {
			if let iced_runtime::Action::Output(BarMessage::NewPopUp { id, .. }) = action {
				return Ok(id);
			}
		}
		Err("missing popup creation".into())
	}

	#[tokio::test]
	async fn popup_adapter_preserves_close_before_create_order()
	-> Result<(), Box<dyn std::error::Error>> {
		let mut bar = empty_bar();
		let root = opened_id(bar.update(BarMessage::Module(
			None,
			Section::Left,
			0,
			ModuleMessage::OpenPopup(ModuleKind::SystemMenu, Rectangle::default()),
		)))
		.await?;
		let child = opened_id(bar.update(BarMessage::Module(
			Some(root),
			Section::Left,
			0,
			ModuleMessage::OpenContextMenu(Rectangle::default()),
		)))
		.await?;
		let task = bar.update(BarMessage::Module(
			None,
			Section::Right,
			1,
			ModuleMessage::OpenPopup(ModuleKind::Clock, Rectangle::default()),
		));
		let mut actions = iced_runtime::task::into_stream(task).ok_or("missing task")?;
		let mut sequence = Vec::new();
		while let Some(action) = actions.next().await {
			match action {
				iced_runtime::Action::Window(iced_runtime::window::Action::Close(id)) => {
					sequence.push((0, id));
				}
				iced_runtime::Action::Output(BarMessage::NewPopUp { id, .. }) => {
					sequence.push((1, id));
				}
				_ => {}
			}
		}
		assert!(
			matches!(sequence.as_slice(), [(0, first), (0, second), (1, _)] if *first == child && *second == root)
		);
		assert!(!bar.window_scales.contains_key(&root));
		assert!(!bar.window_scales.contains_key(&child));
		Ok(())
	}

	#[tokio::test]
	async fn popup_focus_and_stale_descendant_requests_preserve_current_chain()
	-> Result<(), Box<dyn std::error::Error>> {
		let mut bar = empty_bar();
		let parent = Id::unique();
		bar.window_scales.insert(parent, 1.2);
		let root = opened_id(bar.update(BarMessage::Module(
			Some(parent),
			Section::Left,
			0,
			ModuleMessage::OpenPopup(ModuleKind::SystemMenu, Rectangle::default()),
		)))
		.await?;
		assert_eq!(bar.scale_factor(root), 1.2);
		let child = opened_id(bar.update(BarMessage::Module(
			Some(root),
			Section::Left,
			0,
			ModuleMessage::OpenContextMenu(Rectangle::default()),
		)))
		.await?;
		let _ = bar.popup_unfocused(root);
		assert!(bar.popups.route(root).is_some());
		let _ = bar.close_context_descendants(root, Id::unique());
		assert!(bar.popups.route(child).is_some());
		let _ = bar.update(BarMessage::Module(
			Some(Id::unique()),
			Section::Left,
			0,
			ModuleMessage::OpenContextMenu(Rectangle::default()),
		));
		assert!(bar.popups.route(child).is_some());
		let _ = bar.popup_unfocused(child);
		assert!(bar.popups.route(child).is_none());
		assert!(bar.popups.route(root).is_some());
		let _ = bar.popup_unfocused(root);
		assert!(bar.popups.route(root).is_none());
		assert!(!bar.window_scales.contains_key(&root));
		assert_eq!(bar.window_closed(root).units(), 0);
		Ok(())
	}

	#[test]
	fn tray_context_menu_point_uses_icon_bottom_center_on_output() {
		let bounds = Rectangle {
			x: 12.4,
			y: 30.2,
			width: 24.0,
			height: 24.0,
		};

		assert_eq!(tray_context_menu_point((1920, -100), bounds), (1944, -46));
	}
}
