//! Isolated service tests: never change the runner's session/system bus address.

use std::{
	io::BufRead,
	process::{Command, Stdio},
};

pub type Error = Box<dyn std::error::Error>;

struct Broker(std::process::Child);

impl Drop for Broker {
	fn drop(&mut self) {
		let _ = self.0.kill();
		let _ = self.0.wait();
	}
}

pub fn run(test: &str) -> Result<(), Error> {
	run_with_session(test, None)
}

pub fn run_without_session(test: &str) -> Result<(), Error> {
	run_with_session(test, Some("unix:path=/proc/self/fd/subniri-missing-bus"))
}

fn run_with_session(test: &str, session: Option<&str>) -> Result<(), Error> {
	let mut broker = Broker(
		Command::new("dbus-daemon")
			.args(["--session", "--nofork", "--print-address=1"])
			.stdout(Stdio::piped())
			.spawn()?,
	);
	let mut address = String::new();
	std::io::BufReader::new(broker.0.stdout.take().ok_or("missing broker stdout")?)
		.read_line(&mut address)?;
	if !address.starts_with("unix:") {
		return Err("unexpected private bus address".into());
	}
	let output = Command::new(std::env::current_exe()?)
		.args(["--exact", test, "--ignored", "--nocapture"])
		.env(
			"DBUS_SESSION_BUS_ADDRESS",
			session.unwrap_or(address.trim()),
		)
		.env("DBUS_SYSTEM_BUS_ADDRESS", address.trim())
		.env("SUBNIRI_PRIVATE_BUS_CHILD", "1")
		.output()?;
	assert!(
		output.status.success(),
		"private-bus child failed:\n{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	Ok(())
}

pub fn require_child() -> Result<(), Error> {
	if std::env::var_os("SUBNIRI_PRIVATE_BUS_CHILD").is_none() {
		return Err("run this test through its private-bus parent test".into());
	}
	Ok(())
}

pub async fn task_message<M: Send + 'static>(task: iced::Task<M>) -> Result<M, Error> {
	use futures::StreamExt;
	let mut actions = iced_runtime::task::into_stream(task).ok_or("missing task")?;
	while let Some(action) = actions.next().await {
		if let iced_runtime::Action::Output(message) = action {
			return Ok(message);
		}
	}
	Err("task produced no message".into())
}
