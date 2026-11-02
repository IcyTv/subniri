//! Per-caller authorization sessions, token exchange, and secret storage.

use std::sync::{Arc, Weak};

use daemon_common::{HOMEASSISTANT_REDIRECT_URI, Homeassistant, HomeassistantStatus};
use futures::{SinkExt, StreamExt};
use hass_rs::HassClient;
use rand::distr::{Alphanumeric, SampleString};
use secret_store::{SecretAttributes, SecretStore};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::Message;
use zbus::message::Header;

use super::{HomeassistantInner, HomeassistantService, connection::websocket_url};

const OAUTH_CLIENT_ID: &str = "http://127.0.0.1:8765";
const LONG_LIVED_TOKEN_LIFESPAN_DAYS: u64 = 3650;

#[derive(Clone)]
pub(super) struct AuthorizationSession {
	token: String,
	url: url::Url,
	generation: u64,
	in_progress: bool,
}

struct AuthorizationGuard {
	service: Weak<HomeassistantInner>,
	caller: String,
	generation: u64,
	armed: bool,
}

impl AuthorizationGuard {
	fn disarm(&mut self) {
		self.armed = false;
	}
}

impl Drop for AuthorizationGuard {
	fn drop(&mut self) {
		if !self.armed {
			return;
		}

		let Some(inner) = self.service.upgrade() else {
			return;
		};

		let caller = self.caller.clone();
		let generation = self.generation;

		tokio::spawn(async move {
			HomeassistantService { inner }
				.cancel_authorization_for(&caller, Some(generation))
				.await;
		});
	}
}

impl HomeassistantService {
	pub(super) async fn secret_store(&self) -> Result<Arc<SecretStore>, secret_store::Error> {
		if let Some(store) = self.inner.state.read().await.store.clone() {
			return Ok(store);
		}

		let store = Arc::new(SecretStore::from_connection(self.inner.connection.clone()).await?);
		self.inner.state.write().await.store = Some(store.clone());
		Ok(store)
	}

	pub(super) async fn cancel_authorization_for(&self, caller: &str, generation: Option<u64>) {
		let mut state = self.inner.state.write().await;

		let removed = state.csrf_tokens.get(caller).is_some_and(|session| {
			generation.is_none_or(|expected| session.generation == expected)
		});
		if !removed {
			return;
		}

		state.csrf_tokens.remove(caller);

		if state.status == HomeassistantStatus::Authorizing {
			state.status = if state.authenticated {
				HomeassistantStatus::Ready
			} else {
				HomeassistantStatus::NoLongLivedToken
			};
		}
	}
}

impl Homeassistant for HomeassistantService {
	async fn status(&self) -> HomeassistantStatus {
		self.inner.state.read().await.status
	}

	async fn authenticated(&self) -> bool {
		self.inner.state.read().await.authenticated
	}

	async fn begin_authorization(&self, header: Header<'_>) -> zbus::fdo::Result<String> {
		let caller = Self::caller(&header)?;
		let csrf_token = Alphanumeric.sample_string(&mut rand::rng(), 16);

		let url = {
			let mut state = self.inner.state.write().await;
			let Some(url) = state.url.clone() else {
				return Err(zbus::fdo::Error::Failed(
					"Home Assistant URL is not configured".to_string(),
				));
			};

			let generation = state.generation;
			state.csrf_tokens.insert(
				caller.to_string(),
				AuthorizationSession {
					token: csrf_token.clone(),
					url: url.clone(),
					generation,
					in_progress: false,
				},
			);

			state.status = HomeassistantStatus::Authorizing;
			url
		};

		let auth_url = match authorization_url(&url, &csrf_token) {
			Ok(url) => url,
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to construct authorization URL: {e}"
				)));
			}
		};

		Ok(auth_url)
	}

	async fn cancel_authorization(&self, header: Header<'_>) -> zbus::fdo::Result<()> {
		let caller = Self::caller(&header)?;

		self.cancel_authorization_for(caller.as_str(), None).await;

		Ok(())
	}

	async fn complete_authorization(
		&self, code: &str, csrf_token: &str, header: Header<'_>,
	) -> zbus::fdo::Result<bool> {
		let caller = Self::caller(&header)?;
		let caller = caller.to_string();

		let (url, generation) = {
			let mut state = self.inner.state.write().await;
			let generation = state.generation;
			let configured_url = state.url.clone();

			let Some(session) = state.csrf_tokens.get_mut(caller.as_str()) else {
				return Err(zbus::fdo::Error::Failed(
					"CSRF token not found for caller".to_string(),
				));
			};

			if session.generation != generation || configured_url.as_ref() != Some(&session.url) {
				return Err(zbus::fdo::Error::Failed(
					"Home Assistant configuration changed during authorization".to_string(),
				));
			}

			if session.token != csrf_token {
				return Err(zbus::fdo::Error::Failed(
					"CSRF token does not match".to_string(),
				));
			}

			if session.in_progress {
				return Err(zbus::fdo::Error::Failed(
					"Home Assistant authorization is already being completed".to_string(),
				));
			}

			session.in_progress = true;

			(session.url.clone(), session.generation)
		};

		let mut authorization_guard = AuthorizationGuard {
			service: Arc::downgrade(&self.inner),
			caller: caller.clone(),
			generation,
			armed: true,
		};

		let client = reqwest::Client::new();
		let params = [
			("grant_type", "authorization_code"),
			("code", code),
			("client_id", OAUTH_CLIENT_ID),
		];

		let token_url = url
			.join("/auth/token")
			.map_err(|e| zbus::fdo::Error::Failed(format!("Failed to construct token URL: {e}")))?;

		let resp = match client.post(token_url).form(&params).send().await {
			Ok(resp) if !resp.status().is_success() => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to exchange code for token: {}",
					resp.status()
				)));
			}
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to exchange code for token: {e}"
				)));
			}
			Ok(resp) => resp,
		};

		let res = match resp.json::<TokenResponse>().await {
			Ok(res) => res,
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to parse token response: {e}"
				)));
			}
		};

		let Some(websocket_url) = websocket_url(&url) else {
			return Err(zbus::fdo::Error::Failed(
				"Failed to construct websocket URL".to_string(),
			));
		};

		// TODO: Move this to hass-rs...
		let (mut stream, _) = match tokio_tungstenite::connect_async(&websocket_url).await {
			Ok(stream) => stream,
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to connect to Home Assistant websocket: {e}"
				)));
			}
		};

		let msg = match next_websocket_json(&mut stream).await {
			Ok(msg) => msg,
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to receive message from Home Assistant websocket: {e}"
				)));
			}
		};

		match msg.get("type").and_then(|t| t.as_str()) {
			Some("auth_required") => (),
			Some("auth_invalid") => {
				let msg = msg
					.get("message")
					.and_then(|m| m.as_str())
					.unwrap_or("Unknown error");

				return Err(zbus::fdo::Error::Failed(format!(
					"Home Assistant websocket authentication failed: {msg}"
				)));
			}
			_ => {
				return Err(zbus::fdo::Error::Failed(
					"Unexpected message from Home Assistant websocket".to_string(),
				));
			}
		}

		let auth_msg = serde_json::json!({
			"type": "auth",
			"access_token": res.access_token,
		});
		let auth_msg = serde_json::to_string(&auth_msg).map_err(|e| {
			zbus::fdo::Error::Failed(format!("Failed to serialize auth message: {e}"))
		})?;

		if let Err(e) = stream.send(Message::Text(auth_msg.into())).await {
			return Err(zbus::fdo::Error::Failed(format!(
				"Failed to send auth message to Home Assistant websocket: {e}"
			)));
		}

		let msg = match next_websocket_json(&mut stream).await {
			Ok(msg) => msg,
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to receive message from Home Assistant websocket: {e}"
				)));
			}
		};

		match msg.get("type").and_then(|t| t.as_str()) {
			Some("auth_ok") => (),
			Some("auth_invalid") => {
				let msg = msg
					.get("message")
					.and_then(|m| m.as_str())
					.unwrap_or("Unknown error");

				return Err(zbus::fdo::Error::Failed(format!(
					"Home Assistant websocket authentication failed: {msg}"
				)));
			}
			_ => {
				return Err(zbus::fdo::Error::Failed(
					"Unexpected message from Home Assistant websocket".to_string(),
				));
			}
		}

		let token_req = serde_json::json!({
			"id": 1,
			"type": "auth/long_lived_access_token",
			"client_name": "Subniri Shell",
			"lifespan": LONG_LIVED_TOKEN_LIFESPAN_DAYS,
		});

		let token_req = serde_json::to_string(&token_req).map_err(|e| {
			zbus::fdo::Error::Failed(format!("Failed to serialize token request: {e}"))
		})?;

		if let Err(e) = stream.send(Message::Text(token_req.into())).await {
			return Err(zbus::fdo::Error::Failed(format!(
				"Failed to send auth message to Home Assistant websocket: {e}"
			)));
		}

		let long_lived_token = loop {
			let msg = match next_websocket_json(&mut stream).await {
				Ok(msg) => msg,
				Err(e) => {
					return Err(zbus::fdo::Error::Failed(format!(
						"Failed to receive message from Home Assistant websocket: {e}"
					)));
				}
			};

			if msg.get("id").and_then(|id| id.as_u64()) != Some(1) {
				continue;
			}

			if msg
				.get("type")
				.and_then(|t| t.as_str())
				.map_or(true, |t| t != "result")
			{
				return Err(zbus::fdo::Error::Failed(
					"Unexpected message from Home Assistant websocket".to_string(),
				));
			}

			if msg.get("success").and_then(|s| s.as_bool()) != Some(true) {
				let error = msg
					.get("error")
					.and_then(|e| e.get("message"))
					.and_then(|m| m.as_str())
					.unwrap_or("Unknown error");

				log::warn!("Failed to create long-lived token: {msg}");

				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to create long-lived token: {error}"
				)));
			}

			let Some(token) = msg
				.get("result")
				.and_then(|t| t.as_str())
				.map(|t| t.to_string())
			else {
				return Err(zbus::fdo::Error::Failed(
					"Failed to get long-lived token from response".to_string(),
				));
			};

			break token;
		};

		// let revoke_url = url
		// 	.join("/auth/revoke")
		// 	.map_err(|e| zbus::fdo::Error::Failed(format!("Failed to construct revoke URL: {e}")))?;

		'revoke: {
			// Try to revoke refresh token, if it fails, continue gracefully
			let revoke_url = match url.join("/auth/revoke") {
				Ok(u) => u,
				Err(e) => {
					log::warn!("Failed to construct revoke URL: {e}");
					break 'revoke;
				}
			};

			let params = [("token", res.refresh_token.as_str())];

			if let Err(e) = client.post(revoke_url).form(&params).send().await {
				log::warn!("Failed to revoke refresh token: {e}");
			}
		}

		drop(stream);

		let client = match HassClient::new(&websocket_url).await {
			Ok(client) => Arc::new(client),
			Err(e) => {
				return Err(zbus::fdo::Error::Failed(format!(
					"Failed to create Home Assistant client: {e}"
				)));
			}
		};

		if let Err(e) = client.auth_with_longlivedtoken(&long_lived_token).await {
			return Err(zbus::fdo::Error::Failed(format!(
				"Failed to authenticate with Home Assistant: {e}"
			)));
		}

		let store = match self.secret_store().await {
			Ok(store) => Some(store),
			Err(error) => {
				log::warn!("Failed to access secret store: {error}");
				None
			}
		};

		// Prevent reconfiguration between validating this authorization and persisting its token.
		let mut state = self.inner.state.write().await;
		if state.generation != generation || state.url.as_ref() != Some(&url) {
			return Err(zbus::fdo::Error::Failed(
				"Home Assistant configuration changed during authorization".to_string(),
			));
		}

		let session_active = state
			.csrf_tokens
			.remove(caller.as_str())
			.is_some_and(|session| {
				session.generation == generation
					&& session.url == url
					&& session.token == csrf_token
					&& session.in_progress
			});

		if !session_active {
			return Err(zbus::fdo::Error::Failed(
				"Home Assistant authorization was cancelled".to_string(),
			));
		}
		if let Some(store) = store {
			let attributes = homeassistant_attributes().insert("type", "long_lived_token");
			if let Err(error) = store
				.store(
					"Homeassistant Long Lived Token",
					&attributes,
					&long_lived_token,
				)
				.await
			{
				log::warn!("Failed to store Home Assistant long-lived token: {error}");
			}
		}

		state.client = Some(client.clone());
		state.status = HomeassistantStatus::Connecting;
		state.authenticated = true;

		drop(state);

		if !self.start_catalog_task(client.clone(), generation).await {
			return Err(zbus::fdo::Error::Failed(
				"Failed to initialize Home Assistant entity catalog".to_string(),
			));
		}

		if !self.mark_catalog_ready(&client, generation).await {
			return Err(zbus::fdo::Error::Failed(
				"Home Assistant entity catalog stopped during initialization".to_string(),
			));
		}

		authorization_guard.disarm();

		Ok(true)
	}
}

pub(super) fn homeassistant_attributes() -> SecretAttributes {
	SecretAttributes::new()
		.insert("application", "subniri")
		.insert("service", "homeassistant")
}

fn authorization_url(url: &url::Url, csrf_token: &str) -> Result<String, url::ParseError> {
	let mut url = url.join("/auth/authorize")?;

	url.query_pairs_mut()
		.append_pair("response_type", "code")
		.append_pair("client_id", OAUTH_CLIENT_ID)
		.append_pair("redirect_uri", HOMEASSISTANT_REDIRECT_URI)
		.append_pair("state", csrf_token)
		.append_pair("scope", "read write");

	Ok(url.to_string())
}

#[derive(Deserialize, Serialize, Debug)]
struct TokenResponse {
	access_token: String,
	token_type: String,
	refresh_token: String,
	expires_in: u64,
}

async fn next_websocket_json<S>(stream: &mut S) -> Result<serde_json::Value, String>
where
	S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
		+ SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error>
		+ Unpin,
{
	while let Some(msg) = stream.next().await {
		match msg {
			Ok(Message::Text(text)) => match serde_json::from_str::<serde_json::Value>(&text) {
				Ok(msg) => return Ok(msg),
				Err(e) => return Err(format!("Error parsing json: {e}")),
			},
			Ok(Message::Ping(bytes)) => {
				if let Err(e) = stream.send(Message::Pong(bytes)).await {
					return Err(format!("Failed to respond to websocket ping: {e}"));
				}
			}
			Ok(Message::Close(_)) => return Err("Websocket stream closed".to_string()),
			Ok(_) => continue,
			Err(e) => {
				return Err(format!("Websocket api error: {e}"));
			}
		}
	}

	Err("Websocket stream closed".to_string())
}
