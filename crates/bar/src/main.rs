use gtk4::gdk::Display;
use gtk4::prelude::*;
use process_guard::{EnsureOutcome, ExistingInstancePolicy};

fn main() {
	if let EnsureOutcome::AlreadyRunning =
		process_guard::ensure_single_instance("subniri-bar", ExistingInstancePolicy::ReplaceExisting)
	{
		return;
	}

	gtk4::init().expect("Failed to initialize GTK4");

	let app = gtk4::Application::builder()
		.application_id("com.icytv.subniri.bar")
		.build();

	app.connect_startup(|_| {
		bar::init_resources();
		icons::register_bundled_icons();
	});

	app.connect_activate(|app| {
		let display = Display::default().expect("Could not get a display");
		let notifications_overlay = bar::NotificationsOverlay::new_primary(&display);
		let bars = bar::Bar::for_all_monitors(&display);

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
	});

	app.run_with_args::<String>(&[]);
}
