use std::{hash::Hash, pin::pin, time::Duration};

use bluer::{Adapter, AdapterEvent, AdapterProperty, Address, ErrorKind, Session};
use futures::{Stream, StreamExt};
use iced::{
	Element, Font, Length, Subscription,
	advanced::graphics::futures::MaybeSend,
	alignment::Vertical,
	font,
	widget::{column, row, space, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::Colors,
	widgets::{NeoButton, neo_button, neo_card, neo_scrollable},
};

use crate::modules::{ICON_HEIGHT, MODULE_HEIGHT, MODULE_RADIUS};
use utilities::Hashable;

pub(super) mod integration;

#[derive(Debug, Clone)]
pub enum Message {
	DeviceConnected(BtDevice),
	DevicesUpdated(Vec<BtDevice>),
	DeviceDisconnected(Address),
	AdapterFound,
	AdapterLost,
	Power(bool),
	Disconnected(String),
}

#[derive(Debug, Clone)]
pub struct Bluetooth {
	data: BluetoothData,
	devices: Vec<BtDevice>,
	has_adapter: bool,
	powered_on: bool,
}

impl Bluetooth {
	pub async fn new() -> Result<Self, String> {
		let watcher = zbus::Connection::system()
			.await
			.map_err(|error| error.to_string())?;
		let owner = bluez_owner(&watcher).await?;
		let session = Session::new().await.map_err(|error| error.to_string())?;
		let adapter = match session.default_adapter().await {
			Ok(a) => Some(a),
			Err(bluer::Error {
				kind: ErrorKind::NotFound,
				..
			}) => None,
			Err(error) => return Err(error.to_string()),
		};

		let powered_on = if let Some(adapter) = &adapter {
			adapter
				.is_powered()
				.await
				.map_err(|error| error.to_string())?
		} else {
			false
		};

		let devices = vec![];

		Ok(Self {
			powered_on,
			has_adapter: adapter.is_some(),
			devices,
			data: BluetoothData {
				watcher: Hashable::new(watcher),
				owner,
				session,
				default_adapter: adapter,
			},
		})
	}

	pub fn subscription(&self) -> Subscription<Message> {
		Subscription::run_with(self.data.clone(), |data| stream(data.clone()))
	}

	pub fn connection(&self) -> Hashable<zbus::Connection> {
		self.data.watcher.clone()
	}

	pub fn update(&mut self, message: Message) {
		match message {
			Message::DeviceConnected(device) => {
				self.devices.retain(|existing| existing.addr != device.addr);
				self.devices.push(device);
			}
			Message::DevicesUpdated(devices) => self.devices = devices,
			Message::DeviceDisconnected(addr) => self.devices.retain(|d| d.addr != addr),
			Message::AdapterFound => {
				log::trace!("Adapter found");
				self.has_adapter = true;
			}
			Message::AdapterLost => {
				log::trace!("Adapter lost");
				self.has_adapter = false;
				self.devices.clear();
			}
			Message::Power(is_on) => {
				println!("Power state changed {is_on}");
				self.powered_on = is_on;
			}
			Message::Disconnected(_) => {} // Handled by the session owner.
		}
	}

	pub fn view(&self, colors: Colors) -> NeoButton<'_, Message> {
		let icon = if self.has_adapter && self.powered_on {
			phosphor_icon!("bluetooth", "bold")
		} else {
			phosphor_icon!("bluetooth-slash", "bold")
		};

		neo_button(
			row![
				svg(icon)
					.width(Length::Shrink)
					.height(ICON_HEIGHT)
					.style(move |_, _| svg::Style {
						color: Some(colors.on_accent),
					}),
				super::module_label(
					text(format!(
						"{}",
						self.devices
							.iter()
							.filter(|device| device.connected)
							.count()
					))
					.font(Font {
						weight: font::Weight::Bold,
						..Font::DEFAULT
					})
					.color(colors.on_accent)
					.size(18)
					.align_y(Vertical::Center)
				)
			]
			.spacing(5.)
			.align_y(Vertical::Center),
		)
		.with_colors(colors)
		.height(MODULE_HEIGHT)
		.radius(MODULE_RADIUS)
		.background(colors.decorative.blue)
	}

	pub fn view_popup(&self, colors: Colors) -> Element<'_, Message> {
		let status = if !self.has_adapter {
			"No adapter"
		} else if self.powered_on {
			"On"
		} else {
			"Off"
		};
		let header = row![
			svg(phosphor_icon!("bluetooth", "bold"))
				.width(24)
				.height(24)
				.style(move |_, _| svg::Style {
					color: Some(colors.text)
				}),
			text("Bluetooth")
				.size(20)
				.color(colors.text)
				.weight(font::Weight::Bold),
			space::horizontal(),
			text(status).size(12).color(colors.secondary),
		]
		.spacing(12)
		.align_y(Vertical::Center);

		let mut devices = column![].spacing(8);

		for device in &self.devices {
			let content = neo_card(
				row![
					svg(phosphor_icon!("bluetooth", "bold"))
						.width(20)
						.height(20)
						.style(move |_, _| svg::Style {
							color: Some(colors.text)
						}),
					column![
						text(device.name.as_deref().unwrap_or("Unnamed device"))
							.color(colors.text)
							.size(16)
							.wrapping(text::Wrapping::None)
							.ellipsis(text::Ellipsis::End),
						text(if device.connected {
							"Connected"
						} else {
							"Paired"
						})
						.color(colors.secondary)
						.size(12),
					]
					.spacing(4)
					.width(Length::Fill),
				]
				.spacing(12)
				.align_y(Vertical::Center),
			)
			.with_colors(colors)
			.background(if device.connected {
				colors.selection
			} else {
				colors.surface
			})
			.width(Length::Fill);

			devices = devices.push(content);
		}

		if !self.has_adapter {
			devices = devices.push(
				text("No Bluetooth adapter available")
					.color(colors.secondary)
					.size(14),
			);
		} else if !self.powered_on {
			devices = devices.push(
				text("Bluetooth is turned off")
					.color(colors.secondary)
					.size(14),
			);
		} else if self.devices.is_empty() {
			devices = devices.push(text("No paired devices").color(colors.secondary).size(14));
		}

		neo_card(
			column![
				header,
				neo_scrollable(devices)
					.with_colors(colors)
					.height(Length::Shrink.max(300)),
			]
			.spacing(16),
		)
		.with_colors(colors)
		.width(320)
		.radius(MODULE_RADIUS)
		.background(colors.body)
		.into()
	}
}

async fn bluez_owner(connection: &zbus::Connection) -> Result<String, String> {
	let proxy = zbus::fdo::DBusProxy::new(connection)
		.await
		.map_err(|error| error.to_string())?;
	let name = zbus::names::BusName::try_from("org.bluez").map_err(|error| error.to_string())?;
	proxy
		.get_name_owner(name)
		.await
		.map(|owner| owner.to_string())
		.map_err(|error| error.to_string())
}

fn stream(data: BluetoothData) -> impl Stream<Item = Message> + MaybeSend + 'static {
	async_stream::stream! {
		let watcher = data.watcher.clone();
		let proxy = match zbus::fdo::DBusProxy::new(&watcher).await {
			Ok(proxy) => proxy,
			Err(error) => { yield Message::Disconnected(error.to_string()); return; }
		};
		let mut owners = match tokio::time::timeout(Duration::from_secs(5), proxy.receive_name_owner_changed()).await {
			Ok(Ok(owners)) => owners,
			Ok(Err(error)) => { yield Message::Disconnected(error.to_string()); return; }
			Err(_) => { yield Message::Disconnected("Bluetooth owner observation timed out".into()); return; }
		};
		// Subscribe before checking identity so replacement during setup is not lost.
		match tokio::time::timeout(Duration::from_secs(5), bluez_owner(&watcher)).await {
			Ok(Ok(owner)) if owner == data.owner => {}
			_ => { yield Message::Disconnected("BlueZ owner changed during initialization".into()); return; }
		}
		let mut events = Box::pin(device_events(data));
		loop {
			tokio::select! {
				() = watcher.closed() => { yield Message::Disconnected("Bluetooth bus disconnected".into()); return; }
				owner = owners.next() => {
					match owner {
						Some(owner) if owner.args().is_ok_and(|args| args.name().as_str() != "org.bluez") => {}
						_ => { yield Message::Disconnected("BlueZ owner observation ended or changed".into()); return; }
					}
				}
				event = events.next() => {
					if let Some(event) = event {
						let terminal = matches!(event, Message::Disconnected(_));
						yield event;
						if terminal { return; }
					} else {
						yield Message::Disconnected("Bluetooth observation ended".into());
						return;
					}
				}
			}
		}
	}
}

fn device_events(data: BluetoothData) -> impl Stream<Item = Message> + MaybeSend + 'static {
	let BluetoothData {
		session,
		default_adapter,
		..
	} = data;

	let session_stream = async_stream::stream! {
		let session_events = match session.events().await {
			Ok(events) => events,
			Err(error) => {
				log::warn!("Failed to subscribe to Bluetooth session events: {error}");
				return;
			}
		};
		let mut session_stream = pin!(session_events);

		while let Some(_ev) = session_stream.next().await {
			let adapter = match get_default_adapter(&session).await {
				Ok(adapter) => adapter,
				Err(error) => {
					log::warn!("Failed to get default Bluetooth adapter: {error}");
					None
				}
			};
			yield adapter;
		}
	};

	let device_stream = async_stream::stream! {
		let mut session_stream = pin!(session_stream);
		let mut maybe_adapter = Some(default_adapter);

		while let Some(ref ma) = maybe_adapter {
			let adapter = if let Some(adapter) = ma {
					   yield Message::AdapterFound;
					   adapter
				   } else {
					   yield  Message::AdapterLost;
					   maybe_adapter = session_stream.next().await;
					   continue;
				   };

			let adapter_events = match adapter.events().await {
				Ok(events) => events,
				Err(error) => {
					log::warn!("Failed to subscribe to Bluetooth adapter events: {error}");
					yield Message::Disconnected(error.to_string());
					return;
				}
			};
			let mut adapter_stream = pin!(adapter_events);
			let mut refresh = tokio::time::interval(Duration::from_secs(3));
			refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

			loop {
				tokio::select! {
					_ = refresh.tick() => {
						match tokio::time::timeout(Duration::from_secs(2), device_snapshot(adapter)).await {
							Ok(Ok(devices)) => yield Message::DevicesUpdated(devices),
							Ok(Err(error)) => log::warn!("Failed to refresh Bluetooth devices: {error}"),
							Err(_) => log::warn!("Bluetooth device refresh timed out"),
						}
					}
					new_maybe_adapter = session_stream.next() => {
						maybe_adapter = new_maybe_adapter;
						break;
					}
					event = adapter_stream.next() => {
						match event {
							Some(AdapterEvent::DeviceAdded(addr)) => {
								let device = match adapter.device(addr) {
									Ok(d) => d,
									Err(e) => {
										log::warn!("Device not found... {e}");
										continue;
									}
								};

								let name = match device.name().await {
									Ok(name) => name,
									Err(e) => {
										log::warn!("Cannot get bt device name: {e}");
										continue;
									}
								};
								let is_connected = device.is_connected().await.unwrap_or_default();
								let is_paired = device.is_paired().await.unwrap_or_default();
								if !is_connected && !is_paired {
									continue;
								}

								let bt_device = BtDevice {
									addr,
									name,
									connected: is_connected,
								};

								yield Message::DeviceConnected(bt_device);
							},
							Some(AdapterEvent::DeviceRemoved(addr)) => yield Message::DeviceDisconnected(addr),
							Some(AdapterEvent::PropertyChanged(AdapterProperty::Powered(powered))) => yield Message::Power(powered),
							Some(_) => (),
							None => { yield Message::Disconnected("Bluetooth adapter observation ended".into()); return; },
						}
					}
				}
			}

		}
	};

	Box::pin(device_stream)
}

async fn get_default_adapter(session: &Session) -> bluer::Result<Option<Adapter>> {
	match session.default_adapter().await {
		Ok(a) => Ok(Some(a)),
		Err(bluer::Error {
			kind: ErrorKind::NotFound,
			..
		}) => Ok(None),
		Err(error) => Err(error),
	}
}

async fn device_snapshot(adapter: &Adapter) -> bluer::Result<Vec<BtDevice>> {
	let mut devices = Vec::new();
	for addr in adapter.device_addresses().await? {
		let device = adapter.device(addr)?;
		let connected = device.is_connected().await?;
		if connected || device.is_paired().await? {
			devices.push(BtDevice {
				addr,
				name: device.name().await?,
				connected,
			});
		}
	}
	devices.sort_by(|left, right| {
		right
			.connected
			.cmp(&left.connected)
			.then_with(|| left.name.cmp(&right.name))
	});

	Ok(devices)
}

#[derive(Clone, Debug)]
struct BluetoothData {
	watcher: Hashable<zbus::Connection>,
	owner: String,
	session: Session,
	default_adapter: Option<Adapter>,
}

impl PartialEq for BluetoothData {
	fn eq(&self, other: &Self) -> bool {
		self.watcher.as_ptr() == other.watcher.as_ptr()
	}
}

impl Hash for BluetoothData {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		self.watcher.as_ptr().hash(state);
	}
}

#[derive(Clone, Debug)]
pub(crate) struct BtDevice {
	addr: Address,
	name: Option<String>,
	connected: bool,
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::modules::test_bus;
	use std::time::Duration;

	struct TestAdapter;
	#[zbus::interface(name = "org.bluez.Adapter1")]
	impl TestAdapter {
		#[zbus(property)]
		fn powered(&self) -> bool {
			true
		}
	}

	async fn bluez() -> Result<zbus::Connection, test_bus::Error> {
		Ok(zbus::connection::Builder::session()?
			.name("org.bluez")?
			.serve_at("/", zbus::fdo::ObjectManager)?
			.serve_at("/org/bluez/hci0", TestAdapter)?
			.build()
			.await?)
	}

	#[test]
	#[ignore = "requires a private D-Bus and headless graphics renderer"]
	fn bluetooth_popup_layout() -> Result<(), test_bus::Error> {
		test_bus::run("modules::bluetooth::tests::popup_layout_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn popup_layout_child() -> Result<(), test_bus::Error> {
		use crate::modules::layout_tests;

		test_bus::require_child()?;
		let _service = bluez().await?;
		let mut bluetooth = Bluetooth::new().await?;
		let mut renderer = layout_tests::renderer().await?;
		for (theme, colors) in [
			("light", neo_widgets::style::LIGHT),
			("dark", neo_widgets::style::DARK),
		] {
			for (state, adapter, powered, count) in [
				("empty", true, true, 0),
				("off", true, false, 0),
				("no-adapter", false, false, 0),
				("devices", true, true, 12),
			] {
				bluetooth.has_adapter = adapter;
				bluetooth.powered_on = powered;
				bluetooth.devices = (0..count)
					.map(|index| BtDevice {
						addr: Address::new([0, 0, 0, 0, 0, index]),
						name: Some(if index == 0 {
							"Headphones with a deliberately long Bluetooth device name".into()
						} else {
							format!("Device {index}")
						}),
						connected: index == 0,
					})
					.collect();

				let popup = layout_tests::capture(
					bluetooth.view_popup(colors),
					&mut renderer,
					colors,
					&format!("bluetooth-{state}-{theme}"),
				)?;
				let bounds = popup.nodes.first().ok_or("missing popup")?;
				assert_eq!(bounds.width, 320.0);
				assert!(bounds.height <= 380.0);
				assert!(popup.texts.contains_key("Bluetooth"));
				let button = layout_tests::capture(
					bluetooth.view(colors).into(),
					&mut renderer,
					colors,
					&format!("bar-bluetooth-{state}-{theme}"),
				)?;
				assert_eq!(
					button.nodes.first().ok_or("missing button")?.height,
					MODULE_HEIGHT
				);
				for bounds in button.texts.values() {
					assert!((bounds.center_y() - 23.0).abs() < 0.01);
				}
			}
		}
		Ok(())
	}

	#[test]
	#[ignore = "requires dbus-daemon; uses only a private bus"]
	fn bluetooth_owner_loss_reaches_module() -> Result<(), test_bus::Error> {
		test_bus::run("modules::bluetooth::tests::owner_loss_child")
	}

	#[tokio::test]
	#[ignore = "private-bus subprocess helper"]
	async fn owner_loss_child() -> Result<(), test_bus::Error> {
		use crate::modules::{Module, ModuleMessage};
		let session = |connection, message| {
			ModuleMessage::Bluetooth(integration::Message::Session(connection, message))
		};
		let ready =
			|module: &Module| matches!(module, Module::Bluetooth(bluetooth) if bluetooth.ready());
		test_bus::require_child()?;
		let daemon = bluez().await?;
		let bluetooth = Bluetooth::new().await?;
		let old_connection = bluetooth.connection();
		let mut events = Box::pin(stream(bluetooth.data.clone()));
		assert!(matches!(
			tokio::time::timeout(Duration::from_secs(2), events.next()).await?,
			Some(Message::AdapterFound)
		));
		assert!(
			tokio::time::timeout(Duration::from_millis(100), events.next())
				.await
				.is_err()
		);
		daemon.close().await?;
		let lost = tokio::time::timeout(Duration::from_secs(2), events.next()).await?;
		assert!(
			matches!(lost, Some(Message::Disconnected(_))),
			"owner loss must reach the lifecycle owner"
		);
		drop(events);
		let mut module = Module::Bluetooth(integration::Integration::connected(bluetooth));
		let task = module.update(session(
			old_connection.clone(),
			lost.ok_or("missing terminal event")?,
		));
		assert!(task.units() > 0);
		assert_eq!(
			module
				.update(session(
					old_connection.clone(),
					Message::Disconnected("duplicate".into())
				))
				.units(),
			0
		);
		let _replacement = bluez().await?;
		let message =
			tokio::time::timeout(Duration::from_secs(3), test_bus::task_message(task)).await??;
		let _ = module.update(message);
		assert!(ready(&module));
		assert_eq!(
			module
				.update(session(
					old_connection,
					Message::Disconnected("stale".into())
				))
				.units(),
			0
		);
		assert!(ready(&module));
		Ok(())
	}
}
