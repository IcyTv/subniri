use std::time::Duration;

use daemon_common::{
	BATTERY_INTERFACE, BATTERY_OBJECT_PATH, BatteryProxy, BatteryState, DEFAULT_BUS_NAME,
};
use futures::{SinkExt, StreamExt};
use iced::{
	Alignment, Length, Size, Subscription,
	widget::{column, row, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::Colors,
	widgets::{NeoButton, neo_button},
};

#[derive(Debug, Clone, Copy)]
pub enum Message {
	State(BatteryState),
	Disconnected,
}

#[derive(Debug, Default, Clone)]
pub struct Battery {
	state: BatteryState,
}

impl Battery {
	pub fn visible(&self) -> bool {
		self.state.present
	}

	pub fn update(&mut self, message: Message) {
		self.state = match message {
			Message::State(state) => state,
			Message::Disconnected => BatteryState::default(),
		};
	}

	pub fn subscription() -> Subscription<Message> {
		Subscription::run(|| {
			iced::stream::channel(16, |mut output| async move {
				let mut backoff = Duration::from_secs(1);
				loop {
					if let Err(error) = observe(&mut output, &mut backoff).await {
						log::debug!("Battery connection unavailable: {error}");
					}
					if output.send(Message::Disconnected).await.is_err() {
						return;
					}
					tokio::time::sleep(backoff).await;
					backoff = (backoff * 2).min(Duration::from_secs(30));
				}
			})
		})
	}

	pub fn view_tile(&self, size: Size, colors: Colors) -> NeoButton<'_, super::Message> {
		let state = self.state;
		let status = if !state.present {
			"Unavailable"
		} else {
			status(state.state)
		};
		let icon = if state.state == 1 {
			phosphor_icon!("battery-charging")
		} else {
			phosphor_icon!("battery-full")
		};
		let percentage = if state.present {
			format!("{:.0}%", state.percentage)
		} else {
			"Battery".into()
		};
		let content = row![
			svg(icon)
				.width(28)
				.height(28)
				.style(move |_, _| svg::Style {
					color: Some(colors.text)
				}),
			column![
				text(percentage).size(20).color(colors.text),
				text(status).size(12).color(colors.text),
			]
			.spacing(2),
		]
		.spacing(8)
		.align_y(Alignment::Center);
		let inset = neo_widgets::widgets::NeoContentSurfaceStyle::with_colors(colors).inset();

		// No press or context-menu handlers: battery is informational, never an action.
		neo_button(neo_widgets::widgets::scale_to_fit(
			content,
			Size::new(
				(size.width - inset.width).max(1.0),
				(size.height - inset.height).max(1.0),
			),
		))
		.with_colors(colors)
		.background(colors.surface)
		.width(Length::Fill)
		.height(Length::Fill)
	}
}

fn status(state: u32) -> &'static str {
	match state {
		1 => "Charging",
		2 => "Discharging",
		3 => "Empty",
		4 => "Full",
		5 => "Pending charge",
		6 => "Pending discharge",
		_ => "Unknown",
	}
}

async fn observe(
	output: &mut futures::channel::mpsc::Sender<Message>, backoff: &mut Duration,
) -> zbus::Result<()> {
	let connection = zbus::Connection::session().await?;
	let proxy = BatteryProxy::builder(&connection)
		.cache_properties(zbus::proxy::CacheProperties::No)
		.build()
		.await?;
	let mut owners = proxy.inner().receive_owner_changed().await?;
	let bus = zbus::fdo::DBusProxy::new(&connection).await?;
	let owner = bus.get_name_owner(DEFAULT_BUS_NAME).await?;
	let proxy = BatteryProxy::builder(&connection)
		.destination(owner.as_str())?
		.cache_properties(zbus::proxy::CacheProperties::No)
		.build()
		.await?;
	let properties = zbus::fdo::PropertiesProxy::builder(&connection)
		.destination(owner.as_str())?
		.path(BATTERY_OBJECT_PATH)?
		.build()
		.await?;
	let mut changes = properties
		.receive_properties_changed_with_args(&[(0, BATTERY_INTERFACE.as_str())])
		.await?;
	let mut messages = zbus::MessageStream::from(&connection);

	loop {
		let state = proxy.state().await?;
		if output.send(Message::State(state)).await.is_err() {
			return Ok(());
		}
		*backoff = Duration::from_secs(1);

		loop {
			tokio::select! {
				change = changes.next() => {
					let Some(change) = change else { return Ok(()); };
					change.args()?;
					break;
				}
				_ = owners.next() => return Ok(()),
				message = messages.next() => {
					if !matches!(message, Some(Ok(_))) { return Ok(()); }
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn battery_disconnect_clears_stale_state_and_can_recover() {
		let mut battery = Battery::default();
		assert!(!battery.visible());

		let state = BatteryState {
			present: true,
			percentage: 42.0,
			state: 1,
		};
		battery.update(Message::State(state));
		assert!(battery.visible());
		assert_eq!(status(battery.state.state), "Charging");

		battery.update(Message::Disconnected);
		assert!(!battery.visible());
		assert_eq!(battery.state, BatteryState::default());

		battery.update(Message::State(state));
		assert!(battery.visible());
		battery.update(Message::State(BatteryState::default()));
		assert!(!battery.visible());
	}

	#[test]
	fn battery_labels_cover_upower_states() {
		for (state, label) in [
			(0, "Unknown"),
			(1, "Charging"),
			(2, "Discharging"),
			(3, "Empty"),
			(4, "Full"),
			(5, "Pending charge"),
			(6, "Pending discharge"),
			(99, "Unknown"),
		] {
			assert_eq!(status(state), label);
		}
	}
}
