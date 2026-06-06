use std::{hash::Hash, pin::pin};

use bluer::{Adapter, AdapterEvent, AdapterProperty, Address, ErrorKind, Session};
use futures::{Stream, StreamExt};
use iced::{
	Element, Font, Length, Subscription,
	advanced::graphics::futures::MaybeSend,
	alignment::Vertical,
	font,
	widget::{column, row, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button, neo_card},
};

use crate::modules::{ICON_HEIGHT, MODULE_HEIGHT, MODULE_RADIUS};

#[derive(Debug, Clone)]
pub enum Message {
	DeviceConnected(BtDevice),
	DeviceDisconnected(Address),
	AdapterFound,
	AdapterLost,
	Power(bool),
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

		// let device_count = if let Some(adapter) = &adapter {
		// 	let mut connected = 0;
		// 	for addr in adapter
		// 		.device_addresses()
		// 		.await
		// 		.map_err(|e| e.to_string())?
		// 	{
		// 		let device = adapter.device(addr).map_err(|e| e.to_string())?;
		//
		// 		if device.is_connected().await.map_err(|e| e.to_string())? {
		// 			connected += 1;
		// 		}
		// 	}
		//
		// 	connected
		// } else {
		// 	0
		// };

		let devices = vec![];

		Ok(Self {
			powered_on,
			has_adapter: adapter.is_some(),
			devices,
			data: BluetoothData {
				session,
				default_adapter: adapter,
			},
		})
	}

	pub fn subscription(&self) -> Subscription<Message> {
		Subscription::run_with(self.data.clone(), |data| stream(data.clone()))
	}

	pub fn update(&mut self, message: Message) {
		match message {
			Message::DeviceConnected(device) => self.devices.push(device),
			Message::DeviceDisconnected(addr) => self.devices.retain(|d| d.addr != addr),
			Message::AdapterFound => {
				log::trace!("Adapter found");
				self.has_adapter = true;
			}
			Message::AdapterLost => {
				log::trace!("Adapter lost");
				self.has_adapter = false;
			}
			Message::Power(is_on) => {
				println!("Power state changed {is_on}");
				self.powered_on = is_on;
			}
		}
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		let icon = if self.has_adapter && self.powered_on {
			phosphor_icon!("bluetooth", "bold")
		} else {
			phosphor_icon!("bluetooth-slash", "bold")
		};

		neo_button(
			row![
				svg(icon).width(Length::Shrink).height(ICON_HEIGHT),
				text(format!("{}", self.devices.len()))
					.font(Font {
						weight: font::Weight::Bold,
						..Font::DEFAULT
					})
					.color(COLORS.text)
					.size(18)
					.align_y(Vertical::Center)
			]
			.spacing(5.)
			.align_y(Vertical::Center),
		)
		.height(MODULE_HEIGHT)
		.radius(MODULE_RADIUS)
		.background(COLORS.decorative.blue)
	}

	pub fn view_popup(&self) -> Element<'_, Message> {
		let mut col = column![];

		for device in &self.devices {
			let content = neo_button(text(device.name.as_deref().unwrap_or("<No name>")));

			col = col.push(content);
		}

		if !self.powered_on || !self.has_adapter {
			col = col.push(text("No adapter"));
		}

		neo_card(col)
			.width(200)
			.background(COLORS.decorative.blue)
			.into()
	}
}

fn stream(data: BluetoothData) -> impl Stream<Item = Message> + MaybeSend + 'static {
	let BluetoothData {
		session,
		default_adapter,
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
					maybe_adapter = session_stream.next().await;
					continue;
				}
			};
			let mut adapter_stream = pin!(adapter_events);

			loop {
				tokio::select! {
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
								if !is_connected {
									continue;
								}

								let bt_device = BtDevice {
									addr,
									name,
								};

								yield Message::DeviceConnected(bt_device);
							},
							Some(AdapterEvent::DeviceRemoved(addr)) => yield Message::DeviceDisconnected(addr),
							Some(AdapterEvent::PropertyChanged(AdapterProperty::Powered(powered))) => yield Message::Power(powered),
							Some(_) => (),
							None => break,
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

#[derive(Clone, Debug)]
struct BluetoothData {
	session: Session,
	default_adapter: Option<Adapter>,
}

impl PartialEq for BluetoothData {
	fn eq(&self, other: &Self) -> bool {
		self.default_adapter
			.as_ref()
			.map(bluer::Adapter::name)
			.eq(&other.default_adapter.as_ref().map(bluer::Adapter::name))
	}
}

impl Hash for BluetoothData {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		if let Some(adapter) = &self.default_adapter {
			adapter.name().hash(state);
		}
	}
}

#[derive(Clone, Debug)]
pub(crate) struct BtDevice {
	addr: Address,
	name: Option<String>,
}
