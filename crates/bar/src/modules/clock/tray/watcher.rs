use std::{
	collections::{HashMap, HashSet},
	hash::{Hash, Hasher},
	sync::{Arc, Mutex, MutexGuard},
};

use futures::StreamExt;
use zbus::{
	Connection, Proxy,
	message::Header,
	names::{BusName, InterfaceName, OwnedBusName, UniqueName},
	object_server::SignalEmitter,
	zvariant::{ObjectPath, OwnedObjectPath},
};

use super::Message;
use super::dbusmenu::DbusMenuProxy;
use super::item::{self, ItemAddress, TrayItem};

const WATCHER_SERVICE: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_BUS_NAME: BusName<'static> = BusName::from_static_str_checked(WATCHER_SERVICE);
const WATCHER_OBJECT_PATH: ObjectPath<'static> = ObjectPath::from_static_str_checked(WATCHER_PATH);
const WATCHER_INTERFACE: InterfaceName<'static> =
	InterfaceName::from_static_str_checked("org.kde.StatusNotifierWatcher");

#[derive(Debug, Clone)]
pub enum Command {
	Activate {
		service: String,
		x: i32,
		y: i32,
	},
	ContextMenu {
		service: String,
		x: i32,
		y: i32,
	},
	ActivateWithDbusMenu {
		service: String,
		bus_name: OwnedBusName,
		path: OwnedObjectPath,
	},
	OpenDbusSubmenu {
		service: String,
		bus_name: OwnedBusName,
		path: OwnedObjectPath,
		item_id: i32,
	},
	DbusMenuEvent {
		service: String,
		bus_name: OwnedBusName,
		path: OwnedObjectPath,
		item_id: i32,
	},
}

impl Command {
	pub fn service(&self) -> &str {
		match self {
			Command::Activate { service, .. } => service,
			Command::ContextMenu { service, .. } => service,
			Command::ActivateWithDbusMenu { service, .. } => service,
			Command::OpenDbusSubmenu { service, .. } => service,
			Command::DbusMenuEvent { service, .. } => service,
		}
	}
}

#[derive(Debug, Clone)]
pub struct CommandReceiver {
	receiver: async_channel::Receiver<Command>,
	id: Arc<()>,
}

impl Hash for CommandReceiver {
	fn hash<H: Hasher>(&self, state: &mut H) {
		Arc::as_ptr(&self.id).hash(state);
	}
}

pub fn command_channel() -> (async_channel::Sender<Command>, CommandReceiver) {
	let (sender, receiver) = async_channel::bounded(16);
	(
		sender,
		CommandReceiver {
			receiver,
			id: Arc::new(()),
		},
	)
}

#[derive(Debug, Default)]
struct WatcherState {
	items: HashSet<String>,
	hosts: HashSet<String>,
}

impl WatcherState {
	fn lock(state: &Mutex<Self>) -> zbus::fdo::Result<MutexGuard<'_, Self>> {
		state
			.lock()
			.map_err(|_| zbus::fdo::Error::Failed("watcher state lock poisoned".into()))
	}

	fn remove_owner(&mut self, owner: &str) -> Vec<String> {
		let prefix = format!("{owner}/");
		self.items
			.extract_if(|item| item == owner || item.starts_with(&prefix))
			.collect()
	}
}

#[derive(Debug, Clone)]
struct InternalWatcher {
	state: Arc<Mutex<WatcherState>>,
}

#[zbus::interface(name = WATCHER_INTERFACE)]
impl InternalWatcher {
	async fn register_status_notifier_item(
		&self, #[zbus(header)] header: Header<'_>, service: String,
		#[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
	) -> zbus::fdo::Result<()> {
		let service = normalize_service(service, header.sender())?;
		let inserted = WatcherState::lock(&self.state)?
			.items
			.insert(service.clone());
		if inserted {
			Self::status_notifier_item_registered(&emitter, service).await?;
		}
		Ok(())
	}

	async fn register_status_notifier_host(
		&self, service: String, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
	) -> zbus::fdo::Result<()> {
		let inserted = WatcherState::lock(&self.state)?.hosts.insert(service);
		if inserted {
			Self::status_notifier_host_registered(&emitter).await?;
		}
		Ok(())
	}

	#[zbus(property)]
	async fn registered_status_notifier_items(&self) -> Vec<String> {
		self.state
			.lock()
			.map(|state| state.items.iter().cloned().collect())
			.unwrap_or_default()
	}

	#[zbus(property)]
	async fn is_status_notifier_host_registered(&self) -> bool {
		self.state
			.lock()
			.map(|state| !state.hosts.is_empty())
			.unwrap_or(false)
	}

	#[zbus(property)]
	fn protocol_version(&self) -> i32 {
		0
	}

	#[zbus(signal)]
	async fn status_notifier_item_registered(
		emitter: &SignalEmitter<'_>, service: String,
	) -> zbus::Result<()>;

	#[zbus(signal)]
	async fn status_notifier_item_unregistered(
		emitter: &SignalEmitter<'_>, service: String,
	) -> zbus::Result<()>;

	#[zbus(signal)]
	async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

	#[zbus(signal)]
	async fn status_notifier_host_unregistered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

fn normalize_service(
	service: String, sender: Option<&UniqueName<'_>>,
) -> zbus::fdo::Result<String> {
	if service.starts_with('/') {
		let sender = sender.ok_or_else(|| zbus::fdo::Error::Failed("missing sender".into()))?;
		Ok(format!("{sender}{service}"))
	} else if service.is_empty() {
		sender
			.map(ToString::to_string)
			.ok_or_else(|| zbus::fdo::Error::Failed("missing sender".into()))
	} else {
		Ok(service)
	}
}

pub fn events(commands: &CommandReceiver) -> impl futures::Stream<Item = Message> + use<> {
	let commands = commands.receiver.clone();
	async_stream::stream! {
		let mut events = Box::pin(try_events(commands));
		while let Some(event) = events.next().await {
			match event {
				Ok(event) => yield event,
				Err(error) => {
					log::error!("System tray watcher stopped: {error}");
					return;
				}
			}
		}
	}
}

fn try_events(
	commands: async_channel::Receiver<Command>,
) -> impl futures::Stream<Item = zbus::Result<Message>> + use<> {
	async_stream::try_stream! {
		let connection = Connection::session().await?;
		let unique_name = connection
			.unique_name()
			.ok_or_else(|| zbus::Error::Failure("missing D-Bus unique name".into()))?
			.to_string();
		let state = Arc::new(Mutex::new(WatcherState::default()));
		let (menu_event_tx, menu_events) = async_channel::unbounded();
		let mut menu_signal_task: Option<tokio::task::JoinHandle<()>> = None;
		let (item_event_tx, item_events) = async_channel::unbounded();
		let mut item_signal_tasks = HashMap::<String, tokio::task::JoinHandle<()>>::new();

		// Clients often register immediately when the well-known name gets an owner.
		connection
			.object_server()
			.at(WATCHER_OBJECT_PATH, InternalWatcher { state: state.clone() })
			.await?;
		let internal = connection.request_name(WATCHER_SERVICE).await.is_ok();

		let watcher = Proxy::new(
			&connection,
			WATCHER_BUS_NAME,
			WATCHER_OBJECT_PATH,
			WATCHER_INTERFACE,
		)
		.await?;
		watcher
			.call_method("RegisterStatusNotifierHost", &(unique_name,))
			.await?;
		log::info!(
			"System tray host started with {} watcher",
			if internal { "internal" } else { "external" }
		);

		let mut registered = watcher.receive_signal("StatusNotifierItemRegistered").await?;
		let mut unregistered = watcher.receive_signal("StatusNotifierItemUnregistered").await?;
		let dbus = Proxy::new(
			&connection,
			"org.freedesktop.DBus",
			"/org/freedesktop/DBus",
			"org.freedesktop.DBus",
		)
		.await?;
		let mut owner_changes = dbus.receive_signal("NameOwnerChanged").await?;

		let services = watcher
			.get_property::<Vec<String>>("RegisteredStatusNotifierItems")
			.await?;
		for service in services {
			state.lock().map(|mut state| state.items.insert(service.clone())).ok();
			spawn_item_watch(
				&mut item_signal_tasks,
				connection.clone(),
				service,
				item_event_tx.clone(),
			);
		}

		loop {
			tokio::select! {
				command = commands.recv() => {
					let Ok(command) = command else {
						return;
					};
					let service = command.service().to_string();
					let closes_menu_on_failure = matches!(
						command,
						Command::ActivateWithDbusMenu { .. } | Command::OpenDbusSubmenu { .. }
					);
					let menu_signals = match &command {
						Command::ActivateWithDbusMenu { bus_name, path, .. } => {
							Some((bus_name.clone(), path.clone()))
						}
						_ => None,
					};
					match handle_command(&connection, command).await {
						Ok(msg) => {
							if let Some((bus_name, path)) = menu_signals {
								if let Some(task) = menu_signal_task.take() {
									task.abort();
								}
								menu_signal_task = Some(tokio::spawn(watch_dbusmenu(
									connection.clone(),
									service.clone(),
									bus_name,
									path,
									menu_event_tx.clone(),
								)));
							}
							if let Some(msg) = msg {
								yield msg;
							}
						}
						Err(error) => {
							log::warn!("Failed to invoke tray item action on {service}: {error}");
							if closes_menu_on_failure {
								yield Message::DbusMenuFailed { service };
							}
						}
					}
				}
				menu_event = menu_events.recv() => {
					if let Ok(menu_event) = menu_event {
						yield menu_event;
					}
				}
				item_event = item_events.recv() => {
					if let Ok(item_event) = item_event {
						let is_active = match &item_event {
							Message::Registered(service, _) => state
								.lock()
								.is_ok_and(|state| state.items.contains(service)),
							_ => false,
						};
						if is_active {
							yield item_event;
						}
					}
				}
				Some(signal) = registered.next() => {
					if let Ok(service) = signal.body().deserialize::<String>() {
						state.lock().map(|mut state| state.items.insert(service.clone())).ok();
						spawn_item_watch(
							&mut item_signal_tasks,
							connection.clone(),
							service,
							item_event_tx.clone(),
						);
					}
				}
				Some(signal) = unregistered.next() => {
					if let Ok(service) = signal.body().deserialize::<String>() {
						state.lock().map(|mut state| state.items.remove(&service)).ok();
						if let Some(task) = item_signal_tasks.remove(&service) {
							task.abort();
						}
						yield Message::Removed(service);
					}
				}
				Some(signal) = owner_changes.next() => {
					if let Ok((name, old_owner, new_owner)) = signal.body().deserialize::<(String, String, String)>()
						&& !old_owner.is_empty()
						&& new_owner.is_empty()
					{
						let removed = state
							.lock()
							.map(|mut state| state.remove_owner(&name))
							.unwrap_or_default();
						for service in removed {
							if let Some(task) = item_signal_tasks.remove(&service) {
								task.abort();
							}
							yield Message::Removed(service);
						}
					}
				}
				else => return,
			}
		}
	}
}

fn spawn_item_watch(
	tasks: &mut HashMap<String, tokio::task::JoinHandle<()>>, connection: Connection,
	service: String, events: async_channel::Sender<Message>,
) {
	if let Some(task) = tasks.remove(&service) {
		task.abort();
	}
	let task_service = service.clone();
	tasks.insert(
		service,
		tokio::spawn(async move {
			watch_item(connection, task_service, events).await;
		}),
	);
}

async fn watch_item(
	connection: Connection, service: String, events: async_channel::Sender<Message>,
) {
	let address = match ItemAddress::parse(&service) {
		Ok(address) => address,
		Err(error) => {
			log::warn!("Failed to parse tray item address {service}: {error}");
			return;
		}
	};
	let proxy = match Proxy::new_owned(
		connection.clone(),
		address.bus_name,
		address.object_path,
		item::ITEM_INTERFACE.to_string(),
	)
	.await
	{
		Ok(proxy) => proxy,
		Err(error) => {
			log::warn!("Failed to create tray item watcher for {service}: {error}");
			return;
		}
	};
	let mut signals = match proxy.receive_all_signals().await {
		Ok(signals) => signals,
		Err(error) => {
			log::warn!("Failed to watch tray item {service}: {error}");
			if let Some(item) = read_item(&connection, service).await {
				let _ = events.send(item).await;
			}
			return;
		}
	};

	if let Some(item) = read_item(&connection, service.clone()).await
		&& events.send(item).await.is_err()
	{
		return;
	}

	while let Some(signal) = signals.next().await {
		let header = signal.header();
		let Some(member) = header.member().map(|member| member.as_str()) else {
			continue;
		};
		if !refreshes_tray_item(member) {
			continue;
		}
		if let Some(item) = read_item(&connection, service.clone()).await
			&& events.send(item).await.is_err()
		{
			return;
		}
	}
}

fn refreshes_tray_item(member: &str) -> bool {
	matches!(
		member,
		"NewTitle"
			| "NewIcon"
			| "NewAttentionIcon"
			| "NewOverlayIcon"
			| "NewToolTip"
			| "NewStatus"
			| "NewMenu"
			| "NewIconThemePath"
	)
}

async fn watch_dbusmenu(
	connection: Connection, service: String, bus_name: OwnedBusName, path: OwnedObjectPath,
	events: async_channel::Sender<Message>,
) {
	let proxy = match DbusMenuProxy::builder(&connection)
		.destination(bus_name)
		.and_then(|builder| builder.path(path))
	{
		Ok(builder) => match builder.build().await {
			Ok(proxy) => proxy,
			Err(error) => {
				log::warn!("Failed to watch DBusMenu for {service}: {error}");
				return;
			}
		},
		Err(error) => {
			log::warn!("Failed to create DBusMenu watcher for {service}: {error}");
			return;
		}
	};
	let mut layout_updates = match proxy.receive_layout_updated().await {
		Ok(updates) => updates,
		Err(error) => {
			log::warn!("Failed to watch DBusMenu layout for {service}: {error}");
			return;
		}
	};
	let mut property_updates = match proxy.receive_items_properties_updated().await {
		Ok(updates) => updates,
		Err(error) => {
			log::warn!("Failed to watch DBusMenu properties for {service}: {error}");
			return;
		}
	};

	loop {
		tokio::select! {
			Some(signal) = layout_updates.next() => {
				if let Ok(args) = signal.args()
					&& let Ok((revision, layout)) = proxy.get_layout(args.parent, -1, vec![]).await
					&& events.send(Message::ShowDbusSubmenu {
						service: service.clone(),
						parent_id: args.parent,
						revision,
						layout,
					}).await.is_err()
				{
					return;
				}
			}
			Some(signal) = property_updates.next() => {
				if let Ok(args) = signal.args()
					&& events.send(Message::UpdateDbusMenuProperties {
						service: service.clone(),
						updated: args.updated_props,
						removed: args.removed_props,
					}).await.is_err()
				{
					return;
				}
			}
			else => return,
		}
	}
}

async fn handle_command(
	connection: &Connection, command: Command,
) -> zbus::Result<Option<Message>> {
	match command {
		Command::Activate { service, x, y } => {
			item::activate(connection, &service, x, y).await?;
			Ok(None)
		}
		Command::ContextMenu { service, x, y } => {
			item::context_menu(connection, &service, x, y).await?;
			Ok(None)
		}
		Command::ActivateWithDbusMenu {
			service,
			bus_name,
			path,
		} => {
			let (revision, layout) = item::dbusmenu(connection, bus_name, path).await?;
			Ok(Some(Message::ShowDbusMenu {
				service,
				revision,
				layout,
			}))
		}
		Command::OpenDbusSubmenu {
			service,
			bus_name,
			path,
			item_id,
		} => {
			let (revision, layout) =
				item::dbusmenu_submenu(connection, bus_name, path, item_id).await?;
			Ok(Some(Message::ShowDbusSubmenu {
				service,
				parent_id: item_id,
				revision,
				layout,
			}))
		}
		Command::DbusMenuEvent {
			service: _,
			bus_name,
			path,
			item_id,
		} => {
			item::dbusmenu_clicked(connection, bus_name, path, item_id).await?;
			Ok(None)
		}
	}
}

async fn read_item(connection: &Connection, service: String) -> Option<Message> {
	match TrayItem::read(connection, &service).await {
		Ok(item) => Some(Message::Registered(service, item)),
		Err(error) => {
			log::warn!("Failed to read tray item {service}: {error}");
			None
		}
	}
}

#[cfg(test)]
mod tests {
	use super::refreshes_tray_item;

	#[test]
	fn standard_item_change_signals_refresh_the_snapshot() {
		for member in [
			"NewTitle",
			"NewIcon",
			"NewAttentionIcon",
			"NewOverlayIcon",
			"NewToolTip",
			"NewStatus",
			"NewMenu",
			"NewIconThemePath",
		] {
			assert!(refreshes_tray_item(member), "ignored {member}");
		}
		assert!(!refreshes_tray_item("Activate"));
	}
}
