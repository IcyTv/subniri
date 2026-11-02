use super::*;
use std::{
	path::PathBuf,
	process::{Child, Command, Stdio},
};

const REMOTE: &str = "subniri-pipewire-test";
const SINK: &str = "subniri-test-sink";

// No session manager, hardware discovery, real-time scheduling or D-Bus.
const CONFIG: &str = r#"
context.properties = {
    core.daemon = true
    core.name = subniri-pipewire-test
    support.dbus = false
}
context.spa-libs = {
    audio.convert.* = audioconvert/libspa-audioconvert
    support.* = support/libspa-support
}
context.modules = [
    { name = libpipewire-module-protocol-native }
    { name = libpipewire-module-metadata }
    { name = libpipewire-module-spa-node-factory }
    { name = libpipewire-module-adapter }
    { name = libpipewire-module-access args = { access.socket = { subniri-pipewire-test = "unrestricted" } } }
]
context.objects = [
    { factory = metadata args = { metadata.name = default } }
    { factory = adapter args = {
        factory.name = support.null-audio-sink
        node.name = subniri-test-sink
        node.description = "Subniri isolated test sink"
        media.class = Audio/Sink
        audio.position = [ FL FR ]
    } }
]
"#;

type Error = Box<dyn std::error::Error>;

#[test]
fn volume_properties_round_trip_and_mute_omits_channels()
-> Result<(), pipewire_native_spa::pod::Error> {
	let channels = [0.125, 0.421875];
	let pod = build_route_props_pod(Some(&channels), Some(false))?;
	let decoded = decode_props(&pod)?;
	assert_eq!(decoded.channel_volums.as_deref(), Some(channels.as_slice()));
	assert_eq!(decoded.mute, Some(false));
	let mute = decode_props(&build_route_props_pod(None, Some(true))?)?;
	assert_eq!(mute.mute, Some(true));
	assert!(mute.channel_volums.is_none());
	Ok(())
}

struct PrivatePipeWire {
	directory: PathBuf,
	daemon: Option<Child>,
}

impl Drop for PrivatePipeWire {
	fn drop(&mut self) {
		if let Some(daemon) = &mut self.daemon {
			let _ = daemon.kill();
			let _ = daemon.wait();
		}
		// Only individually remove the known files created in this test's own
		// exclusive directory. Never recursively remove a runtime directory.
		for name in [
			"audit.conf",
			REMOTE,
			"subniri-pipewire-test.lock",
			"subniri-pipewire-test-manager",
			"subniri-pipewire-test-manager.lock",
		] {
			let _ = std::fs::remove_file(self.directory.join(name));
		}
		let _ = std::fs::remove_dir(&self.directory);
	}
}

fn run_private(test: &str) -> Result<(), Error> {
	use std::os::unix::fs::DirBuilderExt;
	let nonce = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)?
		.as_nanos();
	let directory =
		std::env::temp_dir().join(format!("subniri-pipewire-{}-{nonce}", std::process::id()));
	std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
	let mut fixture = PrivatePipeWire {
		directory,
		daemon: None,
	};
	std::fs::write(fixture.directory.join("audit.conf"), CONFIG)?;
	fixture.daemon = Some(
		Command::new("pipewire")
			.args(["-c"])
			.arg(fixture.directory.join("audit.conf"))
			.env("PIPEWIRE_RUNTIME_DIR", &fixture.directory)
			.env("XDG_RUNTIME_DIR", &fixture.directory)
			.stdout(Stdio::null())
			.stderr(Stdio::piped())
			.spawn()?,
	);
	let deadline = Instant::now() + Duration::from_secs(3);
	loop {
		if std::os::unix::net::UnixStream::connect(fixture.directory.join(REMOTE)).is_ok() {
			break;
		}
		if let Some(daemon) = &mut fixture.daemon
			&& let Some(status) = daemon.try_wait()?
		{
			use std::io::Read;
			let mut error = String::new();
			if let Some(mut stderr) = daemon.stderr.take() {
				stderr.read_to_string(&mut error)?;
			}
			return Err(format!("private PipeWire exited {status}: {error}").into());
		}
		if Instant::now() >= deadline {
			return Err("private PipeWire socket did not become ready".into());
		}
		std::thread::sleep(Duration::from_millis(10));
	}
	let output = Command::new(std::env::current_exe()?)
		.args(["--exact", test, "--ignored", "--nocapture"])
		.env("SUBNIRI_PRIVATE_PIPEWIRE_CHILD", "1")
		.env("PIPEWIRE_RUNTIME_DIR", &fixture.directory)
		.env("XDG_RUNTIME_DIR", &fixture.directory)
		.env("PIPEWIRE_REMOTE", REMOTE)
		.output()?;
	assert!(
		output.status.success(),
		"private PipeWire child failed:\n{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	Ok(())
}

fn require_child() -> Result<(), Error> {
	if std::env::var_os("SUBNIRI_PRIVATE_PIPEWIRE_CHILD").is_none()
		|| std::env::var("PIPEWIRE_REMOTE").as_deref() != Ok(REMOTE)
	{
		return Err("run through the private PipeWire parent test".into());
	}
	Ok(())
}

async fn receive_until(volume: &mut Volume, ready: impl Fn(&Volume) -> bool) -> Result<(), Error> {
	tokio::time::timeout(Duration::from_secs(2), async {
		while !ready(volume) {
			let event = volume.event_rx.recv().await?;
			let _ = volume.update(Message::PwEvent(event));
		}
		Ok::<_, Error>(())
	})
	.await?
}

#[test]
#[ignore = "requires pipewire and pw-cli; creates only a private null sink"]
fn virtual_sink_remains_writable_after_external_update() -> Result<(), Error> {
	run_private("modules::volume::tests::virtual_sink_child")
}

#[tokio::test]
#[ignore = "private PipeWire subprocess helper"]
async fn virtual_sink_child() -> Result<(), Error> {
	require_child()?;
	let mut volume = Volume::new()?;
	receive_until(&mut volume, |volume| {
		volume.devices.values().any(|node| {
			node.name.as_deref() == Some(SINK) && !node.volume.channel_volums.is_empty()
		})
	})
	.await
	.map_err(|error| format!("initial sink volume was not received: {error}"))?;
	let _ = volume.update(Message::PwEvent(PwEvent::DefaultSinkChanged(SINK.into())));
	let id = volume.default_sink.ok_or("missing test sink")?;
	let output = Command::new("pw-cli")
		.args([
			"-r",
			REMOTE,
			"set-param",
			&id.to_string(),
			"Props",
			"{ channelVolumes = [ 0.125 0.125 ] }",
		])
		.output()?;
	assert!(
		output.status.success(),
		"external test write failed: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	receive_until(&mut volume, |volume| {
		volume
			.devices
			.get(&id)
			.is_some_and(|node| (node.volume.normalized() - 0.5).abs() < 0.01)
	})
	.await
	.map_err(|error| format!("external update was not observed: {error}"))?;
	// Incoming updates demonstrably work. Now drive the same write path as
	// the UI and require an observed server-side change, not an optimistic UI update.
	let _ = volume.update(Message::DefaultSinkVolumeSetRequest(0.75));
	receive_until(&mut volume, |volume| {
		volume
			.devices
			.get(&id)
			.is_some_and(|node| (node.volume.normalized() - 0.75).abs() < 0.01)
	})
	.await
	.map_err(|error| {
		format!(
			"UI write did not change the server volume despite working external updates: {error}"
		)
	})?;
	let _ = volume.update(Message::DefaultSinkVolumeChangeRequest(-0.25));
	receive_until(&mut volume, |volume| {
		volume
			.devices
			.get(&id)
			.is_some_and(|node| (node.volume.normalized() - 0.5).abs() < 0.01)
	})
	.await?;
	let channels = volume
		.devices
		.get(&id)
		.ok_or("missing sink")?
		.volume
		.channel_volums
		.clone();
	let _ = volume.update(Message::DefaultSinkMuteRequest(true));
	receive_until(&mut volume, |volume| {
		volume.devices.get(&id).is_some_and(|node| node.volume.mute)
	})
	.await?;
	assert_eq!(
		volume
			.devices
			.get(&id)
			.ok_or("missing sink")?
			.volume
			.channel_volums,
		channels,
		"mute-only commands must not replace channel volumes"
	);
	let _ = volume.update(Message::DefaultSinkMuteToggleRequest);
	receive_until(&mut volume, |volume| {
		volume
			.devices
			.get(&id)
			.is_some_and(|node| !node.volume.mute)
	})
	.await?;
	Ok(())
}

#[test]
#[ignore = "requires pipewire; creates only a private null sink"]
fn default_metadata_before_node_is_reconciled() -> Result<(), Error> {
	run_private("modules::volume::tests::default_before_node_child")
}

#[tokio::test]
#[ignore = "private PipeWire subprocess helper"]
async fn default_before_node_child() -> Result<(), Error> {
	require_child()?;
	let mut volume = Volume::new()?;
	let _ = volume.update(Message::PwEvent(PwEvent::DefaultSinkChanged(SINK.into())));
	receive_until(&mut volume, |volume| {
		volume
			.devices
			.values()
			.any(|node| node.name.as_deref() == Some(SINK))
	})
	.await?;
	assert!(
		volume.default_sink.is_some(),
		"default name must be retained until its node arrives"
	);
	let id = volume.default_sink.ok_or("missing default sink")?;
	let node = volume.devices.get(&id).ok_or("missing node")?.clone();
	let proxy = volume
		.node_proxies
		.get(&id)
		.ok_or("missing node proxy")?
		.clone();
	// Replay removal/reappearance through the reducer; this tests default
	// bookkeeping, not physical route/profile recovery.
	let _ = volume.update(Message::PwEvent(PwEvent::Disconnected(id)));
	assert!(volume.default_sink.is_none());
	let _ = volume.update(Message::PwEvent(PwEvent::UpsertNode { node, proxy }));
	assert_eq!(volume.default_sink, Some(id));
	let _ = volume.update(Message::PwEvent(PwEvent::DefaultSinkChanged(String::new())));
	assert!(volume.default_sink.is_none());
	assert!(volume.default_sink_name.is_none());
	Ok(())
}
