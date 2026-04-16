use clap::{Parser, Subcommand};
use zbus::{Connection, Result, proxy};

#[derive(Parser)]
struct Args {
	#[command(subcommand)]
	command: Command,
}

#[derive(Subcommand)]
enum Command {
	Player {
		#[command(subcommand)]
		command: PlayerCommand,
	},
}

impl Command {
	async fn run(&self) -> Result<()> {
		match self {
			Self::Player { command } => command.run().await,
		}
	}
}

#[derive(Subcommand)]
enum PlayerCommand {
	Cycle,
	PlayPause,
	Next,
	Previous,
}

impl PlayerCommand {
	async fn run(&self) -> Result<()> {
		let connection = Connection::session().await?;

		let proxy = BarManagerProxy::new(&connection).await?;

		match self {
			Self::Cycle => proxy.cycle_player().await,
			Self::PlayPause => proxy.toggle_play_pause().await,
			Self::Next => proxy.next().await,
			Self::Previous => proxy.previous().await,
		}
	}
}

// TODO: Create a unified crate for this interface...
#[proxy(
	interface = "de.icytv.subniri.Bar",
	default_service = "de.icytv.subniri.Bar",
	default_path = "/de/icytv/subniri/Bar"
)]
trait BarManager {
	async fn cycle_player(&self) -> Result<()>;
	async fn toggle_play_pause(&self) -> Result<()>;
	async fn next(&self) -> Result<()>;
	async fn previous(&self) -> Result<()>;
}

#[tokio::main]
async fn main() -> Result<()> {
	let args = Args::parse();

	args.command.run().await?;

	Ok(())
}
