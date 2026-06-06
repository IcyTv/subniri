use clap::{Parser, Subcommand};
use cliclack::{confirm, intro, log, note, outro, outro_cancel, spinner};
use comfy_table::{Cell, Color, Table, modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL};
use daemon::NightlightProxy;
use zbus::{Connection, names::BusName, proxy::Defaults, zvariant::ObjectPath};

#[derive(Parser)]
struct Args {
	#[clap(subcommand)]
	command: Command,

	#[clap(long, global = true)]
	/// Accept any prompts with yes by default.
	///
	/// This is mainly for usage with non-TTY terminals and/or tools
	yes: bool,
}

#[derive(Subcommand)]
enum Command {
	Nightlight {
		#[clap(subcommand)]
		command: NightlightSubcommand,
	},
	Launcher {
		#[clap(subcommand)]
		command: LauncherCommand,
	},
}

#[derive(clap::ValueEnum, Clone, Default, Debug)]
enum NightlightPreset {
	#[default]
	Day,
	Night,
}

#[derive(Subcommand)]
enum NightlightSubcommand {
	SetBrightness { brightness: f64 },
	SetTemperature { temperature: u32 },
	Preset { preset: NightlightPreset },
	Enable,
	Disable,
	Toggle,
}

/// Control the application launcher (avalaunch)
#[derive(Subcommand)]
enum LauncherCommand {
	/// Open avalanch
	Open,
	/// Close avalaunch
	Close,
	/// Exit the avalaunch daemon. This will not allow you to relaunch it using dbus!
	Exit,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	let args = Args::parse();

	let res = match args.command {
		Command::Nightlight { command } => nightlight(&command).await,
		Command::Launcher { ref command } => launcher(&args, command).await,
	};

	if let Err(e) = res {
		outro_cancel(e)?;
	} else {
		outro("Done")?;
	}

	Ok(())
}

async fn nightlight(cmd: &NightlightSubcommand) -> Result<(), Box<dyn std::error::Error>> {
	intro("Nightlight")?;
	let spinner = spinner();
	spinner.start("Connecting to permafrostd");

	let conn = Connection::session().await?;

	if !is_proxy_ready(
		&conn,
		NightlightProxy::DESTINATION.as_ref().unwrap(),
		NightlightProxy::PATH.as_ref().unwrap(),
	)
	.await?
	{
		spinner.error("The daemon `permafrostd` isn't running");
		outro_cancel("Run `permafrostd` in a console or as a startup service.")?;
		std::process::exit(-1);
	}

	let proxy = NightlightProxy::new(&conn).await?;

	spinner.set_message("Sending command");

	match cmd {
		NightlightSubcommand::SetBrightness { brightness } => {
			// NightlightCommand::SetBrightness(*brightness)
			proxy.set_brightness(*brightness).await?
		}
		NightlightSubcommand::SetTemperature { temperature } => {
			proxy.set_temperature(*temperature).await?;
		}
		NightlightSubcommand::Preset { preset } => match preset {
			NightlightPreset::Day => proxy.set_preset("day").await?,
			NightlightPreset::Night => proxy.set_preset("night").await?,
		},
		NightlightSubcommand::Enable => proxy.set_enabled(true).await?,
		NightlightSubcommand::Disable => proxy.set_enabled(false).await?,
		NightlightSubcommand::Toggle => proxy.toggle().await?,
	}

	spinner.set_message("Getting current nightlight state");

	let response = proxy.state().await;

	match response {
		Ok((active, _, brightness, temperature, preset)) => {
			let mut table = Table::new();

			table
				.load_preset(UTF8_FULL)
				.apply_modifier(UTF8_ROUND_CORNERS);

			table.add_row(vec![
				Cell::new("Enabled").fg(Color::Yellow),
				Cell::new(format!("{}", active)),
			]);

			table.add_row(vec![
				Cell::new("Brightness").fg(Color::Yellow),
				Cell::new(format!("{:.2}", brightness)),
			]);

			table.add_row(vec![
				Cell::new("Temperature").fg(Color::Yellow),
				Cell::new(format!("{}", temperature)),
			]);

			table.add_row(vec![
				Cell::new("Preset").fg(Color::Yellow),
				Cell::new(format!("{}", preset)),
			]);

			note("New nightlight settings", table)?;
		}
		Err(e) => {
			spinner.error(&e);
			outro_cancel(e)?;

			std::process::exit(-1);
		}
	}

	spinner.clear();

	Ok(())
}

async fn launcher(
	args: &Args, command: &LauncherCommand,
) -> Result<(), Box<dyn std::error::Error>> {
	let conn = zbus::Connection::session().await?;

	intro("Launcher")?;

	let spinner = spinner();
	spinner.start("Connecting to Launcher");

	{
		if !is_proxy_ready(
			&conn,
			launcher_common::LauncherProxy::DESTINATION.as_ref().unwrap(),
			launcher_common::LauncherProxy::PATH.as_ref().unwrap(),
		)
		.await?
		{
			spinner.error("The launcher daemon isn't running!");
			outro_cancel(
				"Run `avalaunch` in a console or as a startup service to use the launcher",
			)?;

			std::process::exit(-1);
		}

		let proxy = launcher_common::LauncherProxy::new(&conn).await?;

		spinner.set_message("Sending Message");

		match command {
			LauncherCommand::Open => {
				proxy.open().await?;
				log::info("Opened Launcher")?;
			}
			LauncherCommand::Close => {
				proxy.close().await?;
				log::info("Closed Launcher")?;
			}
			LauncherCommand::Exit => {
				let answer = args.yes
					|| confirm("Are you sure you want to exit the launcher daemon?")
						.initial_value(args.yes)
						.interact()?;

				if answer {
					proxy.exit().await?;
				}

				log::info("Killed launcher")?;
			}
		}
	}

	conn.graceful_shutdown().await;

	spinner.clear();

	Ok(())
}

async fn is_proxy_ready(
	conn: &Connection, dest: &BusName<'_>, path: &ObjectPath<'_>,
) -> zbus::Result<bool> {
	let peer = zbus::fdo::PeerProxy::builder(conn)
		.destination(dest)?
		.path(path)?
		.build()
		.await?;

	Ok(peer.ping().await.is_ok())
}
