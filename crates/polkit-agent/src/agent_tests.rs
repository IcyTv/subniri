//! Actual D-Bus dispatch, isolated from the host authority and helper.

use std::{process::Stdio, sync::atomic::AtomicUsize, time::Duration};

use tokio::{
	io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
	net::{UnixListener, UnixStream},
	process::{Child, Command as ProcessCommand},
	task::{AbortHandle, JoinHandle},
	time::timeout,
};

use super::*;
use crate::model::Command;

const LIMIT: Duration = Duration::from_secs(20);
const QUIET: Duration = Duration::from_millis(80);
const INTERFACE: &str = "org.freedesktop.PolicyKit1.AuthenticationAgent";
static NEXT_SOCKET: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
	broker: Child,
	directory: PathBuf,
	socket: PathBuf,
	listener: UnixListener,
	authority: Connection,
	foreign: Connection,
	agent: Connection,
	state: Arc<Mutex<State>>,
	events: async_channel::Receiver<Event>,
	tasks: Vec<AbortHandle>,
}

impl Fixture {
	async fn new() -> Self {
		let mut broker = ProcessCommand::new("dbus-daemon")
			.args(["--session", "--nofork", "--print-address=1"])
			.stdout(Stdio::piped())
			.kill_on_drop(true)
			.spawn()
			.expect("private dbus-daemon must be available");
		let mut address = String::new();
		BufReader::new(broker.stdout.take().unwrap())
			.read_line(&mut address)
			.await
			.unwrap();
		assert!(
			!address.trim().is_empty(),
			"broker did not publish an address"
		);

		let authority = connect(address.trim()).await;
		authority.request_name(AUTHORITY).await.unwrap();
		let foreign = connect(address.trim()).await;
		let agent = connect(address.trim()).await;

		let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.join("../../.opencode/thawth")
			.join(format!(
				"agent-{}-{}",
				std::process::id(),
				NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
			));
		std::fs::create_dir_all(directory.parent().unwrap()).unwrap();
		std::fs::create_dir(&directory).unwrap();
		let socket = directory.join("helper.sock");
		let listener = UnixListener::bind(&socket).unwrap();

		let (events, receiver) = async_channel::unbounded();
		let state = Arc::new(Mutex::new(State {
			owner: Some(authority.unique_name().unwrap().to_string()),
			..Default::default()
		}));
		agent
			.object_server()
			.at(
				AGENT_PATH,
				Agent {
					connection: agent.clone(),
					state: state.clone(),
					events,
					next_id: AtomicU64::new(1),
					helper_socket: socket.clone(),
				},
			)
			.await
			.unwrap();

		Self {
			broker,
			directory,
			socket,
			listener,
			authority,
			foreign,
			agent,
			state,
			events: receiver,
			tasks: Vec::new(),
		}
	}

	fn begin(&mut self, cookie: &str) -> JoinHandle<zbus::Result<()>> {
		let connection = self.authority.clone();
		let destination = self.agent.unique_name().unwrap().to_string();
		let cookie = cookie.to_owned();
		let task = tokio::spawn(async move { begin(&connection, &destination, &cookie).await });
		self.tasks.push(task.abort_handle());
		task
	}

	async fn cancel(&self, connection: &Connection, cookie: &str) -> zbus::Result<()> {
		Proxy::new(
			connection,
			self.agent.unique_name().unwrap().as_str(),
			AGENT_PATH,
			INTERFACE,
		)
		.await?
		.call("CancelAuthentication", &(cookie,))
		.await
	}

	async fn started(&self) -> (u64, async_channel::Sender<Command>) {
		let Event::Started(request, commands) = self.events.recv().await.unwrap() else {
			panic!("expected exactly one Started event");
		};
		assert_eq!(request.action, "org.subniri.test");
		assert_eq!(request.identities[0].uid, nix::unistd::getuid().as_raw());

		(request.id, commands)
	}

	async fn prompt(&self, id: u64, cookie: &str) -> (BufReader<UnixStream>, u64) {
		assert!(
			matches!(self.events.recv().await.unwrap(), Event::Reset { id: actual, uid }
			if actual == id && uid == nix::unistd::getuid().as_raw())
		);
		let (stream, _) = self.listener.accept().await.unwrap();
		let mut stream = BufReader::new(stream);

		let user = nix::unistd::User::from_uid(nix::unistd::getuid())
			.unwrap()
			.unwrap();
		assert_eq!(line(&mut stream).await, format!("{}\n", user.name));
		assert_eq!(line(&mut stream).await, format!("{cookie}\n"));

		stream
			.get_mut()
			.write_all(b"PAM_PROMPT_ECHO_OFF Password:\n")
			.await
			.unwrap();
		let Event::Prompt {
			id: actual,
			generation,
			text,
			secret,
		} = self.events.recv().await.unwrap()
		else {
			panic!("expected prompt");
		};
		assert_eq!(actual, id);
		assert_eq!(text, "Password:");
		assert!(secret);

		(stream, generation)
	}

	async fn finished(&self, id: u64) {
		assert!(
			matches!(self.events.recv().await.unwrap(), Event::Finished { id: actual } if actual == id)
		);
		assert!(
			self.events.try_recv().is_err(),
			"duplicate or cross-request event"
		);
	}

	async fn stop(&mut self) {
		self.broker.kill().await.unwrap();
		self.broker.wait().await.unwrap();
	}
}

impl Drop for Fixture {
	fn drop(&mut self) {
		for task in &self.tasks {
			task.abort();
		}
		let _ = std::fs::remove_file(&self.socket);
		let _ = std::fs::remove_dir(&self.directory);
	}
}

async fn connect(address: &str) -> Connection {
	zbus::connection::Builder::address(address)
		.unwrap()
		.build()
		.await
		.unwrap()
}

async fn begin(connection: &Connection, destination: &str, cookie: &str) -> zbus::Result<()> {
	let identities = vec![(
		"unix-user".to_owned(),
		HashMap::from([(
			"uid".to_owned(),
			OwnedValue::from(nix::unistd::getuid().as_raw()),
		)]),
	)];
	Proxy::new(connection, destination, AGENT_PATH, INTERFACE)
		.await?
		.call(
			"BeginAuthentication",
			&(
				"org.subniri.test",
				"Test authentication",
				"",
				HashMap::<String, String>::new(),
				cookie,
				identities,
			),
		)
		.await
}

fn error(result: zbus::Result<()>, suffix: &str) {
	let Err(zbus::Error::MethodError(name, _, _)) = result else {
		panic!("expected D-Bus method error {suffix}");
	};
	assert_eq!(
		name.as_str(),
		format!("org.freedesktop.PolicyKit1.Error.{suffix}")
	);
}

async fn line(stream: &mut BufReader<UnixStream>) -> String {
	let mut line = String::new();
	assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
	line
}

async fn pending(task: &mut JoinHandle<zbus::Result<()>>) {
	assert!(
		timeout(QUIET, task).await.is_err(),
		"Begin returned before completion"
	);
}

async fn closed(stream: &mut BufReader<UnixStream>) {
	let mut byte = [0];
	assert_eq!(
		stream.read(&mut byte).await.unwrap(),
		0,
		"helper must close without an ACK"
	);
}

#[tokio::test]
async fn admission_sender_validation_and_cookie_cancellation() {
	timeout(LIMIT, async {
		let mut fixture = Fixture::new().await;
		let mut first = fixture.begin("first-cookie");
		let (first_id, old_commands) = fixture.started().await;
		let (mut helper, _) = fixture.prompt(first_id, "first-cookie").await;
		pending(&mut first).await;

		error(fixture.begin("busy-cookie").await.unwrap(), "Failed");
		error(
			begin(
				&fixture.foreign,
				fixture.agent.unique_name().unwrap().as_str(),
				"foreign",
			)
			.await,
			"NotAuthorized",
		);
		error(
			fixture.cancel(&fixture.foreign, "first-cookie").await,
			"NotAuthorized",
		);
		fixture
			.cancel(&fixture.authority, "wrong-cookie")
			.await
			.unwrap();
		pending(&mut first).await;
		assert!(fixture.events.try_recv().is_err());

		fixture
			.cancel(&fixture.authority, "first-cookie")
			.await
			.unwrap();
		error(first.await.unwrap(), "Cancelled");
		closed(&mut helper).await;
		fixture.finished(first_id).await;
		assert!(old_commands.send(Command::Retry).await.is_err());

		let mut second = fixture.begin("second-cookie");
		let (second_id, commands) = fixture.started().await;
		assert_ne!(first_id, second_id);
		let (mut helper, generation) = fixture.prompt(second_id, "second-cookie").await;

		// Late cancellation for the old request cannot cancel its replacement.
		fixture
			.cancel(&fixture.authority, "first-cookie")
			.await
			.unwrap();
		pending(&mut second).await;
		commands
			.send(Command::Respond {
				generation,
				response: Secret::new("test-response".into()),
			})
			.await
			.unwrap();
		assert_eq!(line(&mut helper).await, "test-response\n");

		helper.get_mut().write_all(b"SUCCESS\n").await.unwrap();
		second.await.unwrap().unwrap();
		closed(&mut helper).await;
		fixture.finished(second_id).await;
		fixture.stop().await;
	})
	.await
	.expect("isolated D-Bus cancellation test timed out");
}

#[tokio::test]
async fn authority_disappearance_state_reset_cancels_pending_request() {
	timeout(LIMIT, async {
		let mut fixture = Fixture::new().await;
		let mut call = fixture.begin("owner-loss-cookie");
		let (id, commands) = fixture.started().await;
		let (mut helper, _) = fixture.prompt(id, "owner-loss-cookie").await;
		pending(&mut call).await;

		fixture.authority.release_name(AUTHORITY).await.unwrap();
		// Exercise the production state transition used by the owner monitor.
		// The fixture deliberately does not start system-session registration.
		fixture.state.lock().await.change_owner(None);

		error(call.await.unwrap(), "Cancelled");
		closed(&mut helper).await;
		fixture.finished(id).await;
		assert!(commands.send(Command::Retry).await.is_err());
		error(
			fixture.begin("after-owner-loss").await.unwrap(),
			"NotAuthorized",
		);
		assert!(fixture.events.try_recv().is_err());
		fixture.stop().await;
	})
	.await
	.expect("isolated D-Bus owner-loss test timed out");
}

#[tokio::test]
async fn retry_uses_fresh_helper_and_rejects_stale_prompt_responses() {
	timeout(LIMIT, async {
		let mut fixture = Fixture::new().await;
		let mut call = fixture.begin("retry-cookie");
		let (id, commands) = fixture.started().await;
		let (mut first_helper, first_generation) = fixture.prompt(id, "retry-cookie").await;
		first_helper
			.get_mut()
			.write_all(b"FAILURE\n")
			.await
			.unwrap();
		assert!(
			matches!(fixture.events.recv().await.unwrap(), Event::Retry { id: actual } if actual == id)
		);
		closed(&mut first_helper).await;
		pending(&mut call).await;

		commands.send(Command::Retry).await.unwrap();
		let (mut second_helper, second_generation) = fixture.prompt(id, "retry-cookie").await;
		assert!(second_generation > first_generation);
		commands
			.send(Command::Respond {
				generation: first_generation,
				response: Secret::new("stale-response".into()),
			})
			.await
			.unwrap();
		commands
			.send(Command::Respond {
				generation: second_generation,
				response: Secret::new("fresh-response".into()),
			})
			.await
			.unwrap();
		assert_eq!(line(&mut second_helper).await, "fresh-response\n");
		pending(&mut call).await;

		second_helper
			.get_mut()
			.write_all(b"SUCCESS\n")
			.await
			.unwrap();
		call.await.unwrap().unwrap();
		closed(&mut second_helper).await;
		fixture.finished(id).await;
		fixture.stop().await;
	})
	.await
	.expect("isolated D-Bus retry test timed out");
}

struct TestAuthority {
	registrations: async_channel::Sender<String>,
	conflict: bool,
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.Authority")]
impl TestAuthority {
	async fn register_authentication_agent(
		&self, subject: (String, HashMap<String, OwnedValue>), _locale: String, object_path: String,
	) -> Result<(), AgentError> {
		// Polkit 127's Authority XML declares object_path as `s`, not `o`.
		assert_eq!(object_path, AGENT_PATH);

		if self.conflict {
			return Err(AgentError::Failed(CONFLICT_MESSAGE.into()));
		}
		assert_eq!(subject.0, "unix-session");
		let session = <&str>::try_from(subject.1.get("session-id").unwrap()).unwrap();
		self.registrations.send(session.to_owned()).await.unwrap();

		Ok(())
	}
}

#[tokio::test]
async fn registration_and_owner_monitor_reregister_after_authority_restart() {
	timeout(LIMIT, async {
		let mut fixture = Fixture::new().await;
		fixture
			.agent
			.object_server()
			.remove::<Agent, _>(AGENT_PATH)
			.await
			.unwrap();
		let (registrations, registered) = async_channel::unbounded();
		fixture
			.authority
			.object_server()
			.at(
				AUTHORITY_PATH,
				TestAuthority {
					registrations,
					conflict: false,
				},
			)
			.await
			.unwrap();

		fixture.events = register_agent(
			fixture.agent.clone(),
			"test-session".into(),
			fixture.socket.clone(),
		)
		.await
		.unwrap();
		assert_eq!(registered.recv().await.unwrap(), "test-session");
		let call = fixture.begin("restart-cookie");
		let (id, _commands) = fixture.started().await;
		let (mut helper, _) = fixture.prompt(id, "restart-cookie").await;

		fixture.authority.release_name(AUTHORITY).await.unwrap();

		error(call.await.unwrap(), "Cancelled");
		closed(&mut helper).await;
		fixture.finished(id).await;
		fixture.authority.request_name(AUTHORITY).await.unwrap();
		assert_eq!(registered.recv().await.unwrap(), "test-session");

		let call = fixture.begin("after-restart");
		let (id, commands) = fixture.started().await;
		let (mut helper, _) = fixture.prompt(id, "after-restart").await;
		commands.send(Command::Cancel).await.unwrap();

		error(call.await.unwrap(), "Cancelled");
		closed(&mut helper).await;
		fixture.finished(id).await;
		fixture.stop().await;
	})
	.await
	.expect("isolated registration/owner-monitor test timed out");
}

#[tokio::test]
async fn duplicate_registration_returns_dedicated_failing_status() {
	timeout(LIMIT, async {
		let mut fixture = Fixture::new().await;
		let (registrations, _) = async_channel::unbounded();
		fixture
			.authority
			.object_server()
			.at(
				AUTHORITY_PATH,
				TestAuthority {
					registrations,
					conflict: true,
				},
			)
			.await
			.unwrap();

		let failure = register(
			&fixture.agent,
			fixture.authority.unique_name().unwrap().as_str(),
			"test-session",
		)
		.await
		.unwrap_err();

		assert_eq!(failure.code, CONFLICT_EXIT_CODE);
		assert_ne!(failure.code, 0);
		assert!(failure.message.contains("already registered"));
		fixture.stop().await;
	})
	.await
	.expect("isolated registration conflict test timed out");
}

struct StartupChurnAuthority {
	connection: Connection,
	registered: std::sync::atomic::AtomicBool,
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.Authority")]
impl StartupChurnAuthority {
	async fn register_authentication_agent(
		&self, _subject: (String, HashMap<String, OwnedValue>), _locale: String,
		object_path: String,
	) -> Result<(), AgentError> {
		assert_eq!(object_path, AGENT_PATH);

		if self.registered.swap(true, Ordering::Relaxed) {
			return Err(AgentError::Failed(CONFLICT_MESSAGE.into()));
		}

		// Queue owner transitions during registration, before its reply arrives.
		self.connection.release_name(AUTHORITY).await.unwrap();
		self.connection.request_name(AUTHORITY).await.unwrap();

		Ok(())
	}
}

#[tokio::test]
async fn startup_owner_history_does_not_trigger_duplicate_registration() {
	timeout(LIMIT, async {
		let mut fixture = Fixture::new().await;
		fixture
			.agent
			.object_server()
			.remove::<Agent, _>(AGENT_PATH)
			.await
			.unwrap();
		fixture
			.authority
			.object_server()
			.at(
				AUTHORITY_PATH,
				StartupChurnAuthority {
					connection: fixture.authority.clone(),
					registered: std::sync::atomic::AtomicBool::new(false),
				},
			)
			.await
			.unwrap();

		let events = register_agent(
			fixture.agent.clone(),
			"test-session".into(),
			fixture.socket.clone(),
		)
		.await
		.unwrap();

		assert!(
			timeout(QUIET, events.recv()).await.is_err(),
			"startup history caused a false registration conflict"
		);
		fixture.stop().await;
	})
	.await
	.expect("isolated startup-owner-history test timed out");
}

struct MetadataAuthority {
	lookup_started: async_channel::Sender<()>,
	stall: bool,
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.Authority")]
impl MetadataAuthority {
	async fn enumerate_actions(&self, _locale: String) -> Vec<crate::metadata::ActionDescription> {
		let _ = self.lookup_started.send(()).await;
		if self.stall {
			return std::future::pending().await;
		}

		// Polkit 127: a(ssssssuuua{ss}), including all three u32 defaults.
		vec![(
			"org.subniri.test".into(),
			"Run a test operation".into(),
			"Test message".into(),
			"Test vendor".into(),
			String::new(),
			"catalog-icon".into(),
			0,
			0,
			0,
			HashMap::new(),
		)]
	}
}

#[tokio::test]
async fn metadata_uses_authority_catalog_and_keeps_target_separate_from_identity() {
	timeout(LIMIT, async {
		let mut fixture = Fixture::new().await;
		let (lookup_started, _) = async_channel::unbounded();
		fixture
			.authority
			.object_server()
			.at(
				AUTHORITY_PATH,
				MetadataAuthority {
					lookup_started,
					stall: false,
				},
			)
			.await
			.unwrap();
		let authority = fixture.authority.clone();
		let destination = fixture.agent.unique_name().unwrap().to_string();
		let call = tokio::spawn(async move {
			let identities = vec![(
				"unix-user",
				HashMap::from([("uid", OwnedValue::from(nix::unistd::getuid().as_raw()))]),
			)];
			let details = HashMap::from([
				("command_line", "/bin/tool --token=not-for-logs"),
				("program", "/bin/tool"),
				("user", "root"),
			]);
			Proxy::new(&authority, destination, AGENT_PATH, INTERFACE)
				.await
				.unwrap()
				.call::<_, _, ()>(
					"BeginAuthentication",
					&(
						"org.subniri.test",
						"Authorize a test operation",
						"supplied-icon",
						details,
						"metadata-cookie",
						identities,
					),
				)
				.await
		});
		fixture.tasks.push(call.abort_handle());

		let Event::Started(request, commands) = fixture.events.recv().await.unwrap() else {
			panic!("expected Started")
		};
		assert_eq!(
			request.presentation.description.as_deref(),
			Some("Run a test operation")
		);
		assert_eq!(
			request.presentation.operation(&request.message),
			"/bin/tool --token=not-for-logs"
		);
		assert_eq!(request.presentation.target_user.as_deref(), Some("root"));
		assert_eq!(request.presentation.icon_name, "supplied-icon");
		assert_eq!(request.identities[0].uid, nix::unistd::getuid().as_raw());
		assert!(!format!("{request:?}").contains("not-for-logs"));
		let id = request.id;
		let (mut helper, _) = fixture.prompt(id, "metadata-cookie").await;

		commands.send(Command::Cancel).await.unwrap();

		error(call.await.unwrap(), "Cancelled");
		closed(&mut helper).await;
		fixture.finished(id).await;
		fixture.stop().await;
	})
	.await
	.expect("isolated metadata test timed out");
}

#[tokio::test]
async fn cancellation_interrupts_optional_metadata_lookup() {
	timeout(LIMIT, async {
		let mut fixture = Fixture::new().await;
		let (lookup_started, started) = async_channel::unbounded();
		fixture
			.authority
			.object_server()
			.at(
				AUTHORITY_PATH,
				MetadataAuthority {
					lookup_started,
					stall: true,
				},
			)
			.await
			.unwrap();
		let call = fixture.begin("metadata-cancel");
		started.recv().await.unwrap();

		fixture
			.cancel(&fixture.authority, "metadata-cancel")
			.await
			.unwrap();

		error(call.await.unwrap(), "Cancelled");
		assert!(matches!(
			fixture.events.recv().await.unwrap(),
			Event::Finished { .. }
		));
		assert!(fixture.events.try_recv().is_err());
		assert!(timeout(QUIET, fixture.listener.accept()).await.is_err());
		fixture.stop().await;
	})
	.await
	.expect("isolated metadata cancellation test timed out");
}
