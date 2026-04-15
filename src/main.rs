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
		let bars = bar::Bar::for_all_monitors(&display);
		for bar in bars {
			app.add_window(&bar.window);
			bar.window.present();
		}

		if let Some(overlay) = notifications_overlay {
			// SAFETY: `gtk4::Application` is a GObject and stores the overlay
			// for the full application lifetime.
			unsafe {
				app.set_data("subniri.notifications-overlay", overlay.clone());
			}
			app.add_window(&overlay.window);
		}
	}
}
