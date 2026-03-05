mod folder_button;
mod image;

use std::path::PathBuf;

use astal_apps::Apps;
use astal_apps::prelude::*;
use gtk4::CompositeTemplate;
use gtk4::gio::{self};
use gtk4::prelude::{BoxExt, *};
use gtk4::subclass::prelude::*;
use niri_ipc::socket::Socket;
use niri_ipc::{Action, Request, Response};

use crate::icons::Icon;

static FOLDERS: &[(&str, Icon, fn() -> Option<PathBuf>)] = &[
	("Downloads", Icon::FolderDown, dirs::download_dir),
	("Documents", Icon::BriefcaseBusiness, dirs::document_dir),
	("Videos", Icon::Folder, dirs::video_dir),
	("Pictures", Icon::Folder, dirs::picture_dir),
	("Projects", Icon::FolderCode, || {
		dirs::home_dir().map(|h| h.join("projects"))
	}),
	("Home", Icon::Folder, dirs::home_dir),
];

static POWER: &[(Icon, &str, fn())] = &[
	(Icon::Power, "power-button", power_off),
	(Icon::RotateCcw, "restart-button", restart),
	(Icon::LogOut, "logout-button", logout),
	(Icon::Moon, "suspend-button", suspend),
];

static QUICKLAUNCH_APPS: &[(Icon, &str, fn())] = &[
	(Icon::Firefox, "firefox-button", launch_firefox),
	(Icon::Discord, "discord-button", launch_discord),
	(Icon::Spotify, "spotify-button", launch_spotify),
	(Icon::Search, "search-button", launch_search),
];

static TOOLS: &[(Icon, &str, fn())] = &[
	(Icon::Pipette, "colorpicker-button", noop),
	(Icon::Terminal, "terminal-button", launch_terminal),
	(Icon::Camera, "screenshot-button", screenshot),
	(Icon::Circle, "record-button", noop),
];

static STATUS_BUTTONS: &[(Icon, &str)] = &[
	(Icon::Wifi, "wifi-button"),
	(Icon::Bluetooth, "bluetooth-button"),
	(Icon::Bell, "notifications-button"),
	(Icon::Volume2, "speaker-button"),
	(Icon::Mic, "microphone-button"),
];

glib::wrapper! {
	pub struct LauncherPopup(ObjectSubclass<imp::LauncherPopup>)
		@extends gtk4::Popover, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Buildable, gtk4::Constraint, gtk4::ConstraintTarget, gtk4::ShortcutManager, gtk4::Native;
}

impl LauncherPopup {
	pub fn new() -> Self {
		glib::Object::builder().build()
	}
}

mod imp {

	use super::*;
	use crate::popups::launcher::folder_button::FolderButton;

	#[derive(Default, CompositeTemplate)]
	#[template(file = "./src/popups/launcher/launcher.blp")]
	pub struct LauncherPopup {
		#[template_child]
		gif_box: TemplateChild<gtk4::Box>,
		#[template_child]
		power_icons: TemplateChild<gtk4::Box>,
		#[template_child]
		quick_apps_grid: TemplateChild<gtk4::Grid>,
		#[template_child]
		tool_grid: TemplateChild<gtk4::Grid>,
		#[template_child]
		status_buttons: TemplateChild<gtk4::Box>,
		#[template_child]
		folder_grid: TemplateChild<gtk4::Grid>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for LauncherPopup {
		type ParentType = gtk4::Popover;
		type Type = super::LauncherPopup;

		const NAME: &'static str = "LauncherPopup";

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
			// Self::Type::bind_template_callback(klass);
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	impl ObjectImpl for LauncherPopup {
		fn constructed(&self) {
			self.parent_constructed();

			for (index, (label, icon, dir)) in FOLDERS.iter().enumerate() {
				let row = (index / 2) as i32;
				let col = (index % 2) as i32;

				let folder_button = FolderButton::new(*icon, label);

				if let Some(dir) = dir() {
					let dir = dir.clone();
					let dir = dir.to_string_lossy();
					let uri = format!("file://{}", dir);
					folder_button.connect_clicked(move |_| {
						println!("Opening folder: {}", uri.as_str());
						let _ = gio::AppInfo::launch_default_for_uri(uri.as_str(), None::<&gio::AppLaunchContext>);
					});
				} else {
					println!("Directory for {} not found", label);
				}

				self.folder_grid.attach(&folder_button, col, row, 1, 1);
			}

			let gif_image = image::LauncherImage::random();
			(*self.gif_box).append(&gif_image);

			for (icon, class_name, on_click) in POWER {
				let button = gtk4::Button::builder()
					.css_classes(vec![class_name.to_string()])
					.icon_name(icon.name())
					.vexpand(true)
					.hexpand(true)
					.build();

				button.connect_clicked(move |_| {
					on_click();
				});

				self.power_icons.append(&button);
			}

			for (index, (icon, class_name, on_click)) in QUICKLAUNCH_APPS.iter().enumerate() {
				let row = (index / 2) as i32;
				let col = (index % 2) as i32;

				let button = gtk4::Button::builder()
					.css_classes(vec![class_name.to_string()])
					.icon_name(icon.name())
					.hexpand(true)
					.build();

				button.connect_clicked(move |_| {
					on_click();
				});

				self.quick_apps_grid.attach(&button, col, row, 1, 1);
			}

			for (index, (icon, class_name, on_click)) in TOOLS.iter().enumerate() {
				let row = (index / 2) as i32;
				let col = (index % 2) as i32;

				let button = gtk4::Button::builder()
					.css_classes(vec![class_name.to_string()])
					.icon_name(icon.name())
					.hexpand(true)
					.build();

				button.connect_clicked(move |_| {
					on_click();
				});

				self.tool_grid.attach(&button, col, row, 1, 1);
			}

			for (icon, class_name) in STATUS_BUTTONS {
				let button = gtk4::Button::builder()
					.css_classes(vec![class_name.to_string()])
					.icon_name(icon.name())
					.hexpand(true)
					.build();
				self.status_buttons.append(&button);
			}
		}
	}

	impl WidgetImpl for LauncherPopup {}

	impl PopoverImpl for LauncherPopup {}
}

fn power_off() {
	match system_shutdown::shutdown() {
		Ok(_) => (),
		Err(error) => {
			// Open a message dialog with the error
			let dialog = gtk4::AlertDialog::builder()
				.buttons(["OK"])
				.message("Failed to power off")
				.detail(&error.to_string())
				.build();
			dialog.show(None::<&gtk4::Window>);
		}
	}
}

fn restart() {
	match system_shutdown::reboot() {
		Ok(_) => (),
		Err(error) => {
			// Open a message dialog with the error
			let dialog = gtk4::AlertDialog::builder()
				.buttons(["OK"])
				.message("Failed to restart")
				.detail(&error.to_string())
				.build();
			dialog.show(None::<&gtk4::Window>);
		}
	}
}

fn logout() {
	match system_shutdown::logout() {
		Ok(_) => (),
		Err(error) => {
			// Open a message dialog with the error
			let dialog = gtk4::AlertDialog::builder()
				.buttons(["OK"])
				.message("Failed to log out")
				.detail(&error.to_string())
				.build();
			dialog.show(None::<&gtk4::Window>);
		}
	}
}

fn suspend() {
	match system_shutdown::sleep() {
		Ok(_) => (),
		Err(error) => {
			// Open a message dialog with the error
			let dialog = gtk4::AlertDialog::builder()
				.buttons(["OK"])
				.message("Failed to suspend")
				.detail(&error.to_string())
				.build();
			dialog.show(None::<&gtk4::Window>);
		}
	}
}

fn launch(name: &str) {
	let apps = Apps::default();
	let mut apps = apps.exact_query(Some(name));
	apps.sort_by_key(|app| app.name().len());

	if let Some(app) = apps.first() {
		app.launch();
	} else {
		eprintln!("Failed to launch app: {}", name);
	}
}

fn focus_or_launch(name: &str) {
	let socket = Socket::connect();
	if let Ok(mut socket) = socket {
		let windows = match socket.send(Request::Windows) {
			Ok(Ok(Response::Windows(windows))) => windows,
			_ => {
				launch(name);
				return;
			}
		};

		let apps = Apps::default();
		let apps = apps.exact_query(Some(name));
		let app_ids = apps
			.into_iter()
			.map(|a| a.app())
			.filter_map(|info| info.id())
			.map(|id| id.strip_suffix(".desktop").unwrap_or(&*id).to_string())
			.collect::<Vec<_>>();

		println!("App IDs for {}: {:?}", name, app_ids);

		for window in windows {
			if let Some(app_id) = &window.app_id {
				println!("Window app ID: {}", app_id);
				if app_ids.contains(app_id) {
					let _ = socket.send(Request::Action(Action::FocusWindow { id: window.id }));
					return;
				}
			}
		}

		launch(name);
	} else {
		launch(name);
	}
}

fn launch_firefox() {
	launch("Firefox");
}

fn launch_discord() {
	focus_or_launch("Discord")
}

fn launch_spotify() {
	focus_or_launch("Spotify")
}

fn launch_search() {
	let _ = gio::AppInfo::launch_default_for_uri("search:", None::<&gio::AppLaunchContext>);
}

fn screenshot() {
	let mut socket = Socket::connect().expect("niri socket");

	let reply = socket.send(Request::Action(Action::Screenshot {
		show_pointer: false,
		path: None,
	}));
	match reply {
		Ok(Ok(Response::Handled)) => (),
		_ => eprintln!("Failed to take screenshot"),
	}
}

fn launch_terminal() {
	launch("kitty"); // TODO: support other terminals... Maybe use xdg to figure out what terminals
	// exist? Maybe .desktop categories?
}

fn noop() {}
