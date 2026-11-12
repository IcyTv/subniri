//! Copy-only access through the document portal's FileTransfer interface.
//!
//! A closed portal transfer is not evidence that a data operation completed. Keep
//! the export alive while offering its token, and keep `Received` alive throughout
//! the copy job. This adapter never deletes sources or implements move semantics.

use std::{collections::HashMap, fmt, fs::File, path::PathBuf, sync::Arc};

use zbus::{Connection, Proxy};
use zvariant::{Fd, Value};

const MAX_TOKEN_BYTES: usize = 4096;
const FD_BATCH_SIZE: usize = 16;

#[derive(Clone)]
pub(crate) struct Export {
	inner: Arc<ExportStorage>,
}

struct ExportStorage {
	connection: Connection,
	key: String,
	// Retain the source descriptors alongside the connection until the last clone drops.
	_files: Vec<File>,
}

impl fmt::Debug for Export {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.debug_struct("Export").finish_non_exhaustive()
	}
}

impl Export {
	pub fn payload(&self) -> Vec<u8> {
		self.inner.key.as_bytes().to_vec()
	}

	/// Prevent future retrievals; this does not finish or cancel a copy job.
	pub async fn stop(&self) -> Result<(), String> {
		let proxy = proxy(&self.inner.connection).await?;
		stop_transfer(&proxy, &self.inner.key).await
	}
}

/// Register worker-opened, identity-checked source descriptors. The caller must
/// not open paths on the UI thread or replace symlinks with their referents.
pub(crate) async fn export(files: Vec<File>) -> Result<Export, String> {
	if files.is_empty() {
		return Err("Cannot export an empty file list".into());
	}

	let connection = Connection::session()
		.await
		.map_err(|error| format!("Cannot connect to the session bus: {error}"))?;
	let portal = proxy(&connection).await?;
	let options = HashMap::from([
		("autostop", Value::from(false)),
		("writable", Value::from(false)),
	]);
	let key: String = portal
		.call("StartTransfer", &(options,))
		.await
		.map_err(|error| format!("Cannot start portal transfer: {error}"))?;

	if let Err(error) = validate_token(key.as_bytes()) {
		let _ = stop_transfer(&portal, &key).await;
		return Err(error);
	}

	for batch in files.chunks(FD_BATCH_SIZE) {
		let descriptors: Vec<Fd<'_>> = batch.iter().map(Fd::from).collect();
		let result: zbus::Result<()> = portal
			.call("AddFiles", &(key.as_str(), descriptors, empty_options()))
			.await;

		if let Err(error) = result {
			let _ = stop_transfer(&portal, &key).await;
			return Err(format!("Cannot add files to portal transfer: {error}"));
		}
	}

	drop(portal);
	Ok(Export {
		inner: Arc::new(ExportStorage {
			connection,
			key,
			_files: files,
		}),
	})
}

/// Retain this value (or a clone) until the copy job has finished using its paths.
#[derive(Clone, Debug)]
pub(crate) struct Received {
	pub paths: Vec<PathBuf>,
	_connection: Connection,
}

/// Resolve a token once, when accepting a transfer, never for hover previews.
pub(crate) async fn retrieve(payload: Vec<u8>) -> Result<Received, String> {
	let key = validate_token(&payload)?;
	let connection = Connection::session()
		.await
		.map_err(|error| format!("Cannot connect to the session bus: {error}"))?;
	let portal = proxy(&connection).await?;
	let paths: Vec<String> = portal
		.call("RetrieveFiles", &(key, empty_options()))
		.await
		.map_err(|error| format!("Cannot retrieve portal files: {error}"))?;

	// The protocol returns UTF-8 D-Bus strings, not URIs or NUL-terminated paths.
	let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
	if paths.len() > 10_000 || paths.iter().any(|path| !path.is_absolute()) {
		return Err("Portal returned too many files or a non-absolute path".into());
	}
	drop(portal);
	Ok(Received {
		paths,
		_connection: connection,
	})
}

async fn proxy(connection: &Connection) -> Result<Proxy<'_>, String> {
	Proxy::new(
		connection,
		"org.freedesktop.portal.Documents",
		"/org/freedesktop/portal/documents",
		"org.freedesktop.portal.FileTransfer",
	)
	.await
	.map_err(|error| format!("Cannot access the file transfer portal: {error}"))
}

async fn stop_transfer(portal: &Proxy<'_>, key: &str) -> Result<(), String> {
	portal
		.call("StopTransfer", &(key,))
		.await
		.map_err(|error| format!("Cannot stop portal transfer: {error}"))
}

fn empty_options() -> HashMap<&'static str, Value<'static>> {
	HashMap::new()
}

fn validate_token(payload: &[u8]) -> Result<&str, String> {
	// The optional transport terminator does not count toward the token limit.
	let token = payload.strip_suffix(&[0]).unwrap_or(payload);
	if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
		return Err("Portal token must contain 1 to 4096 bytes".into());
	}
	if token.iter().any(|byte| matches!(byte, 0 | b'\n' | b'\r')) {
		return Err("Portal token contains a NUL or newline".into());
	}

	std::str::from_utf8(token).map_err(|_| "Portal token is not UTF-8".into())
}

#[cfg(test)]
mod tests {
	use super::{MAX_TOKEN_BYTES, validate_token};

	#[test]
	fn accepts_opaque_utf8_token_and_optional_terminator() {
		for payload in [b"opaque-token".as_slice(), b"opaque-token\0"] {
			assert_eq!(validate_token(payload).unwrap(), "opaque-token");
		}
		assert_eq!(validate_token("opaque-雪".as_bytes()).unwrap(), "opaque-雪");
	}

	#[test]
	fn rejects_empty_and_malformed_tokens() {
		for payload in [
			b"".as_slice(),
			b"\0",
			b"embedded\0nul",
			b"duplicate\0\0",
			b"line\nfeed",
			b"carriage\rreturn",
			b"trailing\n\0",
			b"\xff",
			b"\xff\0",
		] {
			assert!(validate_token(payload).is_err());
		}
	}

	#[test]
	fn bounds_token_bytes_excluding_optional_terminator() {
		let mut payload = vec![b'x'; MAX_TOKEN_BYTES];
		assert!(validate_token(&payload).is_ok());

		payload.push(0);
		assert_eq!(validate_token(&payload).unwrap().len(), MAX_TOKEN_BYTES);

		payload[MAX_TOKEN_BYTES] = b'x';
		assert!(validate_token(&payload).is_err());

		payload.push(0);
		assert!(validate_token(&payload).is_err());
	}

	#[test]
	fn limit_counts_utf8_bytes_not_characters() {
		let payload = "é".repeat(MAX_TOKEN_BYTES / 2);
		assert!(validate_token(payload.as_bytes()).is_ok());

		let oversized = format!("{payload}é");
		assert!(validate_token(oversized.as_bytes()).is_err());
	}
}
