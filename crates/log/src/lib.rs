use std::{fmt, str::FromStr};

pub use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

enum LogBackend {
	Auto,
	Journald,
	Console,
}

impl FromStr for LogBackend {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_ascii_lowercase().as_str() {
			"auto" => Ok(Self::Auto),
			"journald" => Ok(Self::Journald),
			"console" => Ok(Self::Console),
			_ => Err(format!("Invalid log backend: {}", s)),
		}
	}
}

impl fmt::Display for LogBackend {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Auto => write!(f, "auto"),
			Self::Journald => write!(f, "journald"),
			Self::Console => write!(f, "console"),
		}
	}
}

#[macro_export]
macro_rules! init {
    ($($target:expr),+ $(,)?) => {
        $crate::init_for_targets(&[$($target),+])
    };
}

pub fn init_for_targets(targets: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
	let default_filter = if cfg!(debug_assertions) {
		let targets = targets
			.iter()
			.map(|target| format!("{target}=trace"))
			.collect::<Vec<_>>()
			.join(",");

		format!("warn,{targets}")
	} else {
		let targets = targets
			.iter()
			.map(|target| format!("{target}=info"))
			.collect::<Vec<_>>()
			.join(",");

		format!("warn,{targets}")
	};

	try_init(default_filter.as_str())
}

pub fn try_init(default_filter: &str) -> Result<(), Box<dyn std::error::Error>> {
	let backend = log_backend()?;

	match backend {
		LogBackend::Auto => {
			if let Err(err) = init_journald(default_filter) {
				init_console(default_filter)?;
				warn!(error = %err, "journald logging unavailable, using console log backend");
			} else {
				info!("using journald log backend");
			}
		}
		LogBackend::Journald => {
			init_journald(default_filter)?;
			info!("using journald log backend");
		}
		LogBackend::Console => {
			init_console(default_filter)?;
			info!("using console log backend");
		}
	}

	Ok(())
}

fn log_backend() -> Result<LogBackend, Box<dyn std::error::Error>> {
	match std::env::var("SUBNIRI_LOG_BACKEND")
		.ok()
		.or_else(|| std::env::var("LOG_BACKEND").ok())
	{
		Some(value) => Ok(value.parse()?),
		None => Ok(LogBackend::Auto),
	}
}

fn env_filter(default_filter: &str) -> EnvFilter {
	EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter))
}

fn init_journald(default_filter: &str) -> Result<(), Box<dyn std::error::Error>> {
	let journald = tracing_journald::layer()?;

	tracing_subscriber::registry()
		.with(env_filter(default_filter))
		.with(journald)
		.try_init()
		.map_err(|err| -> Box<dyn std::error::Error> { std::io::Error::other(err).into() })?;

	Ok(())
}

fn init_console(default_filter: &str) -> Result<(), Box<dyn std::error::Error>> {
	let subscriber = tracing_subscriber::fmt().with_env_filter(env_filter(default_filter));

	#[cfg(debug_assertions)]
	let subscriber = subscriber.pretty();

	subscriber
		.try_init()
		.map_err(|err| -> Box<dyn std::error::Error> { std::io::Error::other(err).into() })?;

	Ok(())
}
