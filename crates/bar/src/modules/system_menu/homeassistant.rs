use std::{sync::Arc, time::Instant};

use daemon_common::HomeassistantEntityProxy;
use futures::StreamExt;
use iced::{
	Animation, Border, Length, Subscription,
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
	NameUpdated(String),
	Noop,
}

#[derive(Debug, Clone)]
pub struct HomeassistantEntity {
	id: String,
	connection: Hashable<zbus::Connection>,
	name: String,
	enabled: Animation<bool>,
	ty: HomeassistantEntityType,
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
		}
	}
	//
	// pub fn init(&self) -> Task<Message> {
	// 	let connection = self.connection.clone();
	// 	Task::future(async move {
	// 		let proxy = HomeassistantEntityProxy::builder(&connection).destination(format!(
	// 			"/de/icytv/subniri/Homeassistant/entity/{}",
	// 			id.replace('.', "/")
	// 		))?;
	// 	})
	// }

	pub fn subscription(&self) -> Subscription<Message> {
		// TODO: This is horrid...
		Subscription::run_with(
			Arc::new((self.connection.clone(), self.id.clone())),
			move |data| {
				let (connection, id) = Arc::unwrap_or_clone(data.clone());
				async_stream::stream! {
					let path = format!(
								"/de/icytv/subniri/Homeassistant/entity/{}",
								id.replace('.', "/")
							);

					log::trace!("Trying to connect to object at '{path}'");

					let proxy =
						HomeassistantEntityProxy::builder(&connection)
							.path(path)
							.unwrap()
							.build()
							.await
							.unwrap();

					match proxy.observe().await {
						Ok(true) => log::info!("Observing homeassistant entity '{id}'"),
						Ok(false) => log::warn!("Failed to observe homeassistant entity '{id}'"),
						Err(e) => log::warn!("Failed to observe homeassistant entity '{id}': {e}"),
					}

					let name = proxy.friendly_name().await.unwrap();

					yield Message::NameUpdated(name);

					let mut changed_stream = proxy.receive_friendly_name_changed().await;
					while let Some(event) = changed_stream.next().await {
						let new_name = match event.get().await {
							Ok(name) => name,
							Err(e) => {
								log::warn!("Failed to get property '{}' of '{id}': {e}", event.name());
								continue;
							}
						};

						yield Message::NameUpdated(new_name);
					}

					log::warn!("Change stream for entity '{id}' lost...");


					let _ = proxy.unobserve().await;
				}
			},
		)
	}

	pub fn update(&mut self, message: Message) {
		match message {
			Message::NameUpdated(name) => self.name = name,
			Message::Noop => (),
		}
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		let icon_color =
			self.enabled
				.interpolate(COLORS.white, COLORS.decorative.green, Instant::now());
		let background =
			self.enabled
				.interpolate(COLORS.white, COLORS.decorative.green, Instant::now());

		// neo_toggle_button(
		// 	self.icon(),
		// 	&self.name,
		// 	&self.id,
		// 	self.enabled.value(),
		// 	Some(icon_color),
		// )
		// .background(background)
		use HomeassistantEntityType::*;
		match self.ty {
			Script => neo_button(
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
			),
			_ => neo_toggle_button(
				self.icon(),
				&self.name,
				&self.id,
				self.enabled.value(),
				Some(icon_color),
			)
			.background(background),
		}
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
