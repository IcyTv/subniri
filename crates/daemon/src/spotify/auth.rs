//! One owner for paired credentials. No implicit login/profile checks.
use std::{
	sync::{
		Arc,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
	time::Duration,
};

use daemon_common::SpotifyPairing;
use secret_store::{SecretAttributes, SecretStore};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::rate_limit::{Cooldown, Failure, cooldown_origin, diagnose, retry_after};

const SCOPES: &str = "streaming user-library-read user-library-modify";

#[derive(Clone, Serialize, Deserialize)]
pub struct Token {
	pub access_token: String,
	pub refresh_token: String,
	pub expires_at: i64,
	pub client_id: String,
}

#[derive(Deserialize)]
struct TokenResponse {
	access_token: String,
	refresh_token: Option<String>,
	expires_in: u64,
}

impl TokenResponse {
	fn into_token(self, client_id: &str, previous_refresh: Option<&str>) -> Result<Token, Failure> {
		let refresh_token = self
			.refresh_token
			.or_else(|| previous_refresh.map(str::to_owned))
			.filter(|value| !value.is_empty())
			.ok_or(Failure::InvalidResponse)?;
		let lifetime = i64::try_from(self.expires_in).map_err(|_| Failure::InvalidResponse)?;
		if self.access_token.is_empty() || lifetime <= 0 {
			return Err(Failure::InvalidResponse);
		}
		Ok(Token {
			access_token: self.access_token,
			refresh_token,
			expires_at: chrono::Utc::now()
				.timestamp()
				.checked_add(lifetime)
				.ok_or(Failure::InvalidResponse)?,
			client_id: client_id.to_owned(),
		})
	}
}

#[derive(Deserialize)]
pub struct DeviceGrant {
	device_code: String,
	user_code: String,
	verification_uri: String,
	expires_in: u64,
	#[serde(default = "default_interval")]
	pub interval: u64,
}

const fn default_interval() -> u64 {
	5
}

impl DeviceGrant {
	pub fn public(&self) -> Result<SpotifyPairing, Failure> {
		// Don't allow the settings UI to open an arbitrary URL supplied by a response.
		if self.verification_uri != "https://spotify.com/pair"
			|| self.user_code.is_empty()
			|| self.expires_in == 0
		{
			return Err(Failure::InvalidResponse);
		}
		Ok(SpotifyPairing {
			verification_uri: self.verification_uri.clone(),
			user_code: self.user_code.clone(),
			expires_at: u64::try_from(chrono::Utc::now().timestamp())
				.unwrap_or(0)
				.saturating_add(self.expires_in.min(3600)),
		})
	}
}

pub enum Approval {
	Pending,
	SlowDown,
	Authorized(Token),
}

pub struct Auth {
	client: reqwest::Client,
	store: Option<Arc<SecretStore>>,
	token: Mutex<Option<Token>>,
	budget: Arc<Mutex<Cooldown>>,
	client_id: String,
	accounts: String,
	service: &'static str,
	dirty: AtomicBool,
	storage_lock: Arc<Mutex<()>>,
	storage_epoch: Arc<AtomicU64>,
	own_epoch: AtomicU64,
	pending_identity: AtomicBool,
}

impl Auth {
	pub fn new(store: Arc<SecretStore>) -> Result<Self, Failure> {
		Self::with_store(Some(store), "https://accounts.spotify.com".to_owned())
	}

	/// Developer capture has its own grant; never reuse or rotate production credentials.
	pub fn developer(store: Arc<SecretStore>) -> Result<Self, Failure> {
		let mut auth = Self::new(store)?;
		auth.service = "spotify-developer-pairing-v1";
		Ok(auth)
	}

	fn with_store(store: Option<Arc<SecretStore>>, accounts: String) -> Result<Self, Failure> {
		if cfg!(test)
			&& !url::Url::parse(&accounts)
				.ok()
				.and_then(|url| url.host_str()?.parse::<std::net::IpAddr>().ok())
				.is_some_and(|ip| ip.is_loopback())
		{
			return Err(Failure::InvalidResponse);
		}
		Ok(Self {
			client: reqwest::Client::builder()
				.timeout(Duration::from_secs(20))
				.redirect(reqwest::redirect::Policy::none())
				.build()
				.map_err(|_| Failure::Unavailable)?,
			store,
			token: Mutex::new(None),
			budget: Arc::new(Mutex::new(Cooldown::default())),
			client_id: librespot_core::SessionConfig::default().client_id,
			accounts,
			service: "spotify-pairing-v1",
			dirty: AtomicBool::new(false),
			storage_lock: Arc::new(Mutex::new(())),
			storage_epoch: Arc::new(AtomicU64::new(0)),
			own_epoch: AtomicU64::new(0),
			pending_identity: AtomicBool::new(false),
		})
	}

	pub fn client_id(&self) -> &str {
		&self.client_id
	}

	pub(super) fn fresh_identity(&self) -> Self {
		Self {
			client: self.client.clone(),
			store: self.store.clone(),
			token: Mutex::new(None),
			budget: self.budget.clone(),
			client_id: self.client_id.clone(),
			accounts: self.accounts.clone(),
			service: self.service,
			dirty: AtomicBool::new(false),
			storage_lock: self.storage_lock.clone(),
			storage_epoch: self.storage_epoch.clone(),
			own_epoch: AtomicU64::new(self.storage_epoch.load(Ordering::Acquire)),
			pending_identity: AtomicBool::new(true),
		}
	}

	#[cfg(test)]
	pub(super) async fn test_auth() -> Arc<Self> {
		let auth = Arc::new(Self::with_store(None, "http://127.0.0.1:9".into()).unwrap());
		auth.accept(Token {
			access_token: "mock-token".into(),
			refresh_token: "mock-refresh".into(),
			expires_at: chrono::Utc::now().timestamp() + 3600,
			client_id: auth.client_id.clone(),
		})
		.await
		.unwrap();
		auth
	}

	fn attributes(&self) -> SecretAttributes {
		SecretAttributes::new()
			.insert("application", "subniri")
			.insert("service", self.service)
			.insert("client_id", &self.client_id)
			.insert("type", "oauth-credentials")
	}

	pub async fn load(&self) -> Result<bool, Failure> {
		if self.token.lock().await.is_some() {
			return Ok(true);
		}
		let Some(store) = &self.store else {
			return Ok(false);
		};
		let stored = tokio::time::timeout(Duration::from_secs(15), store.get(&self.attributes()))
			.await
			.map_err(|_| Failure::Unavailable)?
			.map_err(|_| Failure::Unavailable)?;
		let token = stored
			.map(|text| serde_json::from_str::<Token>(&text).map_err(|_| Failure::InvalidResponse))
			.transpose()?;
		if token
			.as_ref()
			.is_some_and(|t| t.client_id != self.client_id || t.refresh_token.is_empty())
		{
			return Err(Failure::InvalidResponse);
		}
		let found = token.is_some();
		*self.token.lock().await = token;
		Ok(found)
	}

	async fn persist(&self, token: &Token) -> Result<(), Failure> {
		// Serialize account replacement and late refresh writes. Pending pairing
		// does not retire the old grant until the new grant is actually accepted.
		let _storage = self.storage_lock.lock().await;
		// Check before claiming: an older pending pairing may finish after a newer
		// identity has already committed. It must not reclaim storage ownership.
		if self.own_epoch.load(Ordering::Acquire) != self.storage_epoch.load(Ordering::Acquire) {
			return Ok(());
		}
		if self.pending_identity.swap(false, Ordering::AcqRel) {
			self.own_epoch.store(
				self.storage_epoch
					.fetch_add(1, Ordering::AcqRel)
					.wrapping_add(1),
				Ordering::Release,
			);
		}
		if self.own_epoch.load(Ordering::Acquire) != self.storage_epoch.load(Ordering::Acquire) {
			return Ok(());
		}
		if let Some(store) = &self.store {
			let serialized = serde_json::to_string(token).map_err(|_| Failure::InvalidResponse)?;
			tokio::time::timeout(
				Duration::from_secs(15),
				store.store(
					"Subniri Spotify paired credentials",
					&self.attributes(),
					serialized,
				),
			)
			.await
			.map_err(|_| Failure::Unavailable)?
			.map_err(|_| Failure::Unavailable)?;
		}
		Ok(())
	}

	pub async fn accept(&self, token: Token) -> Result<(), Failure> {
		let mut current = self.token.lock().await;
		if token.client_id != self.client_id {
			return Err(Failure::InvalidResponse);
		}
		*current = Some(token);
		self.dirty.store(true, Ordering::Release);
		if let Some(token) = current.as_ref() {
			self.persist(token).await?;
		}
		self.dirty.store(false, Ordering::Release);
		Ok(())
	}

	pub async fn access_token(&self) -> Result<String, Failure> {
		// Hold across refresh so concurrent API/session users never rotate twice.
		let mut current = self.token.lock().await;
		let token = current.as_ref().ok_or(Failure::AuthenticationRequired)?;
		if self.dirty.load(Ordering::Acquire) {
			self.persist(token).await?;
			self.dirty.store(false, Ordering::Release);
		}
		if token.expires_at > chrono::Utc::now().timestamp().saturating_add(60) {
			return Ok(token.access_token.clone());
		}
		let response = self
			.post(
				"token_refresh",
				"/api/token",
				&[
					("grant_type", "refresh_token"),
					("refresh_token", &token.refresh_token),
					("client_id", &self.client_id),
				],
			)
			.await?;
		let status = response.status().as_u16();
		if !response.status().is_success() {
			diagnose(
				"token_refresh",
				Some(status),
				"none",
				None,
				Failure::Unavailable,
			);
		}
		if response.status() == reqwest::StatusCode::BAD_REQUEST {
			let error: serde_json::Value = response
				.json()
				.await
				.map_err(|_| Failure::InvalidResponse)?;
			if error.get("error").and_then(serde_json::Value::as_str) == Some("invalid_grant") {
				*current = None;
				return Err(Failure::AuthenticationRequired);
			}
			return Err(Failure::InvalidResponse);
		}
		if !response.status().is_success() {
			return Err(Failure::Unavailable);
		}
		let decoded = response
			.json::<TokenResponse>()
			.await
			.map_err(|_| Failure::InvalidResponse)
			.and_then(|response| response.into_token(&self.client_id, Some(&token.refresh_token)));
		let refreshed = match decoded {
			Ok(token) => token,
			Err(error) => {
				let wait = self.budget.lock().await.defer(None);
				diagnose(
					"token_refresh_decode",
					Some(status),
					"local_backoff",
					Some(wait),
					error.clone(),
				);
				return Err(error);
			}
		};
		let value = refreshed.access_token.clone();
		*current = Some(refreshed);
		// Keep rotation in memory even if Secret Service temporarily fails. Do not
		// refresh again using an obsolete token and lose the new refresh credential.
		self.dirty.store(true, Ordering::Release);
		if let Some(token) = current.as_ref() {
			self.persist(token).await?;
		}
		self.dirty.store(false, Ordering::Release);
		Ok(value)
	}

	pub async fn invalidate_access(&self) {
		if let Some(token) = self.token.lock().await.as_mut() {
			token.expires_at = 0;
		}
	}

	async fn post(
		&self, stage: &'static str, path: &str, form: &[(&str, &str)],
	) -> Result<reqwest::Response, Failure> {
		let mut budget = self.budget.lock().await;
		budget.check().map_err(|failure| {
			let retry = match &failure {
				Failure::RateLimited(delay) => Some(*delay),
				_ => None,
			};
			diagnose(stage, None, "existing_local_cooldown", retry, failure)
		})?;
		let Ok(response) = self
			.client
			.post(format!("{}{path}", self.accounts))
			.form(form)
			.send()
			.await
		else {
			let wait = budget.defer(None);
			return Err(diagnose(
				stage,
				None,
				"local_backoff",
				Some(wait),
				Failure::Unavailable,
			));
		};
		if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
			|| response.status().is_server_error()
		{
			let retry = retry_after(response.headers());
			let wait = budget.defer(retry);
			return Err(diagnose(
				stage,
				Some(response.status().as_u16()),
				cooldown_origin(retry),
				Some(wait),
				Failure::RateLimited(wait),
			));
		}
		budget.success();
		Ok(response)
	}

	pub async fn begin_pairing(&self) -> Result<DeviceGrant, Failure> {
		let response = self
			.post(
				"pairing_authorize",
				"/oauth2/device/authorize",
				&[("client_id", &self.client_id), ("scope", SCOPES)],
			)
			.await?;
		if !response.status().is_success() {
			return Err(Failure::Forbidden);
		}
		let grant = response
			.json::<DeviceGrant>()
			.await
			.map_err(|_| Failure::InvalidResponse)?;
		grant.public()?;
		Ok(grant)
	}

	pub async fn poll_pairing(&self, grant: &DeviceGrant) -> Result<Approval, Failure> {
		let response = self
			.post(
				"pairing_poll",
				"/api/token",
				&[
					("client_id", &self.client_id),
					("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
					("device_code", &grant.device_code),
				],
			)
			.await?;
		if response.status().is_success() {
			return Ok(Approval::Authorized(
				response
					.json::<TokenResponse>()
					.await
					.map_err(|_| Failure::InvalidResponse)?
					.into_token(&self.client_id, None)?,
			));
		}
		let value: serde_json::Value = response
			.json()
			.await
			.map_err(|_| Failure::InvalidResponse)?;
		match value.get("error").and_then(serde_json::Value::as_str) {
			Some("authorization_pending") => Ok(Approval::Pending),
			Some("slow_down") => Ok(Approval::SlowDown),
			Some("access_denied" | "expired_token") => Err(Failure::AuthenticationRequired),
			_ => Err(Failure::InvalidResponse),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn diagnostic_refresh_failure_retains_server_wait_and_suppresses_retries() {
		use tokio::io::{AsyncReadExt, AsyncWriteExt};
		if std::env::var_os("SPOTIFY_TEST_DIAGNOSTICS").is_some() {
			let _ = log::try_init("daemon::spotify=warn");
		}
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let accounts = format!("http://{}", listener.local_addr().unwrap());
		let server = tokio::spawn(async move {
			let (mut socket, _) = listener.accept().await.unwrap();
			let mut bytes = [0; 8192];
			socket.read(&mut bytes).await.unwrap();
			let body = "SENSITIVE_REFRESH_RESPONSE_MUST_NOT_APPEAR";
			socket.write_all(format!("HTTP/1.1 503 Service Unavailable\r\nRetry-After: 120\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
		});
		let auth = Auth::with_store(None, accounts).unwrap();
		auth.accept(Token {
			access_token: "SECRET_ACCESS_MUST_NOT_APPEAR".into(),
			refresh_token: "SECRET_REFRESH_MUST_NOT_APPEAR".into(),
			expires_at: 0,
			client_id: auth.client_id.clone(),
		})
		.await
		.unwrap();
		assert!(
			matches!(auth.access_token().await,Err(Failure::RateLimited(delay)) if delay>=Duration::from_secs(120))
		);
		server.await.unwrap();
		assert!(matches!(
			auth.access_token().await,
			Err(Failure::RateLimited(_))
		));
	}

	#[tokio::test]
	async fn retired_pending_identity_cannot_reclaim_newer_storage() {
		let original = Auth::test_auth().await;
		let retired = original.fresh_identity();
		let replacement = original.fresh_identity();
		let token = original.token.lock().await.clone().unwrap();
		replacement.accept(token.clone()).await.unwrap();
		assert_eq!(original.storage_epoch.load(Ordering::Acquire), 1);
		retired.accept(token.clone()).await.unwrap();
		original.persist(&token).await.unwrap();
		assert_eq!(original.storage_epoch.load(Ordering::Acquire), 1);
		assert_eq!(replacement.own_epoch.load(Ordering::Acquire), 1);
		assert_eq!(retired.own_epoch.load(Ordering::Acquire), 0);
	}
	#[test]
	fn omitted_refresh_token_is_preserved_and_rotations_replace_it() {
		let response = TokenResponse {
			access_token: "new".into(),
			refresh_token: None,
			expires_in: 3600,
		};
		assert_eq!(
			response
				.into_token("client", Some("old"))
				.unwrap()
				.refresh_token,
			"old"
		);
		let response = TokenResponse {
			access_token: "new".into(),
			refresh_token: Some("rotated".into()),
			expires_in: 3600,
		};
		assert_eq!(
			response
				.into_token("client", Some("old"))
				.unwrap()
				.refresh_token,
			"rotated"
		);
	}
	#[test]
	fn pairing_rejects_untrusted_browser_targets() {
		let grant = DeviceGrant {
			device_code: "secret".into(),
			user_code: "PUBLIC".into(),
			verification_uri: "https://attacker.invalid".into(),
			expires_in: 300,
			interval: 5,
		};
		assert!(grant.public().is_err());
	}
	#[test]
	fn tests_cannot_construct_a_real_accounts_transport() {
		assert!(Auth::with_store(None, "https://accounts.spotify.com".into()).is_err());
	}
	#[tokio::test]
	async fn concurrent_users_refresh_once_using_only_a_loopback_mock() {
		use tokio::io::{AsyncReadExt, AsyncWriteExt};
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let url = format!("http://{}", listener.local_addr().unwrap());
		let server = tokio::spawn(async move {
			let (mut socket, _) = listener.accept().await.unwrap();
			let mut bytes = vec![0; 8192];
			let count = socket.read(&mut bytes).await.unwrap();
			let request = String::from_utf8_lossy(&bytes[..count]);
			assert!(request.starts_with("POST /api/token "));
			assert!(request.contains("refresh_token=refresh"));
			let body = r#"{"access_token":"fresh","expires_in":3600,"refresh_token":"rotated"}"#;
			socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
		});
		let auth = Auth::with_store(None, url).unwrap();
		auth.accept(Token {
			access_token: "expired".into(),
			refresh_token: "refresh".into(),
			expires_at: 0,
			client_id: auth.client_id.clone(),
		})
		.await
		.unwrap();
		let (a, b) = tokio::join!(auth.access_token(), auth.access_token());
		assert_eq!(a.unwrap(), "fresh");
		assert_eq!(b.unwrap(), "fresh");
		assert_eq!(
			auth.token.lock().await.as_ref().unwrap().refresh_token,
			"rotated"
		);
		server.await.unwrap();
	}

	#[tokio::test]
	async fn replacing_identity_preserves_cooldown_but_never_shares_tokens() {
		let auth = Auth::test_auth().await;
		auth.budget
			.lock()
			.await
			.defer(Some(Duration::from_secs(7200)));
		let replacement = auth.fresh_identity();
		assert!(replacement.token.lock().await.is_none());
		assert!(matches!(
			replacement.budget.lock().await.check(),
			Err(Failure::RateLimited(_))
		));
		assert!(auth.token.lock().await.is_some());
	}
}
