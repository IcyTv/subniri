use clap::Parser;
use gtk4::gdk::Display;
use gtk4::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

#[derive(Clone, Parser)]
pub struct Args {
	/// Run with gtk inspector enabled
	#[clap(long)]
	inspect: bool,

	#[clap(long)]
	launcher: bool,
}

fn main() {
	ensure_single_instance();

	let args = Args::parse();

	gtk4::init().expect("Failed to initialize GTK4");

	let app = gtk4::Application::builder().application_id("com.icytv.niribar").build();

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

fn ensure_single_instance() {
	let pid_file = pid_file_path();

	if let Some(existing_pid) = read_existing_pid(&pid_file)
		&& existing_pid != std::process::id()
		&& is_niribar_process(existing_pid)
	{
		let _ = Command::new("kill").args(["-TERM", &existing_pid.to_string()]).status();

		for _ in 0..20 {
			if !Path::new(&format!("/proc/{existing_pid}")).exists() {
				break;
			}
			thread::sleep(Duration::from_millis(100));
		}
	}

	if let Some(parent) = pid_file.parent() {
		let _ = fs::create_dir_all(parent);
	}

	let _ = fs::write(&pid_file, std::process::id().to_string());
}

fn pid_file_path() -> PathBuf {
	if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
		return Path::new(&runtime_dir).join("niribar.pid");
	}

	Path::new("/tmp").join("niribar.pid")
}

fn read_existing_pid(pid_file: &Path) -> Option<u32> {
	let pid_text = fs::read_to_string(pid_file).ok()?;
	pid_text.trim().parse::<u32>().ok()
}

fn is_niribar_process(pid: u32) -> bool {
	let cmdline_path = format!("/proc/{pid}/cmdline");
	let Ok(cmdline) = fs::read(cmdline_path) else {
		return false;
	};

	cmdline
		.split(|byte| *byte == 0)
		.filter(|part| !part.is_empty())
		.filter_map(|part| std::str::from_utf8(part).ok())
		.any(|part| part.contains("niribar"))
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
				app.set_data("niribar.notifications-overlay", overlay.clone());
			}
			app.add_window(&overlay.window);
		}
	}
}
