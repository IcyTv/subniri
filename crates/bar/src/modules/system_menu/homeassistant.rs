use std::{
	sync::Arc,
	time::{Duration, Instant},
};

use daemon_common::{
	HomeassistantCallableProxy, HomeassistantCapability, HomeassistantEntityProxy,
	HomeassistantTogglableProxy,
};
use futures::StreamExt;
use iced::{Animation, Subscription, Task, widget::svg};
use neo_widgets::{phosphor_icon, style::Colors, widgets::NeoButton};
use utilities::Hashable;

pub(super) mod integration;
mod interaction;
use interaction::{Command, Event, Interaction};

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
	interaction: Interaction,
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
			interaction: Interaction::default(),
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
		let event = match message {
			Message::Snapshot {
				name,
				state,
				available,
				capabilities,
			} => {
				self.name = name;
				Event::Snapshot {
					state,
					available,
					capabilities,
				}
			}
			Message::Disconnected => Event::Disconnected,
			Message::Activate => Event::Activate,
			Message::CommandFinished(result) => {
				if let Err(error) = &result {
					log::warn!("Home Assistant command for '{}' failed: {error}", self.id);
				}
				Event::Finished {
					success: result.is_ok(),
				}
			}
		};

		let change = self.interaction.update(event);

		if let Some(enabled) = change.animate {
			self.enabled.go_mut(enabled, Instant::now());
		}

		if let Some(command) = change.command {
			let connection = self.connection.clone();
			let id = self.id.clone();
			return Task::future(async move {
				Message::CommandFinished(
					activate(&connection, &id, command)
						.await
						.map_err(|error| error.to_string()),
				)
			});
		}

		Task::none()
	}

	pub fn title(&self) -> &str {
		&self.name
	}

	pub fn is_animating(&self, now: Instant) -> bool {
		self.enabled.is_animating(now)
	}

	pub fn view_tile(&self, size: iced::Size, colors: Colors) -> NeoButton<'_, Message> {
		let now = Instant::now();

		neo_widgets::widgets::neo_tile_with_colors(
			colors,
			self.icon(),
			self.name.as_str(),
			self.id.as_str(),
			if self.interaction.callable() {
				None
			} else {
				Some(self.enabled.value())
			},
			Some(
				self.enabled
					.interpolate(colors.surface, colors.decorative.green, now),
			),
			size,
		)
		.on_press(Message::Activate)
		.enabled(self.interaction.actionable())
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

async fn activate(connection: &zbus::Connection, id: &str, command: Command) -> zbus::Result<()> {
	if let Command::Call = command {
		HomeassistantCallableProxy::builder(connection)
			.path(entity_path(id))?
			.build()
			.await?
			.call()
			.await?;
	} else if let Command::SetEnabled(target) = command {
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

#[cfg(test)]
mod tests {
	use super::*;
	use crate::modules::test_bus;

	struct ObservedEntity(tokio::sync::mpsc::UnboundedSender<&'static str>);

	#[zbus::interface(name = "de.icytv.subniri.Homeassistant.Entity")]
	impl ObservedEntity {
		fn observe(&self) -> bool {
			let _ = self.0.send("observe");
			true
		}
		fn unobserve(&self) {
			let _ = self.0.send("unobserve");
		}
		#[zbus(property)]
		fn friendly_name(&self) -> &str {
			"Observed"
		}
		#[zbus(property)]
		fn state(&self) -> &str {
			let _ = self.0.send("read");
			"off"
		}
		#[zbus(property)]
		fn available(&self) -> bool {
			true
		}
		#[zbus(property)]
		fn capabilities(&self) -> Vec<HomeassistantCapability> {
			vec![HomeassistantCapability::Togglable]
		}
	}

	#[test]
	#[ignore = "requires dbus-daemon; uses only a private bus"]
	fn observation_cancellation_releases_daemon_interest() -> Result<(), test_bus::Error> {
		test_bus::run("modules::system_menu::homeassistant::tests::observation_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn observation_child() -> Result<(), test_bus::Error> {
		test_bus::require_child()?;
		let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
		let _daemon = zbus::connection::Builder::session()?
			.name(daemon_common::DEFAULT_BUS_NAME.as_str())?
			.serve_at(entity_path("light.test"), ObservedEntity(tx))?
			.build()
			.await?;
		// Ignore GetAll reads made while registering the fixture itself.
		while rx.try_recv().is_ok() {}
		let card =
			HomeassistantEntity::new(zbus::Connection::session().await?, "light.test".into());
		let mut stream = futures::stream::select_all(
			iced::advanced::subscription::into_recipes(card.subscription())
				.into_iter()
				.map(|recipe| recipe.stream(futures::stream::pending().boxed())),
		);
		let first = tokio::time::timeout(Duration::from_secs(2), stream.next()).await?;
		assert!(matches!(
			first,
			Some(Message::Snapshot {
				available: true,
				..
			})
		));
		assert_eq!(rx.recv().await, Some("observe"));
		assert_eq!(rx.recv().await, Some("read"));
		drop(stream);
		assert_eq!(
			tokio::time::timeout(Duration::from_secs(2), rx.recv()).await?,
			Some("unobserve")
		);
		Ok(())
	}

	#[test]
	#[ignore = "requires dbus-daemon; uses only a private bus"]
	fn card_interaction_reconciles_snapshots_and_commands() -> Result<(), test_bus::Error> {
		test_bus::run("modules::system_menu::homeassistant::tests::interaction_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn interaction_child() -> Result<(), test_bus::Error> {
		test_bus::require_child()?;
		let connection = zbus::Connection::session().await?;
		let mut card = HomeassistantEntity::new(connection, "light.test".into());
		let snapshot = |capabilities| Message::Snapshot {
			name: "Test".into(),
			state: "off".into(),
			available: true,
			capabilities,
		};
		assert_eq!(card.update(Message::Activate).units(), 0);
		let _ = card.update(snapshot(vec![]));
		assert_eq!(card.update(Message::Activate).units(), 0);
		let _ = card.update(snapshot(vec![HomeassistantCapability::Togglable]));
		assert!(card.update(Message::Activate).units() > 0);
		assert!(card.enabled.value());
		assert_eq!(card.update(Message::Activate).units(), 0);
		let _ = card.update(snapshot(vec![HomeassistantCapability::Togglable]));
		assert!(
			card.enabled.value(),
			"pending optimistic state wins until completion"
		);
		let _ = card.update(Message::CommandFinished(Err("rejected".into())));
		assert!(!card.enabled.value());
		let _ = card.update(snapshot(vec![
			HomeassistantCapability::Callable,
			HomeassistantCapability::Togglable,
		]));
		assert!(card.update(Message::Activate).units() > 0);
		assert!(card.enabled.value());
		let _ = card.update(Message::CommandFinished(Ok(())));
		assert!(
			!card.enabled.value(),
			"callable completion reconciles to reported state"
		);
		let _ = card.update(Message::Disconnected);
		assert_eq!(card.update(Message::Activate).units(), 0);
		Ok(())
	}
}
