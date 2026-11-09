//! Messages shared by the authority, helper conversation, and dialog.

use std::fmt;

use zeroize::Zeroizing;

pub type RequestId = u64;

/// Never format cookies or authentication responses, even in debug builds.
#[derive(Clone)]
pub struct Secret(pub Zeroizing<String>);

impl Secret {
	pub fn new(value: String) -> Self {
		Self(Zeroizing::new(value))
	}

	pub fn as_str(&self) -> &str {
		self.0.as_str()
	}
}

impl fmt::Debug for Secret {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str("[redacted]")
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
	pub uid: u32,
	pub username: String,
}

impl fmt::Display for Identity {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{} (uid {})", self.username, self.uid)
	}
}

#[derive(Clone)]
pub struct Request {
	pub id: RequestId,
	pub message: String,
	pub action: String,
	pub identities: Vec<Identity>,
	pub presentation: crate::metadata::Presentation,
}

impl fmt::Debug for Request {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter
			.debug_struct("Request")
			.field("id", &self.id)
			.field("action", &self.action)
			.field("presentation", &self.presentation)
			.finish_non_exhaustive()
	}
}

#[derive(Debug, Clone)]
pub enum Command {
	Respond { generation: u64, response: Secret },
	SelectIdentity(u32),
	Retry,
	Cancel,
}

#[derive(Debug, Clone)]
pub enum Event {
	Started(Request, async_channel::Sender<Command>),
	Prompt {
		id: RequestId,
		generation: u64,
		text: String,
		secret: bool,
	},
	Info {
		id: RequestId,
		text: String,
		error: bool,
	},
	Retry {
		id: RequestId,
	},
	Reset {
		id: RequestId,
		uid: u32,
	},
	Finished {
		id: RequestId,
	},
	Fatal {
		code: u8,
		message: String,
	},
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn secret_debug_is_redacted() {
		let command = Command::Respond {
			generation: 1,
			response: Secret::new("not-for-logs".into()),
		};

		let debug = format!("{command:?}");

		assert!(debug.contains("[redacted]"));
		assert!(!debug.contains("not-for-logs"));
	}
}
