use config::ConfigFile;
use daemon_common::{SpotifyPairing, SpotifyProxy, SpotifyStatus};
use iced::{
	Alignment, Border, Color, Element, Length, Task, font,
	widget::{Svg, button, column, container, grid, row, space, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{neo_card, neo_toggle, neo_toggle_button},
};

use crate::setting::{InputCardOptions, input_card};

#[derive(Clone, Debug)]
pub enum Message {
	UpdateAuthenticationStatus(bool, String),
	AuthenticationFailed(String),

	StartAuthFlow,

	PairingStarted(u64, Result<(SpotifyPairing, zbus::Connection), String>),
	PairingFinished(u64, Result<(bool, String), String>),
	CancelPairing,
	OpenPairing,
	PollIntervalChanged(String),
	SyncPollInterval(u64),
	ToggleEnabled(bool),

	UpdateConfig,
}

pub struct Spotify {
	authenticated: bool,
	status: String,
	auth_enabled: bool,
	poll_interval: String,
	pairing: Option<SpotifyPairing>,
	generation: u64,
	pairing_connection: Option<zbus::Connection>,
}

impl Spotify {
	pub fn new() -> Self {
		Self {
			authenticated: false,
			status: "Checking daemon".to_string(),
			auth_enabled: true,
			poll_interval: "60".to_string(),
			pairing: None,
			generation: 0,
			pairing_connection: None,
		}
	}

	pub fn init(config: &ConfigFile) -> Task<Message> {
		Task::batch([
			Task::done(Message::SyncPollInterval(
				config.spotify.poll_interval_seconds,
			)),
			Task::future(async {
				match spotify_status().await {
					Ok((authenticated, status)) => {
						Message::UpdateAuthenticationStatus(authenticated, status)
					}
					Err(error) => Message::AuthenticationFailed(error),
				}
			}),
		])
	}

	pub fn update(&mut self, config: &mut ConfigFile, message: Message) -> Task<Message> {
		match message {
			Message::UpdateAuthenticationStatus(authenticated, status) => {
				self.authenticated = authenticated;
				self.status = status;
				self.auth_enabled = true;
				Task::none()
			}
			Message::AuthenticationFailed(error) => {
				log::warn!("Spotify authentication flow failed: {error}");
				self.authenticated = false;
				self.status = format!("Error: {error}");
				self.auth_enabled = true;
				Task::none()
			}
			Message::StartAuthFlow => {
				self.auth_enabled = false;
				self.status = "Starting Spotify pairing".to_string();
				self.generation = self.generation.wrapping_add(1);
				let generation = self.generation;
				Task::future(
					async move { Message::PairingStarted(generation, begin_pairing().await) },
				)
			}
			Message::PairingStarted(generation, result) => {
				if generation != self.generation {
					return Task::none();
				}
				match result {
					Ok((pairing, connection)) => {
						let expires_at = pairing.expires_at;
						self.pairing = Some(pairing);
						self.pairing_connection = Some(connection.clone());
						self.status = "Waiting for approval at spotify.com/pair".into();
						Task::future(async move {
							Message::PairingFinished(
								generation,
								wait_for_pairing(connection, expires_at).await,
							)
						})
					}
					Err(error) => self.update(config, Message::AuthenticationFailed(error)),
				}
			}
			Message::PairingFinished(generation, result) => {
				if generation != self.generation {
					return Task::none();
				}
				self.pairing = None;
				self.pairing_connection = None;
				match result {
					Ok((authenticated, status)) => self.update(
						config,
						Message::UpdateAuthenticationStatus(authenticated, status),
					),
					Err(error) => self.update(config, Message::AuthenticationFailed(error)),
				}
			}
			Message::CancelPairing => {
				self.generation = self.generation.wrapping_add(1);
				self.pairing = None;
				self.auth_enabled = true;
				let connection = self.pairing_connection.take();
				Task::future(async move {
					let result = async {
						let Some(connection) = connection else {
							return spotify_status().await;
						};
						SpotifyProxy::new(&connection)
							.await
							.map_err(|error| error.to_string())?
							.cancel_pairing()
							.await
							.map_err(|error| error.to_string())?;
						spotify_status().await
					}
					.await;
					match result {
						Ok((a, s)) => Message::UpdateAuthenticationStatus(a, s),
						Err(error) => Message::AuthenticationFailed(error),
					}
				})
			}
			Message::OpenPairing => Task::future(async {
				let _ =
					tokio::task::spawn_blocking(|| open::that("https://spotify.com/pair")).await;
				Message::UpdateConfig
			}),
			Message::PollIntervalChanged(interval) => {
				self.poll_interval.clone_from(&interval);
				let Ok(interval) = interval.parse::<u64>() else {
					return Task::none();
				};
				if !(60..=3600).contains(&interval) {
					return Task::none();
				}
				config.spotify.poll_interval_seconds = interval;
				Task::done(Message::UpdateConfig)
			}
			Message::SyncPollInterval(interval) => {
				self.poll_interval = interval.max(60).to_string();
				Task::none()
			}
			Message::ToggleEnabled(on) => {
				config.spotify.enabled = on;
				Task::done(Message::UpdateConfig)
			}
			Message::UpdateConfig => Task::none(),
		}
	}

	pub fn view<'a>(&'a self, config: &'a ConfigFile) -> Element<'a, Message> {
		let poll_interval = self.poll_interval.parse::<u64>().ok();
		let poll_interval_valid =
			poll_interval.is_some_and(|seconds| (60..=3600).contains(&seconds));
		column![
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
						text("SPOTIFY")
							.width(Length::Fill)
							.color(COLORS.text)
							.size(34)
							.weight(font::Weight::Bold)
							.stretch(font::Stretch::ExtraExpanded)
							.wrapping(text::Wrapping::WordOrGlyph),
						text("Feel your music")
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
						.toggled(config.spotify.enabled)
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
				neo_toggle_button(
					phosphor_icon!("user"),
					"Pair Spotify",
					&self.status,
					self.is_authenticated(),
					Some(COLORS.decorative.green)
				)
				.enabled(self.auth_enabled && config.spotify.enabled)
				.padding(16)
				.width(Length::Fill)
				.height(112)
				.on_press(Message::StartAuthFlow),
				poll_interval_card(&self.poll_interval, poll_interval_valid),
			]
			.height(Length::Shrink)
			.columns(2)
			.spacing(14),
			poll_interval_note(poll_interval_valid),
			self.pairing_view(),
		]
		.spacing(16)
		.into()
	}

	pub const fn accent_color() -> Color {
		COLORS.decorative.green
	}

	pub fn icon<'a>() -> Svg<'a> {
		svg(phosphor_icon!("spotify-logo"))
	}

	fn is_authenticated(&self) -> bool {
		self.authenticated
	}

	fn pairing_view(&self) -> Element<'_, Message> {
		if let Some(pairing) = &self.pairing {
			neo_card(
				column![
					text("Enter this code at spotify.com/pair"),
					text(&pairing.user_code).size(32),
					row![
						button("Open Spotify").on_press(Message::OpenPairing),
						button("Cancel").on_press(Message::CancelPairing)
					]
					.spacing(12)
				]
				.spacing(12),
			)
			.padding(16)
			.into()
		} else {
			text(
				"Pair once; credentials refresh automatically. No developer account or client ID needed.",
			)
			.into()
		}
	}
}

fn poll_interval_card(value: &str, valid: bool) -> Element<'_, Message> {
	input_card(
		InputCardOptions::<_, &str, _, _>::new(value, Message::PollIntervalChanged)
			.background(COLORS.decorative.yellow90)
			.border(if valid {
				COLORS.border
			} else {
				COLORS.feedback.danger
			})
			.display("RECOVERY REFRESH (SECONDS)")
			.placeholder("60"),
	)
}

fn poll_interval_note(valid: bool) -> Element<'static, Message> {
	text(if valid {
		"Saved tracks update live. This interval is used only while reconnecting; Spotify cooldowns take precedence."
	} else {
		"Recovery interval must be between 60 and 3600 seconds."
	})
	.color(if !valid {
		COLORS.feedback.danger
	} else {
		COLORS.text.scale_alpha(0.76)
	})
	.size(13)
	.into()
}

async fn spotify_status() -> Result<(bool, String), String> {
	let connection = zbus::Connection::session()
		.await
		.map_err(|error| error.to_string())?;
	let proxy = SpotifyProxy::new(&connection)
		.await
		.map_err(|error| error.to_string())?;
	let authenticated = proxy
		.authenticated()
		.await
		.map_err(|error| error.to_string())?;
	let status = proxy
		.status()
		.await
		.map_err(|error| error.to_string())?
		.to_string();
	Ok((authenticated, status))
}

async fn begin_pairing() -> Result<(SpotifyPairing, zbus::Connection), String> {
	let connection = zbus::Connection::session()
		.await
		.map_err(|error| error.to_string())?;
	let proxy = SpotifyProxy::new(&connection)
		.await
		.map_err(|error| error.to_string())?;
	let pairing = proxy
		.begin_pairing()
		.await
		.map_err(|error| error.to_string())?;
	Ok((pairing, connection))
}

async fn wait_for_pairing(
	connection: zbus::Connection, expires_at: u64,
) -> Result<(bool, String), String> {
	loop {
		// Local D-Bus status polling only: the daemon alone polls the OAuth endpoint.
		tokio::time::sleep(std::time::Duration::from_secs(1)).await;
		let proxy = SpotifyProxy::new(&connection)
			.await
			.map_err(|error| error.to_string())?;
		let authenticated = proxy
			.authenticated()
			.await
			.map_err(|error| error.to_string())?;
		let status = proxy.status().await.map_err(|error| error.to_string())?;
		if authenticated {
			return Ok((true, status.to_string()));
		}
		if matches!(
			status,
			SpotifyStatus::AuthenticationRequired | SpotifyStatus::Error | SpotifyStatus::Disabled
		) {
			return Err(status.to_string());
		}
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.map_err(|error| error.to_string())?
			.as_secs();
		if now >= expires_at {
			return Err("Spotify pairing expired; try again".into());
		}
	}
}
