//! Single-attempt native collection HTTP. The supervisor owns AP/Login5/token
//! renewal; this owner preserves response status, cooldowns and cancellation.
use std::{
	sync::{
		Arc, Mutex as SyncMutex,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
	time::Duration,
};

use librespot_core::Session;
use tokio::sync::{Mutex, watch};

use super::{
	collection,
	rate_limit::{Cooldown, Failure, cooldown_origin, diagnose, retry_after},
};

/// Token roles are deliberately separate from the paired OAuth grant. No Debug
/// implementation: credentials and endpoint URLs must never appear in logs.
#[derive(Clone)]
pub(super) struct NativeSession {
	session: Option<Session>, // None is constructed only by loopback test fixtures.
	base: url::Url,
	username: String,
	access_token: String,
	client_token: String,
}

impl NativeSession {
	pub fn new(
		session: Session, base: &str, access_token: String, client_token: String,
	) -> Result<Self, Failure> {
		let base = validate_base(base)?;
		Ok(Self {
			username: session.username(),
			session: Some(session),
			base,
			access_token,
			client_token,
		})
	}
	fn valid(&self) -> bool {
		self.session
			.as_ref()
			.is_none_or(|session| !session.is_invalid())
			&& !self.username.is_empty()
			&& !self.access_token.is_empty()
			&& !self.client_token.is_empty()
	}
	fn shutdown(&self) {
		if let Some(session) = &self.session {
			session.shutdown();
		}
	}
	#[cfg(test)]
	fn mock(base: String) -> Self {
		Self {
			session: None,
			base: validate_base(&base).unwrap(),
			username: "fixture-user".into(),
			access_token: "mock-login5-token".into(),
			client_token: "mock-client-token".into(),
		}
	}
}

fn validate_base(base: &str) -> Result<url::Url, Failure> {
	let base = url::Url::parse(base).map_err(|_| Failure::InvalidResponse)?;
	if cfg!(test) {
		if !base
			.host_str()
			.and_then(|host| host.parse::<std::net::IpAddr>().ok())
			.is_some_and(|ip| ip.is_loopback())
		{
			return Err(Failure::InvalidResponse);
		}
	} else if base.scheme() != "https" || base.host_str() == Some("api.spotify.com") {
		return Err(Failure::InvalidResponse);
	}
	if base.host_str().is_none()
		|| !base.username().is_empty()
		|| base.password().is_some()
		|| base.query().is_some()
		|| base.fragment().is_some()
		|| base.path() != "/"
	{
		return Err(Failure::InvalidResponse);
	}
	Ok(base)
}

#[derive(Clone)]
struct Context {
	id: u64,
	credentials: NativeSession,
}

pub struct Api {
	client: reqwest::Client,
	budget: Arc<Mutex<Cooldown>>,
	context: SyncMutex<Option<Context>>,
	epoch: AtomicU64,
	changed: watch::Sender<u64>,
	retired: AtomicBool,
}

/// Dropping a connection detaches only its own lease, never a newer session.
pub(super) struct SessionLease {
	api: Arc<Api>,
	id: u64,
}

impl SessionLease {
	pub fn refresh(&self, credentials: NativeSession) -> Result<(), Failure> {
		let mut current = self
			.api
			.context
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		if self.api.retired.load(Ordering::Acquire) || !credentials.valid() {
			return Err(Failure::Unavailable);
		}
		let context = current
			.as_mut()
			.filter(|context| context.id == self.id)
			.ok_or(Failure::Unavailable)?;
		context.credentials = credentials;
		Ok(())
	}
}

impl Drop for SessionLease {
	fn drop(&mut self) {
		self.api.detach(self.id);
	}
}

enum Operation<'a> {
	Contains(&'a [String]),
	Write(&'a str, bool),
}

impl Api {
	pub fn new() -> Result<Self, Failure> {
		let client = reqwest::Client::builder()
			.timeout(Duration::from_secs(20))
			.redirect(reqwest::redirect::Policy::none())
			.build()
			.map_err(|_| Failure::Unavailable)?;
		Ok(Self::empty(
			client,
			Arc::new(Mutex::new(Cooldown::default())),
		))
	}
	fn empty(client: reqwest::Client, budget: Arc<Mutex<Cooldown>>) -> Self {
		let (changed, _) = watch::channel(0);
		Self {
			client,
			budget,
			context: SyncMutex::new(None),
			epoch: AtomicU64::new(1),
			changed,
			retired: AtomicBool::new(false),
		}
	}
	pub fn replacement(&self) -> Self {
		self.retire();
		Self::empty(self.client.clone(), self.budget.clone())
	}
	pub fn retire(&self) {
		self.retired.store(true, Ordering::Release);
		let old = {
			let mut current = self
				.context
				.lock()
				.unwrap_or_else(std::sync::PoisonError::into_inner);
			let old = current.take();
			self.changed.send_replace(0);
			old
		};
		if let Some(old) = old {
			old.credentials.shutdown();
		}
	}
	pub fn attach(self: &Arc<Self>, credentials: NativeSession) -> Result<SessionLease, Failure> {
		if !credentials.valid() {
			return Err(Failure::Unavailable);
		}
		let id = self.epoch.fetch_add(1, Ordering::AcqRel);
		let old = {
			let mut current = self
				.context
				.lock()
				.unwrap_or_else(std::sync::PoisonError::into_inner);
			if self.retired.load(Ordering::Acquire) {
				return Err(Failure::Unavailable);
			}
			let old = current.replace(Context { id, credentials });
			self.changed.send_replace(id);
			old
		};
		if let Some(old) = old {
			old.credentials.shutdown();
		}
		Ok(SessionLease {
			api: self.clone(),
			id,
		})
	}
	fn detach(&self, id: u64) {
		let old = {
			let mut current = self
				.context
				.lock()
				.unwrap_or_else(std::sync::PoisonError::into_inner);
			if current.as_ref().is_none_or(|context| context.id != id) {
				return;
			}
			let old = current.take();
			self.changed.send_replace(0);
			old
		};
		if let Some(old) = old {
			old.credentials.shutdown();
		}
	}
	fn snapshot(&self) -> Result<Context, Failure> {
		let current = self
			.context
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		if self.retired.load(Ordering::Acquire) {
			return Err(Failure::Unavailable);
		}
		current
			.as_ref()
			.filter(|context| context.credentials.valid())
			.cloned()
			.ok_or(Failure::Unavailable)
	}
	pub async fn ready(&self) -> Result<(), Failure> {
		self.budget
			.lock()
			.await
			.check()
			.map_err(|failure| local_cooldown("library_dispatch", failure))?;
		self.snapshot().map(|_| ())
	}

	async fn request(&self, operation: Operation<'_>) -> Result<Vec<u8>, Failure> {
		let stage = match operation {
			Operation::Contains(_) => "library_read",
			Operation::Write(..) => "library_mutation",
		};
		let mut changed = self.changed.subscribe();
		let mut budget = self.budget.lock().await;
		budget
			.check()
			.map_err(|failure| local_cooldown(stage, failure))?;
		let context = self.snapshot()?;
		if *changed.borrow_and_update() != context.id {
			return Err(Failure::Unavailable);
		}
		let (path, body, contains) = match operation {
			Operation::Contains(ids) => (
				collection::CONTAINS,
				collection::contains(&context.credentials.username, ids)?,
				true,
			),
			Operation::Write(id, saved) => (
				collection::WRITE,
				collection::write(
					&context.credentials.username,
					id,
					saved,
					chrono::Utc::now().timestamp(),
				)?,
				false,
			),
		};
		// Single attempt, cancelled on retirement/disconnect/replacement. In-flight
		// requests retain old credentials; they can never borrow a newer account's.
		tokio::select! {
			biased;
			_ = changed.changed() => Err(Failure::Unavailable),
			result = self.perform(&context, path, body, contains, stage, &mut budget) => result,
		}
	}

	async fn perform(
		&self, context: &Context, path: &str, body: Vec<u8>, contains: bool, stage: &'static str,
		budget: &mut Cooldown,
	) -> Result<Vec<u8>, Failure> {
		let credentials = &context.credentials;
		let url = credentials
			.base
			.join(path)
			.map_err(|_| Failure::InvalidResponse)?;
		let client_token = sensitive_header(&credentials.client_token)?;
		let Ok(mut response) = self
			.client
			.post(url)
			.bearer_auth(&credentials.access_token)
			.header("client-token", client_token)
			.header(reqwest::header::CONTENT_TYPE, collection::MEDIA_TYPE)
			.header(reqwest::header::ACCEPT, collection::MEDIA_TYPE)
			.body(body)
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
		let status = response.status();
		if status != reqwest::StatusCode::OK {
			let retry = retry_after(response.headers());
			let (failure, wait, origin) = match status {
				reqwest::StatusCode::TOO_MANY_REQUESTS => {
					let wait = budget.defer(retry);
					(Failure::RateLimited(wait), wait, cooldown_origin(retry))
				}
				reqwest::StatusCode::UNAUTHORIZED => {
					// This is a Login5/session token, not the paired OAuth grant.
					// Force supervised session renewal; never replay a mutation here.
					self.detach(context.id);
					(
						Failure::Unavailable,
						budget.defer(Some(Duration::from_secs(5))),
						"local_backoff",
					)
				}
				reqwest::StatusCode::FORBIDDEN => {
					(Failure::Forbidden, budget.defer(None), "local_backoff")
				}
				code if code.is_server_error() => {
					let wait = budget.defer(retry);
					(Failure::RateLimited(wait), wait, cooldown_origin(retry))
				}
				_ => (
					Failure::InvalidResponse,
					budget.defer(None),
					"local_backoff",
				),
			};
			return Err(diagnose(
				stage,
				Some(status.as_u16()),
				origin,
				Some(wait),
				failure,
			));
		}
		let mut body = Vec::new();
		if contains {
			loop {
				let Ok(chunk) = response.chunk().await else {
					let wait = budget.defer(None);
					return Err(diagnose(
						"library_read_body",
						Some(200),
						"local_backoff",
						Some(wait),
						Failure::Unavailable,
					));
				};
				let Some(chunk) = chunk else {
					break;
				};
				if body.len().saturating_add(chunk.len()) > collection::MAX_BODY {
					let wait = budget.defer(None);
					return Err(diagnose(
						"library_read_size",
						Some(200),
						"local_backoff",
						Some(wait),
						Failure::InvalidResponse,
					));
				}
				body.extend_from_slice(&chunk);
			}
		}
		budget.success();
		Ok(body)
	}

	pub async fn contains(&self, ids: &[String]) -> Result<Vec<bool>, Failure> {
		let bytes = self.request(Operation::Contains(ids)).await?;
		match collection::decode_contains(&bytes, ids.len()) {
			Ok(states) => Ok(states),
			Err(failure) => {
				let wait = self.budget.lock().await.defer(None);
				Err(diagnose(
					"library_read_decode",
					Some(200),
					"local_backoff",
					Some(wait),
					failure,
				))
			}
		}
	}
	pub async fn set_saved(&self, id: String, saved: bool) -> Result<bool, Failure> {
		self.request(Operation::Write(&id, saved)).await?;
		Ok(saved)
	}
	#[cfg(test)]
	pub(super) fn test_with_base(base: String) -> Self {
		let api = Self::new().unwrap();
		*api.context.lock().unwrap() = Some(Context {
			id: 1,
			credentials: NativeSession::mock(base),
		});
		api.epoch.store(2, Ordering::Release);
		api.changed.send_replace(1);
		api
	}
}

fn local_cooldown(stage: &'static str, failure: Failure) -> Failure {
	let retry = match &failure {
		Failure::RateLimited(delay) => Some(*delay),
		_ => None,
	};
	diagnose(stage, None, "existing_local_cooldown", retry, failure)
}

fn sensitive_header(value: &str) -> Result<reqwest::header::HeaderValue, Failure> {
	let mut header =
		reqwest::header::HeaderValue::from_str(value).map_err(|_| Failure::InvalidResponse)?;
	// Unlike bearer_auth, custom headers are not sensitive by default. Prevent
	// HTTP dependency Debug/trace output from exposing the native client token.
	header.set_sensitive(true);
	Ok(header)
}

#[cfg(test)]
mod tests {
	use super::*;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	const ID: &str = "3uADmRFuyQBoCwUn19B6lV";

	#[test]
	fn client_token_header_is_sensitive_to_dependency_debug_logging() {
		let header = sensitive_header("SECRET_CLIENT_TOKEN").unwrap();
		assert!(header.is_sensitive());
		assert!(!format!("{header:?}").contains("SECRET_CLIENT_TOKEN"));
		assert!(sensitive_header("bad\r\nheader").is_err());
	}

	async fn mock_response(
		status: &str, headers: &str, body: &[u8],
	) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = format!("http://{}", listener.local_addr().unwrap());
		let mut response = format!(
			"HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
			body.len()
		)
		.into_bytes();
		response.extend_from_slice(body);
		let server = tokio::spawn(async move {
			let (mut socket, _) = listener.accept().await.unwrap();
			let mut request = Vec::new();
			loop {
				let mut bytes = [0; 2048];
				let n = socket.read(&mut bytes).await.unwrap();
				assert_ne!(n, 0);
				request.extend_from_slice(&bytes[..n]);
				if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
					let head = String::from_utf8_lossy(&request[..end]);
					let len: usize = head
						.lines()
						.find_map(|line| {
							line.to_ascii_lowercase()
								.strip_prefix("content-length: ")
								.map(str::to_owned)
						})
						.unwrap()
						.parse()
						.unwrap();
					if request.len() >= end + 4 + len {
						break;
					}
				}
			}
			socket.write_all(&response).await.unwrap();
			assert!(
				tokio::time::timeout(Duration::from_millis(100), listener.accept())
					.await
					.is_err(),
				"native operation must not retry"
			);
			request
		});
		(address, server)
	}

	#[tokio::test]
	async fn batched_native_read_has_login5_client_token_headers_and_protobuf_body() {
		let (base, server) = mock_response("200 OK", "", &[10, 2, 1, 0]).await;
		let api = Api::test_with_base(base);
		let ids = vec![ID.into(), "4iV5W9uYEdYUVa79Axb7Rh".into()];
		assert_eq!(api.contains(&ids).await.unwrap(), [true, false]);
		let request = server.await.unwrap();
		let end = request
			.windows(4)
			.position(|window| window == b"\r\n\r\n")
			.unwrap();
		let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
		assert!(head.starts_with("post /collection/v2/contains http/1.1"));
		assert!(head.contains("authorization: bearer mock-login5-token"));
		assert!(head.contains("client-token: mock-client-token"));
		assert!(head.contains(&format!("content-type: {}", collection::MEDIA_TYPE)));
		assert!(head.contains(&format!("accept: {}", collection::MEDIA_TYPE)));
		assert_eq!(
			&request[end + 4..],
			collection::contains("fixture-user", &ids).unwrap()
		);
	}

	#[tokio::test]
	async fn read_and_write_share_server_and_fallback_cooldowns() {
		for headers in ["Retry-After: 7200\r\n", ""] {
			let (base, server) =
				mock_response("429 Too Many Requests", headers, b"private body").await;
			let api = Api::test_with_base(base);
			let minimum = if headers.is_empty() { 60 } else { 7200 };
			assert!(
				matches!(api.contains(&[ID.into()]).await, Err(Failure::RateLimited(delay)) if delay >= Duration::from_secs(minimum))
			);
			server.await.unwrap();
			assert!(matches!(
				api.set_saved(ID.into(), true).await,
				Err(Failure::RateLimited(_))
			));
			assert!(matches!(
				api.contains(&[ID.into()]).await,
				Err(Failure::RateLimited(_))
			));
		}
	}

	#[tokio::test]
	async fn writes_are_native_and_never_replayed_on_success_or_ambiguous_failure() {
		for status in ["200 OK", "503 Service Unavailable"] {
			let (base, server) =
				mock_response(status, "Retry-After: 120\r\n", b"private body").await;
			let api = Api::test_with_base(base);
			let result = api.set_saved(ID.into(), false).await;
			if status.starts_with("200") {
				assert_eq!(result, Ok(false));
			} else {
				assert!(matches!(result, Err(Failure::RateLimited(_))));
			}
			let request = server.await.unwrap();
			assert!(request.starts_with(b"POST /collection/v2/write HTTP/1.1"));
		}
	}

	#[tokio::test]
	async fn redirects_are_not_followed_and_unauthorized_sessions_are_detached() {
		for (status, headers) in [
			(
				"302 Found",
				"Location: https://api.spotify.com/DO-NOT-FOLLOW\r\n",
			),
			("401 Unauthorized", ""),
		] {
			let (base, server) = mock_response(status, headers, b"private body").await;
			let api = Api::test_with_base(base);
			assert!(api.set_saved(ID.into(), true).await.is_err());
			server.await.unwrap();
			if status.starts_with("401") {
				assert!(api.snapshot().is_err());
			}
		}
	}

	#[tokio::test]
	async fn retired_identity_cancels_inflight_response_and_preserves_budget() {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let api = Arc::new(Api::test_with_base(format!(
			"http://{}",
			listener.local_addr().unwrap()
		)));
		let request = {
			let api = api.clone();
			tokio::spawn(async move { api.contains(&[ID.into()]).await })
		};
		let (_socket, _) = listener.accept().await.unwrap();
		api.retire();
		assert_eq!(
			tokio::time::timeout(Duration::from_secs(1), request)
				.await
				.unwrap()
				.unwrap(),
			Err(Failure::Unavailable)
		);
		api.budget
			.lock()
			.await
			.defer(Some(Duration::from_secs(7200)));
		let replacement = api.replacement();
		assert!(matches!(
			replacement.ready().await,
			Err(Failure::RateLimited(_))
		));
		assert!(api.snapshot().is_err());
		assert!(replacement.snapshot().is_err());
	}

	#[tokio::test]
	async fn old_connection_drop_cannot_detach_a_new_connection() {
		let api = Arc::new(Api::new().unwrap());
		assert_eq!(api.ready().await, Err(Failure::Unavailable));
		let old = api
			.attach(NativeSession::mock("http://127.0.0.1:9".into()))
			.unwrap();
		let new = api
			.attach(NativeSession::mock("http://127.0.0.1:9".into()))
			.unwrap();
		assert_eq!(
			old.refresh(NativeSession::mock("http://127.0.0.1:9".into())),
			Err(Failure::Unavailable)
		);
		drop(old);
		assert_eq!(api.ready().await, Ok(()));
		drop(new);
		assert_eq!(api.ready().await, Err(Failure::Unavailable));
		assert!(validate_base("https://spclient.wg.spotify.com").is_err());
	}

	#[tokio::test]
	async fn malformed_or_oversized_membership_establishes_a_shared_cooldown() {
		for body in [vec![10, 9, 1], vec![0; collection::MAX_BODY + 1]] {
			let (base, server) = mock_response("200 OK", "", &body).await;
			let api = Api::test_with_base(base);
			assert_eq!(
				api.contains(&[ID.into()]).await,
				Err(Failure::InvalidResponse)
			);
			server.await.unwrap();
			assert!(matches!(
				api.set_saved(ID.into(), true).await,
				Err(Failure::RateLimited(_))
			));
		}
	}
}
