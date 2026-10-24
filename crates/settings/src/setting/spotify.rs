use config::ConfigFile;
use daemon_common::{SPOTIFY_REDIRECT_URL, SpotifyProxy};
use iced::{
	Alignment, Border, Color, Element, Length, Task, font,
	widget::{Svg, column, container, grid, row, space, svg, text},
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
	UpdateAuthenticationStatus(bool, String),
	AuthenticationFailed(String),

	StartAuthFlow,

	OnClientIdChanged(String),
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
}

impl Spotify {
	pub fn new() -> Self {
		Self {
			authenticated: false,
			status: "Checking daemon".to_string(),
			auth_enabled: true,
			poll_interval: "15".to_string(),
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
				self.status = "Starting authorization".to_string();
				Task::future(async {
					match run_authorization().await {
						Ok((authenticated, status)) => {
							Message::UpdateAuthenticationStatus(authenticated, status)
						}
						Err(error) => Message::AuthenticationFailed(error),
					}
				})
			}
			Message::OnClientIdChanged(client_id) => {
				config.spotify.client_id.clone_from(&client_id);
				Task::done(Message::UpdateConfig)
			}
			Message::PollIntervalChanged(interval) => {
				self.poll_interval.clone_from(&interval);
				let Ok(interval) = interval.parse::<u64>() else {
					return Task::none();
				};
				if !(5..=3600).contains(&interval) {
					return Task::none();
				}
				config.spotify.poll_interval_seconds = interval;
				Task::done(Message::UpdateConfig)
			}
			Message::SyncPollInterval(interval) => {
				self.poll_interval = interval.to_string();
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
		let client_id_valid = Self::credentials_are_valid(&config.spotify.client_id);
		let poll_interval = self.poll_interval.parse::<u64>().ok();
		let poll_interval_valid =
			poll_interval.is_some_and(|seconds| (5..=3600).contains(&seconds));
		let poll_interval_warning = poll_interval.is_some_and(|seconds| (5..15).contains(&seconds));
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
				input_card(
					InputCardOptions::<_, &str, _, _>::new(
						&config.spotify.client_id,
						Message::OnClientIdChanged
					)
					.background(COLORS.decorative.green50)
					.border(if client_id_valid {
						COLORS.border
					} else {
						COLORS.feedback.danger
					})
					.display("CLIENT ID")
					.placeholder("https://developer.spotify.com/dashboard")
				),
				neo_toggle_button(
					phosphor_icon!("user"),
					"Is Authenticated?",
					&self.status,
					self.is_authenticated(),
					Some(COLORS.decorative.green)
				)
				.enabled(self.auth_enabled)
				.padding(16)
				.width(Length::Fill)
				.height(112)
				.on_press(Message::StartAuthFlow),
				poll_interval_card(&self.poll_interval, poll_interval_valid),
			]
			.height(Length::Shrink)
			.columns(2)
			.spacing(14),
			poll_interval_note(poll_interval_valid, poll_interval_warning)
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

	fn credentials_are_valid(client_id: &str) -> bool {
		let plausible_regex = lazy_regex::lazy_regex!(r"^[0-9a-fA-F]{32}$");
		plausible_regex.is_match(client_id)
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
			.display("POLL INTERVAL (SECONDS)")
			.placeholder("15"),
	)
}

fn poll_interval_note(valid: bool, warning: bool) -> Element<'static, Message> {
	text(if warning {
		"Polling faster than 15 seconds increases Spotify API usage and may trigger rate limits."
	} else if valid {
		"Spotify checks all observed tracks in one batched request per interval."
	} else {
		"Polling interval must be between 5 and 3600 seconds."
	})
	.color(if warning || !valid {
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

async fn run_authorization() -> Result<(bool, String), String> {
	let listener = CallbackListener::bind(SPOTIFY_REDIRECT_URL)
		.await
		.map_err(|error| error.to_string())?;
	let connection = zbus::Connection::session()
		.await
		.map_err(|error| error.to_string())?;
	let proxy = SpotifyProxy::new(&connection)
		.await
		.map_err(|error| error.to_string())?;
	let url = proxy
		.begin_authorization()
		.await
		.map_err(|error| error.to_string())?;
	let open_result = tokio::task::spawn_blocking(move || open::that(url))
		.await
		.map_err(|error| error.to_string())?
		.map_err(|error| error.to_string());
	if let Err(error) = open_result {
		let _ = proxy.cancel_authorization().await;
		return Err(error);
	}

	let callback = match tokio::time::timeout(OAUTH_CALLBACK_TIMEOUT, listener.receive()).await {
		Ok(Ok(callback)) => callback,
		Ok(Err(error)) => {
			let _ = proxy.cancel_authorization().await;
			return Err(error.to_string());
		}
		Err(_) => {
			let _ = proxy.cancel_authorization().await;
			return Err("Spotify authorization timed out".to_string());
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
			return Err("Spotify did not authenticate the user".to_string());
		}
		Err(error) => {
			callback.failure().await;
			let _ = proxy.cancel_authorization().await;
			return Err(error.to_string());
		}
	}
	spotify_status().await
}
