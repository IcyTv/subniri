use std::{cell::RefCell, fmt, hash::Hash, ops::Deref, rc::Rc};

use async_channel::{Receiver, Sender};
use float_ord::FloatOrd;
use iced::{
	Element, Font, Length, Subscription,
	alignment::Vertical,
	font,
	mouse::ScrollDelta,
	widget::{mouse_area, row, svg, text},
};
use libpulse_binding::{
	callbacks::ListResult,
	context::{
		Context, FlagSet,
		introspect::{Introspector, SinkInfo},
		subscribe::{Facility, InterestMaskSet, Operation},
	},
	proplist::{Proplist, properties::APPLICATION_NAME},
	volume::Volume as PulseVolume,
};
use libpulse_tokio::TokioMain;
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button, neo_card},
};
use tokio::task::LocalSet;

use crate::modules::{ICON_HEIGHT, MODULE_HEIGHT, MODULE_RADIUS};

#[derive(Debug, Clone, PartialEq, Hash)]
pub enum Message {
	VolumeChanged(FloatOrd<f64>, bool),
	VolumeChangeRequest(FloatOrd<f64>),
	Noop,
}

#[derive(Debug, Clone, Copy)]
enum VolumeRequest {
	ChangeBy(f64),
}

#[derive(Clone)]
struct EventReceiver(Receiver<Message>);

impl Deref for EventReceiver {
	type Target = Receiver<Message>;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl Hash for EventReceiver {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		0xdeadbeefu32.hash(state)
	}
}

#[derive(Clone)]
pub struct Volume {
	events: EventReceiver,
	requests: Sender<VolumeRequest>,
	volume: f64,
	muted: bool,
}

impl fmt::Debug for Volume {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Volume")
			.field("volume", &self.volume)
			.field("muted", &self.muted)
			.finish()
	}
}

impl Volume {
	pub async fn new() -> Result<Self, String> {
		let (event_tx, event_rx) = async_channel::unbounded();
		let (request_tx, request_rx) = async_channel::unbounded();

		std::thread::spawn(move || {
			// Create a single-threaded Tokio runtime for this thread
			let rt = tokio::runtime::Builder::new_current_thread()
				.enable_all()
				.build()
				.unwrap();

			let localset = LocalSet::new();
			let _local_guard = localset.enter();
			localset.block_on(&rt, async move {
				let mut main = TokioMain::new();
				let mut proplist = Proplist::new().unwrap();
				proplist.set_str(APPLICATION_NAME, "polarbar").unwrap();

				let mut context = Context::new_with_proplist(&main, "polarbar", &proplist)
					.expect("Failed to get pulseaudio context. Is pipewire-pulse running?");

				context
					.connect(None, FlagSet::NOFLAGS, None)
					.expect("Failed to connect");

				main.wait_for_ready(&context).await.unwrap();

				let introspector = Rc::new(RefCell::new(context.introspect()));

				request_default_sink_update(introspector.clone(), event_tx.clone());

				tokio::task::spawn_local(handle_volume_requests(introspector.clone(), request_rx));

				context.set_subscribe_callback(Some(Box::new(
					move |facility, operation, index| {
						if is_sink_update(facility, operation) {
							request_sink_update_if_default(
								introspector.clone(),
								event_tx.clone(),
								index,
							);
						}
					},
				)));

				context.subscribe(InterestMaskSet::SINK, |success| {
					assert!(success, "Failed to subscribe")
				});

				main.run().await;
			});
		});

		Ok(Self {
			events: EventReceiver(event_rx),
			requests: request_tx,
			volume: 0.0,
			muted: true,
		})
	}

	pub fn subscription(&self) -> Subscription<Message> {
		let events = self.events.clone();
		Subscription::run_with(events, move |events| {
			let events = events.clone();
			async_stream::stream! {
				while let Ok(event) = events.recv().await {
					yield event;
				}
				log::warn!("PulseAudio event stream disconnected");
			}
		})
	}

	pub fn update(&mut self, message: Message) {
		match message {
			Message::VolumeChanged(volume, muted) => {
				self.volume = volume.0;
				self.muted = muted;
			}
			Message::VolumeChangeRequest(by) => {
				if self
					.requests
					.send_blocking(VolumeRequest::ChangeBy(by.0))
					.is_err()
				{
					log::warn!("PulseAudio volume request receiver disconnected");
				}
			}
			Message::Noop => (),
		}
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		// TODO Icon for no audio device speaker-slash

		let icon = if self.volume < 0.001 || self.muted {
			phosphor_icon!("speaker-x")
		} else if self.volume < 30.0 {
			phosphor_icon!("speaker-none")
		} else if self.volume < 60.0 {
			phosphor_icon!("speaker-low")
		} else {
			phosphor_icon!("speaker-high")
		};

		neo_button(
			mouse_area(
				row![
					svg(icon).width(Length::Shrink).height(ICON_HEIGHT),
					text(format!("{:.0}%", self.volume))
						.font(Font {
							weight: font::Weight::Bold,
							..Font::DEFAULT
						})
						.color(COLORS.text)
						.size(18)
						.align_y(Vertical::Center)
				]
				.spacing(5.)
				.align_y(Vertical::Center),
			)
			.on_scroll(Self::on_scroll),
		)
		.height(MODULE_HEIGHT)
		.radius(MODULE_RADIUS)
		.background(COLORS.decorative.yellow)
		.into()
	}

	fn on_scroll(delta: ScrollDelta) -> Message {
		let delta = match delta {
			ScrollDelta::Pixels { y, .. } => {
				let step = (y / 120.0) * 0.01;
				step
			}
			ScrollDelta::Lines { y, .. } => y * 0.01,
		};

		if (delta.abs() * 100.0).round() > f32::EPSILON {
			Message::VolumeChangeRequest(FloatOrd(delta as f64))
		} else {
			Message::Noop
		}
	}

	pub fn view_popup(&self) -> Element<'_, Message> {
		neo_card("")
			.width(320)
			.background(COLORS.decorative.yellow)
			.radius(MODULE_RADIUS)
			.into()
	}
}

fn is_sink_update(facility: Option<Facility>, operation: Option<Operation>) -> bool {
	facility == Some(Facility::Sink) && operation != Some(Operation::Removed)
}

async fn handle_volume_requests(
	introspector: Rc<RefCell<Introspector>>, requests: Receiver<VolumeRequest>,
) {
	while let Ok(request) = requests.recv().await {
		match request {
			VolumeRequest::ChangeBy(delta) => {
				request_default_sink_volume_change(introspector.clone(), delta);
			}
		}
	}

	log::warn!("PulseAudio volume request stream disconnected");
}

fn request_default_sink_update(introspector: Rc<RefCell<Introspector>>, tx: Sender<Message>) {
	let server_introspector = introspector.clone();

	server_introspector
		.borrow()
		.get_server_info(move |server_info| {
			let Some(default_sink) = server_info.default_sink_name.as_ref() else {
				return;
			};

			let tx = tx.clone();
			introspector
				.borrow()
				.get_sink_info_by_name(default_sink, move |result| {
					send_sink_update(&tx, result);
				});
		});
}

fn request_sink_update_if_default(
	introspector: Rc<RefCell<Introspector>>, tx: Sender<Message>, index: u32,
) {
	let server_introspector = introspector.clone();

	server_introspector
		.borrow()
		.get_server_info(move |server_info| {
			let Some(default_sink_name) =
				server_info.default_sink_name.as_deref().map(str::to_owned)
			else {
				return;
			};

			let tx = tx.clone();
			introspector
				.borrow()
				.get_sink_info_by_index(index, move |result| {
					let ListResult::Item(info) = result else {
						return;
					};

					if info.name.as_deref() == Some(default_sink_name.as_str()) {
						send_sink_info_update(&tx, info);
					}
				});
		});
}

fn request_default_sink_volume_change(introspector: Rc<RefCell<Introspector>>, delta: f64) {
	let server_introspector = introspector.clone();

	server_introspector
		.borrow()
		.get_server_info(move |server_info| {
			let Some(default_sink) = server_info.default_sink_name.as_ref() else {
				return;
			};

			let volume_introspector = introspector.clone();
			introspector
				.clone()
				.borrow()
				.get_sink_info_by_name(default_sink, move |result| {
					let ListResult::Item(info) = result else {
						return;
					};

					set_sink_volume(volume_introspector.clone(), info, delta);
				});
		});
}

fn set_sink_volume(introspector: Rc<RefCell<Introspector>>, info: &SinkInfo, delta: f64) {
	let current = sink_volume_percent(info);
	let target = (current + delta * 100.0).clamp(0.0, max_volume_percent());
	let raw = (target / 100.0 * PulseVolume::NORMAL.0 as f64).round();

	let mut volume = info.volume;
	volume.set(volume.len(), PulseVolume(raw as u32));

	introspector.borrow_mut().set_sink_volume_by_index(
		info.index,
		&volume,
		Some(Box::new(|success| {
			if !success {
				log::warn!("Failed to set PulseAudio sink volume");
			}
		})),
	);
}

fn send_sink_update(tx: &Sender<Message>, result: ListResult<&SinkInfo>) {
	let ListResult::Item(info) = result else {
		return;
	};

	send_sink_info_update(tx, info);
}

fn send_sink_info_update(tx: &Sender<Message>, info: &SinkInfo) {
	let percent = sink_volume_percent(info);
	let message = Message::VolumeChanged(FloatOrd(percent), info.mute);

	if tx.send_blocking(message).is_err() {
		log::warn!("Pulseaudio volume update receiver disconnected");
	}
}

fn sink_volume_percent(info: &SinkInfo) -> f64 {
	let volume = info.volume.avg();

	volume.0 as f64 / PulseVolume::NORMAL.0 as f64 * 100.0
}

fn max_volume_percent() -> f64 {
	PulseVolume::ui_max().0 as f64 / PulseVolume::NORMAL.0 as f64 * 100.0
}

// fn get_name() -> String {
// let name = info
//     .proplist
//     .get("node.nick")
//     .or_else(|| info.proplist.get("device.nick"))
//     .or_else(|| info.proplist.get("alsa.card_name"))
//     .or_else(|| info.proplist.get("device.product.name"))
//     .or_else(|| info.proplist.get("device.description"))
//     .or_else(|| info.proplist.get("device.description"))
//     .unwrap_or(b"<unknown>");
// let name = std::str::from_utf8(name)
//     .ok()
//     .or_else(|| info.name.as_deref())
//     .unwrap_or("<unknown>");
// }
