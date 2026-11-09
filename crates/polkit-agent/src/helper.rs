//! Async transport for the polkit 127 socket helper (no terminal ACK).

use std::{io, path::Path};

use tokio::{
	io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
	net::UnixStream,
};

pub const SOCKET_PATH: &str = "/run/polkit/agent-helper.socket";
const MAX_LINE_BYTES: usize = 64 * 1024;
// Linux PAM's PAM_MAX_RESP_SIZE is 512, including newline and terminating NUL.
const MAX_RESPONSE_BYTES: usize = 510;

#[derive(Debug, PartialEq, Eq)]
pub enum Record {
	Prompt { text: String, secret: bool },
	Info { text: String, error: bool },
	Success,
	Failure,
}

pub struct Session {
	transport: Option<BufReader<UnixStream>>,
	// Retain partial lines when a caller cancels a pending next() future.
	line: Vec<u8>,
	awaiting_response: bool,
}

impl Session {
	pub async fn connect(path: &Path, username: &str, cookie: &str) -> io::Result<Self> {
		validate_input(username, MAX_LINE_BYTES)?;
		validate_input(cookie, MAX_LINE_BYTES)?;

		let stream = UnixStream::connect(path).await?;
		Self::from_stream(stream, username, cookie).await
	}

	async fn from_stream(mut stream: UnixStream, username: &str, cookie: &str) -> io::Result<Self> {
		validate_input(username, MAX_LINE_BYTES)?;
		validate_input(cookie, MAX_LINE_BYTES)?;

		stream.write_all(username.as_bytes()).await?;
		stream.write_all(b"\n").await?;
		stream.write_all(cookie.as_bytes()).await?;
		stream.write_all(b"\n").await?;

		Ok(Self {
			transport: Some(BufReader::new(stream)),
			line: Vec::new(),
			awaiting_response: false,
		})
	}

	pub async fn next(&mut self) -> io::Result<Record> {
		let result = self.read_record().await;

		match &result {
			Ok(Record::Success | Record::Failure) | Err(_) => {
				// polkit 127 never reads an ACK; release the connection now.
				self.transport = None;
				self.awaiting_response = false;
			}
			Ok(Record::Prompt { .. }) => self.awaiting_response = true,
			Ok(Record::Info { .. }) => {}
		}

		result
	}

	pub async fn respond(&mut self, response: &str) -> io::Result<()> {
		validate_input(response, MAX_RESPONSE_BYTES)?;

		let transport = self.transport.as_mut().ok_or_else(closed)?;
		if !self.awaiting_response {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				"helper is not awaiting a response",
			));
		}

		// Do not C-escape responses: the helper consumes a raw line.
		transport.get_mut().write_all(response.as_bytes()).await?;
		transport.get_mut().write_all(b"\n").await?;
		self.awaiting_response = false;

		Ok(())
	}

	async fn read_record(&mut self) -> io::Result<Record> {
		let transport = self.transport.as_mut().ok_or_else(closed)?;

		loop {
			let available = transport.fill_buf().await?;
			if available.is_empty() {
				return Err(io::Error::new(
					io::ErrorKind::UnexpectedEof,
					"helper disconnected before a terminal record",
				));
			}

			let newline = available.iter().position(|byte| *byte == b'\n');
			let count = newline.unwrap_or(available.len());
			if count > MAX_LINE_BYTES - self.line.len() {
				return Err(invalid_record());
			}

			self.line.extend_from_slice(&available[..count]);
			transport.consume(count + usize::from(newline.is_some()));

			if newline.is_some() {
				let result = parse_record(&self.line);
				self.line.clear();
				return result;
			}
		}
	}
}

fn validate_input(value: &str, limit: usize) -> io::Result<()> {
	if value.len() > limit || value.bytes().any(|byte| matches!(byte, 0 | b'\n' | b'\r')) {
		return Err(io::Error::new(
			io::ErrorKind::InvalidInput,
			"helper input contains a forbidden byte or exceeds its size limit",
		));
	}

	Ok(())
}

fn parse_record(line: &[u8]) -> io::Result<Record> {
	match line {
		b"SUCCESS" => return Ok(Record::Success),
		b"FAILURE" => return Ok(Record::Failure),
		_ => {}
	}

	let separator = line
		.iter()
		.position(|byte| *byte == b' ')
		.ok_or_else(invalid_record)?;
	let text = unescape(&line[separator + 1..])?;

	match &line[..separator] {
		b"PAM_PROMPT_ECHO_OFF" => Ok(Record::Prompt { text, secret: true }),
		b"PAM_PROMPT_ECHO_ON" => Ok(Record::Prompt {
			text,
			secret: false,
		}),
		b"PAM_ERROR_MSG" => Ok(Record::Info { text, error: true }),
		b"PAM_TEXT_INFO" => Ok(Record::Info { text, error: false }),
		_ => Err(invalid_record()),
	}
}

fn unescape(input: &[u8]) -> io::Result<String> {
	let mut output = Vec::with_capacity(input.len());
	let mut cursor = 0;

	while cursor < input.len() {
		let mut byte = input[cursor];
		cursor += 1;

		if byte == b'\\' {
			byte = *input.get(cursor).ok_or_else(invalid_record)?;
			cursor += 1;

			byte = match byte {
				b'b' => 8,
				b'f' => 12,
				b'n' => b'\n',
				b'r' => b'\r',
				b't' => b'\t',
				b'v' => 11,
				b'\\' => b'\\',
				b'"' => b'"',
				b'0'..=b'7' => {
					let mut value = u16::from(byte - b'0');
					for _ in 0..2 {
						match input.get(cursor) {
							Some(digit @ b'0'..=b'7') => {
								value = value * 8 + u16::from(*digit - b'0');
								cursor += 1;
							}
							_ => break,
						}
					}

					u8::try_from(value).map_err(|_| invalid_record())?
				}
				_ => return Err(invalid_record()),
			};
		} else if !(0x20..=0x7e).contains(&byte) {
			// g_strescape emits non-ASCII and control bytes as escapes.
			return Err(invalid_record());
		}

		if byte == 0 {
			return Err(invalid_record());
		}
		output.push(byte);
	}

	String::from_utf8(output).map_err(|_| invalid_record())
}

fn invalid_record() -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, "invalid helper record")
}

fn closed() -> io::Error {
	io::Error::new(io::ErrorKind::NotConnected, "helper session is closed")
}

#[cfg(test)]
#[path = "helper_tests.rs"]
mod tests;
