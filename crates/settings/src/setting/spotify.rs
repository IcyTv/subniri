use std::time::Duration;

use config::ConfigFile;
use daemon_common::SpotifyProxy;
use iced::{
	Alignment, Border, Color, Element, Length, Task, font,
	widget::{Svg, column, container, grid, row, space, svg, text, text_input},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{neo_card, neo_toggle, neo_toggle_button},
};
#[derive(Clone, Debug)]
pub enum Message {
	UpdateAuthenticationStatus(bool, String),
	AuthenticationFailed(String),

	StartAuthFlow,

	OnClientIdChanged(String),
	ToggleEnabled(bool),

	UpdateConfig,
}

pub struct Spotify {
	authenticated: bool,
	status: String,
	auth_enabled: bool,
}

impl Spotify {
	pub fn new() -> Self {
		Self {
			authenticated: false,
			status: "Checking daemon".to_string(),
			auth_enabled: true,
		}
	}

	pub fn init(_config: &ConfigFile) -> Task<Message> {
		Task::future(async {
			match spotify_status().await {
				Ok((authenticated, status)) => {
					Message::UpdateAuthenticationStatus(authenticated, status)
				}
				Err(error) => Message::AuthenticationFailed(error),
			}
		})
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
			Message::ToggleEnabled(on) => {
				config.spotify.enabled = on;
				Task::done(Message::UpdateConfig)
			}
			Message::UpdateConfig => Task::none(),
		}
	}

	pub fn view<'a>(&'a self, config: &'a ConfigFile) -> Element<'a, Message> {
		let client_id_valid = Self::credentials_are_valid(&config.spotify.client_id);
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
					InputCardOptions::<&str, _>::new(
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
				.on_press(Message::StartAuthFlow)
			]
			.height(Length::Shrink)
			.columns(2)
			.spacing(14)
		]
		.spacing(16)
		.into()
	}

	#[inline(always)]
	pub const fn accent_color() -> Color {
		COLORS.decorative.green
	}

	#[inline(always)]
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
	let status = proxy.status().await.map_err(|error| error.to_string())?;
	Ok((authenticated, status))
}

async fn run_authorization() -> Result<(bool, String), String> {
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

	// Give the daemon's callback listener a chance to bind before opening the browser.
	tokio::time::sleep(Duration::from_millis(100)).await;
	tokio::task::spawn_blocking(move || open::that(url))
		.await
		.map_err(|error| error.to_string())?
		.map_err(|error| error.to_string())?;

	for _ in 0..300 {
		tokio::time::sleep(Duration::from_secs(1)).await;
		let authenticated = proxy
			.authenticated()
			.await
			.map_err(|error| error.to_string())?;
		let status = proxy.status().await.map_err(|error| error.to_string())?;
		if authenticated || status != "Authorizing" {
			return Ok((authenticated, status));
		}
	}

	Err("Spotify authorization timed out".to_string())
}

#[derive(Debug)]
struct InputCardOptions<'a, D: text::IntoFragment<'a>, F: Fn(String) -> Message + 'a> {
	background: Color,
	border: Color,
	display: D,
	value: &'a str,
	placeholder: &'a str,
	on_input: F,
}

impl<'a, D: text::IntoFragment<'a>, F: Fn(String) -> Message + 'a> InputCardOptions<'a, D, F> {
	fn new(value: &'a str, on_input: F) -> InputCardOptions<'a, &'a str, F> {
		InputCardOptions {
			background: COLORS.white,
			border: COLORS.border,
			display: "Input",
			value,
			placeholder: "Enter value",
			on_input,
		}
	}

	fn background(mut self, color: Color) -> Self {
		self.background = color;
		self
	}

	fn border(mut self, color: Color) -> Self {
		self.border = color;
		self
	}

	fn display(mut self, display: D) -> Self {
		self.display = display;
		self
	}

	fn placeholder(mut self, placeholder: &'a str) -> Self {
		self.placeholder = placeholder;
		self
	}
}

fn input_card<'a>(
	options: InputCardOptions<'a, impl text::IntoFragment<'a>, impl Fn(String) -> Message + 'a>,
) -> Element<'a, Message> {
	neo_card(
		column![
			row![
				text(options.display)
					.width(Length::Fill)
					.color(COLORS.text)
					.size(18)
					.weight(font::Weight::Bold)
			]
			.spacing(8),
			text_input(options.placeholder, options.value)
				.on_input(options.on_input)
				.style(move |_, _status| {
					text_input::Style {
						border: Border {
							width: 2.0,
							color: options.border,
							radius: 3.into(),
							..Default::default()
						},
						background: COLORS.white.into(),
						placeholder: COLORS.text.scale_alpha(0.76),
						value: COLORS.text,
						selection: options.background,
					}
				})
				.padding(6)
		]
		.spacing(12),
	)
	.width(Length::Fill)
	.height(112)
	.padding(16)
	.background(options.background)
	.into()
}
