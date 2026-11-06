use std::{fmt, str::FromStr};

pub use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

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
		.with(
			journald.with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
				safe_dependency_target(metadata.target())
			})),
		)
		.try_init()
		.map_err(|err| -> Box<dyn std::error::Error> { std::io::Error::other(err).into() })?;

	Ok(())
}

fn init_console(default_filter: &str) -> Result<(), Box<dyn std::error::Error>> {
	let layer = tracing_subscriber::fmt::layer();

	#[cfg(debug_assertions)]
	let layer = layer.pretty();

	tracing_subscriber::registry()
		.with(env_filter(default_filter))
		.with(
			layer.with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
				safe_dependency_target(metadata.target())
			})),
		)
		.try_init()
		.map_err(|err| -> Box<dyn std::error::Error> { std::io::Error::other(err).into() })?;

	Ok(())
}

// These dependencies log authentication tokens or WebSocket token URLs at trace
// level. This guard cannot be overridden by RUST_LOG=trace or a more-specific
// module directive. Spotify's own owner emits credential-safe status/errors.
fn safe_dependency_target(target: &str) -> bool {
	![
		"librespot_core",
		"librespot_oauth",
		"tungstenite",
		"tokio_tungstenite",
	]
	.iter()
	.any(|prefix| target == *prefix || target.starts_with(&format!("{prefix}::")))
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn sensitive_dependency_logs_are_suppressed_even_with_user_trace_filters() {
		assert!(!safe_dependency_target("librespot_core::login5"));
		assert!(!safe_dependency_target("tungstenite::handshake::client"));
		assert!(safe_dependency_target("daemon::spotify"));
	}
}
