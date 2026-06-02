use clap::{Parser, Subcommand};
use daemon::{NightlightClient, NightlightCommand};
use modern_terminal::{
	components::{
		table::{Size, Table},
		text::{Text, TextAlignment},
	},
	core::{console::Console, style::Style},
};

#[derive(Parser)]
struct Args {
	#[clap(subcommand)]
	command: Command,
}

#[derive(Subcommand)]
enum Command {
	Nightlight {
		#[clap(subcommand)]
		command: NightlightSubcommand,
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
	SetBrightness { brightness: f32 },
	SetTemperature { temperature: u32 },
	Preset { preset: NightlightPreset },
	Enable,
	Disable,
	Toggle,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let args = Args::parse();

	match args.command {
		Command::Nightlight { command } => nightlight(&command)?,
	}

	Ok(())
}

fn nightlight(cmd: &NightlightSubcommand) -> Result<(), Box<dyn std::error::Error>> {
	let send_cmd = match cmd {
		NightlightSubcommand::SetBrightness { brightness } => {
			NightlightCommand::SetBrightness(*brightness)
		}
		NightlightSubcommand::SetTemperature { temperature } => {
			NightlightCommand::SetTemperature(*temperature)
		}
		NightlightSubcommand::Preset { preset } => match preset {
			NightlightPreset::Day => {
				NightlightCommand::SetNightlight(daemon::NightlightPreset::Day)
			}
			NightlightPreset::Night => {
				NightlightCommand::SetNightlight(daemon::NightlightPreset::Night)
			}
		},
		NightlightSubcommand::Enable => NightlightCommand::SetEnabled(true),
		NightlightSubcommand::Disable => NightlightCommand::SetEnabled(false),
		NightlightSubcommand::Toggle => NightlightCommand::ToggleNightlight,
	};

	let client = NightlightClient::new()?;
	let response = client.send(send_cmd)?;

	match response {
		daemon::NightlightResponse::State(state) => {
			let mut writer = std::io::stdout();
			let mut console = Console::from_fd(&mut writer);

			let component = Table {
				column_sizes: vec![Size::Cells(20), Size::Cells(8)],
				rows: vec![
					vec![
						name("Brightness"),
						value(format!("{:.2}", state.brightness)),
					],
					vec![
						name("Color Temperature"),
						value(format!("{}K", state.temperature)),
					],
					vec![name("Preset"), value(format!("{:?}", state.preset))],
				],
			};

			console.render(&component)?;
		}
		daemon::NightlightResponse::Error(e) => {
			let mut writer = std::io::stderr();
			let mut console = Console::from_fd(&mut writer);

			let component = Text {
				align: TextAlignment::Left,
				text: format!("Error: {e}"),
				styles: vec![Style::Foreground("red".to_string())],
			};

			console.render(&component)?;

			std::process::exit(-1);
		}
	}

	Ok(())
}

fn name(text: &str) -> Box<Text> {
	Box::new(Text {
		align: TextAlignment::Left,
		text: String::from(text),
		styles: vec![Style::Bold, Style::Foreground("yellow".to_string())],
	})
}

fn value(text: impl Into<String>) -> Box<Text> {
	Box::new(Text {
		align: TextAlignment::Left,
		text: text.into(),
		styles: vec![],
	})
}
