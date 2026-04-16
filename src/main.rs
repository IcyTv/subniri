use clap::Parser;
use gtk4::gdk::Display;
use gtk4::prelude::*;
use process_guard::{EnsureOutcome, ExistingInstancePolicy};

#[derive(Clone, Parser)]
pub struct Args {
	/// Run with gtk inspector enabled
	#[clap(long)]
	inspect: bool,

	#[clap(long)]
	launcher: bool,
}

fn main() {
	if let EnsureOutcome::AlreadyRunning =
		process_guard::ensure_single_instance("subniri-shell", ExistingInstancePolicy::ReplaceExisting)
	{
		return;
	}

	let args = Args::parse();

	gtk4::init().expect("Failed to initialize GTK4");

	let app = gtk4::Application::builder().application_id("com.icytv.subniri").build();

	gtk4::Window::set_interactive_debugging(args.inspect);

	app.connect_startup(|_| {
		println!("=== STARTUP CALLED ===");
		load_css();
		bar::init_resources();
		icons::register_bundled_icons();
	});
	app.connect_activate(build_ui(args.clone()));

	app.run_with_args::<String>(&[]);
}
fn load_css() {
	let provider = gtk4::CssProvider::new();
	provider.load_from_string(include_str!("./style.css"));

	gtk4::style_context_add_provider_for_display(
		&Display::default().unwrap(),
		&provider,
		gtk4::STYLE_PROVIDER_PRIORITY_USER,
	);
}

fn build_ui(_args: Args) -> impl Fn(&gtk4::Application) {
	move |app| {
		let display = Display::default().expect("Could not get a display");
		let notifications_overlay = bar::NotificationsOverlay::new_primary(&display);
		let player_model = bar::player::PlayerModel::new();
		let (command_send, command_recv) = bar::player::channel();
		bar::player::spawn_controller(player_model.clone(), command_recv);

		let player_manager = bar::dbus::DbusManager::new(command_send.clone());

		let bars = bar::Bar::for_all_monitors(&display, player_model.clone(), command_send);

		for bar in bars {
			app.add_window(&bar.window);
			bar.window.present();
		}

		if let Some(overlay) = notifications_overlay {
			unsafe {
				app.set_data("bar.notifications-overlay", overlay.clone());
			}
			app.add_window(&overlay.window);
		}

		unsafe {
			app.set_data("bar.player-model", player_model.clone());
		}

		let app = app.clone();
		gtk4::glib::spawn_future_local(async move {
			let conn = zbus::connection::Builder::session()
				.unwrap()
				.name("de.icytv.subniri.Bar")
				.unwrap()
				.serve_at("/de/icytv/subniri/Bar", player_manager)
				.unwrap()
				.build()
				.await
				.unwrap();

			unsafe {
				app.set_data("bar.player-manager-connection", conn);
			}
		});
	}
}
