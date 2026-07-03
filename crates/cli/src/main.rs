use clap::{Parser, Subcommand};
use cliclack::{confirm, intro, log, note, outro, outro_cancel, spinner};
use comfy_table::{Cell, Color, Table, modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL};
use daemon::NightlightProxy;
use jiff::SignedDuration;
use systemd::{JournalRecord, journal};
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
	/// Work with subniri component logs in journald
	Logs {
		#[clap(subcommand)]
		command: LogsSubcommand,
	},
}

#[derive(Subcommand)]
enum LogsSubcommand {
	/// Read subniri component logs from journald
	Show(LogsShowCommand),
}

#[derive(clap::Args)]
struct LogsShowCommand {
	/// Components to filter. Omit to show all subniri components.
	components: Vec<LogComponent>,

	/// Number of recent entries to print before exiting or following.
	#[clap(short = 'n', long, default_value_t = 100)]
	lines: usize,

	/// Keep reading new log entries.
	#[clap(short, long)]
	follow: bool,

	/// Read the system journal instead of the current user's journal.
	#[clap(long, conflicts_with = "all_users")]
	system: bool,

	/// Read all readable journals.
	#[clap(long)]
	all_users: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum LogComponent {
	Bar,
	Daemon,
	FileManager,
	Indexer,
	Launcher,
	Logout,
	Settings,
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
	Suspend { duration: String },
	Unsuspend,
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
		Command::Logs { ref command } => logs(command),
	};

	if let Err(e) = res {
		outro_cancel(e)?;
	} else {
		outro("Done")?;
	}

	Ok(())
}

fn logs(command: &LogsSubcommand) -> Result<(), Box<dyn std::error::Error>> {
	match command {
		LogsSubcommand::Show(command) => show_logs(command),
	}
}

fn show_logs(command: &LogsShowCommand) -> Result<(), Box<dyn std::error::Error>> {
	intro("Logs")?;

	let mut options = journal::OpenOptions::default();
	options.local_only(true);

	if command.system {
		options.system(true);
	} else if !command.all_users {
		options.current_user(true);
	}

	let mut journal = options.open()?;
	apply_log_filters(&mut journal, &command.components)?;

	if command.lines > 0 {
		journal.seek_tail()?;

		let mut entries = Vec::new();
		while entries.len() < command.lines {
			let Some(entry) = journal.previous_entry()? else {
				break;
			};

			entries.push(entry);
		}

		for entry in entries.iter().rev() {
			print_log_entry(entry);
		}
	}

	if command.follow {
		journal.seek_tail()?;

		loop {
			if let Some(entry) = journal.await_next_entry(None)? {
				print_log_entry(&entry);
			}
		}
	}

	Ok(())
}

fn apply_log_filters(
	journal: &mut systemd::Journal, components: &[LogComponent],
) -> Result<(), Box<dyn std::error::Error>> {
	let fields = if components.is_empty() {
		all_log_components()
			.into_iter()
			.flat_map(log_fields)
			.copied()
			.collect::<Vec<_>>()
	} else {
		components
			.iter()
			.copied()
			.flat_map(log_fields)
			.copied()
			.collect()
	};

	for (index, (field, value)) in fields.iter().enumerate() {
		if index > 0 {
			journal.match_or()?;
		}

		journal.match_add(field, *value)?;
	}

	Ok(())
}

fn all_log_components() -> [LogComponent; 7] {
	[
		LogComponent::Bar,
		LogComponent::Daemon,
		LogComponent::FileManager,
		LogComponent::Indexer,
		LogComponent::Launcher,
		LogComponent::Logout,
		LogComponent::Settings,
	]
}

fn log_fields(component: LogComponent) -> &'static [(&'static str, &'static str)] {
	match component {
		LogComponent::Bar => &[("SYSLOG_IDENTIFIER", "polarbar"), ("TARGET", "bar")],
		LogComponent::Daemon => &[("SYSLOG_IDENTIFIER", "permafrostd"), ("TARGET", "daemon")],
		LogComponent::FileManager => &[
			("SYSLOG_IDENTIFIER", "file-manager"),
			("TARGET", "file_manager"),
		],
		LogComponent::Indexer => &[("SYSLOG_IDENTIFIER", "icepickd"), ("TARGET", "indexer")],
		LogComponent::Launcher => &[("SYSLOG_IDENTIFIER", "avalaunch"), ("TARGET", "launcher")],
		LogComponent::Logout => &[("SYSLOG_IDENTIFIER", "iceout"), ("TARGET", "logout")],
		LogComponent::Settings => &[("SYSLOG_IDENTIFIER", "snowconf"), ("TARGET", "settings")],
	}
}

fn print_log_entry(entry: &JournalRecord) {
	let timestamp = entry
		.get("__REALTIME_TIMESTAMP")
		.map_or("-", String::as_str);
	let component = entry
		.get("SYSLOG_IDENTIFIER")
		.or_else(|| entry.get("TARGET"))
		.map_or("-", String::as_str);
	let priority = entry
		.get("PRIORITY")
		.map_or("?", |priority| priority_label(priority));
	let message = entry.get("MESSAGE").map_or("", String::as_str);

	println!("{timestamp} {component} {priority}: {message}");
}

fn priority_label(priority: &str) -> &'static str {
	match priority {
		"0" => "emerg",
		"1" => "alert",
		"2" => "crit",
		"3" => "error",
		"4" => "warn",
		"5" => "notice",
		"6" => "info",
		"7" => "debug",
		_ => "?",
	}
}

async fn nightlight(cmd: &NightlightSubcommand) -> Result<(), Box<dyn std::error::Error>> {
	intro("Nightlight")?;
	let spinner = spinner();
	spinner.start("Connecting to permafrostd");

	let conn = Connection::session().await?;

	let destination = NightlightProxy::DESTINATION
		.as_ref()
		.ok_or("nightlight proxy is missing a DBus destination")?;
	let path = NightlightProxy::PATH
		.as_ref()
		.ok_or("nightlight proxy is missing a DBus object path")?;

	if !is_proxy_ready(&conn, destination, path).await? {
		spinner.error("The daemon `permafrostd` isn't running");
		outro_cancel("Run `permafrostd` in a console or as a startup service.")?;
		std::process::exit(-1);
	}

	let proxy = NightlightProxy::new(&conn).await?;

	spinner.set_message("Sending command");

	match cmd {
		NightlightSubcommand::SetBrightness { brightness } => {
			// NightlightCommand::SetBrightness(*brightness)
			proxy.set_brightness(*brightness).await?;
		}
		NightlightSubcommand::SetTemperature { temperature } => {
			proxy.set_temperature(*temperature).await?;
		}
		NightlightSubcommand::Preset { preset } => match preset {
			NightlightPreset::Day => proxy.set_preset("day").await?,
			NightlightPreset::Night => proxy.set_preset("night").await?,
		},
		NightlightSubcommand::Suspend { duration } => {
			proxy.suspend(parse_duration_secs(duration)?).await?;
		}
		NightlightSubcommand::Unsuspend => proxy.unsuspend().await?,
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
				Cell::new(format!("{active}")),
			]);

			table.add_row(vec![
				Cell::new("Brightness").fg(Color::Yellow),
				Cell::new(format!("{brightness:.2}")),
			]);

			table.add_row(vec![
				Cell::new("Temperature").fg(Color::Yellow),
				Cell::new(format!("{temperature}")),
			]);

			table.add_row(vec![
				Cell::new("Preset").fg(Color::Yellow),
				Cell::new(preset.clone()),
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

fn parse_duration_secs(duration: &str) -> Result<u64, Box<dyn std::error::Error>> {
	let duration: SignedDuration = duration.parse()?;
	if duration.is_negative() {
		return Err("duration must not be negative".into());
	}

	let seconds = u64::try_from(duration.as_secs())?;
	if duration.subsec_nanos() > 0 {
		return seconds
			.checked_add(1)
			.ok_or_else(|| "duration is too large".into());
	}

	Ok(seconds)
}

async fn launcher(
	args: &Args, command: &LauncherCommand,
) -> Result<(), Box<dyn std::error::Error>> {
	let conn = zbus::Connection::session().await?;

	intro("Launcher")?;

	let spinner = spinner();
	spinner.start("Connecting to Launcher");

	{
		let destination = launcher_common::LauncherProxy::DESTINATION
			.as_ref()
			.ok_or("launcher proxy is missing a DBus destination")?;
		let path = launcher_common::LauncherProxy::PATH
			.as_ref()
			.ok_or("launcher proxy is missing a DBus object path")?;

		if !is_proxy_ready(&conn, destination, path).await? {
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
