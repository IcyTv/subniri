use std::sync::Arc;

use clap::{Parser, builder::PossibleValue};
use config::{ConfigFile, LauncherProvider};
use iced_layershell::{
	daemon,
	reexport::{Anchor, KeyboardInteractivity, Layer},
	settings::{LayerShellSettings, StartMode},
};

use crate::{
	launcher::Launcher,
	providers::{
		Provider, applications::ApplicationProvider, calculator::CalcProvider, files::FileProvider,
		nix_shell::NixShellProvider,
	},
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

	#[clap(short, long, num_args = 0..)]
	providers: Vec<ClapLauncherProvider>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	log::init!("launcher", "avalaunch")?;

	let args = Args::parse();

	let (_doc, config) = ConfigFile::load()
		.inspect_err(|e| log::error!("Failed to load config: {e}"))
		.unwrap_or_default();

	let providers = if args.providers.is_empty() {
		config.launcher.providers.clone()
	} else {
		args.providers.clone().into_iter().map(|p| p.0).collect()
	};
	log::trace!("Providers: {:?}", providers);

	let providers: Vec<Arc<dyn Provider>> = providers
		.into_iter()
		.map(|provider| match provider {
			LauncherProvider::Applications => Arc::new(ApplicationProvider::new(
				config.launcher.fuzzy_search.clone(),
			)) as _,
			LauncherProvider::Calculator => Arc::new(CalcProvider::new()) as _,
			LauncherProvider::Files => Arc::new(FileProvider::new()) as _,
			LauncherProvider::Nix => Arc::new(NixShellProvider::new()) as _,
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

#[derive(Clone, Debug)]
#[repr(transparent)]
struct ClapLauncherProvider(LauncherProvider);

impl clap::ValueEnum for ClapLauncherProvider {
	fn value_variants<'a>() -> &'a [Self] {
		&[
			Self(LauncherProvider::Applications),
			Self(LauncherProvider::Calculator),
			Self(LauncherProvider::Files),
			Self(LauncherProvider::Nix),
		]
	}

	fn to_possible_value(&self) -> Option<PossibleValue> {
		match self.0 {
			LauncherProvider::Applications => {
				Some(PossibleValue::new("applications").alias("apps"))
			}
			LauncherProvider::Calculator => Some(PossibleValue::new("calculator").alias("calc")),
			LauncherProvider::Files => Some(PossibleValue::new("files")),
			LauncherProvider::Nix => Some(PossibleValue::new("nix")),
		}
	}

	fn from_str(input: &str, _ignore_case: bool) -> Result<Self, String> {
		match input.to_lowercase().as_str() {
			"applications" | "apps" => Ok(Self(LauncherProvider::Applications)),
			"calculator" | "calc" => Ok(Self(LauncherProvider::Calculator)),
			"files" => Ok(Self(LauncherProvider::Files)),
			"nix" => Ok(Self(LauncherProvider::Nix)),
			_ => Err(format!("Invalid provider: {input}")),
		}
	}
}
