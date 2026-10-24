use config::ConfigFile;
use daemon_common::{
	HOMEASSISTANT_OBJECT_PATH, HOMEASSISTANT_REDIRECT_URI,
	HomeassistantEntityObjectManagerProxyExt, HomeassistantEntityProxy, HomeassistantEntitySummary,
	HomeassistantProxy, HomeassistantStatus,
};
use iced::{
	Alignment, Border, Color, Element, Length, Task, font,
	widget::{Svg, column, container, grid, row, scrollable, space, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{neo_card, neo_toggle, neo_toggle_button},
};

use crate::{
	oauth::{CallbackListener, OAUTH_CALLBACK_TIMEOUT},
	setting::{InputCardOptions, input_card},
};

#[derive(Clone, Debug)]
pub enum Message {
	UpdateAuthenticationStatus(bool, HomeassistantStatus),
	UpdateEntitiesAndStatus(Vec<HomeassistantEntity>, bool, HomeassistantStatus),
	AuthenticationFailed(String),

	EntitiesFailed(String),

	StartAuthFlow,

	ToggleEnabled(bool),
	ToggleEntity(String, bool),
	OnUrlChanged(String),

	UpdateConfig,
}

pub struct Homeassistant {
	authenticated: bool,
	auth_enabled: bool,
	url: String,
	status: HomeassistantStatus,

	entities: Option<Vec<HomeassistantEntity>>,
}

impl Homeassistant {
	pub fn new() -> Self {
		Self {
			authenticated: false,
			auth_enabled: true,
			url: String::new(),
			status: HomeassistantStatus::NoUrl,

			entities: None,
		}
	}

	pub fn init(config: &ConfigFile) -> Task<Message> {
		let status = Task::future(async {
			// match homeassistant_status(None).await {
			// 	Ok((authenticated, status)) => {
			// 		Message::UpdateAuthenticationStatus(authenticated, status)
			// 	}
			// 	Err(error) => Message::AuthenticationFailed(error.to_string()),
			// }
			let connection = match zbus::Connection::session().await {
				Ok(conn) => conn,
				Err(err) => {
					log::warn!("Failed to connect to D-Bus: {err}");
					return Err(Box::new(err) as Box<dyn std::error::Error + Send + Sync>);
				}
			};
			let status = wait_for_catalog(&connection).await?;

			Ok((status, connection))
		})
		.map_err(InitError::Status)
		.and_then(|((connected, status), connection)| {
			if !connected || status != HomeassistantStatus::Ready {
				Task::done(Ok(Message::UpdateAuthenticationStatus(connected, status)))
			} else {
				Task::future(async move {
					let entities = list_entities(Some(connection))
						.await
						.map_err(InitError::Entities)?;

					Ok(Message::UpdateEntitiesAndStatus(
						entities, connected, status,
					))
				})
			}
		})
		.then(|res| match res {
			Ok(message) => Task::done(message),
			Err(InitError::Status(err)) => {
				Task::done(Message::AuthenticationFailed(err.to_string()))
			}
			Err(InitError::Entities(err)) => {
				// TODO: Handle entity list error
				log::warn!("Failed to list Homeassistant entities: {err}");
				Task::done(Message::EntitiesFailed(err.to_string()))
			}
		});

		let url = if let Some(url) = &config.homeassistant.url {
			Task::done(Message::OnUrlChanged(url.to_string()))
		} else {
			Task::none()
		};

		Task::batch([status, url])
	}

	pub fn update(&mut self, config: &mut ConfigFile, message: Message) -> Task<Message> {
		match message {
			Message::UpdateAuthenticationStatus(authenticated, status) => {
				self.authenticated = authenticated;
				self.auth_enabled = true;
				self.status = status;
				Task::none()
			}
			Message::UpdateEntitiesAndStatus(entities, authenticated, status) => {
				self.entities = Some(entities);
				self.authenticated = authenticated;
				self.auth_enabled = true;
				self.status = status;
				Task::none()
			}
			Message::AuthenticationFailed(error) => {
				log::warn!("Failed to authenticate HA: {error}");
				self.authenticated = false;
				self.auth_enabled = true;
				self.status = HomeassistantStatus::Error;
				Task::none()
			}
			Message::EntitiesFailed(error) => {
				log::warn!("Failed to list HA entities: {error}");
				self.entities = None;
				Task::none()
			}
			Message::StartAuthFlow => {
				self.auth_enabled = false;
				Task::future(async {
					match run_authorization().await {
						Ok((entities, authenticated, status)) => {
							Message::UpdateEntitiesAndStatus(entities, authenticated, status)
						}
						Err(err) => Message::AuthenticationFailed(err.to_string()),
					}
				})
			}
			Message::ToggleEnabled(on) => {
				config.homeassistant.enabled = on;
				Task::done(Message::UpdateConfig)
			}
			Message::ToggleEntity(entity_id, tracked) => {
				if tracked {
					if !config.homeassistant.tracked_devices.contains(&entity_id) {
						config.homeassistant.tracked_devices.push(entity_id);
					}
				} else {
					config
						.homeassistant
						.tracked_devices
						.retain(|current| current != &entity_id);
				}
				Task::done(Message::UpdateConfig)
			}
			Message::OnUrlChanged(url) => {
				self.url = url.clone();
				config.homeassistant.url = if url.is_empty() {
					None
				} else {
					match url.parse() {
						Ok(url) => Some(url),
						Err(e) => {
							log::warn!("Invalid URL: {e}");
							return Task::none();
						}
					}
				};
				Task::done(Message::UpdateConfig)
			}
			Message::UpdateConfig => Task::none(),
		}
	}

	pub fn view<'a>(&'a self, config: &'a ConfigFile) -> Element<'a, Message> {
		let url_valid = self.url.is_empty() || config.homeassistant.url.is_some();

		let mut main = column![
			neo_card(
				row![
					container(Self::icon())
						.width(58)
						.height(58)
						.padding(12)
						.style(|_| container::Style {
							background: Some(iced::Background::Color(COLORS.white)),
							border: Border {
								color: COLORS.border,
								width: 2.0,
								radius: 3.into(),
							},
							..Default::default()
						}),
					column![
						text("HOMEASSISTANT")
							.width(Length::Fill)
							.color(COLORS.text)
							.size(34)
							.weight(font::Weight::Bold)
							.stretch(font::Stretch::ExtraExpanded)
							.wrapping(text::Wrapping::WordOrGlyph),
						text("Control real life")
							.width(Length::Fill)
							.color(COLORS.text.scale_alpha(0.76))
							.size(14)
							.weight(font::Weight::Bold),
					]
					.spacing(4),
					space::horizontal(),
					neo_toggle()
						.width(40)
						.height(18)
						.toggled(config.homeassistant.enabled)
						.on_toggled(Message::ToggleEnabled)
				]
				.align_y(Alignment::Center)
				.spacing(16)
			)
			.width(Length::Fill)
			.height(116)
			.background(Self::accent_color())
			.padding(18),
			grid![
				input_card(
					InputCardOptions::<_, &str, _, _>::new(&self.url, Message::OnUrlChanged)
						.background(COLORS.decorative.green50)
						.border(if url_valid {
							COLORS.border
						} else {
							COLORS.feedback.danger
						})
						.display("Homeassistant URL")
						.placeholder("http://homeassistant.local:8123")
				),
				neo_toggle_button(
					phosphor_icon!("user"),
					"Is Authenticated?",
					self.status.human_string(),
					self.is_authenticated(),
					Some(COLORS.decorative.green)
				)
				.enabled(self.auth_enabled)
				.padding(16)
				.width(Length::Fill)
				.height(112)
				.on_press(Message::StartAuthFlow),
			]
			.height(Length::Shrink)
			.columns(2)
			.spacing(14),
			// neo_card(grid(
			//
			// ).columns(2)).width(Length::Fill).padding(16)
		]
		.spacing(16);

		if let Some(entities) = &self.entities {
			main = main.push(view_entities(
				entities,
				&config.homeassistant.tracked_devices,
			));
		}

		main.into()
	}

	pub fn is_authenticated(&self) -> bool {
		self.authenticated
	}

	pub fn accent_color() -> Color {
		COLORS.decorative.blue
	}

	pub fn icon<'a>() -> Svg<'a> {
		svg(phosphor_icon!("house"))
	}
}

async fn homeassistant_status(
	connection: Option<zbus::Connection>,
) -> Result<(bool, HomeassistantStatus), Box<dyn std::error::Error + Send + Sync>> {
	// TODO: Share zbus session connection
	let connection = if let Some(connection) = connection {
		connection
	} else {
		zbus::Connection::session().await?
	};

	let proxy = HomeassistantProxy::new(&connection).await?;
	let (authenticated, status) = tokio::join!(proxy.authenticated(), proxy.status());

	let (authenticated, status) = (authenticated?, status?);

	Ok((authenticated, status))
}

async fn wait_for_catalog(
	connection: &zbus::Connection,
) -> Result<(bool, HomeassistantStatus), Box<dyn std::error::Error + Send + Sync>> {
	let initial = homeassistant_status(Some(connection.clone())).await?;
	if initial.1 != HomeassistantStatus::Connecting {
		return Ok(initial);
	}

	// The daemon allows 30 seconds for each of two subscriptions and the initial catalog fetch.
	match tokio::time::timeout(std::time::Duration::from_secs(95), async {
		loop {
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
			let status = homeassistant_status(Some(connection.clone())).await?;
			if status.1 != HomeassistantStatus::Connecting {
				return Ok(status);
			}
		}
	})
	.await
	{
		Ok(status) => status,
		Err(_) => Ok(initial),
	}
}

enum InitError {
	Status(Box<dyn std::error::Error + Send + Sync>),
	Entities(Box<dyn std::error::Error + Send + Sync>),
}

#[derive(Debug, Clone)]
pub struct HomeassistantEntity {
	info: HomeassistantEntitySummary,
}

async fn list_entities(
	connection: Option<zbus::Connection>,
) -> Result<Vec<HomeassistantEntity>, Box<dyn std::error::Error + Send + Sync>> {
	let connection = if let Some(connection) = connection {
		connection
	} else {
		zbus::Connection::session().await?
	};

	let object_manager = HomeassistantEntityProxy::builder(&connection)
		.path(HOMEASSISTANT_OBJECT_PATH)?
		.build()
		.await?;

	let objects =
		HomeassistantEntityObjectManagerProxyExt::get_managed_objects_typed(&object_manager)
			.await?;

	let entities = objects
		.into_iter()
		.filter_map(|(_, object)| object.homeassistant_entity)
		.map(|info| HomeassistantEntity { info })
		.collect::<Vec<_>>();

	log::debug!("Found {} Homeassistant entities", entities.len());

	Ok(entities)
}

// TODO: Add search & filtering
fn view_entities<'a>(
	entities: &'a [HomeassistantEntity], tracked_entities: &'a [String],
) -> Element<'a, Message> {
	neo_card(scrollable(
		grid(
			entities
				.iter()
				.filter(filter_entities)
				.map(|entity| view_entity(entity, tracked_entities))
				.flatten(),
		)
		.columns(2)
		.spacing(6)
		.height(Length::Shrink),
	))
	.width(Length::Fill)
	.height(Length::Shrink)
	.padding(16)
	.into()
}

fn filter_entities(entity: &&HomeassistantEntity) -> bool {
	entity.info.entity_category.as_deref() != Some("diagnostic")
}

fn view_entity<'a>(
	entity: &'a HomeassistantEntity, tracked_entities: &'a [String],
) -> impl Iterator<Item = Element<'a, Message>> + 'a {
	let name = if entity.info.friendly_name.is_empty() {
		&entity.info.entity_id
	} else {
		&entity.info.friendly_name
	};

	let tracked = tracked_entities.contains(&entity.info.entity_id);
	let entity_id = entity.info.entity_id.clone();
	[neo_toggle_button(
		phosphor_icon!("question-mark", "bold"),
		name,
		&entity.info.entity_id,
		tracked,
		None,
	)
	.width(Length::Fill)
	.on_press(Message::ToggleEntity(entity_id, !tracked))]
	.into_iter()
	.map(|e| e.into())
}

async fn run_authorization() -> Result<
	(Vec<HomeassistantEntity>, bool, HomeassistantStatus),
	Box<dyn std::error::Error + Send + Sync>,
> {
	let listener = CallbackListener::bind(HOMEASSISTANT_REDIRECT_URI).await?;
	let connection = zbus::Connection::session().await?;
	let proxy = HomeassistantProxy::new(&connection).await?;

	let url = proxy.begin_authorization().await?;
	if let Err(e) = tokio::task::spawn_blocking(move || open::that(url)).await? {
		let _ = proxy.cancel_authorization().await;
		return Err(e.into());
	};

	let callback = match tokio::time::timeout(OAUTH_CALLBACK_TIMEOUT, listener.receive()).await {
		Ok(Ok(callback)) => callback,
		Ok(Err(error)) => {
			let _ = proxy.cancel_authorization().await;
			return Err(error.into());
		}
		Err(_) => {
			let _ = proxy.cancel_authorization().await;
			return Err("Homeassistant authorization timed out...".into());
		}
	};
	let result = proxy
		.complete_authorization(&callback.code, &callback.state)
		.await;

	match result {
		Ok(true) => callback.success().await,
		Ok(false) => {
			callback.failure().await;
			let _ = proxy.cancel_authorization().await;
			return Err("Homeassistant did not authenticate user".into());
		}
		Err(err) => {
			callback.failure().await;
			let _ = proxy.cancel_authorization().await;
			return Err(err.into());
		}
	}

	let (authenticated, status) = homeassistant_status(Some(connection.clone())).await?;
	let entities = list_entities(Some(connection)).await?;
	Ok((entities, authenticated, status))
}
