use std::sync::Arc;

use clap::Parser;
use config::{ConfigFile, LauncherProvider};
use iced_layershell::{
	daemon,
	reexport::{Anchor, KeyboardInteractivity, Layer},
	settings::{LayerShellSettings, StartMode},
};

use crate::{
	launcher::Launcher,
	providers::{Provider, applications::ApplicationProvider, calculator::CalcProvider},
};

mod dbus;
mod launcher;
mod providers;
mod utils;

#[derive(Parser, Clone)]
#[command(version, about)]
struct Args {
	/// Open the launcher on startup
	#[clap(long)]
	open: bool,
	/// Exit the launcher on close
	#[clap(long)]
	exit_on_close: bool,
	/// Don't close the launcher when loosing focus
	#[clap(long)]
	no_focus: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let _ = pretty_env_logger::try_init();

	let args = Args::parse();

	let (_doc, config) = ConfigFile::load()
		.inspect_err(|e| log::error!("Failed to load config: {e}"))
		.unwrap_or_default();

	let providers: Vec<Arc<dyn Provider>> = config
		.launcher
		.providers
		.iter()
		.map(|provider| match provider {
			LauncherProvider::Applications => Arc::new(ApplicationProvider::new(
				config.launcher.typo_search.clone(),
			)) as _,
			LauncherProvider::Calculator => Arc::new(CalcProvider::new()) as _,
		})
		.collect::<Vec<_>>();
	let providers = Arc::<[Arc<dyn Provider>]>::from(providers);

	let app = daemon(
		move || Launcher::new(&args, providers.clone()),
		Launcher::namespace,
		Launcher::update,
		Launcher::view,
	)
	.style(Launcher::style)
	.subscription(Launcher::subscription)
	.layer_settings(LayerShellSettings {
		size: None,
		anchor: Anchor::all(),
		start_mode: StartMode::Background,
		layer: Layer::Overlay,
		keyboard_interactivity: KeyboardInteractivity::None,
		..Default::default()
	});

	tokio::task::block_in_place(move || app.run().map_err(Into::into))
}
