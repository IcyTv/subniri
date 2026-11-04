//! Real writer/reload tests in child processes with exclusively owned files.
//! Never mutate the test runner's environment or read the user's configuration.
use super::*;
use crate::{Bar, Section, modules::ModuleMessage};
use config::SystemMenuWidgets::{Bluetooth, Nightlight, Wifi};
use futures::StreamExt;
use std::{
	path::PathBuf,
	process::Command,
	time::{Duration, SystemTime, UNIX_EPOCH},
};

type Error = Box<dyn std::error::Error>;
const MARKER: &str = "subniri isolated config writer fixture";

struct Sandbox(PathBuf);

impl Sandbox {
	fn new() -> Result<Self, Error> {
		use std::os::unix::fs::DirBuilderExt;
		let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
		let root =
			std::env::temp_dir().join(format!("subniri-writer-{}-{nonce}", std::process::id()));
		std::fs::DirBuilder::new().mode(0o700).create(&root)?;
		let sandbox = Self(root);
		std::fs::write(sandbox.0.join("fixture.marker"), MARKER)?;
		Ok(sandbox)
	}
}

impl Drop for Sandbox {
	fn drop(&mut self) {
		// Remove only fixed paths created by this fixture, never recursively.
		for name in ["config.kdl", "fixture.marker", "override/config.kdl"] {
			let _ = std::fs::remove_file(self.0.join(name));
		}
		let override_parent = self.0.join("override");
		if override_parent.is_file() {
			let _ = std::fs::remove_file(override_parent);
		} else {
			let _ = std::fs::remove_dir(override_parent);
		}
		let _ = std::fs::remove_dir(&self.0);
	}
}

fn run(child: &str, with_override: bool) -> Result<(), Error> {
	let sandbox = Sandbox::new()?;
	let mut command = Command::new(std::env::current_exe()?);
	command
		.args(["--exact", child, "--ignored", "--nocapture"])
		.env("SUBNIRI_WRITER_TEST_ROOT", &sandbox.0)
		.env("SUBNIRI_CONFIG_FILE", sandbox.0.join("config.kdl"))
		.env("XDG_CONFIG_HOME", &sandbox.0)
		.env_remove("SUBNIRI_CONFIG_OVERRIDE_FILE");
	if with_override {
		command.env(
			"SUBNIRI_CONFIG_OVERRIDE_FILE",
			sandbox.0.join("override/config.kdl"),
		);
	}
	let output = command.output()?;
	assert!(
		output.status.success(),
		"isolated writer child failed:\n{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	Ok(())
}

/// Refuse direct invocation or inherited user config paths before any I/O task.
fn require_sandbox(with_override: bool) -> Result<PathBuf, Error> {
	let root = PathBuf::from(
		std::env::var_os("SUBNIRI_WRITER_TEST_ROOT")
			.ok_or("run through the isolated parent test")?,
	);
	if std::fs::read_to_string(root.join("fixture.marker"))? != MARKER
		|| ConfigFile::path()? != root.join("config.kdl")
		|| ConfigFile::override_path() != with_override.then(|| root.join("override/config.kdl"))
	{
		return Err("refusing non-fixture configuration paths".into());
	}
	Ok(root)
}

fn seed(root: &std::path::Path) -> Result<(ConfigFile, String), Error> {
	let mut config = ConfigFile::default();
	config.spotify.poll_interval_seconds = 37;
	config.spotify.client_id = "0123456789abcdef0123456789abcdef".into();
	config.system_menu.widgets = vec![Bluetooth, Nightlight];
	let mut document = "// unrelated configuration comment\n".parse::<kdl::KdlDocument>()?;
	config.write_to_file(&mut document, root.join("config.kdl"))?;
	let contents = std::fs::read_to_string(root.join("config.kdl"))?;
	Ok((config, contents))
}

async fn output(task: Task<BarMessage>) -> Result<BarMessage, Error> {
	let mut actions =
		iced_runtime::task::into_stream(task).ok_or("expected a writer/reload task")?;
	tokio::time::timeout(Duration::from_secs(5), async move {
		while let Some(action) = actions.next().await {
			if let iced_runtime::Action::Output(message) = action {
				return Ok(message);
			}
		}
		Err::<_, Error>("task ended without a result".into())
	})
	.await?
}

fn save(bar: &mut Bar, widgets: Vec<SystemMenuWidgets>) -> Task<BarMessage> {
	bar.update(BarMessage::Module(
		None,
		Section::Left,
		0,
		ModuleMessage::SaveSystemMenuWidgets(widgets),
	))
}

fn assert_saved(message: &BarMessage, success: bool) {
	assert!(
		matches!(message, BarMessage::SystemMenuWidgetsSaved(result) if result.is_ok() == success),
		"unexpected write result: {message:?}"
	);
}

async fn finish_and_reload(bar: &mut Bar, result: BarMessage) -> Result<(), Error> {
	let reload = output(bar.update(result)).await?;
	assert!(matches!(reload, BarMessage::ConfigUpdated));
	// Run the production reload, not merely an assertion that it was scheduled.
	assert_eq!(bar.update(reload).units(), 0, "fixture has no live modules");
	assert!(bar.widget_config.latest().is_none());
	Ok(())
}

#[test]
fn real_writes_coalesce_and_reload_without_losing_unrelated_settings() -> Result<(), Error> {
	run("widget_config::file_tests::direct_write_child", false)
}

#[tokio::test]
#[ignore = "isolated config subprocess helper; run its parent test"]
async fn direct_write_child() -> Result<(), Error> {
	let root = require_sandbox(false)?;
	let (initial, _) = seed(&root)?;
	let mut bar = crate::tests::empty_bar();
	let _ = bar.update(BarMessage::ConfigUpdated);
	assert_eq!(bar.config_file, initial);

	let first = output(save(&mut bar, vec![Wifi])).await?;
	assert_saved(&first, true);
	assert_eq!(ConfigFile::load()?.1.system_menu.widgets, vec![Wifi]);
	// The write finished, but its result has not reached update: later user
	// edits must coalesce while that completion is still in flight.
	assert_eq!(save(&mut bar, vec![Bluetooth]).units(), 0);
	assert_eq!(save(&mut bar, vec![Nightlight]).units(), 0);
	let (mut document, mut external) = ConfigFile::load()?;
	external.spotify.poll_interval_seconds = 67;
	external.spotify.client_id = "abcdef0123456789abcdef0123456789".into();
	external.write_to_file(&mut document, root.join("config.kdl"))?;
	let mut expected = external;
	expected.system_menu.widgets = vec![Nightlight];

	let next_write = bar.update(first);
	let _ = bar.update(BarMessage::ConfigUpdated);
	assert_eq!(
		bar.config_file, expected,
		"reload during a pending write must retain the latest layout and fresh unrelated settings"
	);
	let final_result = output(next_write).await?;
	assert_saved(&final_result, true);
	finish_and_reload(&mut bar, final_result).await?;
	assert_eq!(
		ConfigFile::load()?.1,
		expected,
		"compare the whole persisted config, not only layout"
	);
	assert_eq!(bar.config_file, expected);
	let persisted = std::fs::read_to_string(root.join("config.kdl"))?;
	assert!(persisted.contains("unrelated configuration comment"));

	// An external invalid config causes a real load failure inside the writer.
	// It must not overwrite those bytes or leave the UI permanently optimistic.
	let malformed = "system-menu {";
	std::fs::write(root.join("config.kdl"), malformed)?;
	let failed = output(save(&mut bar, vec![Bluetooth])).await?;
	assert_saved(&failed, false);
	assert!(
		matches!(&failed, BarMessage::SystemMenuWidgetsSaved(Err(error)) if error.contains("Failed to parse config file")),
		"expected a real config read/parse failure: {failed:?}"
	);
	assert_eq!(std::fs::read_to_string(root.join("config.kdl"))?, malformed);
	std::fs::write(root.join("config.kdl"), persisted)?;
	finish_and_reload(&mut bar, failed).await?;
	assert_eq!(bar.config_file, expected);
	assert_eq!(ConfigFile::load()?.1, expected);
	Ok(())
}

#[test]
fn real_write_failure_recovers_and_preserves_declarative_and_unrelated_settings()
-> Result<(), Error> {
	run(
		"widget_config::file_tests::failed_override_write_child",
		true,
	)
}

#[tokio::test]
#[ignore = "isolated config subprocess helper; run its parent test"]
async fn failed_override_write_child() -> Result<(), Error> {
	let root = require_sandbox(true)?;
	let (initial, original) = seed(&root)?;
	let blocker = root.join("override");
	// A regular file where the override's parent directory belongs: reads of
	// the valid declarative config succeed, but actual fs::write fails with
	// ENOTDIR. Deterministic even as root; no permission-based fault simulation.
	std::fs::write(&blocker, "injected non-directory output parent")?;
	let mut bar = crate::tests::empty_bar();
	let _ = bar.update(BarMessage::ConfigUpdated);
	assert_eq!(bar.config_file, initial);
	let failed = output(save(&mut bar, vec![Wifi])).await?;
	assert_saved(&failed, false);
	assert!(
		matches!(&failed, BarMessage::SystemMenuWidgetsSaved(Err(error)) if error.contains("os error 20")),
		"expected ENOTDIR from the real file write: {failed:?}"
	);
	assert_eq!(std::fs::read_to_string(root.join("config.kdl"))?, original);
	assert_eq!(
		std::fs::read_to_string(&blocker)?,
		"injected non-directory output parent"
	);
	assert!(!root.join("override/config.kdl").exists());
	finish_and_reload(&mut bar, failed).await?;
	assert_eq!(
		bar.config_file, initial,
		"a terminal failed write must reload confirmed disk state"
	);

	let failed = output(save(&mut bar, vec![Wifi])).await?;
	assert_saved(&failed, false);
	assert_eq!(save(&mut bar, vec![Bluetooth]).units(), 0);
	assert_eq!(save(&mut bar, vec![Nightlight]).units(), 0);
	let (mut document, mut external) = ConfigFile::load_declarative()?;
	external.spotify.poll_interval_seconds = 89;
	external.spotify.client_id = "abcdef0123456789abcdef0123456789".into();
	external.write_to_file(&mut document, root.join("config.kdl"))?;
	let declarative = std::fs::read_to_string(root.join("config.kdl"))?;
	std::fs::remove_file(&blocker)?; // This test created this exact fixture file.
	std::fs::create_dir(&blocker)?;
	let mut expected = external;
	expected.system_menu.widgets = vec![Nightlight];
	let next_write = bar.update(failed);
	assert_eq!(bar.widget_config.latest(), Some([Nightlight].as_slice()));
	let _ = bar.update(BarMessage::ConfigUpdated);
	assert_eq!(bar.config_file, expected);
	let saved = output(next_write).await?;
	assert_saved(&saved, true);
	finish_and_reload(&mut bar, saved).await?;
	assert_eq!(ConfigFile::active_path()?, root.join("override/config.kdl"));
	assert_eq!(ConfigFile::load()?.1, expected);
	assert_eq!(bar.config_file, expected);
	assert_eq!(
		std::fs::read_to_string(root.join("config.kdl"))?,
		declarative,
		"override writes must leave the declarative file byte-for-byte unchanged"
	);
	assert!(
		std::fs::read_to_string(root.join("override/config.kdl"))?
			.contains("unrelated configuration comment")
	);

	// An existing runtime override can itself contain unrelated settings that
	// differ from the declarative file. A later layout save must retain them.
	let (mut document, mut active) = ConfigFile::load()?;
	active.spotify.poll_interval_seconds = 101;
	active.write_to_file(&mut document, root.join("override/config.kdl"))?;
	expected = active;
	expected.system_menu.widgets = vec![Wifi];
	let saved = output(save(&mut bar, vec![Wifi])).await?;
	assert_saved(&saved, true);
	finish_and_reload(&mut bar, saved).await?;
	assert_eq!(ConfigFile::load()?.1, expected);
	assert_eq!(bar.config_file, expected);
	assert_eq!(
		ConfigFile::load_declarative()?
			.1
			.spotify
			.poll_interval_seconds,
		89
	);
	assert_eq!(
		std::fs::read_to_string(root.join("config.kdl"))?,
		declarative
	);
	Ok(())
}
