use std::{
	collections::HashMap,
	sync::{Arc, Weak},
	time::{SystemTime, UNIX_EPOCH},
};

use daemon_common::{
	Action, LogLevel, Notification, NotificationId, Notifications, NotificationsServer,
};
use itertools::Itertools;
use tokio::sync::Mutex;
use zbus::object_server::SignalEmitter;

use crate::sound::{SoundPlayer, SoundRequest};

pub async fn run<F>(
	state: NotificationsState, shutdown_signal: F,
) -> Result<(), Box<dyn std::error::Error>>
where
	F: Future<Output = Result<(), Box<dyn std::error::Error>>>,
{
	tokio::pin!(shutdown_signal);
	let connection = async {
		let service = NotificationsInterface::new(state.clone());
		Ok::<_, Box<dyn std::error::Error>>(
			zbus::connection::Builder::session()?
				.name("org.freedesktop.Notifications")?
				.serve_at("/org/freedesktop/Notifications", service)?
				.build()
				.await?,
		)
	};
	let connection = tokio::select! {
		result = &mut shutdown_signal => return result,
		result = connection => Arc::new(result?),
	};
	// Only this run owns the strong reference. Cancellation cannot leave a stale
	// connection alive in the process-owned state or clear a newer run's reference.
	*state.standard.lock().await = Arc::downgrade(&connection);

	run_with_connection(async { Ok((*connection).clone()) }, shutdown_signal).await
}

async fn run_with_connection<C, F>(
	connection: C, shutdown_signal: F,
) -> Result<(), Box<dyn std::error::Error>>
where
	C: Future<Output = Result<zbus::Connection, Box<dyn std::error::Error>>>,
	F: Future<Output = Result<(), Box<dyn std::error::Error>>>,
{
	tokio::pin!(shutdown_signal);
	let server = tokio::select! {
		result = &mut shutdown_signal => return result,
		result = connection => result?,
	};

	tokio::select! {
		result = &mut shutdown_signal => result?,
		() = server.closed() => {
			return Err(
				std::io::Error::new(
					std::io::ErrorKind::ConnectionAborted,
					"Notifications D-Bus connection closed",
				).into()
			);
		}
	}

	server.close().await?;

	Ok(())
}

const OBJECT_PATH: &str = "/de/icytv/subniri/Notifications";

#[derive(Clone, Default)]
pub struct NotificationsState {
	inner: Arc<Mutex<StoredNotifications>>,
	standard: Arc<Mutex<Weak<zbus::Connection>>>,
	sound: SoundPlayer,
}

#[derive(Default)]
struct StoredNotifications {
	notifications: HashMap<NotificationId, Notification>,
	current_id: NotificationId,
	custom: Option<SignalEmitter<'static>>,
}

impl NotificationsState {
	#[must_use]
	pub fn new(sound: SoundPlayer) -> Self {
		Self {
			sound,
			..Self::default()
		}
	}

	pub async fn register(&self, connection: &zbus::Connection) -> zbus::Result<()> {
		let emitter = SignalEmitter::new(connection, OBJECT_PATH)?.to_owned();
		let added = connection
			.object_server()
			.at(
				OBJECT_PATH,
				NotificationsServer(SubniriNotificationsInterface {
					state: self.clone(),
				}),
			)
			.await?;
		if !added {
			return Err(zbus::fdo::Error::Failed(
				"Notifications interface already registered".into(),
			)
			.into());
		}
		self.inner.lock().await.custom = Some(emitter);
		Ok(())
	}

	/// Stores a notification and reports whether it is new rather than a replacement of a
	/// visible notification.
	async fn insert(
		&self, replaces_id: NotificationId, mut notification: Notification,
	) -> zbus::fdo::Result<(NotificationId, bool)> {
		let mut state = self.inner.lock().await;
		let id = if replaces_id == 0 {
			loop {
				state.current_id = state.current_id.wrapping_add(1);
				if state.current_id != 0 && !state.notifications.contains_key(&state.current_id) {
					break state.current_id;
				}
			}
		} else {
			replaces_id
		};
		notification.id = id;
		let previous_revision = state
			.notifications
			.get(&id)
			.map(|existing| existing.revision);
		notification.revision = loop {
			let revision = rand::random::<u64>();
			if revision != 0 && Some(revision) != previous_revision {
				break revision;
			}
		};
		let is_new = state
			.notifications
			.get(&id)
			.is_none_or(|existing| existing.closed);
		if let Some(existing) = state.notifications.get(&id) {
			notification.created_at = existing.created_at;
		}
		state.notifications.insert(id, notification.clone());

		// Serialize mutations and events so replacements cannot emit out of order.
		if let Some(emitter) = &state.custom {
			if let Err(error) = emitter
				.emit(
					"de.icytv.subniri.Notifications",
					"NotificationAdded",
					&(notification,),
				)
				.await
			{
				log::error!("Failed to emit notification upsert: {error}");
			}
		}
		Ok((id, is_new))
	}

	async fn close_locked(
		&self, state: &mut StoredNotifications, id: NotificationId, reason: ClosedReason,
	) -> zbus::fdo::Result<()> {
		let notification = state
			.notifications
			.get_mut(&id)
			.filter(|notification| !notification.closed)
			.ok_or_else(|| {
				zbus::fdo::Error::Failed(format!("Notification with ID {id} does not exist"))
			})?;
		notification.closed = true;

		if let Some(emitter) = &state.custom {
			if let Err(error) = emitter
				.emit(
					"de.icytv.subniri.Notifications",
					"NotificationRemoved",
					&(id,),
				)
				.await
			{
				log::error!("Failed to emit notification removal: {error}");
			}
		}
		if let Some(connection) = self.standard.lock().await.upgrade() {
			let emitter = SignalEmitter::new(&*connection, "/org/freedesktop/Notifications")?;
			if let Err(error) =
				NotificationsInterface::notification_closed(&emitter, id, reason as u32).await
			{
				log::error!("Failed to emit NotificationClosed: {error}");
			}
		}
		Ok(())
	}
}

fn created_at() -> i64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_millis()
		.min(i64::MAX as u128) as i64
}

fn hint(notification: &Notification, key: &str) -> bool {
	notification
		.hints
		.get(key)
		.and_then(|value| bool::try_from(value).ok())
		.unwrap_or(false)
}

struct NotificationsInterface {
	state: NotificationsState,
}

impl NotificationsInterface {
	pub fn new(state: NotificationsState) -> Self {
		Self { state }
	}
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl NotificationsInterface {
	#[zbus(name = "GetCapabilities")]
	async fn get_capabilities(&self) -> Vec<&'static str> {
		let mut capabilities = vec![
			"action-icons",
			"actions",
			"body",
			"body-hyperlinks",
			"body-images",
			"body-markup",
			"icon-multi",
			"icon-static",
			"persistence",
		];
		if self.state.sound.enabled() {
			capabilities.push("sound");
		}

		capabilities
	}

	/// Sends a notification to the notification server.
	///
	/// https://specifications.freedesktop.org/notification/latest/protocol.html#command-notify
	#[zbus(name = "Notify")]
	async fn notify(
		&mut self, app_name: String, replaces_id: NotificationId, app_icon: String,
		summary: String, body: String, actions: Vec<String>,
		hints: HashMap<String, zvariant::OwnedValue>, expire_timeout: i32,
	) -> zbus::fdo::Result<NotificationId> {
		let actions = actions
			.into_iter()
			.tuples()
			.map(|(identifier, display)| Action {
				identifier,
				display,
			})
			.collect();
		let sound = SoundRequest::from_hints(&hints);

		let (id, is_new) = self
			.state
			.insert(
				replaces_id,
				Notification {
					id: 0,
					revision: 0,
					app_name,
					app_icon,
					summary,
					body,
					actions,
					hints,
					expire_timeout,
					created_at: created_at(),
					closed: false,
				},
			)
			.await?;

		// The state lock is released here; queuing never blocks Notify.
		if is_new && let Some(sound) = sound {
			self.state.sound.play(sound);
		}

		Ok(id)
	}

	/// Causes a notification to be forcefully closed and removed from the user's view. It can be used, for example, in the event that what the notification pertains to is no longer relevant, or to cancel a notification with no expiration time.
	/// The NotificationClosed signal is emitted by this method.
	/// If the notification no longer exists, an empty D-Bus Error message is sent back.
	///
	/// https://specifications.freedesktop.org/notification/latest/protocol.html#command-close-notification
	#[zbus(name = "CloseNotification")]
	async fn close_notification(&mut self, id: NotificationId) -> zbus::fdo::Result<()> {
		let mut state = self.state.inner.lock().await;
		self.state
			.close_locked(&mut state, id, ClosedReason::ClosedByCall)
			.await
	}

	/// This message returns the information on the server. Specifically, the server name, vendor, and version number.
	///
	/// https://specifications.freedesktop.org/notification/latest/protocol.html#command-get-server-information
	#[zbus(name = "GetServerInformation")]
	async fn get_server_information(&self) -> zbus::fdo::Result<(String, String, String, String)> {
		Ok((
			"polarbar".into(),
			"IcyTv".into(),
			env!("CARGO_PKG_VERSION").into(),
			"1.2".into(),
		))
	}

	#[zbus(signal, name = "NotificationClosed")]
	async fn notification_closed(
		signal_emitter: &SignalEmitter<'_>, id: NotificationId, reason: u32,
	) -> zbus::Result<()>;

	#[zbus(signal, name = "ActionInvoked")]
	async fn action_invoked(
		signal_emitter: &SignalEmitter<'_>, id: NotificationId, action_key: String,
	) -> zbus::Result<()>;

	#[zbus(signal, name = "ActivationToken")]
	async fn activation_token(
		signal_emitter: &SignalEmitter<'_>, id: NotificationId, token: String,
	) -> zbus::Result<()>;
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, serde::Serialize, zvariant::Type)]
pub enum ClosedReason {
	Expired = 1,
	Dismissed = 2,
	ClosedByCall = 3,
	Undefined = 4,
}

pub struct SubniriNotificationsInterface {
	state: NotificationsState,
}

impl Notifications for SubniriNotificationsInterface {
	async fn get_all(&self) -> Vec<Notification> {
		self.state
			.inner
			.lock()
			.await
			.notifications
			.values()
			.filter(|notification| !notification.closed)
			.cloned()
			.collect()
	}

	async fn get(&self, id: NotificationId) -> Option<Notification> {
		self.state
			.inner
			.lock()
			.await
			.notifications
			.get(&id)
			.filter(|notification| !notification.closed)
			.cloned()
	}

	async fn dismiss(&self, id: NotificationId) -> zbus::fdo::Result<()> {
		let mut state = self.state.inner.lock().await;
		self.state
			.close_locked(&mut state, id, ClosedReason::Dismissed)
			.await
	}

	async fn clear_all(&self) -> zbus::fdo::Result<()> {
		let mut state = self.state.inner.lock().await;
		let ids: Vec<_> = state
			.notifications
			.values()
			.filter(|notification| !notification.closed)
			.map(|notification| notification.id)
			.collect();
		for id in ids {
			self.state
				.close_locked(&mut state, id, ClosedReason::Dismissed)
				.await?;
		}
		Ok(())
	}

	async fn invoke_action(&self, id: NotificationId, key: &str) -> zbus::fdo::Result<()> {
		let mut state = self.state.inner.lock().await;
		let notification = state
			.notifications
			.get(&id)
			.filter(|notification| !notification.closed)
			.ok_or_else(|| {
				zbus::fdo::Error::Failed(format!("Notification with ID {id} does not exist"))
			})?;
		if !notification
			.actions
			.iter()
			.any(|action| action.identifier == key)
		{
			return Err(zbus::fdo::Error::InvalidArgs(
				"Unknown notification action".into(),
			));
		}
		let resident = hint(notification, "resident");
		let connection = self.state.standard.lock().await.upgrade().ok_or_else(|| {
			zbus::fdo::Error::Failed("Notifications service is unavailable".into())
		})?;
		let emitter = SignalEmitter::new(&*connection, "/org/freedesktop/Notifications")?;
		NotificationsInterface::action_invoked(&emitter, id, key.to_owned())
			.await
			.map_err(|error| {
				log::error!("Failed to emit ActionInvoked: {error}");
				zbus::fdo::Error::Failed(format!("Failed to emit ActionInvoked: {error}"))
			})?;

		if !resident {
			self.state
				.close_locked(&mut state, id, ClosedReason::Dismissed)
				.await?;
		}
		Ok(())
	}

	async fn expire_transient(
		&self, id: NotificationId, expected_revision: u64,
	) -> zbus::fdo::Result<()> {
		let mut state = self.state.inner.lock().await;
		if state.notifications.get(&id).is_some_and(|notification| {
			notification.revision == expected_revision
				&& !notification.closed
				&& hint(notification, "transient")
		}) {
			self.state
				.close_locked(&mut state, id, ClosedReason::Expired)
				.await?;
		}
		Ok(())
	}

	async fn log(&self, _emitter: SignalEmitter<'_>, level: LogLevel, message: &str) {
		if let Err(error) = self
			.state
			.insert(
				0,
				Notification {
					id: 0,
					revision: 0,
					app_name: "Subniri".into(),
					app_icon: String::new(),
					summary: format!("{level:?}"),
					body: message.to_owned(),
					actions: Vec::new(),
					hints: HashMap::new(),
					expire_timeout: -1,
					created_at: created_at(),
					closed: false,
				},
			)
			.await
		{
			log::error!("Failed to log notification: {error}");
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::time::Duration;

	#[tokio::test]
	async fn shutdown_cancels_pending_connection_setup() {
		let (started_tx, started_rx) = tokio::sync::oneshot::channel();
		let (dropped_tx, mut dropped_rx) = tokio::sync::oneshot::channel::<()>();
		let connection = async move {
			let _drop_guard = dropped_tx;
			started_tx.send(()).unwrap();
			std::future::pending::<Result<zbus::Connection, Box<dyn std::error::Error>>>().await
		};
		let shutdown = async move {
			started_rx.await.unwrap();
			Ok(())
		};

		let result = tokio::time::timeout(
			Duration::from_secs(1),
			run_with_connection(connection, shutdown),
		)
		.await
		.expect("shutdown must not wait for connection setup");

		assert!(result.is_ok());
		assert!(matches!(
			dropped_rx.try_recv(),
			Err(tokio::sync::oneshot::error::TryRecvError::Closed)
		));
	}

	async fn notify(
		interface: &mut NotificationsInterface, replaces_id: NotificationId,
	) -> NotificationId {
		interface
			.notify(
				"app".into(),
				replaces_id,
				"icon".into(),
				"summary".into(),
				"body".into(),
				vec!["default".into(), "Open".into()],
				HashMap::new(),
				0,
			)
			.await
			.unwrap()
	}

	#[tokio::test]
	async fn retains_notifications_and_ids_across_interface_restarts() {
		let state = NotificationsState::default();
		let mut first = NotificationsInterface::new(state.clone());
		let open_id = notify(&mut first, 0).await;
		let closed_id = notify(&mut first, 0).await;
		first.close_notification(closed_id).await.unwrap();
		drop(first);

		let mut restarted = NotificationsInterface::new(state.clone());
		{
			let stored = state.inner.lock().await;
			assert_eq!(stored.notifications.len(), 2);
			let open = &stored.notifications[&open_id];
			assert_eq!(open.id, open_id);
			assert_eq!(open.app_name, "app");
			assert_eq!(open.app_icon, "icon");
			assert_eq!(open.summary, "summary");
			assert_eq!(open.body, "body");
			assert_eq!(open.actions[0].identifier, "default");
			assert_eq!(open.actions[0].display, "Open");
			assert!(open.hints.is_empty());
			assert_eq!(open.expire_timeout, 0);
			assert!(!open.closed);
			assert!(stored.notifications[&closed_id].closed);
		}

		assert_eq!(notify(&mut restarted, open_id).await, open_id);
		assert_eq!(notify(&mut restarted, 0).await, closed_id + 1);
		restarted.close_notification(open_id).await.unwrap();
		assert!(state.inner.lock().await.notifications[&open_id].closed);
	}

	#[tokio::test]
	async fn new_process_state_starts_empty() {
		let old_state = NotificationsState::default();
		let mut old = NotificationsInterface::new(old_state);
		assert_eq!(notify(&mut old, 0).await, 1);

		let fresh_state = NotificationsState::default();
		assert!(fresh_state.inner.lock().await.notifications.is_empty());
		let mut fresh = NotificationsInterface::new(fresh_state);
		assert_eq!(notify(&mut fresh, 0).await, 1);
	}
}
