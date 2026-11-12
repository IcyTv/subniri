//! Serialize potentially blocking sink writes outside Iced's update thread.
use super::*;
use std::sync::{
	Arc,
	atomic::{AtomicU64, Ordering},
	mpsc,
};

pub(super) enum Target {
	Route(PwDeviceState),
	Node(Node),
	#[cfg(test)]
	Probe(Box<dyn FnOnce() -> std::io::Result<()> + Send>),
}

struct Write {
	target: Target,
	channels: Option<Vec<f32>>,
	mute: Option<bool>,
	generation: u64,
	// A proxy only weakly retains its Context; queued work must keep it alive.
	_connection: Option<PwConnection>,
}

impl Write {
	fn apply(self) -> std::io::Result<()> {
		let Self {
			target,
			channels,
			mute,
			_connection,
			..
		} = self;

		match target {
			#[cfg(test)]
			Target::Probe(action) => action(),
			Target::Route(device) => {
				let props = build_route_props_pod(channels.as_deref(), mute)
					.map_err(|error| std::io::Error::other(format!("{error:?}")))?;
				let route = device.route;

				device.proxy.set_param(
					ParamType::Route,
					ObjectType::ParamRoute,
					0,
					Box::new(move |builder| {
						builder
							.push_property(Route::Index, PropertyFlags::empty(), route.index)
							.push_property(Route::Device, PropertyFlags::empty(), route.device)
							.push_property(
								Route::Direction,
								PropertyFlags::empty(),
								SpaPodId(route.direction),
							)
							.push_property(Route::Props, PropertyFlags::empty(), props)
					}),
				)
			}
			Target::Node(proxy) => proxy.set_param(
				ParamType::Props,
				ObjectType::Props,
				0,
				Box::new(move |builder| volume_props(builder, channels.as_deref(), mute)),
			),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn sink_writes_are_ordered_and_invalidated_work_is_discarded()
	-> Result<(), Box<dyn std::error::Error>> {
		let (events, errors) = async_channel::unbounded();
		let writer = SinkWriter::new(events)?;
		let (sent, received) = mpsc::channel();
		for value in [1, 2] {
			let sent = sent.clone();
			writer.submit(
				Target::Probe(Box::new(move || {
					sent.send(value).map_err(std::io::Error::other)
				})),
				None,
				None,
				None,
			)?;
		}
		assert_eq!(received.recv_timeout(Duration::from_secs(2))?, 1);
		assert_eq!(received.recv_timeout(Duration::from_secs(2))?, 2);

		let (started, waiting) = mpsc::channel();
		let (release, blocked) = mpsc::channel();
		writer.submit(
			Target::Probe(Box::new(move || {
				started.send(()).map_err(std::io::Error::other)?;
				blocked
					.recv_timeout(Duration::from_secs(2))
					.map_err(std::io::Error::other)?;
				Err(std::io::Error::other("old connection failed"))
			})),
			None,
			None,
			None,
		)?;
		waiting.recv_timeout(Duration::from_secs(2))?;
		let stale_sent = sent.clone();
		writer.submit(
			Target::Probe(Box::new(move || {
				stale_sent.send(3).map_err(std::io::Error::other)
			})),
			None,
			None,
			None,
		)?;

		writer.invalidate();
		release.send(())?;
		writer.submit(
			Target::Probe(Box::new(move || {
				sent.send(4).map_err(std::io::Error::other)
			})),
			None,
			None,
			None,
		)?;

		assert_eq!(
			received.recv_timeout(Duration::from_secs(2))?,
			4,
			"queued old-connection writes must not run"
		);
		let error = errors.try_recv()?;
		assert!(
			matches!(error, PwEvent::SinkWriteFailed { generation, connection_error: false, .. } if !writer.is_current(generation)),
			"late write failures must carry their stale connection generation"
		);
		Ok(())
	}
}

#[derive(Clone)]
pub(super) struct SinkWriter {
	sender: mpsc::Sender<Write>,
	generation: Arc<AtomicU64>,
}

impl SinkWriter {
	pub(super) fn new(events: async_channel::Sender<PwEvent>) -> Result<Self, String> {
		let (sender, receiver) = mpsc::channel::<Write>();
		let generation = Arc::new(AtomicU64::new(0));
		let current = generation.clone();

		std::thread::Builder::new()
			.name("bar-audio-writes".into())
			.spawn(move || {
				while let Ok(write) = receiver.recv() {
					let generation = write.generation;
					if current.load(Ordering::Acquire) != generation {
						continue;
					}

					if let Err(error) = write.apply() {
						let _ = events.send_blocking(PwEvent::SinkWriteFailed {
							generation,
							connection_error: is_connection_error(&error),
							error: error.to_string(),
						});
					}
				}
			})
			.map_err(|error| format!("Failed to start audio writer: {error}"))?;

		Ok(Self { sender, generation })
	}

	pub(super) fn submit(
		&self, target: Target, channels: Option<Vec<f32>>, mute: Option<bool>,
		connection: Option<PwConnection>,
	) -> Result<(), String> {
		self.sender
			.send(Write {
				target,
				channels,
				mute,
				generation: self.generation.load(Ordering::Acquire),
				_connection: connection,
			})
			.map_err(|_| "Audio writer disconnected".to_owned())
	}

	pub(super) fn invalidate(&self) {
		self.generation.fetch_add(1, Ordering::AcqRel);
	}

	pub(super) fn is_current(&self, generation: u64) -> bool {
		self.generation.load(Ordering::Acquire) == generation
	}
}
