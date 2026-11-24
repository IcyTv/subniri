use std::collections::HashMap;

use zbus::object_server::SignalEmitter;

#[zbus::interface(
	name = "de.icytv.subniri.Notifications",
	proxy(
		// FIXME: Allow constants
		default_service = "de.icytv.subniri.Daemon",
		default_path = "/de/icytv/subniri/Notifications",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait Notifications {
	async fn get_all(&self) -> Vec<Notification>;
	async fn get(&self, id: NotificationId) -> Option<Notification>;
	async fn dismiss(&self, id: NotificationId) -> zbus::fdo::Result<()>;
	async fn clear_all(&self) -> zbus::fdo::Result<()>;
	async fn invoke_action(&self, id: NotificationId, key: &str) -> zbus::fdo::Result<()>;
	async fn expire_transient(
		&self, id: NotificationId, expected_revision: u64,
	) -> zbus::fdo::Result<()>;

	async fn log(
		&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, level: LogLevel, message: &str,
	);

	#[zbus(signal)]
	async fn notification_added(
		signal_emitter: SignalEmitter<'_>, notification: Notification,
	) -> zbus::Result<()>;
	#[zbus(signal)]
	async fn notification_removed(
		signal_emitter: SignalEmitter<'_>, id: NotificationId,
	) -> zbus::Result<()>;
}

pub type NotificationId = u32;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, zvariant::Type)]
pub struct Notification {
	pub id: NotificationId,
	/// Generation token for rejecting stale expiration requests.
	pub revision: u64,
	pub app_name: String,
	pub app_icon: String,
	pub summary: String,
	pub body: String,
	pub actions: Vec<Action>,
	pub hints: HashMap<String, zvariant::OwnedValue>,
	pub expire_timeout: i32,
	/// Creation time in Unix milliseconds, preserved when replaced.
	pub created_at: i64,
	pub closed: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, zvariant::Type)]
pub struct Action {
	pub identifier: String,
	pub display: String,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, zvariant::Type)]
pub enum LogLevel {
	Debug,
	Info,
	Warn,
	Error,
}
