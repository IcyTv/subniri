//! thawth: Subniri's unprivileged polkit authentication agent.

use std::{
	process::ExitCode,
	sync::{
		Arc,
		atomic::{AtomicU8, Ordering},
	},
};

mod agent;
mod app;
mod conversation;
mod dialog;
mod helper;
mod metadata;
mod model;
mod session;

fn main() -> ExitCode {
	if let Err(error) = log::init!("polkit_agent", "thawth") {
		eprintln!("Cannot initialize thawth logging: {error}");
		return ExitCode::FAILURE;
	}
	let runtime = match tokio::runtime::Runtime::new() {
		Ok(runtime) => runtime,
		Err(error) => {
			log::error!("Cannot start authentication runtime: {error}");
			return ExitCode::FAILURE;
		}
	};
	let events = match runtime.block_on(agent::start()) {
		Ok(events) => events,
		Err(error) => {
			log::error!("{}", error.message);
			return ExitCode::from(error.code);
		}
	};
	let exit_code = Arc::new(AtomicU8::new(0));

	if let Err(error) = app::App::run(events, exit_code.clone()) {
		log::error!("Authentication UI failed: {error}");
		return ExitCode::FAILURE;
	}

	ExitCode::from(exit_code.load(Ordering::Relaxed))
}
