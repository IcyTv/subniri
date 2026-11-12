#![allow(non_upper_case_globals)]
#![allow(dead_code)] // TODO: Remove
use std::{
	collections::HashMap,
	fmt,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	time::{Duration, Instant},
};

use iced::{
	Element, Font, Length, Subscription, Task,
	alignment::Alignment,
	font,
	mouse::ScrollDelta,
	time,
	widget::{column, container, mouse_area, row, space, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::Colors,
	widgets::{NeoButton, neo_button, neo_card, neo_slider},
};
use pipewire_native::{
	Id,
	context::Context,
	core::Core,
	core::CoreEvents,
	permission::PermissionBits,
	properties::Properties,
	proxy::{
		device::{Device, DeviceEvents},
		metadata::{Metadata, MetadataEvents},
		node::{Node, NodeEvents},
		registry::{Registry, RegistryEvents},
	},
	thread_loop::ThreadLoop,
};
use pipewire_native_spa::{
	param::{ParamType, props::Prop, route::Route},
	pod::{
		RawPod, RawPodOwned,
		builder::Builder,
		parser::Parser,
		types::{Id as SpaPodId, ObjectType, PropertyFlags, Type},
	},
};
use utilities::Hashable;

use crate::modules::{ICON_HEIGHT, MODULE_HEIGHT, MODULE_RADIUS};

#[cfg(test)]
mod tests;

mod writer;

#[derive(Debug, Clone)]
pub enum Message {
	DefaultSinkVolumeChangeRequest(f32),
	DefaultSinkVolumeSetRequest(f32),
	DefaultSinkMuteRequest(bool),
	DefaultSinkMuteToggleRequest,
	HealthCheck,
	ReconnectFinished(Result<PwConnection, String>),
	PwEvent(PwEvent),
	Noop,
}

#[derive(Clone)]
pub(crate) enum PwEvent {
	UpsertNode {
		node: PwNode,
		proxy: Node,
	},
	UpsertDeviceRoute {
		device_id: Id,
		proxy: Device,
		route: PwRoute,
	},
	Disconnected(Id),
	DefaultSourceChanged(String),
	DefaultSinkChanged(String),
	VolumeChanged {
		node: Id,
		volume: PwVolumeUpdate,
	},
	CoreSyncDone(u32),
	ConnectionLost(String),
	SinkWriteFailed {
		generation: u64,
		error: String,
		connection_error: bool,
	},
}

impl fmt::Debug for PwEvent {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::UpsertNode { node, .. } => {
				f.debug_struct("UpsertNode").field("node", node).finish()
			}
			Self::Disconnected(id) => f.debug_tuple("Disconnected").field(id).finish(),
			Self::UpsertDeviceRoute {
				device_id, route, ..
			} => f
				.debug_struct("UpsertDeviceRoute")
				.field("device_id", device_id)
				.field("route", route)
				.finish(),
			Self::DefaultSourceChanged(name) => {
				f.debug_tuple("DefaultSourceChanged").field(name).finish()
			}
			Self::DefaultSinkChanged(name) => {
				f.debug_tuple("DefaultSinkChanged").field(name).finish()
			}
			Self::VolumeChanged { node, volume } => f
				.debug_struct("VolumeChanged")
				.field("node", node)
				.field("volume", volume)
				.finish(),
			Self::CoreSyncDone(seq) => f.debug_tuple("CoreSyncDone").field(seq).finish(),
			Self::ConnectionLost(reason) => f.debug_tuple("ConnectionLost").field(reason).finish(),
			Self::SinkWriteFailed {
				generation, error, ..
			} => f
				.debug_struct("SinkWriteFailed")
				.field("generation", generation)
				.field("error", error)
				.finish(),
		}
	}
}

#[derive(Clone)]
pub struct PwConnection {
	session: Arc<PwSession>,
}

struct PwSession {
	main_loop: ThreadLoop,
	context: Context,
	core: Core,
	registry: Registry,
	stopped: AtomicBool,
}

impl PwSession {
	fn stop(&self) {
		if !self.stopped.swap(true, Ordering::AcqRel) {
			// Finish callbacks while their Context still exists.
			self.main_loop.quit();
			self.core.disconnect();
		}
	}
}

impl Drop for PwSession {
	fn drop(&mut self) {
		self.stop();
	}
}

impl fmt::Debug for PwConnection {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("PwConnection").finish_non_exhaustive()
	}
}

#[derive(Debug, Clone)]
pub(crate) struct PwNode {
	id: Id,
	device_id: Option<Id>,
	name: Option<String>,
	label: String,
	is_sink: bool,
	is_source: bool,
	volume: PwVolume,
}

#[derive(Debug, Clone)]
pub(crate) struct PwRoute {
	index: i32,
	device: i32,
	direction: u32,
}

#[derive(Clone)]
struct PwDeviceState {
	proxy: Device,
	route: PwRoute,
}

#[derive(Clone)]
pub struct Volume {
	connection: Option<PwConnection>,
	sink_writer: writer::SinkWriter,
	event_tx: async_channel::Sender<PwEvent>,
	event_rx: Hashable<async_channel::Receiver<PwEvent>>,

	devices: HashMap<Id, PwNode>,
	node_proxies: HashMap<Id, Node>,
	device_routes: HashMap<Id, PwDeviceState>,
	default_source: Option<Id>,
	default_sink: Option<Id>,
	default_source_name: Option<String>,
	default_sink_name: Option<String>,
	reconnect_attempt: u32,
	reconnect_in_flight: bool,
	pending_health_check: Option<(u32, Instant)>,
}

impl fmt::Debug for Volume {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Volume").finish_non_exhaustive()
	}
}

impl PwConnection {
	#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
	fn connect(event_tx: async_channel::Sender<PwEvent>) -> Result<Self, String> {
		pipewire_native::init();

		log::trace!("Initialized pipewire");

		let mut props = Properties::new();
		props.set("library.name", "icytv/subniri".to_string());
		props.set("factory.name", "subniri".to_string());
		props.set("node.name", "polarbar".to_string());
		props.set("config.name", "null".to_string());

		let main_loop =
			ThreadLoop::new(&props).ok_or_else(|| "Could not create pw main loop".to_string())?;

		log::trace!("Created thread loop");

		main_loop.run();

		let context = Context::new(main_loop.main_loop(), props).map_err(|e| format!("{e}"))?;
		log::trace!("Got context");
		let core = context.connect(None).map_err(|e| format!("{e}"))?;
		log::trace!("Got core");
		let registry = core.registry().map_err(|e| format!("{e}"))?;
		log::trace!("Got registry");

		let mut core_events = CoreEvents::default();
		core_events.done = Some(Box::new({
			let event_tx = event_tx.clone();
			move |id, seq| {
				if id == 0
					&& let Err(e) = event_tx.send_blocking(PwEvent::CoreSyncDone(seq))
				{
					log::warn!("Failed to dispatch PipeWire core done event: {e}");
				}
			}
		}));
		core_events.error = Some(Box::new({
			let event_tx = event_tx.clone();
			move |id, _seq, res, message| {
				if id == 0 {
					send_connection_lost(
						&event_tx,
						format!("PipeWire core error {res}: {message}"),
					);
				} else {
					log::warn!("PipeWire object {id} rejected a request ({res}): {message}");
				}
			}
		}));
		core.add_listener(core_events);

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

					if let Err(e) = event_tx.send_blocking(PwEvent::UpsertNode {
						node: PwNode {
							id,
							device_id: props.get("device.id").and_then(|id| id.parse().ok()),
							name,
							label,
							is_sink: is_sink || is_duplex,
							is_source: is_source || is_duplex,
							volume: PwVolume::default(),
						},
						proxy: node.clone(),
					}) {
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

						log::trace!("Got pod with type: {:?}", pod.type_());

						let volume = match decode_props(pod) {
							Ok(v) => v,
							Err(e) => {
								log::error!("Failed to parse volumes: {e:?}");
								return;
							}
						};

						log::trace!("volumes: {volume:?}");

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
					}
				} else if ty == pipewire_native::types::interface::DEVICE {
					let device = match registry.bind(id, ty, version) {
						Ok(d) => d,
						Err(e) => {
							log::error!("Failed to bind pw global: {e}");
							return;
						}
					};
					let Some(device) = device.downcast::<Device>() else {
						log::error!("Expected device, found: {}", device.type_());
						return;
					};

					let param = {
						let event_tx = event_tx.clone();
						let device = device.clone();
						move |_seq: u32,
						      param_ty: ParamType,
						      _index: u32,
						      _next: u32,
						      pod: &RawPodOwned| {
							if param_ty != ParamType::Route {
								return;
							}

							let route = match decode_route(pod) {
								Ok(Some(route)) => route,
								Ok(None) => return,
								Err(e) => {
									log::error!("Failed to parse route: {e:?}");
									return;
								}
							};

							if let Err(e) = event_tx.send_blocking(PwEvent::UpsertDeviceRoute {
								device_id: id,
								proxy: device.clone(),
								route,
							}) {
								log::warn!("Failed to dispatch device route event: {e}");
							}
						}
					};

					device.add_listener(DeviceEvents {
						param: Some(Box::new(param)),
						..Default::default()
					});

					if let Err(e) = device.subscribe_params(&[ParamType::Route]) {
						log::warn!("Failed to subscribe to device route params: {e}");
					}
					if let Err(e) = device.enum_params(0, Some(ParamType::Route), 0, u32::MAX, None)
					{
						log::warn!("Failed to enumerate device route params: {e}");
					}
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
									let name = default_node_name(value).unwrap_or_default();
									log::info!("default sink node.name = {name}");
									if let Err(e) =
										event_tx.send_blocking(PwEvent::DefaultSinkChanged(name))
									{
										log::warn!("Failed to dispatch default sink change: {e}");
									}
								}
								Some("default.audio.source") => {
									let name = default_node_name(value).unwrap_or_default();
									log::info!("default source node.name = {name}");
									if let Err(e) =
										event_tx.send_blocking(PwEvent::DefaultSourceChanged(name))
									{
										log::warn!("Failed to dispatch default source change: {e}");
									}
								}
								None => {
									let _ = event_tx
										.send_blocking(PwEvent::DefaultSinkChanged(String::new()));
									let _ = event_tx.send_blocking(PwEvent::DefaultSourceChanged(
										String::new(),
									));
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
			session: Arc::new(PwSession {
				main_loop,
				context,
				core,
				registry,
				stopped: AtomicBool::new(false),
			}),
		})
	}

	fn health_check(&self) -> Result<u32, String> {
		self.session.core.sync().map_err(|e| format!("{e}"))
	}

	fn shutdown(self) {
		self.session.stop();
	}
}

impl Volume {
	pub fn new() -> Result<Self, String> {
		let (event_tx, event_rx) = async_channel::unbounded();
		let sink_writer = writer::SinkWriter::new(event_tx.clone())?;
		let connection = PwConnection::connect(event_tx.clone())?;

		Ok(Self {
			connection: Some(connection),
			sink_writer,
			event_tx,
			event_rx: Hashable::new(event_rx),

			devices: HashMap::new(),
			node_proxies: HashMap::new(),
			device_routes: HashMap::new(),
			default_source: None,
			default_sink: None,
			default_source_name: None,
			default_sink_name: None,
			reconnect_attempt: 0,
			reconnect_in_flight: false,
			pending_health_check: None,
		})
	}

	pub fn subscription(&self) -> Subscription<Message> {
		Subscription::batch([
			Subscription::run_with(self.event_rx.clone(), move |event_rx| {
				let event_rx = event_rx.clone();

				async_stream::stream! {
					while let Ok(event) = event_rx.recv().await {
						yield Message::PwEvent(event);
					}

					log::warn!("PipeWire Event stream disconnected");
					yield Message::PwEvent(PwEvent::ConnectionLost(
						"PipeWire event stream disconnected".to_string(),
					));
				}
			}),
			time::every(Duration::from_secs(5)).map(|_| Message::HealthCheck),
		])
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::PwEvent(PwEvent::UpsertNode { node, proxy }) => {
				self.node_proxies.insert(node.id, proxy);
				self.devices.insert(node.id, node);
				self.resolve_defaults();
				Task::none()
			}
			Message::PwEvent(PwEvent::UpsertDeviceRoute {
				device_id,
				proxy,
				route,
			}) => {
				self.device_routes
					.insert(device_id, PwDeviceState { proxy, route });
				Task::none()
			}
			Message::PwEvent(PwEvent::Disconnected(id)) => {
				let _ = self.node_proxies.remove(&id);
				let _ = self.device_routes.remove(&id);
				let _ = self.devices.remove(&id);
				self.resolve_defaults();
				Task::none()
			}
			Message::PwEvent(PwEvent::DefaultSinkChanged(name)) => {
				self.default_sink_name = (!name.is_empty()).then_some(name);
				self.resolve_defaults();
				Task::none()
			}
			Message::PwEvent(PwEvent::DefaultSourceChanged(name)) => {
				self.default_source_name = (!name.is_empty()).then_some(name);
				self.resolve_defaults();
				Task::none()
			}
			Message::PwEvent(PwEvent::VolumeChanged { node, volume }) => {
				if let Some(node) = self.devices.get_mut(&node) {
					node.volume.merge(volume);
				} else {
					log::warn!("Volume change for unknown device");
				}
				Task::none()
			}
			Message::PwEvent(PwEvent::CoreSyncDone(seq)) => {
				if self
					.pending_health_check
					.as_ref()
					.is_some_and(|(pending_seq, _)| *pending_seq == seq)
				{
					self.pending_health_check = None;
				}
				Task::none()
			}
			Message::PwEvent(PwEvent::ConnectionLost(reason)) => {
				self.handle_connection_lost(reason)
			}
			Message::PwEvent(PwEvent::SinkWriteFailed {
				generation,
				error,
				connection_error,
			}) => {
				if self.sink_writer.is_current(generation) {
					log::warn!("Failed to set default sink properties: {error}");
					if connection_error {
						return self.handle_connection_lost(format!(
							"Failed to set sink properties: {error}"
						));
					}
				}
				Task::none()
			}
			Message::HealthCheck => self.run_health_check(),
			Message::ReconnectFinished(result) => self.finish_reconnect(result),
			Message::DefaultSinkVolumeChangeRequest(delta) => {
				self.change_default_sink_volume(delta)
			}
			Message::DefaultSinkVolumeSetRequest(target) => self.set_default_sink_volume(target),
			Message::DefaultSinkMuteRequest(mute) => self.set_default_sink_mute(mute),
			Message::DefaultSinkMuteToggleRequest => self.default_sink_toggle_mute(),
			Message::Noop => Task::none(),
		}
	}

	fn change_default_sink_volume(&mut self, delta: f32) -> Task<Message> {
		let Some(sink_id) = self.default_sink else {
			return Task::none();
		};
		let Some(node) = self.devices.get(&sink_id) else {
			return Task::none();
		};
		let current = node.volume.normalized();
		self.set_default_sink_volume(current + delta)
	}

	fn set_default_sink_volume(&mut self, target: f32) -> Task<Message> {
		if !target.is_finite() {
			log::warn!("Ignoring non-finite volume request");
			return Task::none();
		}
		let Some(sink_id) = self.default_sink else {
			return Task::none();
		};
		let Some(node) = self.devices.get(&sink_id) else {
			return Task::none();
		};
		let target = target.clamp(0.0, node.volume.max_normalized().max(1.0));
		let channels = node.volume.channel_count();
		let channel_volume = node.volume.raw_from_normalized(target);
		let channel_volumes = vec![channel_volume; channels];
		self.set_default_sink_props(Some(channel_volumes), None)
	}

	fn set_default_sink_mute(&mut self, mute: bool) -> Task<Message> {
		self.set_default_sink_props(None, Some(mute))
	}

	fn set_default_sink_props(
		&mut self, channels: Option<Vec<f32>>, mute: Option<bool>,
	) -> Task<Message> {
		let Some(sink_id) = self.default_sink else {
			return Task::none();
		};
		let Some(node) = self.devices.get(&sink_id) else {
			return Task::none();
		};
		let target = if let Some(device) = node.device_id.and_then(|id| self.device_routes.get(&id))
		{
			writer::Target::Route(device.clone())
		} else if let Some(proxy) = self.node_proxies.get(&sink_id) {
			// Virtual sinks have no physical Device/Route, and hardware routes
			// can temporarily disappear during a profile change. Node Props
			// remain the control interface corresponding to our volume reads.
			writer::Target::Node(proxy.clone())
		} else {
			log::warn!("No control proxy for default sink {sink_id}");
			return Task::none();
		};

		let Some(connection) = self.connection.clone() else {
			return Task::none();
		};
		if let Err(error) = self
			.sink_writer
			.submit(target, channels, mute, Some(connection))
		{
			return self.handle_connection_lost(error);
		}

		Task::none()
	}

	fn resolve_defaults(&mut self) {
		self.default_sink = self.default_sink_name.as_deref().and_then(|name| {
			self.devices
				.values()
				.find(|node| node.is_sink && node.name.as_deref() == Some(name))
				.map(|node| node.id)
		});
		self.default_source = self.default_source_name.as_deref().and_then(|name| {
			self.devices
				.values()
				.find(|node| node.is_source && node.name.as_deref() == Some(name))
				.map(|node| node.id)
		});
	}

	fn default_sink_toggle_mute(&mut self) -> Task<Message> {
		let Some(sink_id) = self.default_sink else {
			return Task::none();
		};
		let Some(node) = self.devices.get(&sink_id) else {
			return Task::none();
		};

		let is_muted = node.volume.mute;

		self.set_default_sink_mute(!is_muted)
	}

	fn run_health_check(&mut self) -> Task<Message> {
		if self.reconnect_in_flight {
			return Task::none();
		}

		if let Some((seq, started_at)) = self.pending_health_check {
			if started_at.elapsed() >= Duration::from_secs(6) {
				self.pending_health_check = None;
				return self.handle_connection_lost(format!(
					"PipeWire health check timed out waiting for core sync ack {seq}"
				));
			}

			return Task::none();
		}

		let Some(connection) = &self.connection else {
			return self.schedule_reconnect();
		};

		match connection.health_check() {
			Ok(seq) => {
				self.pending_health_check = Some((seq, Instant::now()));
				Task::none()
			}
			Err(error) => {
				self.handle_connection_lost(format!("PipeWire health check failed: {error}"))
			}
		}
	}

	fn handle_connection_lost(&mut self, reason: impl AsRef<str>) -> Task<Message> {
		let reason = reason.as_ref();
		log::warn!("PipeWire connection lost: {reason}");

		self.pending_health_check = None;
		self.clear_runtime_state();

		if let Some(connection) = self.connection.take() {
			connection.shutdown();
		}

		self.schedule_reconnect()
	}

	fn finish_reconnect(&mut self, result: Result<PwConnection, String>) -> Task<Message> {
		self.reconnect_in_flight = false;

		match result {
			Ok(connection) => {
				log::info!("Reconnected to PipeWire");
				self.connection = Some(connection);
				self.reconnect_attempt = 0;
				self.pending_health_check = None;
				Task::none()
			}
			Err(error) => {
				log::warn!("Failed to reconnect to PipeWire: {error}");
				self.schedule_reconnect()
			}
		}
	}

	fn schedule_reconnect(&mut self) -> Task<Message> {
		if self.reconnect_in_flight {
			return Task::none();
		}

		self.reconnect_in_flight = true;
		let delay = reconnect_delay(self.reconnect_attempt);
		self.reconnect_attempt = self.reconnect_attempt.saturating_add(1);
		let event_tx = self.event_tx.clone();

		Task::perform(
			async move {
				tokio::time::sleep(delay).await;
				PwConnection::connect(event_tx)
			},
			Message::ReconnectFinished,
		)
	}

	fn clear_runtime_state(&mut self) {
		self.sink_writer.invalidate();
		self.devices.clear();
		self.node_proxies.clear();
		self.device_routes.clear();
		self.default_sink = None;
		self.default_source = None;
		self.default_sink_name = None;
		self.default_source_name = None;
	}

	pub fn view(&self, colors: Colors) -> NeoButton<'_, Message> {
		let Some(sink) = self
			.default_sink
			.as_ref()
			.and_then(|id| self.devices.get(id))
		else {
			return neo_button(super::module_label(text("loading").color(colors.on_accent)))
				.with_colors(colors)
				.height(MODULE_HEIGHT)
				.radius(MODULE_RADIUS)
				.background(colors.decorative.yellow);
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
			mouse_area(container(
				row![
					svg(icon)
						.width(Length::Shrink)
						.height(ICON_HEIGHT)
						.style(move |_, _| svg::Style {
							color: Some(colors.on_accent),
						}),
					super::module_label(
						text(format!("{:.0}%", volume * 100.0))
							.font(Font {
								weight: font::Weight::Bold,
								..Font::DEFAULT
							})
							.color(colors.on_accent)
							.size(18)
							.align_y(Alignment::Center)
					),
				]
				.spacing(5)
				.align_y(Alignment::Center),
			))
			.on_scroll(Self::on_scroll),
		)
		.with_colors(colors)
		.height(MODULE_HEIGHT)
		.radius(MODULE_RADIUS)
		.background(colors.decorative.yellow)
	}

	fn on_scroll(delta: ScrollDelta) -> Message {
		let delta = match delta {
			ScrollDelta::Pixels { y, .. } => (y / 120.0) * 0.01,
			ScrollDelta::Lines { y, .. } => y * 0.01,
		};

		if (delta.abs() * 100.0).round() > f32::EPSILON {
			Message::DefaultSinkVolumeChangeRequest(delta)
		} else {
			Message::Noop
		}
	}

	pub fn view_popup(&self, colors: Colors) -> Element<'_, Message> {
		let sink_available = self
			.default_sink
			.is_some_and(|id| self.devices.contains_key(&id));
		let source_available = self
			.default_source
			.is_some_and(|id| self.devices.contains_key(&id));
		let (sink_name, sink_volume) = self
			.default_sink
			.and_then(|id| self.devices.get(&id))
			.map_or(("Unavailable", PwVolume::default()), |d| {
				(d.label.as_str(), d.volume.clone())
			});

		let speaker_icon = if sink_volume.mute {
			phosphor_icon!("speaker-x")
		} else {
			phosphor_icon!("speaker-high")
		};
		let mut output_slider = neo_slider(0.0..=1.0, sink_volume.normalized())
			.with_colors(colors)
			.enabled(sink_available)
			.step(sink_volume.step());
		if sink_available {
			output_slider = output_slider.on_change_live(Message::DefaultSinkVolumeSetRequest);
		}

		let output = neo_card(
			column![
				row![
					text("Output").color(colors.text).font(Font {
						weight: font::Weight::Bold,
						..Default::default()
					}),
					space::horizontal(),
					text(if !sink_available {
						"Unavailable"
					} else if sink_volume.mute {
						"Muted"
					} else {
						"Active"
					})
					.color(colors.secondary)
					.size(12),
				],
				device_dropdown(sink_name, colors),
				row![
					neo_button(svg(speaker_icon).width(18).height(18).style(move |_, _| {
						svg::Style {
							color: Some(if sink_volume.mute {
								colors.on_accent
							} else {
								colors.text
							}),
						}
					},))
					.with_colors(colors)
					.background(if sink_volume.mute {
						colors.feedback.danger
					} else {
						colors.surface
					})
					.on_press(Message::DefaultSinkMuteToggleRequest)
					.enabled(sink_available)
					.width(36)
					.height(36),
					output_slider,
					text(format!("{:.0}%", sink_volume.percentage()))
						.color(colors.text)
						.size(16)
						.width(16 * 3)
				]
				.spacing(12)
				.align_y(Alignment::Center)
			]
			.spacing(12),
		)
		.with_colors(colors)
		.background(colors.body)
		.width(Length::Fill);

		let (source_name, source_volume) = self
			.default_source
			.and_then(|id| self.devices.get(&id))
			.map_or(("Unavailable", PwVolume::default()), |d| {
				(d.label.as_str(), d.volume.clone())
			});

		let mic_icon = if source_volume.mute {
			phosphor_icon!("microphone-slash")
		} else {
			phosphor_icon!("microphone")
		};

		let input = neo_card(
			column![
				row![
					text("Input").color(colors.text).font(Font {
						weight: font::Weight::Bold,
						..Default::default()
					}),
					space::horizontal(),
					text("Read-only").color(colors.secondary).size(12)
				],
				device_dropdown(source_name, colors),
				row![
					neo_button(svg(mic_icon).width(18).height(18).style(move |_, _| {
						svg::Style {
							color: Some(colors.text),
						}
					}))
					.with_colors(colors)
					.enabled(false)
					.width(36)
					.height(36),
					neo_slider(0.0..=1.0, source_volume.normalized())
						.with_colors(colors)
						.step(source_volume.step())
						.enabled(false),
					text(if source_available {
						format!("{:.0}%", source_volume.percentage())
					} else {
						"—".to_string()
					})
					.color(colors.text)
					.size(16)
					.width(16 * 3)
				]
				.spacing(12)
				.align_y(Alignment::Center)
			]
			.spacing(12),
		)
		.with_colors(colors)
		.background(colors.body)
		.width(Length::Fill);

		pub const POPUP_SIZE: (u32, u32) = (320, 360);

		neo_card(column![output, input].spacing(12))
			.with_colors(colors)
			.width(POPUP_SIZE.0 as f32)
			.background(colors.body)
			.radius(MODULE_RADIUS)
			.into()
	}
}

/// Device switching is deferred; keep its dropdown affordance visible but inert.
fn device_dropdown(label: &str, colors: Colors) -> Element<'_, Message> {
	neo_button(
		row![
			text(label)
				.size(14)
				.color(colors.text)
				.wrapping(text::Wrapping::None)
				.ellipsis(text::Ellipsis::End)
				.width(Length::Fill),
			svg(phosphor_icon!("caret-down", "bold"))
				.width(14)
				.height(14)
				.style(move |_, _| svg::Style {
					color: Some(colors.secondary)
				}),
		]
		.spacing(8)
		.align_y(Alignment::Center),
	)
	.with_colors(colors)
	.enabled(false)
	.width(Length::Fill)
	.into()
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
pub(crate) struct PwVolumeUpdate {
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
		if self.channel_volums.is_empty() {
			self.volume.max(0.0).cbrt()
		} else {
			self.channel_volums
				.iter()
				.copied()
				.map(|v| v.max(0.0).cbrt())
				.sum::<f32>()
				/ self.channel_volums.len() as f32
		}
	}

	fn channel_count(&self) -> usize {
		self.channel_volums.len().max(1)
	}

	fn raw_from_normalized(&self, normalized: f32) -> f32 {
		(normalized * self.linear_base()).powi(3)
	}

	fn normalized(&self) -> f32 {
		self.linear() / self.linear_base()
	}

	fn step(&self) -> f32 {
		if self.step > 0.0 {
			self.step.cbrt() / self.linear_base()
		} else {
			0.01
		}
	}

	fn percentage(&self) -> f32 {
		self.normalized() * 100.0
	}

	fn max_percentage(&self) -> f32 {
		self.max_normalized() * 100.0
	}

	fn max_normalized(&self) -> f32 {
		let max_volume = if self.max_volume > 0.0 {
			self.max_volume
		} else {
			1.0
		};

		max_volume.cbrt() / self.linear_base()
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

fn decode_route(pod: &RawPodOwned) -> Result<Option<PwRoute>, pipewire_native_spa::pod::Error> {
	let mut parser = Parser::new(pod.data());

	let (route, _) = parser.pop_object_raw::<u32, _>(|object, object_type, _id| {
		if object_type != ObjectType::ParamRoute {
			return Ok(None);
		}

		let mut index = None;
		let mut device = None;
		let mut direction = None;

		while let Some((raw_key, _flags, value)) = object.pop_property()? {
			let Ok(key) = Route::try_from(raw_key) else {
				continue;
			};

			match key {
				Route::Index => index = Some(value.decode::<i32>()?),
				Route::Device => device = Some(value.decode::<i32>()?),
				Route::Direction => direction = Some(value.decode::<SpaPodId<u32>>()?.0),
				_ => (),
			}
		}

		Ok(match (index, device, direction) {
			(Some(index), Some(device), Some(direction)) if direction == 1 => Some(PwRoute {
				index,
				device,
				direction,
			}),
			_ => None,
		})
	})?;

	Ok(route)
}

fn build_route_props_pod(
	channel_volumes: Option<&[f32]>, mute: Option<bool>,
) -> Result<RawPodOwned, pipewire_native_spa::pod::Error> {
	let mut route_props = [0u8; 4096];
	let builder = Builder::new(route_props.as_mut_slice()).push_object(
		ObjectType::Props,
		ParamType::Props as u32,
		|builder| volume_props(builder, channel_volumes, mute),
	);

	let built = builder.build()?;
	RawPodOwned::wrap(Vec::from(built))
}

fn volume_props<'a>(
	builder: pipewire_native_spa::pod::builder::ObjectBuilder<'a>, channels: Option<&[f32]>,
	mute: Option<bool>,
) -> pipewire_native_spa::pod::builder::ObjectBuilder<'a> {
	let builder = if let Some(channels) = channels {
		builder.push_property(
			Prop::ChannelVolumes,
			PropertyFlags::empty(),
			channels.to_vec(),
		)
	} else {
		builder
	};
	if let Some(mute) = mute {
		builder.push_property(Prop::Mute, PropertyFlags::empty(), mute)
	} else {
		builder
	}
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
			let Ok(value_pod) = parser.pop_raw_pod() else {
				break;
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
		#[allow(clippy::cast_possible_truncation)]
		Type::Double => Some(pod.decode::<f64>()? as f32),
		Type::Int => Some(pod.decode::<i32>()? as f32),
		Type::Long => Some(pod.decode::<i64>()? as f32),
		_ => None,
	};

	Ok(value)
}

fn send_connection_lost(event_tx: &async_channel::Sender<PwEvent>, reason: String) {
	if let Err(e) = event_tx.send_blocking(PwEvent::ConnectionLost(reason)) {
		log::warn!("Failed to dispatch PipeWire connection loss: {e}");
	}
}

fn reconnect_delay(attempt: u32) -> Duration {
	match attempt {
		0 => Duration::from_millis(250),
		1 => Duration::from_millis(500),
		2 => Duration::from_secs(1),
		3 => Duration::from_secs(2),
		_ => Duration::from_secs(5),
	}
}

fn is_connection_error(error: &std::io::Error) -> bool {
	matches!(
		error.kind(),
		std::io::ErrorKind::NotConnected
			| std::io::ErrorKind::ConnectionAborted
			| std::io::ErrorKind::ConnectionRefused
			| std::io::ErrorKind::ConnectionReset
			| std::io::ErrorKind::BrokenPipe
			| std::io::ErrorKind::UnexpectedEof
	)
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
