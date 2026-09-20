use std::{fmt, path::PathBuf, sync::Arc};

use config::ConfigFile;
use iced::{
	Alignment, Border, Color, Element, Length, Task, font,
	widget::{Svg, column, container, grid, row, space, svg, text, text_input},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{neo_card, neo_toggle, neo_toggle_button},
};
use rspotify::{
	AuthCodePkceSpotify, Credentials, OAuth, Token, TokenCallback,
	prelude::{BaseClient, OAuthClient},
	scopes,
};
use secret_store::{SecretAttributes, SecretStore};

#[derive(Clone)]
pub enum Message {
	UpdateSecretsStore(Arc<SecretStore>),
	UpdateSpotifyClient(Arc<AuthCodePkceSpotify>, String),
	UpdateAuthenticationStatus(bool),

	StartAuthFlow,

	OnClientIdChanged(String),
	ToggleEnabled(bool),
	ToggleAuthEnabled(bool),

	Noop,
	UpdateConfig,
}

impl fmt::Debug for Message {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::UpdateSecretsStore(_) => write!(f, "Message::UpdateSecretsStore"),
			Self::UpdateSpotifyClient(..) => write!(f, "Message::UpdateSpotifyClient"),
			Self::UpdateAuthenticationStatus(authenticated) => {
				write!(f, "Message::UpdateAuthenticationStatus({authenticated})")
			}
			Self::StartAuthFlow => write!(f, "Message::StartAuthFlow"),
			Self::OnClientIdChanged(_) => write!(f, "Message::OnClientIdChanged(******)"),
			Self::ToggleEnabled(on) => write!(f, "Message::ToggleEnabled({on})"),
			Self::ToggleAuthEnabled(on) => write!(f, "Message::ToggleAuthEnabled({on})"),
			Self::Noop => write!(f, "Message::Noop"),
			Self::UpdateConfig => write!(f, "Message::UpdateConfig"),
		}
	}
}

pub struct Spotify {
	secrets: Option<Arc<SecretStore>>,
	spotify_client: Option<Arc<AuthCodePkceSpotify>>,
	authorize_url: Option<String>,
	authenticated: bool,

	auth_enabled: bool,
}

impl Spotify {
	pub fn new() -> Self {
		Self {
			secrets: None,
			spotify_client: None,
			authorize_url: None,
			authenticated: false,
			auth_enabled: true,
		}
	}

	pub fn init(config: &ConfigFile) -> Task<Message> {
		let secret_store = Task::future(async {
			let store = match SecretStore::connect().await {
				Ok(store) => store,
				Err(e) => {
					log::error!("Failed to connect to SecretStore: {e}");
					return Message::Noop;
				}
			};

			Message::UpdateSecretsStore(Arc::new(store))
		});

		let client_id = config.spotify.client_id.clone();
		secret_store.then(move |message| {
			let Message::UpdateSecretsStore(store) = message else {
				return Task::none();
			};

			let store2 = store.clone();
			let client_id = client_id.clone();
			let client_id_task = if client_id.is_empty() {
				Task::none()
			} else {
				init_client_task(client_id, store2)
			};

			Task::batch([
				Task::done(Message::UpdateSecretsStore(store)),
				client_id_task,
			])
		})
	}

	pub fn update(&mut self, config: &mut ConfigFile, message: Message) -> Task<Message> {
		match message {
			Message::UpdateSecretsStore(store) => {
				self.secrets = Some(store);
				Task::none()
			}
			Message::UpdateSpotifyClient(client, url) => {
				log::info!("Spotify client updated");
				self.spotify_client = Some(client.clone());
				self.authorize_url = Some(url);
				self.authenticated = false;

				Task::future(async move {
					let authenticated = match client.current_user().await {
						Ok(_) => true,
						Err(error) => {
							log::warn!("Spotify authentication check failed: {error}");
							false
						}
					};

					Message::UpdateAuthenticationStatus(authenticated)
				})
			}
			Message::UpdateAuthenticationStatus(authenticated) => {
				self.authenticated = authenticated;
				Task::none()
			}
			Message::StartAuthFlow => {
				if let Some((client, url)) =
					self.spotify_client.clone().zip(self.authorize_url.clone())
				{
					let authflow = Task::future(async move {
						let c2 = client.clone();
						let authcode = tokio::task::spawn_blocking(move || {
							c2.get_authcode_listener("127.0.0.1:8888".parse().unwrap())
						});

						if let Err(e) = tokio::task::spawn_blocking(move || {
							if let Err(e) = open::that(url) {
								log::error!("Failed to open browser for Spotify auth flow: {e}");
							}
						})
						.await
						{
							log::warn!("Failed to spawn blocking task for opening browser: {e}");
						};

						let authcode = match authcode.await {
							Ok(Ok(authcode)) => authcode,
							Ok(Err(e)) => {
								log::error!("Failed to get Spotify auth code: {e}");
								return Message::Noop;
							}
							Err(e) => {
								log::error!("Failed to await Spotify auth code task: {e}");
								return Message::Noop;
							}
						};

						if let Err(e) = client.request_token(&authcode).await {
							log::error!("Failed to request Spotify token: {e}");
						}

						// Verify authentication status
						let authenticated = match client.current_user().await {
							Ok(_) => true,
							Err(error) => {
								log::warn!("Spotify authentication check failed: {error}");
								false
							}
						};

						log::info!("Spotify authentication status: {authenticated}");

						Message::UpdateAuthenticationStatus(authenticated)
					});

					Task::done(Message::ToggleAuthEnabled(false))
						.chain(authflow)
						.chain(Task::done(Message::ToggleAuthEnabled(true)))
				} else {
					log::warn!("Spotify is not setup for auth flow");
					Task::none()
				}
			}
			Message::OnClientIdChanged(client_id) => {
				config.spotify.client_id.clone_from(&client_id);
				let save = Task::done(Message::UpdateConfig);
				if Self::credentials_are_valid(&client_id)
					&& let Some(store) = self.secrets.clone()
				{
					Task::batch([save, init_client_task(client_id, store)])
				} else {
					save
				}
			}
			Message::ToggleEnabled(on) => {
				config.spotify.enabled = on;
				Task::done(Message::UpdateConfig)
			}
			Message::ToggleAuthEnabled(on) => {
				self.auth_enabled = on;
				Task::none()
			}
			Message::UpdateConfig | Message::Noop => Task::none(),
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
					"Not connected",
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
		self.spotify_client.is_some() && self.authenticated
	}

	fn credentials_are_valid(client_id: &str) -> bool {
		let plausible_regex = lazy_regex::lazy_regex!(r"^[0-9a-fA-F]{32}$");
		plausible_regex.is_match(client_id)
	}
}

fn init_client_task(client_id: String, store: Arc<SecretStore>) -> Task<Message> {
	Task::future(async move {
		match try_init_client(client_id, store).await {
			Ok((client, url)) => {
				log::info!("Spotify client initialized successfully");
				Message::UpdateSpotifyClient(Arc::new(client), url)
			}
			Err(error) => {
				log::error!("Failed to initialize Spotify client: {error}");
				Message::Noop
			}
		}
	})
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

async fn try_init_client(
	client_id: String, secret_store: Arc<SecretStore>,
) -> Result<(AuthCodePkceSpotify, String), Box<dyn std::error::Error>> {
	if client_id.is_empty() {
		log::warn!("Spotify client ID is empty, skipping initialization");
		return Err("Spotify client ID is empty".into());
	}

	let creds = Credentials::new_pkce(&client_id);

	let oauth = OAuth {
		redirect_uri: "http://127.0.0.1:8888/callback".to_string(),
		scopes: scopes!("user-read-currently-playing user-library-read user-library-modify"),
		..Default::default()
	};

	let store = secret_store.clone();
	let callback = TokenCallback(Box::new(move |token| {
		if token.is_expired() {
			log::warn!("Spotify token is expired, skipping storage");
			return Ok(());
		}

		let secret_store = store.clone();

		std::thread::spawn(move || {
			tokio::runtime::Builder::new_current_thread()
				.enable_all()
				.build()
				.unwrap()
				.block_on(async move {
					let expires_in = token.expires_in;
					let expiry = token
						.expires_at
						.map(|dt| dt.to_rfc3339())
						.unwrap_or_else(|| {
							let now = chrono::Utc::now();
							let expiry = now + expires_in;
							expiry.to_rfc3339()
						});

					let attrs = SecretAttributes::new()
						.insert("application", "subniri")
						.insert("service", "spotify");

					if let Err(e) = secret_store
						.store(
							"Spotify Access Token",
							&attrs.clone().insert("type", "access_token"),
							token.access_token,
						)
						.await
					{
						log::error!("Failed to store Spotify token: {e}");
					}

					if let Err(e) = secret_store
						.store(
							"Spotify Access Token Expiry",
							&attrs.clone().insert("type", "access_token_expiry"),
							expiry,
						)
						.await
					{
						log::error!("Failed to store Spotify token expiry: {e}");
					}

					if let Some(refresh_token) = token.refresh_token {
						if let Err(e) = secret_store
							.store(
								"Spotify Refresh Token",
								&attrs.insert("type", "refresh_token"),
								refresh_token,
							)
							.await
						{
							log::error!("Failed to store Spotify refresh token: {e}");
						}
					}
				});
		});

		Ok(())
	}));

	let config = rspotify::Config {
		token_cached: false,
		token_refreshing: true,
		// TODO
		token_callback_fn: Arc::new(Some(callback)),
		cache_path: PathBuf::from("/dev/null/invalid"),
		..Default::default()
	};

	// Try to read the token from the store
	let attrs = SecretAttributes::new()
		.insert("application", "subniri")
		.insert("service", "spotify");
	let access_token_attrs = attrs.clone().insert("type", "access_token");
	let access_token_expiry_attrs = attrs.clone().insert("type", "access_token_expiry");
	let refresh_token_attrs = attrs.clone().insert("type", "refresh_token");

	let access_token = secret_store.get(&access_token_attrs).await.ok().flatten();
	let access_token_expiry = secret_store
		.get(&access_token_expiry_attrs)
		.await
		.ok()
		.flatten();
	let refresh_token = secret_store.get(&refresh_token_attrs).await.ok().flatten();

	let mut spotify = if let Some((access_token, expiry)) = access_token.zip(access_token_expiry) {
		let expiry = chrono::DateTime::parse_from_rfc3339(&expiry)
			.map(|dt| dt.with_timezone(&chrono::Utc))
			.unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::seconds(3600));
		let expires_in = expiry - chrono::Utc::now();

		let token = Token {
			access_token,
			expires_in,
			expires_at: Some(expiry),
			refresh_token,
			scopes: oauth.scopes.clone(),
		};

		AuthCodePkceSpotify::from_token_with_config(token, creds, oauth, config)
	} else {
		AuthCodePkceSpotify::with_config(creds, oauth, config)
	};

	if let Err(e) = spotify.refresh_token().await {
		log::warn!("Failed to refresh Spotify token: {e}");
	}

	let url = spotify.get_authorize_url(None)?;

	Ok((spotify, url))
}
