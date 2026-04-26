use clap::Parser;
use gtk4::{gdk::Display, prelude::*};
use process_guard::{EnsureOutcome, ExistingInstancePolicy};

use crate::{
	dbus::{LauncherEvent, LauncherManager},
	window::MainLauncherWindow,
};

mod backdrop;
mod candidate;
mod candidate_row;
mod dbus;
mod launcher;
mod window;

#[derive(Parser)]
struct Args {
	#[arg(long)]
	now: bool,
}

fn main() {
	if let EnsureOutcome::AlreadyRunning =
		process_guard::ensure_single_instance("subniri-launcher", ExistingInstancePolicy::ReplaceExisting)
	{
		return;
	}

	let args = Args::parse();

	gtk4::init().expect("Failed to initialize GTK4");

	let app = gtk4::Application::builder()
		.application_id("com.icytv.subniri.launcher")
		.build();

	app.connect_startup(|_| {
		load_css();
		icons::register_bundled_icons();
	});

	app.connect_activate(move |app| {
		let (launcher_manager, receiver) = LauncherManager::new();

		let window = MainLauncherWindow::new();
		app.add_window(&window.window);

		if args.now {
			window.window.present();
			window.launcher_widget.focus_input();
		}

		glib::spawn_future_local(async move {
			while let Ok(event) = receiver.recv().await {
				match event {
					LauncherEvent::Launch => {
						window.window.present();
						window.launcher_widget.focus_input();
					}
					LauncherEvent::Hide => window.window.set_visible(false),
				}
			}
		});

		let app = app.clone();
		glib::spawn_future_local(async move {
			let conn = zbus::connection::Builder::session()
				.unwrap()
				.name("de.icytv.subniri.Launcher")
				.unwrap()
				.serve_at("/de/icytv/subniri/Launcher", launcher_manager)
				.unwrap()
				.build()
				.await
				.unwrap();

			unsafe {
				app.set_data("launcher.dbus-connection", conn);
			}
		});
	});

	app.run_with_args::<String>(&[]);
}

fn load_css() {
	let provider = gtk4::CssProvider::new();
	provider.load_from_string(include_str!("../../../style.css"));

	gtk4::style_context_add_provider_for_display(
		&Display::default().expect("Could not get a display"),
		&provider,
		gtk4::STYLE_PROVIDER_PRIORITY_USER,
	);
}
