use config::ConfigFile;
use iced::alignment::Vertical;
use std::{collections::HashMap, env, fs, path::PathBuf, process::Command, time::Duration};

use futures::StreamExt;
use iced::Length;
use iced::widget::{container, row, stack, text};
use iced::window::Id;
use iced::{Color, Element, Subscription, Task, Theme};
use iced_layershell::actions::{IcedNewPopupSettings, PopupPlacement, PopupSize};
use iced_layershell::reexport::{Anchor, xdg_positioner::ConstraintAdjustment};
use iced_layershell::settings::{LayerShellSettings, StartMode};
use iced_layershell::{Settings, daemon, to_layer_message};
use neo_widgets::{
	style::{COLORS, neo_theme},
	widgets::neo_card,
};
use wayland_client::Connection;

use crate::modules::{Module, ModuleKind, ModuleMessage};

mod modules;

// mod clock;
// mod icons;
// mod mpris;
fn main() -> Result<(), Box<dyn std::error::Error>> {
	let _ = pretty_env_logger::try_init();

	let connection = Connection::connect_to_env()?;

	let (doc, config) = ConfigFile::load()?;

	let app = daemon(
		{
			let conn = connection.clone();
			// FIXME: Don't clone
			let doc = doc.clone();
			let config = config.clone();
			move || Bar::new(&conn, doc.clone(), config.clone())
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
		..Default::default()
	})
	.layer_settings(LayerShellSettings {
		size: Some((0, BASE_BAR_HEIGHT)),
		exclusive_zone: BASE_BAR_HEIGHT.cast_signed(),
		anchor: Anchor::Top | Anchor::Left | Anchor::Right,
		start_mode: StartMode::AllScreens,
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

	open_popup: Option<(Id, Section, usize)>,
	layer_heights: HashMap<Id, u32>,
	window_scales: HashMap<Id, f32>,
	#[expect(dead_code)]
	config_doc: kdl::KdlDocument,
	config_file: ConfigFile,
}

impl Bar {
	fn new(
		connection: &Connection, config_doc: kdl::KdlDocument, config_file: ConfigFile,
	) -> (Self, Task<BarMessage>) {
		let _ = connection;

		let bar = Self {
			left: vec![Module::system_menu(&config_file), Module::taskbar()],
			center: vec![Module::media_controls()],
			right: vec![
				Module::volume(),
				Module::Network,
				Module::bluetooth(),
				Module::clock(),
			],
			open_popup: None,
			layer_heights: HashMap::new(),
			window_scales: HashMap::new(),
			config_doc,
			config_file,
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
			BarMessage::Resumed => Task::future(async {
				tokio::time::sleep(Duration::from_millis(500)).await;
				BarMessage::RestartAfterResume
			}),
			BarMessage::RestartAfterResume => restart_bar_process(),
			BarMessage::WindowEvent(id, event) => match event {
				iced::window::Event::Opened { .. }
				| iced::window::Event::Resized(_)
				| iced::window::Event::Rescaled(_)
				| iced::window::Event::RedrawRequested(_) => self.sync_layer_scale(id),
				iced::window::Event::Closed => self.window_closed(id),
				_ => Task::none(),
			},
			BarMessage::WindowClosed(id) => self.window_closed(id),
			BarMessage::Module(
				source_id,
				section,
				index,
				ModuleMessage::OpenPopup(kind, bounds),
			) => {
				let id = Id::unique();
				let scale = source_id.map_or(1.0, |id| self.scale_factor(id));
				self.window_scales.insert(id, scale);

				let task = if let Some(open_popup_id) = self.open_popup.take() {
					self.window_scales.remove(&open_popup_id.0);
					iced_runtime::task::effect(iced_runtime::Action::Window(
						iced_runtime::window::Action::Close(open_popup_id.0),
					))
				} else {
					Task::none()
				};
				self.open_popup = Some((id, section, index));

				task.chain(Task::done(BarMessage::NewPopUp {
					settings: IcedNewPopupSettings {
						size: PopupSize::FitContent {
							min: (1, 1),
							max: (480, 640),
						},
						#[allow(clippy::cast_possible_truncation)]
						anchor_rect: (
							bounds.x.round() as i32,
							bounds.y.round() as i32,
							bounds.width.round() as i32,
							bounds.height.round() as i32,
						),
						offset: (0, 8),
						placement: PopupPlacement::BottomCenter,
						constraint_adjustment: ConstraintAdjustment::SlideX
							| ConstraintAdjustment::SlideY
							| ConstraintAdjustment::FlipX
							| ConstraintAdjustment::FlipY,
					},
					id,
				}))
				.chain(Task::done(BarMessage::SetPopupId(section, index, kind, id)))
			}
			BarMessage::Module(
				_,
				_,
				_,
				msg @ (ModuleMessage::OpenSettings | ModuleMessage::OpenPowerMenu),
			) => {
				let close_popup = if let Some(open_popup_id) = self.open_popup.take() {
					self.window_scales.remove(&open_popup_id.0);
					iced_runtime::task::effect(iced_runtime::Action::Window(
						iced_runtime::window::Action::Close(open_popup_id.0),
					))
				} else {
					Task::none()
				};

				match msg {
					ModuleMessage::OpenPowerMenu => close_popup.chain(Task::future(async {
						// std::thread::sleep(Duration::from_millis(100));
						open_power_menu();
						BarMessage::Noop
					})),
					ModuleMessage::OpenSettings => close_popup.chain(Task::future(async {
						open_settings();
						BarMessage::Noop
					})),
					_ => unreachable!(),
				}
			}
			BarMessage::SetPopupId(section, index, _kind, id) => {
				if let Some(module) = self.module_mut(section, index) {
					module.set_popup_id(id);
					Task::none()
				} else {
					Task::done(BarMessage::RemoveWindow(id))
				}
			}
			BarMessage::Module(_, section, index, message) => {
				let cf = self.config_file.clone();
				if let Some(module) = self.module_mut(section, index) {
					return module
						.update(message, &cf)
						.map(move |msg| BarMessage::Module(None, section, index, msg));
				}

				Task::none()
			}
			_ => Task::none(),
		}
	}

	fn window_closed(&mut self, id: Id) -> Task<BarMessage> {
		self.layer_heights.remove(&id);
		self.window_scales.remove(&id);
		if self.open_popup.as_ref().is_some_and(|oid| oid.0 == id) {
			self.open_popup = None;
		}
		Task::none()
	}

	fn sync_layer_scale(&mut self, id: Id) -> Task<BarMessage> {
		if self.open_popup.as_ref().is_some_and(|oid| oid.0 == id) {
			return Task::none();
		}

		let height = Self::bar_height_for_scale(self.scale_factor(id));
		if self.layer_heights.insert(id, height) == Some(height) {
			return Task::none();
		}

		Task::batch([
			Task::done(BarMessage::SizeChange {
				id,
				size: (0, height),
			}),
			Task::done(BarMessage::ExclusiveZoneChange {
				id,
				zone_size: height.cast_signed(),
			}),
		])
	}

	fn scale_factor(&self, id: Id) -> f32 {
		if let Some(scale) = self.window_scales.get(&id) {
			return *scale;
		}

		let Some((_, height)) = iced_layershell::window::output_logical_size(id) else {
			return 1.0;
		};

		scale_for_screen(height.max(0).cast_unsigned())
	}

	#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
	fn bar_height_for_scale(scale: f32) -> u32 {
		(BASE_BAR_HEIGHT as f32 * scale).round() as u32
	}

	fn view(&self, id: iced::window::Id) -> Element<'_, BarMessage> {
		if let Some((wid, section, index)) = &self.open_popup
			&& *wid == id
		{
			// neo_card("A").background(COLORS.background).into()
			if let Some(module) = self.module(*section, *index) {
				module
					.view_popup()
					.map(move |message| BarMessage::Module(Some(id), *section, *index, message))
			} else {
				neo_card(text("Something went wrong").color(COLORS.text))
					.background(COLORS.feedback.danger90)
					.into()
			}
		} else {
			let output_name = iced_layershell::window::output_name(id);
			let output_name = output_name.as_deref();

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

	fn style(&self, _theme: &Theme) -> iced::theme::Style {
		let _ = self;
		iced::theme::Style {
			// background_color: Color::from_rgba(1.0, 0.0, 0.0, 0.5),
			background_color: Color::TRANSPARENT,
			text_color: COLORS.text,
		}
	}

	fn subscription(&self) -> Subscription<BarMessage> {
		let mut subscriptions = vec![
			iced::window::events().map(|(id, event)| BarMessage::WindowEvent(id, event)),
			iced::window::close_events().map(BarMessage::WindowClosed),
			resume_events(),
		];

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
	WindowEvent(Id, iced::window::Event),
	WindowClosed(Id),
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
