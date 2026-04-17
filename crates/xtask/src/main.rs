use std::io::{BufRead, BufReader, IsTerminal};
use std::path::{Path, PathBuf};
use std::thread;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
struct Args {
	#[command(subcommand)]
	command: Command,
}

#[derive(Subcommand)]
enum Command {
	#[clap(alias = "r")]
	Run {
		component: Vec<Component>,
	},
	Up {
		#[arg(value_enum, default_value_t = DevOrProd::Dev)]
		mode: DevOrProd,
	},
	Down {
		#[arg(value_enum, default_value_t = DevOrProd::Dev)]
		mode: DevOrProd,
	},
	Status {
		#[arg(value_enum, default_value_t = DevOrProd::Dev)]
		mode: DevOrProd,
	},
	Logs {
		#[arg(value_enum, default_value_t = DevOrProd::Dev)]
		mode: DevOrProd,
		component: Option<Component>,
	},
	Doctor {},
}

#[derive(Clone, Copy, Debug, ValueEnum)]
#[value(rename_all = "lower")]
enum DevOrProd {
	Dev,
	Prod,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
enum Component {
	Systemd,
	Polarbar,
	Avalaunch,
	Subniri,
}

impl Component {
	fn as_str(self) -> &'static str {
		match self {
			Self::Systemd => "systemd",
			Self::Polarbar => "polarbar",
			Self::Avalaunch => "avalaunch",
			Self::Subniri => "subniri",
		}
	}

	fn fast_bin(self) -> Option<FastBin> {
		match self {
			Self::Polarbar => Some(FastBin {
				package: "bar",
				bin: "polarbar",
			}),
			Self::Subniri => Some(FastBin {
				package: "cli",
				bin: "subniri",
			}),
			Self::Avalaunch | Self::Systemd => None,
		}
	}

	fn systemd_unit_suffix(self) -> Option<&'static str> {
		match self {
			Self::Polarbar => Some("bar"),
			Self::Avalaunch => Some("launcher"),
			Self::Subniri => Some("shell"),
			Self::Systemd => None,
		}
	}
}

#[derive(Clone, Copy)]
struct FastBin {
	package: &'static str,
	bin: &'static str,
}

#[derive(Clone, Copy)]
struct UnitSet {
	target: &'static str,
	services: &'static [&'static str],
	link_dir: &'static str,
}

const DEV_UNITS: UnitSet = UnitSet {
	target: "subniri-dev.target",
	services: &["subniri-dev-bar.service", "subniri-dev-launcher.service"],
	link_dir: "systemd/user-dev",
};

const PROD_UNITS: UnitSet = UnitSet {
	target: "subniri.target",
	services: &["subniri-bar.service", "subniri-launcher.service"],
	link_dir: "systemd/user",
};

fn main() {
	if let Err(err) = run() {
		eprintln!("error: {err:#}");
		std::process::exit(1);
	}
}

fn run() -> Result<()> {
	let args = Args::parse();
	match args.command {
		Command::Run { component } => run_command(&component),
		Command::Up { mode } => up(mode),
		Command::Down { mode } => down(mode),
		Command::Status { mode } => status(mode),
		Command::Logs { mode, component } => logs(mode, component),
		Command::Doctor {} => doctor(),
	}
}

fn run_command(components: &[Component]) -> Result<()> {
	let parsed = if components.is_empty() {
		ParsedRun::Fast(vec![Component::Polarbar])
	} else if components.contains(&Component::Systemd) {
		if components.len() > 1 {
			bail!("`systemd` run mode cannot be combined with other components")
		}
		ParsedRun::Systemd
	} else {
		ParsedRun::Fast(components.to_vec())
	};

	match parsed {
		ParsedRun::Fast(components) => run_fast(&components),
		ParsedRun::Systemd => run_systemd(),
	}
}

enum ParsedRun {
	Fast(Vec<Component>),
	Systemd,
}

fn run_fast(components: &[Component]) -> Result<()> {
	let mut readers = Vec::new();
	let force_color = std::io::stdout().is_terminal();

	for component in components {
		let Some(bin) = component.fast_bin() else {
			bail!("component `{}` is not available in fast mode yet", component.as_str());
		};

		let cmd = if force_color {
			duct::cmd!("cargo", "--color", "always", "run", "-p", bin.package, "--bin", bin.bin)
		} else {
			duct::cmd!("cargo", "--color", "never", "run", "-p", bin.package, "--bin", bin.bin)
		};

		let reader = cmd
			.stderr_to_stdout()
			.reader()
			.with_context(|| format!("failed to start `{}` in fast mode", component.as_str()))?;

		readers.push((component, reader));
	}

	if readers.is_empty() {
		bail!("no runnable components selected")
	}

	let mut tasks = Vec::new();
	for (component, reader) in readers {
		let label = component.as_str().to_string();
		tasks.push(thread::spawn(move || -> Result<()> {
			for line in BufReader::new(reader).lines() {
				let line = line.with_context(|| format!("failed to read output from `{label}`"))?;
				let line = line.trim_end_matches('\r');
				println!("{label} | {line}");
			}
			Ok(())
		}));
	}

	let mut had_err = false;
	for task in tasks {
		match task.join() {
			Ok(Ok(())) => {}
			Ok(Err(err)) => {
				had_err = true;
				eprintln!("error: {err:#}");
			}
			Err(_) => {
				had_err = true;
				eprintln!("error: fast-run worker thread panicked");
			}
		}
	}

	if had_err {
		bail!("one or more fast-run components failed")
	}

	Ok(())
}

fn run_systemd() -> Result<()> {
	ensure_prod_stack_inactive()?;
	up(DevOrProd::Dev)?;
	logs(DevOrProd::Dev, None)
}

fn up(mode: DevOrProd) -> Result<()> {
	let units = match mode {
		DevOrProd::Dev => DEV_UNITS,
		DevOrProd::Prod => PROD_UNITS,
	};

	if matches!(mode, DevOrProd::Dev) {
		ensure_prod_stack_inactive()?;
		link_units(units)?;
	}

	cmd_ok("systemctl", &["--user", "daemon-reload"]).context("failed to reload user systemd daemon")?;
	cmd_ok("systemctl", &["--user", "start", units.target])
		.with_context(|| format!("failed to start `{}`", units.target))?;

	Ok(())
}

fn down(mode: DevOrProd) -> Result<()> {
	let units = match mode {
		DevOrProd::Dev => DEV_UNITS,
		DevOrProd::Prod => PROD_UNITS,
	};

	cmd_ok("systemctl", &["--user", "stop", units.target])
		.with_context(|| format!("failed to stop `{}`", units.target))?;

	Ok(())
}

fn status(mode: DevOrProd) -> Result<()> {
	let units = match mode {
		DevOrProd::Dev => DEV_UNITS,
		DevOrProd::Prod => PROD_UNITS,
	};

	let mut args = vec!["--user", "status", units.target];
	args.extend(units.services.iter().copied());
	cmd_stream("systemctl", &args).context("failed to query systemd status")
}

fn logs(mode: DevOrProd, component: Option<Component>) -> Result<()> {
	let unit_name = match component {
		Some(component) => {
			let Some(suffix) = component.systemd_unit_suffix() else {
				bail!("`systemd` is not a loggable component")
			};
			match mode {
				DevOrProd::Dev => format!("subniri-dev-{suffix}.service"),
				DevOrProd::Prod => format!("subniri-{suffix}.service"),
			}
		}
		None => match mode {
			DevOrProd::Dev => DEV_UNITS.target.to_string(),
			DevOrProd::Prod => PROD_UNITS.target.to_string(),
		},
	};

	cmd_stream("journalctl", &["--user", "-f", "-u", &unit_name])
		.with_context(|| format!("failed to follow logs for `{}` (is the unit active?)", unit_name))
}

fn doctor() -> Result<()> {
	cmd_ok("systemctl", &["--user", "--version"]).context("`systemctl --user` is not available")?;

	let mut missing_paths = Vec::new();
	for dir in [DEV_UNITS.link_dir, PROD_UNITS.link_dir] {
		let full = workspace_root().join(dir);
		if !full.exists() {
			missing_paths.push(full);
		}
	}

	if !missing_paths.is_empty() {
		bail!(
			"missing systemd unit directories:\n{}",
			missing_paths
				.iter()
				.map(|path| format!("  - {}", path.display()))
				.collect::<Vec<_>>()
				.join("\n")
		)
	}

	println!("doctor: ok");
	Ok(())
}

fn ensure_prod_stack_inactive() -> Result<()> {
	let is_active = cmd_capture("systemctl", &["--user", "is-active", PROD_UNITS.target])
		.map(|output| output.trim() == "active")
		.unwrap_or(false);

	if is_active {
		bail!(
			"prod stack is active (`{}`); run `cargo xtask down prod` before starting dev systemd mode",
			PROD_UNITS.target
		)
	}

	Ok(())
}

fn link_units(units: UnitSet) -> Result<()> {
	let root = workspace_root();
	let dir = root.join(units.link_dir);

	if !dir.exists() {
		bail!(
			"unit directory `{}` does not exist (expected at `{}`)",
			units.link_dir,
			dir.display()
		)
	}

	let mut paths = Vec::new();
	paths.push(dir.join(units.target));
	for service in units.services {
		paths.push(dir.join(service));
	}

	for path in paths {
		if !path.exists() {
			bail!("missing unit file `{}`", path.display())
		}

		cmd_ok("systemctl", &["--user", "link", path_to_str(&path)?])
			.with_context(|| format!("failed to link unit `{}`", path.display()))?;
	}

	Ok(())
}

fn path_to_str(path: &Path) -> Result<&str> {
	path.to_str()
		.with_context(|| format!("path `{}` is not valid UTF-8", path.display()))
}

fn workspace_root() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR"))
		.parent()
		.and_then(Path::parent)
		.expect("xtask crate to be at crates/xtask")
		.to_path_buf()
}

fn cmd_ok(program: &str, args: &[&str]) -> Result<()> {
	duct::cmd(program, args)
		.stderr_to_stdout()
		.unchecked()
		.run()
		.with_context(|| format!("failed to execute `{}`", format_cmd(program, args)))
		.and_then(|output| {
			if output.status.success() {
				Ok(())
			} else {
				let stdout = String::from_utf8_lossy(&output.stdout);
				bail!(
					"command `{}` failed with status {}\n{}",
					format_cmd(program, args),
					output.status,
					stdout.trim()
				)
			}
		})
}

fn cmd_capture(program: &str, args: &[&str]) -> Result<String> {
	let output = duct::cmd(program, args)
		.stderr_to_stdout()
		.read()
		.with_context(|| format!("failed to execute `{}`", format_cmd(program, args)))?;
	Ok(output)
}

fn cmd_stream(program: &str, args: &[&str]) -> Result<()> {
	duct::cmd(program, args)
		.run()
		.with_context(|| format!("failed to execute `{}`", format_cmd(program, args)))?;
	Ok(())
}

fn format_cmd(program: &str, args: &[&str]) -> String {
	let mut full = String::from(program);
	for arg in args {
		full.push(' ');
		full.push_str(arg);
	}
	full
}
