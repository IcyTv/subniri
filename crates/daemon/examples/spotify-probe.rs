//! Developer-only capture/replay. Not linked into permafrostd; no library writes.
use std::{
	io::{BufRead, Write},
	sync::Arc,
	time::Duration,
};

use daemon::spotify::{
	auth::{Approval, Auth},
	protocol::{self, Event},
};
use futures::StreamExt;
use librespot_core::{
	Session, SessionConfig, authentication::Credentials, dealer::protocol::PayloadValue,
};
use serde_json::{Value, json};

type Error = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() {
	if let Err(error) = run().await {
		eprintln!("{error}");
		std::process::exit(1);
	}
}

async fn run() -> Result<(), Error> {
	let mut args = std::env::args().skip(1);
	let mode = args
		.next()
		.ok_or("Expected --replay PATH or --capture PATH [SECONDS]")?;
	let path = args.next().ok_or("Missing artifact path")?;
	if mode == "--replay" {
		return replay(&path);
	}
	if mode != "--capture" {
		return Err("Unknown mode".into());
	}
	let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
	let seconds = args
		.next()
		.map(|text| text.parse::<u64>())
		.transpose()?
		.unwrap_or(300);
	let mut options = std::fs::OpenOptions::new();
	options.write(true).create_new(true);
	#[cfg(unix)]
	{
		use std::os::unix::fs::OpenOptionsExt;
		options.mode(0o600);
	}
	let mut file = options.open(path)?;
	let auth = Arc::new(Auth::developer(Arc::new(
		secret_store::SecretStore::connect()
			.await
			.map_err(|_| "Secret Service unavailable")?,
	))?);
	if !auth.load().await? {
		let grant = auth.begin_pairing().await?;
		let public = grant.public()?;
		println!(
			"Approve at {} using code {}",
			public.verification_uri, public.user_code
		);
		let mut interval = grant.interval.max(1);
		loop {
			if u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0) >= public.expires_at {
				return Err("Pairing expired".into());
			}
			tokio::select! { _ = tokio::signal::ctrl_c() => return Ok(()), () = tokio::time::sleep(Duration::from_secs(interval)) => {} }
			match auth.poll_pairing(&grant).await? {
				Approval::Pending => {}
				Approval::SlowDown => interval = interval.saturating_add(5),
				Approval::Authorized(token) => {
					auth.accept(token).await?;
					break;
				}
			}
		}
	}
	let token = auth.access_token().await?;
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
	.map_err(|_| "AP timeout")?
	.map_err(|_| "AP authentication failed")?;
	let username = session.username();
	let mut messages = session
		.dealer()
		.add_listen_for("hm://collection")
		.map_err(|_| "Dealer subscription failed")?;
	tokio::time::timeout(Duration::from_secs(45), session.dealer().start())
		.await
		.map_err(|_| "Dealer timeout")?
		.map_err(|_| "Dealer authentication failed")?;
	println!(
		"Capture ready. Manually SAVE a track, wait, UNSAVE it, then press Ctrl-C. No playback device or library writes are made by this tool."
	);
	let deadline = tokio::time::sleep(Duration::from_secs(seconds));
	tokio::pin!(deadline);
	loop {
		tokio::select! {
			_ = tokio::signal::ctrl_c() => break,
			() = &mut deadline => break,
			message = messages.next() => {
				let Some(message) = message else { break; };
				// Only collection messages; redact the account before persisting any data.
				let uri = message.uri.replace(&username,"fixture-user");
				let payload = match message.payload {
					PayloadValue::Json(text) => serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),
					PayloadValue::Raw(bytes) => serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
					PayloadValue::Empty => Value::Null,
				};
				// Binary collection copies are redundant, and arbitrary binary data cannot
				// be reliably scrubbed without a schema. Record only their structural kind.
				let mut envelope = json!({"uri":uri,"payloads":[payload],"headers":{}});
				if !envelope["payloads"][0].is_null() {
					for key in ["Collection-Update-Id","Collection-Source-Revision","MC-ETag"] {
						if let Some(value)=message.headers.get(key) { envelope["headers"][key]=json!(value); }
					}
					redact(&mut envelope);
					writeln!(file,"{}",serde_json::to_string(&envelope)?)?;
					file.flush()?;
					println!("Recorded collection JSON frame (payload kept in protected artifact)");
				}
			}
		}
	}
	session.dealer().close().await;
	Ok(())
}

fn replay(path: &str) -> Result<(), Error> {
	let file = std::fs::File::open(path)?;
	let mut changes = Vec::new();
	for line in std::io::BufReader::new(file).lines() {
		let value: Value = serde_json::from_str(&line?)?;
		match protocol::decode(&value, "fixture-user") {
			Event::Collection { changes: items, .. } => changes.extend(
				items
					.into_iter()
					.map(|item| json!({"id":item.id,"saved":item.saved})),
			),
			Event::Reconcile => {
				return Err(
					"Incompatible relevant collection payload: reconciliation required".into(),
				);
			}
			_ => {}
		}
	}
	if changes.is_empty() {
		return Err(
			"No recognized collection changes; capture is inconclusive or topic changed".into(),
		);
	}
	println!("{}", json!({"status":"compatible","changes":changes}));
	Ok(())
}

fn redact(value: &mut Value) {
	match value {
		Value::Object(fields) => {
			for (key, value) in fields {
				let lower = key.to_ascii_lowercase();
				if [
					"token",
					"secret",
					"authorization",
					"cookie",
					"password",
					"connection",
					"username",
					"email",
				]
				.iter()
				.any(|word| lower.contains(word))
				{
					*value = json!("<REDACTED>");
				} else {
					redact(value);
				}
			}
		}
		Value::Array(values) => {
			for value in values {
				redact(value);
			}
		}
		_ => {}
	}
}

struct SessionGuard(Session);
impl Drop for SessionGuard {
	fn drop(&mut self) {
		self.0.shutdown();
	}
}
