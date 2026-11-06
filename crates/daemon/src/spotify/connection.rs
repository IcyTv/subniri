//! Observable Dealer lifetime: no hidden reconnect task and no device registration.
use std::{sync::Arc, time::Duration};

use futures::{SinkExt, StreamExt};
use librespot_core::{Session, SessionConfig, authentication::Credentials};
use tokio::{
	sync::mpsc,
	time::{Instant, MissedTickBehavior},
};
use tokio_tungstenite::tungstenite::{self, Message};

use super::{
	auth::Auth,
	protocol,
	rate_limit::{Failure, cooldown_origin, diagnose, retry_after},
	requests::{Api, NativeSession, SessionLease},
};

pub enum Update {
	Connected,
	Event(protocol::Event),
}

/// Caller owns/aborts this future on config generation changes. Dropping it also
/// drops the `SessionGuard` and invalidates the AP; no playback is instantiated.
#[allow(clippy::too_many_lines)] // One supervised AP/Dealer/native-credential lifetime.
pub async fn run(auth: Arc<Auth>, api: Arc<Api>, tx: mpsc::Sender<Update>) -> Result<(), Failure> {
	// Unit tests must never establish real AP/Dealer sessions. Protocol and socket
	// behavior is tested independently against fixtures and loopback peers.
	if cfg!(test) {
		return Err(Failure::Unavailable);
	}
	let token = auth.access_token().await.map_err(|failure| {
		diagnose(
			"session_credentials",
			None,
			"see_request_diagnostic",
			None,
			failure,
		)
	})?;
	let session = Session::new(
		SessionConfig {
			client_id: auth.client_id().to_owned(),
			..SessionConfig::default()
		},
		None,
	);
	let _guard = SessionGuard(session.clone());
	tokio::time::timeout(
		Duration::from_secs(45),
		session.connect(Credentials::with_access_token(token), false),
	)
	.await
	.map_err(|_| {
		diagnose(
			"session_login_timeout",
			None,
			"none",
			None,
			Failure::Unavailable,
		)
	})?
	.map_err(|error| core_failure("session_login", error))?;
	let (host, port) = session
		.apresolver()
		.resolve("dealer")
		.await
		.map_err(|error| core_failure("dealer_discovery", error))?;
	let token = tokio::time::timeout(Duration::from_secs(45), session.login5().auth_token())
		.await
		.map_err(|_| diagnose("login5_timeout", None, "none", None, Failure::Unavailable))?
		.map_err(|error| core_failure("login5", error))?;
	let url = format!("wss://{host}:{port}/?access_token={}", token.access_token);
	let config = tungstenite::protocol::WebSocketConfig::default()
		.max_message_size(Some(1024 * 1024))
		.max_frame_size(Some(1024 * 1024));
	let result = tokio::time::timeout(
		Duration::from_secs(30),
		tokio_tungstenite::connect_async_with_config(url, Some(config), false),
	)
	.await
	.map_err(|_| {
		diagnose(
			"dealer_handshake_timeout",
			None,
			"none",
			None,
			Failure::Unavailable,
		)
	})?;
	let (socket, _) = result.map_err(handshake_failure)?;
	let base = tokio::time::timeout(Duration::from_secs(30), session.spclient().base_url())
		.await
		.map_err(|_| {
			diagnose(
				"native_discovery_timeout",
				None,
				"none",
				None,
				Failure::Unavailable,
			)
		})?
		.map_err(|error| core_failure("native_discovery", error))?;
	let client_token =
		tokio::time::timeout(Duration::from_secs(45), session.spclient().client_token())
			.await
			.map_err(|_| {
				diagnose(
					"native_client_token_timeout",
					None,
					"none",
					None,
					Failure::Unavailable,
				)
			})?
			.map_err(|error| core_failure("native_client_token", error))?;
	let lease = api.attach(NativeSession::new(
		session.clone(),
		&base,
		token.access_token,
		client_token,
	)?)?;
	let dealer_session = session.clone();
	// Poll socket and credential renewal concurrently. A slow token refresh must
	// not suspend pongs or hold a mutex needed by the Dealer task.
	tokio::try_join!(
		async {
			drive(socket, session.username(), tx, move || {
				dealer_session.is_invalid()
			})
			.await
			.map_err(|failure| diagnose("dealer_socket", None, "none", None, failure))
		},
		refresh_native(&session, base, lease),
	)
	.map(|_| ())
}

async fn refresh_native(
	session: &Session, base: String, lease: SessionLease,
) -> Result<(), Failure> {
	loop {
		tokio::time::sleep(Duration::from_secs(30)).await;
		// Both managers cache by expiry. This checks cached credentials, not the
		// collection HTTP service; only the supervisor may renew these tokens.
		let token = tokio::time::timeout(Duration::from_secs(45), session.login5().auth_token())
			.await
			.map_err(|_| {
				diagnose(
					"native_login5_refresh_timeout",
					None,
					"none",
					None,
					Failure::Unavailable,
				)
			})?
			.map_err(|error| core_failure("native_login5_refresh", error))?;
		let client_token =
			tokio::time::timeout(Duration::from_secs(45), session.spclient().client_token())
				.await
				.map_err(|_| {
					diagnose(
						"native_client_token_refresh_timeout",
						None,
						"none",
						None,
						Failure::Unavailable,
					)
				})?
				.map_err(|error| core_failure("native_client_token_refresh", error))?;
		lease.refresh(NativeSession::new(
			session.clone(),
			&base,
			token.access_token,
			client_token,
		)?)?;
	}
}

fn handshake_failure(error: tungstenite::Error) -> Failure {
	// Never format the dependency error: it can contain the access-token URL.
	if let tungstenite::Error::Http(response) = error {
		let status = Some(response.status().as_u16());
		if response.status().as_u16() == 429 {
			let retry = retry_after(response.headers()).filter(|delay| !delay.is_zero());
			let wait = retry.unwrap_or(Duration::from_secs(60));
			return diagnose(
				"dealer_handshake",
				status,
				cooldown_origin(retry),
				Some(wait),
				Failure::RateLimited(wait),
			);
		}
		return diagnose(
			"dealer_handshake",
			status,
			"none",
			None,
			Failure::Unavailable,
		);
	}
	diagnose("dealer_handshake", None, "none", None, Failure::Unavailable)
}

async fn drive<S>(
	mut socket: tokio_tungstenite::WebSocketStream<S>, username: String, tx: mpsc::Sender<Update>,
	invalid: impl Fn() -> bool,
) -> Result<(), Failure>
where
	S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
	let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
	heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
	let mut awaiting_pong: Option<Instant> = None;
	let mut seen = protocol::Deduplicator::default();
	let mut revisions = protocol::RevisionTracker::default();
	let mut announced = false;
	let started = Instant::now();
	let mut health = tokio::time::interval(Duration::from_secs(2));
	loop {
		tokio::select! {
			_ = health.tick() => {
				if invalid() || (!announced && started.elapsed()>Duration::from_secs(10)) || awaiting_pong.is_some_and(|start|start.elapsed()>Duration::from_secs(10)) {
					return Err(Failure::Unavailable);
				}
			},
			_ = heartbeat.tick() => {
				socket.send(Message::Ping(Vec::new().into())).await.map_err(|_|Failure::Unavailable)?;
				awaiting_pong = Some(Instant::now());
			},
			message = socket.next() => match message {
				Some(Ok(Message::Text(text))) => {
					let Ok(value) = serde_json::from_str(&text) else { continue; };
					match protocol::decode(&value,&username) {
						protocol::Event::Connected => {
							announced = true;
							tx.try_send(Update::Connected).map_err(|_|Failure::Unavailable)?;
						},
						protocol::Event::Collection { ref update_id,.. } if seen.accept(update_id.as_deref()) => {
							if revisions.gap(&value) { tx.try_send(Update::Event(protocol::Event::Reconcile)).map_err(|_|Failure::Unavailable)?; }
							tx.try_send(Update::Event(protocol::decode(&value,&username))).map_err(|_|Failure::Unavailable)?;
						},
						protocol::Event::Reconcile if announced => {
							tx.try_send(Update::Event(protocol::Event::Reconcile)).map_err(|_|Failure::Unavailable)?;
						},
						_ => {},
					}
				},
				Some(Ok(Message::Pong(_))) => awaiting_pong = None,
				Some(Ok(Message::Ping(data))) => socket.send(Message::Pong(data)).await.map_err(|_|Failure::Unavailable)?,
				Some(Ok(Message::Close(_)) | Err(_)) | None => return Err(Failure::Unavailable),
				_ => {},
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn diagnostic_handshake_failure_reports_status_without_token_url_or_body() {
		use tokio::io::{AsyncReadExt, AsyncWriteExt};
		if std::env::var_os("SPOTIFY_TEST_DIAGNOSTICS").is_some() {
			let _ = log::try_init("daemon::spotify=warn");
		}
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let server = tokio::spawn(async move {
			let (mut socket, _) = listener.accept().await.unwrap();
			let mut bytes = [0; 8192];
			socket.read(&mut bytes).await.unwrap();
			socket.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
		});
		let error = tokio_tungstenite::connect_async(format!(
			"ws://{address}/?access_token=SECRET_DEALER_TOKEN_MUST_NOT_APPEAR"
		))
		.await
		.unwrap_err();
		assert_eq!(
			handshake_failure(error),
			Failure::RateLimited(Duration::from_secs(120))
		);
		server.await.unwrap();
	}
	#[tokio::test]
	async fn loopback_websocket_delivers_saved_changes_and_exposes_disconnect() {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let server = tokio::spawn(async move {
			let (socket, _) = listener.accept().await.unwrap();
			let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
			socket
				.send(Message::Text(
					r#"{"uri":"hm://pusher/v1/connections/test","payloads":[]}"#.into(),
				))
				.await
				.unwrap();
			for line in include_str!("fixtures/collection.jsonl").lines() {
				socket.send(Message::Text(line.into())).await.unwrap();
			}
			socket.close(None).await.unwrap();
		});
		let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
			.await
			.unwrap();
		let (tx, mut rx) = mpsc::channel(16);
		let result = tokio::time::timeout(
			Duration::from_secs(5),
			drive(socket, "fixture-user".into(), tx, || false),
		)
		.await
		.unwrap();
		assert_eq!(result, Err(Failure::Unavailable));
		assert!(matches!(rx.recv().await, Some(Update::Connected)));
		let mut states = Vec::new();
		while let Some(update) = rx.recv().await {
			if let Update::Event(protocol::Event::Collection { changes, .. }) = update {
				states.extend(changes.into_iter().map(|change| change.saved));
			}
		}
		assert_eq!(states, [true, true, false, false]);
		server.await.unwrap();
	}
}

struct SessionGuard(Session);

fn core_failure(stage: &'static str, error: librespot_core::Error) -> Failure {
	// ErrorKind is a fixed dependency enum, unlike the potentially sensitive body.
	let kind = error.kind;
	drop(error); // Discard the dependency's potentially sensitive error body.
	log::warn!(stage, core_error_kind = ?kind, "Spotify core stage failed");
	let failure = if kind == librespot_core::error::ErrorKind::ResourceExhausted {
		// Core's HTTP layer does not expose long Retry-After headers to callers.
		// Do not turn an opaque throttle into an automatic short retry loop.
		Failure::SessionThrottled
	} else {
		Failure::Unavailable
	};
	diagnose(stage, None, "upstream_timing_unavailable", None, failure)
}
impl Drop for SessionGuard {
	fn drop(&mut self) {
		self.0.shutdown();
	}
}
