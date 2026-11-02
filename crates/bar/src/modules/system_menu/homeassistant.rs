use std::{
	sync::Arc,
	time::{Duration, Instant},
};

use daemon_common::{
	HomeassistantCallableProxy, HomeassistantCapability, HomeassistantEntityProxy,
	HomeassistantTogglableProxy,
};
use futures::StreamExt;
use iced::{
	Animation, Border, Length, Subscription, Task,
	alignment::{Horizontal, Vertical},
	widget::{column, container, row, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button, neo_toggle_button},
};
use utilities::Hashable;

#[derive(Debug, Clone)]
pub enum Message {
	Snapshot {
		name: String,
		state: String,
		available: bool,
		capabilities: Vec<HomeassistantCapability>,
	},
	Disconnected,
	Activate,
	CommandFinished(Result<(), String>),
}

#[derive(Debug, Clone)]
pub struct HomeassistantEntity {
	id: String,
	connection: Hashable<zbus::Connection>,
	name: String,
	enabled: Animation<bool>,
	ty: HomeassistantEntityType,
	capabilities: Vec<HomeassistantCapability>,
	available: bool,
	reported_enabled: bool,
	pending: bool,
}

// TODO: Does this belong here? Not really, right?
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum HomeassistantEntityType {
	Automation,
	BinarySensor,
	Button,
	Climate,
	Event,
	Image,
	InputBoolean,
	InputDatetime,
	Light,
	Notify,
	Number,
	Person,
	Scene,
	Script,
	Select,
	Sensor,
	Switch,
	Update,
	Weather,
	Zone,

	Unknown,
	Other(String),
}

impl HomeassistantEntityType {
	// TODO: THis is non-exhaustive and maybe not even the right pattern for this... However, I need
	// a way to classify entities to display them in the UI. Maybe we can (in the future) configure
	// displays for unknown entity types, but for now this works...
	fn from_entity_id(id: &str) -> Self {
		match id.split_once('.').map(|(before, _)| before) {
			Some("automation") => Self::Automation,
			Some("binary_sensor") => Self::BinarySensor,
			Some("button") => Self::Button,
			Some("climate") => Self::Climate,
			Some("event") => Self::Event,
			Some("image") => Self::Image,
			Some("input_boolean") => Self::InputBoolean,
			Some("input_datetime") => Self::InputDatetime,
			Some("light") => Self::Light,
			Some("notify") => Self::Notify,
			Some("number") => Self::Number,
			Some("person") => Self::Person,
			Some("scene") => Self::Scene,
			Some("script") => Self::Script,
			Some("select") => Self::Select,
			Some("sensor") => Self::Sensor,
			Some("switch") => Self::Switch,
			Some("update") => Self::Update,
			Some("weather") => Self::Weather,
			Some("zone") => Self::Zone,
			Some(other) => Self::Other(other.to_string()),
			None => Self::Unknown,
		}
	}
}

impl HomeassistantEntity {
	pub fn new(connection: zbus::Connection, id: String) -> Self {
		Self {
			ty: HomeassistantEntityType::from_entity_id(&id),
			id,
			connection: Hashable::new(connection),
			name: "Unknown".to_string(),
			enabled: Animation::new(false).very_quick(),
			capabilities: Vec::new(),
			available: false,
			reported_enabled: false,
			pending: false,
		}
	}

	pub fn subscription(&self) -> Subscription<Message> {
		Subscription::run_with(
			Arc::new((self.connection.clone(), self.id.clone())),
			move |data| {
				let (connection, id) = Arc::unwrap_or_clone(data.clone());
				async_stream::stream! {
					loop {
						if let Err(error) = entity_subscription(&connection, &id).await {
							log::warn!("Failed to observe Home Assistant entity '{id}': {error}");
							yield Message::Disconnected;
							tokio::time::sleep(Duration::from_secs(2)).await;
							continue;
						}
						let observation = Observation { connection: connection.clone(), id: id.clone(), armed: true };
						// Subscribe before reading, so changes during the initial read are not lost.
						let result = async {
							let properties = zbus::fdo::PropertiesProxy::builder(&*connection)
								.destination(daemon_common::DEFAULT_BUS_NAME)?.path(entity_path(&id))?.build().await?;
							let changes = properties.receive_properties_changed_with_args(&[
								(0, daemon_common::HomeassistantEntityInfo::INTERFACE_NAME),
							]).await?;
							Ok::<_, zbus::Error>(changes)
						}.await;
						let mut changes = match result {
							Ok(changes) => changes,
							Err(error) => {
								log::warn!("Failed to watch Home Assistant entity '{id}': {error}");
								yield Message::Disconnected;
								observation.release().await;
								tokio::time::sleep(Duration::from_secs(2)).await;
								continue;
							}
						};
						loop {
							match read_entity(&connection, &id).await {
								Ok(snapshot) => yield snapshot,
								Err(error) => {
									log::warn!("Failed to read Home Assistant entity '{id}': {error}");
									yield Message::Disconnected;
									break;
								}
							}
							tokio::select! {
								change = changes.next() => if change.is_none() { break; },
								_ = tokio::time::sleep(Duration::from_secs(15)) => {
									// Restore observation after a daemon restart even if the
									// entity was recreated between two successful reads.
									if let Err(error) = entity_subscription(&connection, &id).await {
										log::warn!("Failed to reobserve Home Assistant entity '{id}': {error}");
										yield Message::Disconnected;
										break;
									}
								},
							}
						}
						// Drop the old observation before registering a replacement.
						observation.release().await;
						tokio::time::sleep(Duration::from_secs(2)).await;
					}
				}
			},
		)
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Snapshot {
				name,
				state,
				available,
				capabilities,
			} => {
				self.name = name;
				self.available = available;
				self.capabilities = capabilities;
				self.reported_enabled = state == "on";
				if !self.pending {
					self.enabled.go_mut(self.reported_enabled, Instant::now());
				}
			}
			Message::Disconnected => self.available = false,
			Message::Activate if self.available && !self.pending => {
				let callable = self
					.capabilities
					.contains(&HomeassistantCapability::Callable);
				if !callable
					&& !self
						.capabilities
						.contains(&HomeassistantCapability::Togglable)
				{
					return Task::none();
				}
				let target = callable || !self.enabled.value();
				self.enabled.go_mut(target, Instant::now());
				self.pending = true;
				let connection = self.connection.clone();
				let id = self.id.clone();
				return Task::future(async move {
					Message::CommandFinished(
						activate(&connection, &id, callable, target)
							.await
							.map_err(|error| error.to_string()),
					)
				});
			}
			Message::CommandFinished(result) => {
				self.pending = false;
				if let Err(error) = result {
					log::warn!("Home Assistant command for '{}' failed: {error}", self.id);
					self.enabled.go_mut(self.reported_enabled, Instant::now());
				} else if self
					.capabilities
					.contains(&HomeassistantCapability::Callable)
				{
					self.enabled.go_mut(self.reported_enabled, Instant::now());
				}
			}
			Message::Activate => {}
		}
		Task::none()
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		let icon_color =
			self.enabled
				.interpolate(COLORS.white, COLORS.decorative.green, Instant::now());
		let background =
			self.enabled
				.interpolate(COLORS.white, COLORS.decorative.green90, Instant::now());

		let callable = self
			.capabilities
			.contains(&HomeassistantCapability::Callable);
		let button = if callable {
			neo_button(
				row![
					container(svg(self.icon()).height(Length::Fill))
						.width(42)
						.height(42)
						.style(move |_| {
							container::Style {
								background: Some(icon_color.into()),
								border: Border {
									color: COLORS.black,
									width: 2.0,
									radius: 3.0.into(),
								},
								..Default::default()
							}
						})
						.padding(12),
					column![
						text(&self.name).color(COLORS.text).font(iced::Font {
							weight: iced::font::Weight::Bold,
							..Default::default()
						}),
						text(&self.id).color(COLORS.text).size(12)
					]
					.spacing(4)
					.align_x(Horizontal::Left)
					.width(Length::Fill),
				]
				.align_y(Vertical::Center)
				.width(Length::Fill)
				.spacing(12),
			)
			.background(background)
		} else {
			neo_toggle_button(
				self.icon(),
				&self.name,
				&self.id,
				self.enabled.value(),
				Some(icon_color),
			)
			.background(background)
		};
		// Keep the optimistic colors visible while the command is in flight.
		// update() serializes presses until its result arrives.
		button.on_press(Message::Activate).enabled(
			self.available
				&& (callable
					|| self
						.capabilities
						.contains(&HomeassistantCapability::Togglable)),
		)
	}

	// TODO: Allow for setting custom icons for entities. Maybe HA even provides some icons?
	// Use frontend/get_icons websocket command...
	// For mdi: prefixes: Pass them to the frontend/get_icons WebSocket command to fetch the SVG path dynamically.
	//
	// For integration logos: Use the /api/brands/... REST endpoints.
	//
	// For custom prefixes (e.g., fas:, hue:): You will either have to safely ignore them (falling back to a default icon), manually bundle popular sets like FontAwesome directly into your GUI's code, or attempt to parse and evaluate the user's custom JavaScript resources (which is highly discouraged as it is fragile and prone to breaking).
	pub fn icon<'a>(&self) -> svg::Handle {
		use HomeassistantEntityType::*;
		match self.ty {
			BinarySensor | InputBoolean => phosphor_icon!("toggle-left"),
			Light => phosphor_icon!("lightbulb-filament"),
			Script => phosphor_icon!("file"),
			_ => phosphor_icon!("house"),
		}
	}
}

fn entity_path(id: &str) -> String {
	format!(
		"{}/entity/{}",
		daemon_common::HOMEASSISTANT_OBJECT_PATH,
		id.replace('.', "/")
	)
}

async fn entity_proxy<'a>(
	connection: &'a zbus::Connection, id: &str,
) -> zbus::Result<HomeassistantEntityProxy<'a>> {
	HomeassistantEntityProxy::builder(connection)
		.path(entity_path(id))?
		.cache_properties(zbus::proxy::CacheProperties::No)
		.build()
		.await
}

async fn entity_subscription(connection: &zbus::Connection, id: &str) -> zbus::Result<()> {
	entity_proxy(connection, id).await?.observe().await?;
	Ok(())
}

async fn read_entity(connection: &zbus::Connection, id: &str) -> zbus::Result<Message> {
	let proxy = entity_proxy(connection, id).await?;
	let (name, state, available, capabilities) = tokio::try_join!(
		proxy.friendly_name(),
		proxy.state(),
		proxy.available(),
		proxy.capabilities()
	)?;
	Ok(Message::Snapshot {
		name,
		state,
		available,
		capabilities,
	})
}

async fn activate(
	connection: &zbus::Connection, id: &str, callable: bool, target: bool,
) -> zbus::Result<()> {
	if callable {
		HomeassistantCallableProxy::builder(connection)
			.path(entity_path(id))?
			.build()
			.await?
			.call()
			.await?;
	} else {
		HomeassistantTogglableProxy::builder(connection)
			.path(entity_path(id))?
			.build()
			.await?
			.set_enabled(target)
			.await?;
	}
	Ok(())
}

struct Observation {
	connection: Hashable<zbus::Connection>,
	id: String,
	armed: bool,
}

impl Observation {
	async fn release(mut self) {
		if let Ok(proxy) = entity_proxy(&self.connection, &self.id).await {
			let _ = proxy.unobserve().await;
		}
		self.armed = false;
	}
}

impl Drop for Observation {
	fn drop(&mut self) {
		if !self.armed {
			return;
		}
		let connection = self.connection.clone();
		let id = self.id.clone();
		tokio::spawn(async move {
			if let Ok(proxy) = entity_proxy(&connection, &id).await {
				let _ = proxy.unobserve().await;
			}
		});
	}
}
