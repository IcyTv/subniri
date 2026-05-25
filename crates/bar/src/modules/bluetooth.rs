use std::{hash::Hash, pin::pin};

use bluer::{Adapter, AdapterEvent, AdapterProperty, ErrorKind, Session};
use futures::{Stream, StreamExt};
use iced::{
	Font, Length, Subscription,
	advanced::graphics::futures::MaybeSend,
	alignment::Vertical,
	font,
	widget::{row, svg, text},
};

use crate::{
	modules::{ICON_HEIGHT, MODULE_HEIGHT, MODULE_RADIUS},
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button},
};

#[derive(Debug, Clone)]
pub enum Message {
	DeviceConnected,
	DeviceDisconnected,
	AdapterFound,
	AdapterLost,
	Power(bool),
}

#[derive(Debug, Clone)]
pub struct Bluetooth {
	data: BluetoothData,
	device_count: usize,
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

		let device_count = if let Some(adapter) = &adapter {
			let mut connected = 0;
			for addr in adapter
				.device_addresses()
				.await
				.map_err(|e| e.to_string())?
			{
				let device = adapter.device(addr).map_err(|e| e.to_string())?;

				if device.is_connected().await.map_err(|e| e.to_string())? {
					connected += 1;
				}
			}

			connected
		} else {
			0
		};

		Ok(Self {
			powered_on,
			has_adapter: adapter.is_some(),
			device_count,
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
			Message::DeviceConnected => self.device_count += 1,
			Message::DeviceDisconnected => self.device_count = self.device_count.saturating_sub(1),
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
				text(format!("{}", self.device_count))
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
		.into()
	}
}

fn stream(data: BluetoothData) -> impl Stream<Item = Message> + MaybeSend + 'static {
	let BluetoothData {
		session,
		default_adapter,
	} = data;

	let session_stream = async_stream::stream! {
		let mut session_stream = pin!(session.events().await.unwrap());

		while let Some(_ev) = session_stream.next().await {
			let adapter = get_default_adapter(&session).await.unwrap();
			yield adapter;
		}
	};

	let device_stream = async_stream::stream! {
		let mut session_stream = pin!(session_stream);
		let mut maybe_adapter = Some(default_adapter);

		while let Some(ref ma) = maybe_adapter {
			let adapter = match ma {
				Some(adapter) => {
					yield Message::AdapterFound;
					adapter
				}
				None => {
					yield  Message::AdapterLost;
					maybe_adapter = session_stream.next().await;
					continue;
				}
			};

			let mut adapter_stream = pin!(adapter.events().await.unwrap());

			loop {
				tokio::select! {
					new_maybe_adapter = session_stream.next() => {
						maybe_adapter = new_maybe_adapter;
						break;
					}
					event = adapter_stream.next() => {
						match event {
							Some(AdapterEvent::DeviceAdded(_)) => yield Message::DeviceConnected,
							Some(AdapterEvent::DeviceRemoved(_)) => yield Message::DeviceDisconnected,
							Some(AdapterEvent::PropertyChanged(AdapterProperty::Powered(powered))) => yield Message::Power(powered),
							Some(_) => (),
							None => break,
						};
					}
				}
			}

		}
	};

	Box::pin(device_stream)
}

async fn get_default_adapter(session: &Session) -> bluer::Result<Option<Adapter>> {
	match session.default_adapter().await {
		Ok(a) => return Ok(Some(a)),
		Err(bluer::Error {
			kind: ErrorKind::NotFound,
			..
		}) => return Ok(None),
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
			.map(|a| a.name())
			.eq(&other.default_adapter.as_ref().map(|a| a.name()))
	}
}

impl Hash for BluetoothData {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		if let Some(adapter) = &self.default_adapter {
			adapter.name().hash(state);
		}
	}
}
