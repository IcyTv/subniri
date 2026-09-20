use std::{path::PathBuf, sync::Arc};

use daemon_common::{SPOTIFY_INTERFACE, SPOTIFY_OBJECT_PATH};
use rspotify::{
	AuthCodePkceSpotify, Credentials, OAuth, Token, TokenCallback,
	model::{LibraryId, TrackId},
	prelude::OAuthClient,
	scopes,
};
use secret_store::{SecretAttributes, SecretStore};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::TcpListener,
	sync::{Mutex, RwLock, watch},
	task::JoinHandle,
};

type Error = Box<dyn std::error::Error + Send + Sync>;

const REDIRECT_URI: &str = "http://127.0.0.1:8888/callback";
const CALLBACK_ADDRESS: &str = "127.0.0.1:8888";

#[derive(Clone)]
pub struct SpotifyDbus {
	inner: Arc<SpotifyInner>,
}

struct SpotifyInner {
	store: Arc<SecretStore>,
	state: RwLock<SpotifyState>,
	authorization_task: Mutex<Option<JoinHandle<()>>>,
}

struct SpotifyState {
	client: Option<Arc<AuthCodePkceSpotify>>,
	authorization_url: Option<String>,
	status: String,
	authenticated: bool,
	generation: u64,
}

impl SpotifyDbus {
	fn new(store: SecretStore) -> Self {
		Self {
			inner: Arc::new(SpotifyInner {
				store: Arc::new(store),
				state: RwLock::new(SpotifyState {
					client: None,
					authorization_url: None,
					status: "Unconfigured".to_string(),
					authenticated: false,
					generation: 0,
				}),
				authorization_task: Mutex::new(None),
			}),
		}
	}

	async fn reconfigure(&self, config: config::Spotify) {
		if let Some(task) = self.inner.authorization_task.lock().await.take() {
			task.abort();
		}

		let generation = {
			let mut state = self.inner.state.write().await;
			state.generation = state.generation.wrapping_add(1);
			state.client = None;
			state.authorization_url = None;
			state.authenticated = false;
			state.status = if config.enabled {
				"Connecting".to_string()
			} else {
				"Disabled".to_string()
			};
			state.generation
		};

		if !config.enabled {
			return;
		}
		if !valid_client_id(&config.client_id) {
			self.set_connection_state(generation, None, None, "Unconfigured", false)
				.await;
			return;
		}

		match initialize_client(config.client_id, self.inner.store.clone()).await {
			Ok((client, authorization_url, has_token)) => {
				let client = Arc::new(client);
				let authenticated = has_token && client.current_user().await.is_ok();
				let status = if authenticated {
					"Ready"
				} else {
					"Authentication required"
				};
				self.set_connection_state(
					generation,
					Some(client),
					Some(authorization_url),
					status,
					authenticated,
				)
				.await;
			}
			Err(error) => {
				log::error!("Failed to initialize Spotify: {error}");
				self.set_connection_state(
					generation,
					None,
					None,
					&format!("Error: {error}"),
					false,
				)
				.await;
			}
		}
	}

	async fn set_connection_state(
		&self, generation: u64, client: Option<Arc<AuthCodePkceSpotify>>,
		authorization_url: Option<String>, status: &str, authenticated: bool,
	) {
		let mut state = self.inner.state.write().await;
		if state.generation != generation {
			return;
		}
		state.client = client;
		state.authorization_url = authorization_url;
		state.status = status.to_string();
		state.authenticated = authenticated;
	}

	async fn client(&self) -> zbus::fdo::Result<Arc<AuthCodePkceSpotify>> {
		self.inner
			.state
			.read()
			.await
			.client
			.clone()
			.ok_or_else(|| zbus::fdo::Error::Failed("Spotify is not configured".to_string()))
	}

	async fn finish_authorization(
		self, client: Arc<AuthCodePkceSpotify>, generation: u64, listener: TcpListener,
	) {
		let result = match receive_authorization_code(listener, &client).await {
			Ok(code) => match client.request_token(&code).await {
				Ok(()) => client
					.current_user()
					.await
					.map(|_| ())
					.map_err(|error| error.to_string()),
				Err(error) => Err(error.to_string()),
			},
			Err(error) => Err(error),
		};

		let mut state = self.inner.state.write().await;
		if state.generation != generation {
			return;
		}
		match result {
			Ok(()) => {
				state.status = "Ready".to_string();
				state.authenticated = true;
			}
			Err(error) => {
				log::warn!("Spotify authorization failed: {error}");
				state.status = format!("Error: {error}");
				state.authenticated = false;
			}
		}
	}

	async fn shutdown(&self) {
		if let Some(task) = self.inner.authorization_task.lock().await.take() {
			task.abort();
			let _ = task.await;
		}
	}
}

#[zbus::interface(name = SPOTIFY_INTERFACE)]
impl SpotifyDbus {
	#[zbus(property, name = "Status")]
	async fn status(&self) -> String {
		self.inner.state.read().await.status.clone()
	}

	#[zbus(property, name = "Authenticated")]
	async fn authenticated(&self) -> bool {
		self.inner.state.read().await.authenticated
	}

	async fn begin_authorization(&self) -> zbus::fdo::Result<String> {
		let mut task_slot = self.inner.authorization_task.lock().await;
		if task_slot.as_ref().is_some_and(|task| !task.is_finished()) {
			return Err(zbus::fdo::Error::Failed(
				"Spotify authorization is already in progress".to_string(),
			));
		}
		let listener = TcpListener::bind(CALLBACK_ADDRESS).await.map_err(|error| {
			zbus::fdo::Error::Failed(format!("Failed to listen for Spotify callback: {error}"))
		})?;

		let (client, authorization_url, generation) = {
			let mut state = self.inner.state.write().await;
			let client = state
				.client
				.clone()
				.ok_or_else(|| zbus::fdo::Error::Failed("Spotify is not configured".to_string()))?;
			let authorization_url = state.authorization_url.clone().ok_or_else(|| {
				zbus::fdo::Error::Failed("Spotify authorization is unavailable".to_string())
			})?;
			state.status = "Authorizing".to_string();
			state.authenticated = false;
			(client, authorization_url, state.generation)
		};

		let service = self.clone();
		*task_slot = Some(tokio::spawn(async move {
			service
				.finish_authorization(client, generation, listener)
				.await;
		}));

		Ok(authorization_url)
	}

	async fn track_saved(&self, track_id: &str) -> zbus::fdo::Result<bool> {
		let client = self.client().await?;
		let track_id = TrackId::from_id(track_id)
			.map_err(|error| zbus::fdo::Error::InvalidArgs(error.to_string()))?;
		let mut saved = client
			.library_contains([LibraryId::Track(track_id)])
			.await
			.map_err(spotify_error)?
			.into_iter();
		saved
			.next()
			.ok_or_else(|| zbus::fdo::Error::Failed("Spotify returned no track state".to_string()))
	}

	async fn set_track_saved(&self, track_id: &str, saved: bool) -> zbus::fdo::Result<bool> {
		let client = self.client().await?;
		let track_id = TrackId::from_id(track_id)
			.map_err(|error| zbus::fdo::Error::InvalidArgs(error.to_string()))?;
		let library_id = LibraryId::Track(track_id);
		if saved {
			client.library_add([library_id]).await
		} else {
			client.library_remove([library_id]).await
		}
		.map_err(spotify_error)?;
		Ok(saved)
	}
}

pub async fn run(
	connection: zbus::Connection, mut config: watch::Receiver<config::Spotify>,
	mut shutdown: watch::Receiver<bool>,
) -> Result<(), Error> {
	let store = SecretStore::from_connection(connection.clone()).await?;
	let service = SpotifyDbus::new(store);
	let added = connection
		.object_server()
		.at(SPOTIFY_OBJECT_PATH, service.clone())
		.await?;
	if !added {
		return Err("Spotify D-Bus interface is already registered".into());
	}

	service.reconfigure(config.borrow().clone()).await;
	loop {
		tokio::select! {
			changed = config.changed() => {
				if changed.is_err() {
					break;
				}
				service.reconfigure(config.borrow_and_update().clone()).await;
			}
			changed = shutdown.changed() => {
				if changed.is_err() || *shutdown.borrow_and_update() {
					break;
				}
			}
		}
	}

	service.shutdown().await;
	connection
		.object_server()
		.remove::<SpotifyDbus, _>(SPOTIFY_OBJECT_PATH)
		.await?;
	Ok(())
}

async fn initialize_client(
	client_id: String, store: Arc<SecretStore>,
) -> Result<(AuthCodePkceSpotify, String, bool), Error> {
	let credentials = Credentials::new_pkce(&client_id);
	let oauth = OAuth {
		redirect_uri: REDIRECT_URI.to_string(),
		scopes: scopes!("user-library-read user-library-modify"),
		..Default::default()
	};

	let (token_tx, mut token_rx) = tokio::sync::mpsc::unbounded_channel();
	let callback = TokenCallback(Box::new(move |token| {
		let _ = token_tx.send(token);
		Ok(())
	}));
	let token_store = store.clone();
	tokio::spawn(async move {
		while let Some(token) = token_rx.recv().await {
			if let Err(error) = persist_token(&token_store, token).await {
				log::error!("Failed to persist Spotify token: {error}");
			}
		}
	});

	let rspotify_config = rspotify::Config {
		token_cached: false,
		token_refreshing: true,
		token_callback_fn: Arc::new(Some(callback)),
		cache_path: PathBuf::from("/dev/null/invalid"),
		..Default::default()
	};

	let attributes = spotify_attributes();
	let access_token = store
		.get(&attributes.clone().insert("type", "access_token"))
		.await?;
	let expiry = store
		.get(&attributes.clone().insert("type", "access_token_expiry"))
		.await?;
	let refresh_token = store
		.get(&attributes.insert("type", "refresh_token"))
		.await?;
	let token = access_token.zip(expiry).map(|(access_token, expiry)| {
		let expires_at = chrono::DateTime::parse_from_rfc3339(&expiry)
			.map(|date| date.with_timezone(&chrono::Utc))
			.unwrap_or_else(|_| chrono::Utc::now());
		Token {
			access_token,
			expires_in: expires_at - chrono::Utc::now(),
			expires_at: Some(expires_at),
			refresh_token,
			scopes: oauth.scopes.clone(),
		}
	});
	let has_token = token.is_some();
	let mut client = if let Some(token) = token {
		AuthCodePkceSpotify::from_token_with_config(token, credentials, oauth, rspotify_config)
	} else {
		AuthCodePkceSpotify::with_config(credentials, oauth, rspotify_config)
	};
	let authorization_url = client.get_authorize_url(None)?;
	Ok((client, authorization_url, has_token))
}

async fn persist_token(store: &SecretStore, token: Token) -> Result<(), secret_store::Error> {
	if token.is_expired() {
		return Ok(());
	}
	let expiry = token
		.expires_at
		.unwrap_or_else(|| chrono::Utc::now() + token.expires_in)
		.to_rfc3339();
	let attributes = spotify_attributes();
	store
		.store(
			"Spotify Access Token",
			&attributes.clone().insert("type", "access_token"),
			token.access_token,
		)
		.await?;
	store
		.store(
			"Spotify Access Token Expiry",
			&attributes.clone().insert("type", "access_token_expiry"),
			expiry,
		)
		.await?;
	if let Some(refresh_token) = token.refresh_token {
		store
			.store(
				"Spotify Refresh Token",
				&attributes.insert("type", "refresh_token"),
				refresh_token,
			)
			.await?;
	}
	Ok(())
}

fn spotify_attributes() -> SecretAttributes {
	SecretAttributes::new()
		.insert("application", "subniri")
		.insert("service", "spotify")
}

fn valid_client_id(client_id: &str) -> bool {
	client_id.len() == 32 && client_id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn spotify_error(error: rspotify::ClientError) -> zbus::fdo::Error {
	zbus::fdo::Error::Failed(error.to_string())
}

async fn receive_authorization_code(
	listener: TcpListener, client: &AuthCodePkceSpotify,
) -> Result<String, String> {
	let (mut stream, _) = listener.accept().await.map_err(|error| error.to_string())?;
	let mut request = vec![0; 8192];
	let length = stream
		.read(&mut request)
		.await
		.map_err(|error| error.to_string())?;
	let request = std::str::from_utf8(&request[..length]).map_err(|error| error.to_string())?;
	let target = request
		.lines()
		.next()
		.and_then(|line| line.split_whitespace().nth(1))
		.ok_or_else(|| "Invalid Spotify callback request".to_string())?;
	let callback_url = format!("http://{CALLBACK_ADDRESS}{target}");
	let code = client
		.parse_response_code(&callback_url)
		.ok_or_else(|| "Spotify callback did not contain an authorization code".to_string())?;

	let body = "Spotify authentication completed. You can close this window.";
	let response = format!(
		"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
		body.len()
	);
	stream
		.write_all(response.as_bytes())
		.await
		.map_err(|error| error.to_string())?;
	Ok(code)
}
