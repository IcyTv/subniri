//! System-bus registration and cancellation-aware request admission.

use std::{
	collections::{HashMap, VecDeque},
	path::PathBuf,
	sync::{
		Arc,
		atomic::{AtomicU64, Ordering},
	},
};

use futures::StreamExt;
use tokio::sync::{Mutex, watch};
use zbus::{Connection, Proxy, message::Header};
use zvariant::{OwnedValue, Value};

use crate::{
	conversation,
	metadata::{self, Presentation},
	model::{Event, Identity, Request, Secret},
	session,
};

pub const CONFLICT_EXIT_CODE: u8 = 77;
const AUTHORITY: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AGENT_PATH: &str = "/org/subniri/thawth/AuthenticationAgent";
const CONFLICT_MESSAGE: &str = "An authentication agent already exists for the given subject";
type WireIdentity = (String, HashMap<String, OwnedValue>);

#[derive(Debug)]
pub struct StartupError {
	pub code: u8,
	pub message: String,
}

impl StartupError {
	fn failure(message: impl Into<String>) -> Self {
		Self {
			code: 1,
			message: message.into(),
		}
	}
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error")]
enum AgentError {
	Failed(String),
	Cancelled(String),
	NotAuthorized(String),
}

struct Active {
	id: u64,
	cookie: Secret,
	cancel: watch::Sender<bool>,
}

#[derive(Default)]
struct State {
	owner: Option<String>,
	epoch: u64,
	active: Option<Active>,
	cancelled_cookies: VecDeque<Secret>,
}

impl State {
	fn admit(&mut self, epoch: u64, active: Active) -> Result<(), AgentError> {
		self.check_epoch(epoch)?;
		if let Some(position) = self
			.cancelled_cookies
			.iter()
			.position(|cookie| cookie.as_str() == active.cookie.as_str())
		{
			self.cancelled_cookies.remove(position);
			return Err(AgentError::Cancelled(
				"Authentication was already cancelled".into(),
			));
		}

		// TODO: Add a config option to choose between rejecting,
		// queueing, or concurrently presenting authentication requests.
		if self.active.is_some() {
			return Err(AgentError::Failed(
				"Another client is already authenticating; try again later".into(),
			));
		}
		self.active = Some(active);

		Ok(())
	}

	fn cancel(&mut self, epoch: u64, cookie: Secret) -> Result<(), AgentError> {
		self.check_epoch(epoch)?;
		if let Some(active) = &self.active
			&& active.cookie.as_str() == cookie.as_str()
		{
			let _ = active.cancel.send(true);
			return Ok(());
		}

		// D-Bus handlers run concurrently: Cancel may finish validation before
		// its earlier Begin. Retain a bounded tombstone until admission catches up.
		if self.cancelled_cookies.len() == 128 {
			self.cancelled_cookies.pop_front();
		}
		self.cancelled_cookies.push_back(cookie);

		Ok(())
	}

	fn check_epoch(&self, epoch: u64) -> Result<(), AgentError> {
		if self.owner.is_some() && self.epoch == epoch {
			Ok(())
		} else {
			Err(AgentError::NotAuthorized(
				"Authority changed while dispatching request".into(),
			))
		}
	}

	fn change_owner(&mut self, owner: Option<String>) {
		self.epoch += 1;
		self.owner = owner;
		self.cancelled_cookies.clear();
		if let Some(active) = &self.active {
			let _ = active.cancel.send(true);
		}
	}
}

struct Agent {
	connection: Connection,
	state: Arc<Mutex<State>>,
	events: async_channel::Sender<Event>,
	next_id: AtomicU64,
	helper_socket: PathBuf,
}

impl Agent {
	async fn validate(&self, header: &Header<'_>) -> Result<u64, AgentError> {
		let proxy = zbus::fdo::DBusProxy::new(&self.connection)
			.await
			.map_err(|_| AgentError::NotAuthorized("Cannot verify authority".into()))?;
		let owner = proxy
			.get_name_owner(
				AUTHORITY
					.try_into()
					.map_err(|_| AgentError::Failed("Invalid authority name".into()))?,
			)
			.await
			.map_err(|_| AgentError::NotAuthorized("Authority unavailable".into()))?;
		let state = self.state.lock().await;

		if sender_matches(
			header.sender().map(|sender| sender.as_str()),
			state.owner.as_deref(),
			owner.as_str(),
		) {
			Ok(state.epoch)
		} else {
			Err(AgentError::NotAuthorized(
				"Only the registered polkit authority may call this agent".into(),
			))
		}
	}
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
	async fn begin_authentication(
		&self, action_id: String, message: String, icon_name: String,
		details: HashMap<String, String>, cookie: String, identities: Vec<WireIdentity>,
		#[zbus(header)] header: Header<'_>,
	) -> Result<(), AgentError> {
		let cookie = Secret::new(cookie);
		let epoch = self.validate(&header).await?;
		let owner = header
			.sender()
			.ok_or_else(|| AgentError::NotAuthorized("Missing authority sender".into()))?
			.to_string();
		let identities = tokio::task::spawn_blocking(move || resolve_identities(identities))
			.await
			.map_err(|_| AgentError::Failed("Identity lookup failed".into()))??;
		let id = self.next_id.fetch_add(1, Ordering::Relaxed);
		let (cancel, mut cancelled) = watch::channel(false);
		{
			let mut state = self.state.lock().await;
			state.admit(
				epoch,
				Active {
					id,
					cookie: cookie.clone(),
					cancel,
				},
			)?;
		}

		let mut request = Request {
			id,
			message,
			action: action_id,
			identities,
			presentation: Presentation::from_details(icon_name, &details),
		};
		let (sender, receiver) = async_channel::unbounded();
		let authenticate = async {
			request
				.presentation
				.enrich(&self.connection, &owner, &request.action)
				.await;
			conversation::run(
				request,
				cookie,
				receiver,
				sender,
				self.events.clone(),
				&self.helper_socket,
			)
			.await
		};
		let success = tokio::select! {
			biased;
			_ = cancelled.changed() => false,
			success = authenticate => success,
		};

		{
			let mut state = self.state.lock().await;
			if state.active.as_ref().is_some_and(|active| active.id == id) {
				// Order completion before admitting a replacement request's Started event.
				let _ = self.events.try_send(Event::Finished { id });
				state.active = None;
			}
		}

		if success {
			Ok(())
		} else {
			Err(AgentError::Cancelled("Authentication cancelled".into()))
		}
	}

	async fn cancel_authentication(
		&self, cookie: String, #[zbus(header)] header: Header<'_>,
	) -> Result<(), AgentError> {
		let cookie = Secret::new(cookie);
		let epoch = self.validate(&header).await?;

		self.state.lock().await.cancel(epoch, cookie)
	}
}

pub async fn start() -> Result<async_channel::Receiver<Event>, StartupError> {
	let connection = Connection::system()
		.await
		.map_err(|error| StartupError::failure(format!("Cannot connect to system bus: {error}")))?;
	let session = session::discover(&connection)
		.await
		.map_err(StartupError::failure)?;

	register_agent(connection, session, crate::helper::SOCKET_PATH.into()).await
}

async fn register_agent(
	connection: Connection, session: String, helper_socket: PathBuf,
) -> Result<async_channel::Receiver<Event>, StartupError> {
	let (events, receiver) = async_channel::unbounded();
	let state = Arc::new(Mutex::new(State::default()));
	connection
		.object_server()
		.at(
			AGENT_PATH,
			Agent {
				connection: connection.clone(),
				state: state.clone(),
				events: events.clone(),
				next_id: AtomicU64::new(1),
				helper_socket,
			},
		)
		.await
		.map_err(|error| {
			StartupError::failure(format!("Cannot export authentication agent: {error}"))
		})?;

	let bus = zbus::fdo::DBusProxy::new(&connection)
		.await
		.map_err(|error| StartupError::failure(error.to_string()))?;
	let changes = bus
		.receive_name_owner_changed()
		.await
		.map_err(|error| StartupError::failure(error.to_string()))?;
	let authority_name = AUTHORITY
		.try_into()
		.map_err(|_| StartupError::failure("Invalid authority name"))?;
	let owner = match bus.get_name_owner(authority_name).await {
		Ok(owner) => owner,
		Err(zbus::fdo::Error::NameHasNoOwner(_)) => {
			bus.start_service_by_name(
				AUTHORITY
					.try_into()
					.map_err(|_| StartupError::failure("Invalid authority name"))?,
				0,
			)
			.await
			.map_err(|error| {
				StartupError::failure(format!("Cannot activate system polkit: {error}"))
			})?;
			bus.get_name_owner(
				AUTHORITY
					.try_into()
					.map_err(|_| StartupError::failure("Invalid authority name"))?,
			)
			.await
			.map_err(|error| {
				StartupError::failure(format!("Polkit authority unavailable: {error}"))
			})?
		}
		Err(error) => {
			return Err(StartupError::failure(format!(
				"Cannot resolve polkit authority: {error}"
			)));
		}
	};
	state.lock().await.owner = Some(owner.to_string());
	register(&connection, owner.as_str(), &session).await?;

	let mut registered_at = registration_barrier(&connection, owner.as_str()).await?;

	log::info!("thawth registered for login session {session}");

	// The monitor owns the connection and registration for the daemon lifetime.
	tokio::spawn(async move {
		let mut changes = changes;
		while let Some(change) = changes.next().await {
			if change.message().recv_position() <= registered_at {
				continue;
			}
			let Ok(args) = change.args() else { continue };
			if args.name().as_str() != AUTHORITY {
				continue;
			}
			let owner = args.new_owner().as_ref().map(|owner| owner.to_string());
			{
				let mut state = state.lock().await;
				if owner == state.owner {
					continue;
				}
				state.change_owner(owner.clone());
			}

			if let Some(owner) = owner {
				let registration = async {
					register(&connection, &owner, &session).await?;
					registration_barrier(&connection, &owner).await
				}
				.await;

				match registration {
					Ok(position) => registered_at = position,
					Err(error) => {
						let _ = events
							.send(Event::Fatal {
								code: error.code,
								message: error.message,
							})
							.await;
						return;
					}
				}
			}
		}
		let _ = events
			.send(Event::Fatal {
				code: 1,
				message: "System bus disconnected".into(),
			})
			.await;
	});

	Ok(receiver)
}

async fn registration_barrier(
	connection: &Connection, owner: &str,
) -> Result<zbus::message::Sequence, StartupError> {
	// Buffered owner history must not make our successful registration look
	// like a rival. Use local receive order, not D-Bus sender serial numbers.
	let bus = zbus::fdo::DBusProxy::new(connection)
		.await
		.map_err(|error| StartupError::failure(error.to_string()))?;
	let snapshot = bus
		.inner()
		.call_method("GetNameOwner", &(AUTHORITY,))
		.await
		.map_err(|error| {
			StartupError::failure(format!("Cannot verify registered authority: {error}"))
		})?;
	let current: zbus::names::OwnedUniqueName = snapshot
		.body()
		.deserialize()
		.map_err(|error| StartupError::failure(error.to_string()))?;

	if current.as_str() != owner {
		return Err(StartupError::failure(
			"Polkit authority changed during registration; retry registration",
		));
	}

	Ok(snapshot.recv_position())
}

async fn register(connection: &Connection, owner: &str, session: &str) -> Result<(), StartupError> {
	let authority = Proxy::new(
		connection,
		owner,
		AUTHORITY_PATH,
		"org.freedesktop.PolicyKit1.Authority",
	)
	.await
	.map_err(|error| StartupError::failure(error.to_string()))?;
	let subject = (
		"unix-session",
		HashMap::from([("session-id", Value::from(session))]),
	);
	let locale = metadata::locale();

	// The Authority API carries this object path as a D-Bus string (`s`).
	let result: zbus::Result<()> = authority
		.call(
			"RegisterAuthenticationAgent",
			&(subject, locale, AGENT_PATH),
		)
		.await;

	result.map_err(|error| {
		if let zbus::Error::MethodError(name, Some(message), _) = &error
			&& is_registration_conflict(name.as_str(), message)
		{
			return StartupError {
				code: CONFLICT_EXIT_CODE,
				message: "Another polkit agent is already registered; thawth is yielding".into(),
			};
		}
		StartupError::failure(format!("Cannot register authentication agent: {error}"))
	})
}

fn is_registration_conflict(name: &str, message: &str) -> bool {
	name == "org.freedesktop.PolicyKit1.Error.Failed" && message == CONFLICT_MESSAGE
}

fn sender_matches(sender: Option<&str>, registered: Option<&str>, current: &str) -> bool {
	sender == Some(current) && registered == Some(current)
}

fn resolve_identities(identities: Vec<WireIdentity>) -> Result<Vec<Identity>, AgentError> {
	let mut resolved = Vec::new();
	for (kind, details) in identities {
		if kind != "unix-user" {
			continue;
		}
		let Some(uid) = details
			.get("uid")
			.and_then(|value| u32::try_from(value).ok())
		else {
			continue;
		};
		let user = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
			.map_err(|_| AgentError::Failed("Cannot resolve authentication identity".into()))?;
		if let Some(user) = user
			&& !resolved
				.iter()
				.any(|identity: &Identity| identity.uid == uid)
		{
			resolved.push(Identity {
				uid,
				username: user.name,
			});
		}
	}

	if resolved.is_empty() {
		Err(AgentError::Failed(
			"No supported authentication identities were supplied".into(),
		))
	} else {
		Ok(resolved)
	}
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod integration_tests;

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_exact_duplicate_error_prevents_restarts() {
		assert!(is_registration_conflict(
			"org.freedesktop.PolicyKit1.Error.Failed",
			CONFLICT_MESSAGE
		));
		assert!(!is_registration_conflict(
			"org.freedesktop.DBus.Error.AccessDenied",
			CONFLICT_MESSAGE
		));
		assert!(!is_registration_conflict(
			"org.freedesktop.PolicyKit1.Error.Failed",
			"unrelated failure"
		));
	}

	#[test]
	fn caller_must_match_both_current_and_registered_owner() {
		assert!(sender_matches(Some(":1.1"), Some(":1.1"), ":1.1"));
		assert!(!sender_matches(Some(":1.2"), Some(":1.1"), ":1.1"));
		assert!(!sender_matches(Some(":1.1"), Some(":1.1"), ":1.2"));
		assert!(!sender_matches(None, None, ":1.1"));
	}

	#[test]
	fn cancellation_before_admission_is_not_lost() {
		let mut state = State {
			owner: Some(":1.1".into()),
			..Default::default()
		};
		let (cancel, _) = watch::channel(false);
		state.cancel(0, Secret::new("cookie".into())).unwrap();

		let result = state.admit(
			0,
			Active {
				id: 1,
				cookie: Secret::new("cookie".into()),
				cancel,
			},
		);

		assert!(matches!(result, Err(AgentError::Cancelled(_))));
		assert!(state.active.is_none());
		assert!(state.cancelled_cookies.is_empty());
	}

	#[test]
	fn owner_change_invalidates_admission_and_cancels_active_work() {
		let mut state = State {
			owner: Some(":1.1".into()),
			..Default::default()
		};
		let (cancel, receiver) = watch::channel(false);
		state
			.admit(
				0,
				Active {
					id: 1,
					cookie: Secret::new("cookie".into()),
					cancel,
				},
			)
			.unwrap();

		state.change_owner(Some(":1.2".into()));

		assert!(*receiver.borrow());
		assert!(state.check_epoch(0).is_err());
		assert!(state.check_epoch(1).is_ok());
	}
}
