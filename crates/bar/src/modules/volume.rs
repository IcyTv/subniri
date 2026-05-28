#![allow(non_upper_case_globals)]
use std::{fmt, hash::Hash};

use iced::{
	Element, Font, Length, Subscription,
	alignment::Alignment,
	font,
	mouse::ScrollDelta,
	widget::{mouse_area, row, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button, neo_card},
};
use pipewire_native::{
	Id,
	context::Context,
	core::Core,
	permission::PermissionBits,
	properties::Properties,
	proxy::{
		metadata::{Metadata, MetadataEvents},
		node::{Node, NodeEvents},
		registry::{Registry, RegistryEvents},
	},
	thread_loop::ThreadLoop,
};
use pipewire_native_spa::{
	param::{ParamType, props::Prop},
	pod::{RawPod, RawPodOwned, parser::Parser, types::Type},
};
// use pipewire_native::{
// 	context::ContextRc,
// 	main_loop::MainLoopRc,
// 	node::{Node, NodeChangeMask, NodeListener},
// 	spa::{
// 		param::ParamType,
// 		pod::{Pod, Value, ValueArray, deserialize::PodDeserializer},
// 		sys::{
// 			SPA_PROP_channelVolumes, SPA_PROP_mute, SPA_PROP_volume, SPA_PROP_volumeBase,
// 			SPA_PROP_volumeStep,
// 		},
// 	},
// };
use small_map::SmallMap;

use crate::modules::{ICON_HEIGHT, MODULE_HEIGHT, MODULE_RADIUS};

#[derive(Debug, Clone)]
pub enum Message {
	VolumeChangeRequest(f32),
	MuteRequest(bool),
	PwEvent(PwEvent),
	Noop,
}

#[derive(Debug, Clone)]
enum PwCommand {
	SetVolume(String, f64),
	SetMute(String, bool),
}

#[derive(Debug, Clone)]
enum PwEvent {
	UpsertNode(PwNode),
	Disconnected(Id),
	DefaultSourceChanged(String),
	DefaultSinkChanged(String),
	VolumeChanged { node: Id, volume: PwVolumeUpdate },
}

#[derive(Debug, Clone)]
struct PwNode {
	id: Id,
	name: Option<String>,
	label: String,
	is_sink: bool,
	is_source: bool,
	volume: PwVolume,
}

struct PwEventReceiverHashable(pub async_channel::Receiver<PwEvent>);

impl Hash for PwEventReceiverHashable {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		0xdeadbeefu32.hash(state)
	}
}

#[derive(Clone)]
pub struct Volume {
	main_loop: ThreadLoop,
	context: Context,
	core: Core,
	registry: Registry,
	event_rx: async_channel::Receiver<PwEvent>,

	devices: SmallMap<64, Id, PwNode>,
	default_source: Option<Id>,
	default_sink: Option<Id>,
}

impl fmt::Debug for Volume {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Volume").finish_non_exhaustive()
	}
}

impl Volume {
	pub async fn new() -> Result<Self, String> {
		pipewire_native::init();

		log::trace!("Initialized pipewire");

		let mut props = Properties::new();
		props.set("library.name", "icytv/subniri".to_string());
		props.set("factory.name", "subniri".to_string());
		props.set("node.name", "polarbar".to_string());
		props.set("config.name", "null".to_string());

		let main_loop =
			ThreadLoop::new(&props).ok_or_else(|| format!("Could not create pw main loop"))?;

		log::trace!("Created thread loop");

		main_loop.run();

		let context = Context::new(main_loop.main_loop(), props).map_err(|e| format!("{e}"))?;
		log::trace!("Got context");
		let core = context.connect(None).map_err(|e| format!("{e}"))?;
		log::trace!("Got core");
		let registry = core.registry().map_err(|e| format!("{e}"))?;
		log::trace!("Got registry");

		let (event_tx, event_rx) = async_channel::unbounded();

		let global = {
			let event_tx = event_tx.clone();
			let registry = registry.clone();
			move |id: Id, _perms: PermissionBits, ty: &str, version: u32, props: &Properties| {
				let event_tx = event_tx.clone();
				if ty == pipewire_native::types::interface::NODE {
					let media_class = props.get("media.class");
					let is_source = matches!(media_class, Some("Audio/Source"));
					let is_sink = matches!(media_class, Some("Audio/Sink"));
					let is_duplex = matches!(media_class, Some("Audio/Duplex"));

					if !(is_source || is_sink || is_duplex) {
						return;
					}

					let node = match registry.bind(id, ty, version) {
						Ok(n) => n,
						Err(e) => {
							log::error!("Failed to bind pw global: {e}");
							return;
						}
					};
					let Some(node) = node.downcast::<Node>() else {
						log::error!("Expected node, found: {}", node.type_());
						return;
					};

					let label = get_node_name(props).unwrap_or_else(|| format!("Node {id}"));
					let name = props.get("node.name").map(ToString::to_string);

					if let Err(e) = event_tx.send_blocking(PwEvent::UpsertNode(PwNode {
						id,
						name,
						label,
						is_sink,
						is_source,
						// TODO: Can/Should we get the volume here? Or can we rely on pipewire to
						// quickly give us an event to update the volume?
						volume: PwVolume::default(),
					})) {
						log::warn!("Failed to send pw connected event: {e}");
					}

					let param = move |_seq: u32,
					                  param_ty: ParamType,
					                  _index: u32,
					                  _next: u32,
					                  pod: &RawPodOwned| {
						if param_ty != ParamType::Props {
							return;
						}

						log::info!("Got pod with type: {:?}", pod.type_());

						let volume = match decode_props(pod) {
							Ok(v) => v,
							Err(e) => {
								log::error!("Failed to parse volumes: {e:?}");
								return;
							}
						};

						log::info!("volumes: {volume:?}");

						if let Err(e) =
							event_tx.send_blocking(PwEvent::VolumeChanged { node: id, volume })
						{
							log::warn!("Failed to dispatch volume change event: {e}");
						}
					};

					node.add_listener(NodeEvents {
						param: Some(Box::new(param)),
						..Default::default()
					});

					if let Err(e) = node.subscribe_params(&[ParamType::Props]) {
						log::warn!("Failed to subscribe to node props: {e}");
					};
				} else if ty == pipewire_native::types::interface::METADATA {
					if props.get("metadata.name") != Some("default") {
						return;
					}

					let metadata = match registry.bind(id, ty, version) {
						Ok(m) => m,
						Err(e) => {
							log::error!("Failed to bind pw global: {e}");
							return;
						}
					};
					let Some(metadata) = metadata.downcast::<Metadata>() else {
						log::error!("Expected node, found: {}", metadata.type_());
						return;
					};

					let property = {
						let event_tx = event_tx.clone();
						move |_subject: Id,
						      key: Option<&str>,
						      _ty: Option<&str>,
						      value: Option<&str>| {
							match key {
								Some("default.audio.sink") => {
									if let Some(name) = default_node_name(value) {
										// FIXME: Get ID here
										log::info!("default sink node.name = {name}");
										if let Err(e) = event_tx
											.send_blocking(PwEvent::DefaultSinkChanged(name))
										{
											log::warn!(
												"Failed to dispatch default sink change: {e}"
											);
										}
									}
								}
								Some("default.audio.source") => {
									if let Some(name) = default_node_name(value) {
										log::info!("default source node.name = {name}");
										if let Err(e) = event_tx
											.send_blocking(PwEvent::DefaultSourceChanged(name))
										{
											log::warn!(
												"Failed to dispatch default source change: {e}"
											);
										}
									}
								}
								_ => (),
							}
						}
					};

					metadata.add_listener(MetadataEvents {
						property: Some(Box::new(property)),
					});
				}
			}
		};
		let global_remove = {
			let event_tx = event_tx.clone();
			move |id: Id| {
				if let Err(e) = event_tx.send_blocking(PwEvent::Disconnected(id)) {
					log::warn!("Failed to send pw global disconnect: {e}");
				}
			}
		};

		registry.add_listener(RegistryEvents {
			global: Some(Box::new(global)),
			global_remove: Some(Box::new(global_remove)),
		});

		log::trace!("Added global listeners");

		Ok(Self {
			main_loop,
			context,
			core,
			registry,
			event_rx,

			devices: SmallMap::new(),
			default_source: None,
			default_sink: None,
		})
	}

	pub fn subscription(&self) -> Subscription<Message> {
		Subscription::run_with(
			PwEventReceiverHashable(self.event_rx.clone()),
			move |event_rx| {
				let event_rx = event_rx.0.clone();

				async_stream::stream! {
					while let Ok(event) = event_rx.recv().await {
						yield Message::PwEvent(event);
					}

					log::warn!("PipeWire Event stream disconnected");
				}
			},
		)
	}

	pub fn update(&mut self, message: Message) {
		match message {
			Message::PwEvent(PwEvent::UpsertNode(node)) => {
				self.devices.insert(node.id, node);
			}
			Message::PwEvent(PwEvent::Disconnected(id)) => {
				let _ = self.devices.remove(&id);
			}
			Message::PwEvent(PwEvent::DefaultSinkChanged(name)) => {
				let id = self
					.devices
					.values()
					.find(|d| d.name.as_deref() == Some(&name))
					.map(|d| d.id);
				if id.is_none() {
					log::warn!("Default device '{name}' not found");
				}
				self.default_sink = id;
			}
			Message::PwEvent(PwEvent::DefaultSourceChanged(name)) => {
				let id = self
					.devices
					.values()
					.find(|d| d.name.as_deref() == Some(&name))
					.map(|d| d.id);
				if id.is_none() {
					log::warn!("Default device '{name}' not found");
				}
				self.default_source = id;
			}
			Message::PwEvent(PwEvent::VolumeChanged { node, volume }) => {
				if let Some(node) = self.devices.get_mut(&node) {
					node.volume.merge(volume);
				} else {
					log::warn!("Volume change for unknown device");
				}
			}
			_ => (),
		}
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		let sink = match self
			.default_sink
			.as_ref()
			.and_then(|id| self.devices.get(id))
		{
			Some(sink) => sink,
			None => {
				return neo_button("loading")
					.height(MODULE_HEIGHT)
					.radius(MODULE_RADIUS)
					.background(COLORS.decorative.yellow)
					.into();
			}
		};

		let volume = sink.volume.normalized();

		let icon = if volume < 0.001 || sink.volume.mute {
			phosphor_icon!("speaker-x")
		} else if volume < 0.3 {
			phosphor_icon!("speaker-none")
		} else if volume < 0.6 {
			phosphor_icon!("speaker-low")
		} else {
			phosphor_icon!("speaker-high")
		};

		neo_button(
			mouse_area(
				row![
					svg(icon).width(Length::Shrink).height(ICON_HEIGHT),
					text(format!("{:.0}%", volume * 100.0))
						.font(Font {
							weight: font::Weight::Bold,
							..Font::DEFAULT
						})
						.color(COLORS.text)
						.size(18)
						.align_y(Alignment::Center),
				]
				.spacing(5)
				.align_y(Alignment::Center),
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
			Message::VolumeChangeRequest(delta as f32)
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

#[derive(Debug, Default, Clone)]
struct PwVolume {
	volume: f32,
	channel_volums: Vec<f32>,
	volume_base: f32,
	max_volume: f32,
	step: f32,
	mute: bool,
	soft_volumes: Vec<f32>,
	soft_mute: bool,
}

#[derive(Debug, Default, Clone)]
struct PwVolumeUpdate {
	volume: Option<f32>,
	channel_volums: Option<Vec<f32>>,
	volume_base: Option<f32>,
	max_volume: Option<f32>,
	step: Option<f32>,
	mute: Option<bool>,
	soft_volumes: Option<Vec<f32>>,
	soft_mute: Option<bool>,
}

impl PwVolume {
	fn merge(&mut self, update: PwVolumeUpdate) {
		if let Some(volume) = update.volume {
			self.volume = volume;
		}
		if let Some(channel_volums) = update.channel_volums {
			self.channel_volums = channel_volums;
		}
		if let Some(volume_base) = update.volume_base {
			self.volume_base = volume_base;
		}
		if let Some(max_volume) = update.max_volume {
			self.max_volume = max_volume;
		}
		if let Some(step) = update.step {
			self.step = step;
		}
		if let Some(mute) = update.mute {
			self.mute = mute;
		}
		if let Some(soft_volumes) = update.soft_volumes {
			self.soft_volumes = soft_volumes;
		}
		if let Some(soft_mute) = update.soft_mute {
			self.soft_mute = soft_mute;
		}
	}

	fn linear_base(&self) -> f32 {
		if self.volume_base > 0.0 {
			self.volume_base.cbrt()
		} else {
			1.0
		}
	}

	fn linear(&self) -> f32 {
		if !self.channel_volums.is_empty() {
			self.channel_volums
				.iter()
				.copied()
				.map(|v| v.max(0.0).cbrt())
				.sum::<f32>()
				/ self.channel_volums.len() as f32
		} else {
			self.volume.max(0.0).cbrt()
		}
	}

	fn normalized(&self) -> f32 {
		self.linear() / self.linear_base()
	}

	fn percentage(&self) -> f32 {
		self.normalized() * 100.0
	}

	fn max_percentage(&self) -> f32 {
		let max_volume = if self.max_volume > 0.0 {
			self.max_volume
		} else {
			1.0
		};

		(max_volume / self.linear_base()) * 100.0
	}

	fn step_percent(&self) -> f32 {
		if self.step > 0.0 {
			(self.step.cbrt() / self.linear_base()) * 100.0
		} else {
			1.0
		}
	}
}

fn decode_props(pod: &RawPodOwned) -> Result<PwVolumeUpdate, pipewire_native_spa::pod::Error> {
	let mut parser = Parser::new(pod.data());

	let result = parser.pop_object_raw::<u32, _>(|object, _object_type, _id| {
		let mut out = PwVolumeUpdate::default();
		while let Some((raw_key, flags, value)) = object.pop_property()? {
			let Ok(key) = Prop::try_from(raw_key) else {
				log::debug!("skipping unknown Props key: {raw_key}");
				continue;
			};
			log::trace!(
				"prop {key:?} flags={flags:?} type={:?} -> {value}",
				value.type_()
			);

			match key {
				Prop::Volume => out.volume = Some(value.decode::<f32>()?),
				Prop::ChannelVolumes => out.channel_volums = Some(value.decode::<Vec<f32>>()?),
				Prop::VolumeBase => out.volume_base = Some(value.decode::<f32>()?),
				Prop::Params => out.max_volume = parse_channelmix_max_volume(&value)?,
				Prop::Mute => out.mute = Some(value.decode::<bool>()?),
				Prop::SoftVolumes => out.soft_volumes = Some(value.decode::<Vec<f32>>()?),
				Prop::SoftMute => out.soft_mute = Some(value.decode::<bool>()?),
				Prop::VolumeStep => out.step = Some(value.decode::<f32>()?),
				_ => (),
			}
		}

		Ok(out)
	});

	result.map(|(out, _)| out)
}

fn parse_channelmix_max_volume(
	pod: &RawPod<'_>,
) -> Result<Option<f32>, pipewire_native_spa::pod::Error> {
	if pod.type_() != Type::Struct {
		return Ok(None);
	}

	let mut parser = Parser::new(pod.data());
	let (max_volume, _) = parser.pop_struct(|parser| {
		let mut max_volume = None;

		while parser.available() > 0 {
			let key_pod = parser.pop_raw_pod()?;
			let value_pod = match parser.pop_raw_pod() {
				Ok(pod) => pod,
				Err(_) => break,
			};

			let Ok(key) = key_pod.decode::<String>() else {
				continue;
			};

			if key == "channelmix.max-volume" {
				max_volume = decode_numeric_pod(&value_pod)?;
				break;
			}
		}

		Ok(max_volume)
	})?;

	Ok(max_volume)
}

fn decode_numeric_pod(pod: &RawPod<'_>) -> Result<Option<f32>, pipewire_native_spa::pod::Error> {
	let value = match pod.type_() {
		Type::Float => Some(pod.decode::<f32>()?),
		Type::Double => Some(pod.decode::<f64>()? as f32),
		Type::Int => Some(pod.decode::<i32>()? as f32),
		Type::Long => Some(pod.decode::<i64>()? as f32),
		_ => None,
	};

	Ok(value)
}

fn get_node_name(props: &Properties) -> Option<String> {
	props
		.get("node.nick")
		.or_else(|| props.get("node.description"))
		.or_else(|| props.get("media.name"))
		.or_else(|| props.get("application.name"))
		.or_else(|| props.get("device.description"))
		.or_else(|| props.get("alsa.card_name"))
		.or_else(|| props.get("alsa.long_card_name"))
		.or_else(|| props.get("node.name"))
		.or_else(|| props.get("object.serial"))
		.map(ToString::to_string)
}

fn default_node_name(value: Option<&str>) -> Option<String> {
	let value = value?;
	let json: serde_json::Value = serde_json::from_str(value).ok()?;
	json.get("name")?.as_str().map(str::to_owned)
}
