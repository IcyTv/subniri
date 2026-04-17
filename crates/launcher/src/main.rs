use gtk4::prelude::*;
use process_guard::{EnsureOutcome, ExistingInstancePolicy};

fn main() {
	if let EnsureOutcome::AlreadyRunning =
		process_guard::ensure_single_instance("subniri-launcher", ExistingInstancePolicy::ReplaceExisting)
	{
		return;
	}

	gtk4::init().expect("Failed to initialize GTK4");

	let app = gtk4::Application::builder()
		.application_id("com.icytv.subniri.launcher")
		.build();

	app.connect_activate(|_| {
		let main_loop = glib::MainLoop::new(None, false);
		main_loop.run();
	});

	app.run_with_args::<String>(&[]);
}
