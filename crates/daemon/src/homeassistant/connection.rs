//! Configuration, authenticated connections, and reconnect state transitions.

use std::{sync::Arc, time::Duration};

use daemon_common::HomeassistantStatus;
use hass_rs::{HassClient, HassError};

use super::{
	CATALOG_REQUEST_TIMEOUT, HomeassistantResource, HomeassistantService,
	authorization::homeassistant_attributes,
};

impl HomeassistantService {
	pub async fn reconfigure(&self, config: config::Homeassistant) {
		*self.inner.tracked.write().await = config
			.tracked_devices
			.iter()
			.cloned()
			.map(HomeassistantResource::Entity)
			.collect();

		let connection_config = (config.enabled, config.url.clone());
		let generation = {
			let mut state = self.inner.state.write().await;
			if state.connection_config.as_ref() == Some(&connection_config) {
				return;
			}
			state.connection_config = Some(connection_config);
			state.generation = state.generation.wrapping_add(1);
			state.client = None;
			state.url = config.url.clone();
			state.authenticated = false;
			state.csrf_tokens.clear();
			state.status = if config.enabled {
				HomeassistantStatus::Connecting
			} else {
				HomeassistantStatus::Disabled
			};
			state.generation
		};

		self.stop_catalog_task().await;
		self.clear_entities().await;

		if !config.enabled {
			return;
		}

		let store = match self.secret_store().await {
			Ok(store) => store,
			Err(error) => {
				log::error!("Failed to initialize secret store: {error}");
				self.set_connection_state(generation, None, HomeassistantStatus::Error, false)
					.await;
				return;
			}
		};

		let Some(url) = config.url else {
			self.set_connection_state(generation, None, HomeassistantStatus::NoUrl, false)
				.await;
			return;
		};

		if let Err(error) = check_url(&url).await {
			log::error!("Invalid Home Assistant URL: {error}");
			self.set_connection_state(generation, None, HomeassistantStatus::InvalidUrl, false)
				.await;
			return;
		}

		let Some(websocket_url) = websocket_url(&url) else {
			self.set_connection_state(generation, None, HomeassistantStatus::InvalidUrl, false)
				.await;
			return;
		};

		let client = match HassClient::new(&websocket_url).await {
			Ok(client) => Arc::new(client),
			Err(error) => {
				log::error!("Failed to create Home Assistant client: {error}");
				self.set_connection_state(generation, None, HomeassistantStatus::Error, false)
					.await;
				return;
			}
		};

		// if client.ping().await.is_err() {
		// 	log::error!("Failed to ping Home Assistant at {url}");
		// 	self.set_connection_state(generation, None, HomeassistantStatus::InvalidUrl, false)
		// 		.await;
		// 	return;
		// }

		let attributes = homeassistant_attributes();
		let token = match store
			.get(&attributes.clone().insert("type", "long_lived_token"))
			.await
		{
			Ok(token) => token,
			Err(error) => {
				log::error!("Failed to retrieve Home Assistant token: {error}");
				self.set_connection_state(generation, None, HomeassistantStatus::Error, false)
					.await;
				return;
			}
		};

		let token = match token {
			Some(token) => token,
			None => {
				log::info!("No Home Assistant long-lived token found");
				self.set_connection_state(
					generation,
					Some(client),
					HomeassistantStatus::NoLongLivedToken,
					false,
				)
				.await;
				return;
			}
		};

		// TODO: Check if an invalid token will fail this...
		if let Err(e) = client.auth_with_longlivedtoken(&token).await {
			// log::error!("Failed to authenticate with Home Assistant: {e}");
			// self.set_connection_state(generation, Some(client), HomeassistantStatus::Error, false)
			// 	.await;

			match e {
				HassError::AuthenticationFailed(msg)
					if msg == "Invalid access token or password" =>
				{
					let _ = store
						.delete(&attributes.insert("type", "long_lived_token"))
						.await
						.ok();

					self.set_connection_state(
						generation,
						Some(client),
						HomeassistantStatus::NoLongLivedToken,
						false,
					)
					.await
				}
				_ => {
					self.set_connection_state(
						generation,
						Some(client),
						HomeassistantStatus::Error,
						false,
					)
					.await
				}
			}

			return;
		}

		self.set_connection_state(
			generation,
			Some(client),
			HomeassistantStatus::Connecting,
			true,
		)
		.await;

		if let Some(client) = self.client_for_generation(generation).await {
			if self.start_catalog_task(client.clone(), generation).await {
				self.mark_catalog_ready(&client, generation).await;
			}
		}
	}

	async fn set_connection_state(
		&self, generation: u64, client: Option<Arc<HassClient>>, status: HomeassistantStatus,
		authenticated: bool,
	) {
		let mut state = self.inner.state.write().await;
		if state.generation != generation {
			return;
		}

		state.client = client;
		state.status = status;
		state.authenticated = authenticated;
	}

	async fn client_for_generation(&self, generation: u64) -> Option<Arc<HassClient>> {
		let state = self.inner.state.read().await;
		(state.generation == generation && state.authenticated)
			.then(|| state.client.clone())
			.flatten()
	}

	pub(super) async fn is_current_client(
		&self, client: &Arc<HassClient>, generation: u64,
	) -> bool {
		let state = self.inner.state.read().await;
		state.generation == generation
			&& state.authenticated
			&& state
				.client
				.as_ref()
				.is_some_and(|current| Arc::ptr_eq(current, client))
	}

	pub(super) async fn mark_catalog_ready(
		&self, client: &Arc<HassClient>, generation: u64,
	) -> bool {
		let mut state = self.inner.state.write().await;
		if state.generation != generation
			|| !state.authenticated
			|| !state
				.client
				.as_ref()
				.is_some_and(|current| Arc::ptr_eq(current, client))
		{
			return false;
		}

		state.status = HomeassistantStatus::Ready;
		true
	}

	pub(super) async fn mark_reconnecting(
		&self, client: &Arc<HassClient>, generation: u64,
	) -> bool {
		let mut state = self.inner.state.write().await;
		if state.generation != generation
			|| !state
				.client
				.as_ref()
				.is_some_and(|current| Arc::ptr_eq(current, client))
		{
			return false;
		}
		state.client = None;
		state.authenticated = false;
		state.status = HomeassistantStatus::Connecting;
		true
	}

	pub(super) async fn wait_to_reconnect(&self, generation: u64, delay: Duration) -> bool {
		if !self.is_generation_enabled(generation).await {
			return false;
		}

		let mut shutdown = self.inner.shutdown_tx.subscribe();
		tokio::select! {
			() = tokio::time::sleep(delay) => self.is_generation_enabled(generation).await,
			changed = shutdown.changed() => changed.is_ok() && !*shutdown.borrow() && self.is_generation_enabled(generation).await,
		}
	}

	async fn is_generation_enabled(&self, generation: u64) -> bool {
		let state = self.inner.state.read().await;
		state.generation == generation
			&& state
				.connection_config
				.as_ref()
				.is_some_and(|(enabled, _)| *enabled)
	}

	pub(super) async fn reconnect_client(
		&self, generation: u64,
	) -> Result<Option<Arc<HassClient>>, String> {
		let url = {
			let state = self.inner.state.read().await;
			if state.generation != generation
				|| !state
					.connection_config
					.as_ref()
					.is_some_and(|(enabled, _)| *enabled)
			{
				return Ok(None);
			}
			state.url.clone()
		};

		let Some(url) = url else {
			return Ok(None);
		};

		let websocket_url = websocket_url(&url)
			.ok_or_else(|| "Failed to construct Home Assistant websocket URL".to_string())?;
		let store = self
			.secret_store()
			.await
			.map_err(|error| error.to_string())?;
		let token = store
			.get(&homeassistant_attributes().insert("type", "long_lived_token"))
			.await
			.map_err(|error| error.to_string())?
			.ok_or_else(|| "Home Assistant long-lived token is missing".to_string())?;
		let client = tokio::time::timeout(CATALOG_REQUEST_TIMEOUT, async {
			let client = HassClient::new(&websocket_url).await?;
			client.auth_with_longlivedtoken(&token).await?;
			Ok::<_, HassError>(client)
		})
		.await
		.map_err(|_| "Home Assistant connection attempt timed out".to_string())?
		.map_err(|error| error.to_string())?;

		let client = Arc::new(client);
		let mut state = self.inner.state.write().await;

		if state.generation != generation
			|| !state
				.connection_config
				.as_ref()
				.is_some_and(|(enabled, _)| *enabled)
		{
			return Ok(None);
		}

		state.client = Some(client.clone());
		state.authenticated = true;
		state.status = HomeassistantStatus::Connecting;

		Ok(Some(client))
	}
}

async fn check_url(url: &url::Url) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
	if url.scheme() != "http" && url.scheme() != "https" {
		return Err(format!("Invalid URL scheme: {}", url.scheme()).into());
	}

	if url.host_str().is_none() {
		return Err("URL must have a host".into());
	}

	if !url.path().starts_with('/') {
		return Err("URL path must start with '/'".into());
	}

	let mut url = url.clone();
	url.set_path(format!("{}/manifest.json", url.path()).as_str());

	let response = reqwest::get(url.as_str()).await?;
	let data = response.json::<serde_json::Value>().await?;

	match data.get("name") {
		Some(name) if name == "Home Assistant" => Ok(()),
		_ => Err("Invalid Home Assistant manifest".into()),
	}
}

pub(super) fn websocket_url(url: &url::Url) -> Option<String> {
	let scheme = match url.scheme() {
		"http" => "ws",
		"https" => "wss",
		_ => return None,
	};
	let host = url.host_str()?;
	let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
	let path = url.path();

	Some(format!("{scheme}://{host}{port}{path}/api/websocket"))
}
