use std::time::Duration;

use daemon_common::{
	BatteryProxy, BatteryState, ChargeLimitProxy, ChargeLimitState, PowerProfileProxy,
	PowerProfileState,
};
use futures::StreamExt;
use iced::{
	Alignment, Element, Font, Length, Subscription, Task,
	alignment::Vertical,
	font,
	widget::{column, row, space, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::Colors,
	widgets::{NeoButton, neo_button, neo_card, neo_slider},
};
use utilities::Hashable;
use zbus::Connection;

use crate::modules::{ICON_HEIGHT, MODULE_HEIGHT, MODULE_RADIUS};

pub(super) mod integration;

#[derive(Debug, Clone)]
pub enum Message {
	StateChanged(BatteryState),
	ProfileChanged(PowerProfileState),
	ChargeLimitChanged(ChargeLimitState),

	SetProfile(String),
	SetChargeLimit(u32),

	Disconnected(String),
}

#[derive(Clone, Debug)]
struct Resources {
	connection: Connection,
	battery: BatteryProxy<'static>,
	power_profile: PowerProfileProxy<'static>,
	charge_limit: ChargeLimitProxy<'static>,
}

#[derive(Clone, Debug)]
pub struct Battery {
	resources: Hashable<Resources>,

	state: BatteryState,
	profile: PowerProfileState,
	limit: ChargeLimitState,
}

impl Battery {
	pub fn connection(&self) -> Connection {
		self.resources.connection.clone()
	}

	pub async fn new() -> Result<Self, String> {
		let connection = Connection::session().await.map_err(|error| error.to_string())?;
		let battery = BatteryProxy::new(&connection)
			.await
			.map_err(|error| error.to_string())?;
		let power_profile = PowerProfileProxy::new(&connection)
			.await
			.map_err(|error| error.to_string())?;
		let charge_limit = ChargeLimitProxy::new(&connection)
			.await
			.map_err(|error| error.to_string())?;

		let state = request(battery.state()).await?;
		let profile = request(power_profile.state()).await?;
		let limit = request(charge_limit.state()).await?;

		Ok(Self {
			resources: Hashable::new(Resources {
				connection,
				battery,
				power_profile,
				charge_limit,
			}),
			state,
			profile,
			limit,
		})
	}

	pub fn subscription(&self) -> Subscription<Message> {
		Subscription::run_with(self.resources.clone(), |resources| {
			battery_events(resources.clone())
		})
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::StateChanged(state) => self.state = state,
			Message::ProfileChanged(profile) => self.profile = profile,
			Message::ChargeLimitChanged(limit) => self.limit = limit,
			Message::SetProfile(profile) => {
				let power_profile = self.resources.power_profile.clone();
				return Task::future(async move {
					match request(async {
						power_profile.set_active_profile(profile).await?;
						power_profile.state().await
					})
					.await
					{
						Ok(profile) => Message::ProfileChanged(profile),
						Err(error) => Message::Disconnected(error),
					}
				});
			}
			Message::SetChargeLimit(value) => {
				let charge_limit = self.resources.charge_limit.clone();
				return Task::future(async move {
					match request(async {
						charge_limit.set_end_threshold(value).await?;
						charge_limit.state().await
					})
					.await
					{
						Ok(limit) => Message::ChargeLimitChanged(limit),
						Err(error) => Message::Disconnected(error),
					}
				});
			}
			Message::Disconnected(_) => (), // Handled by the module's connection lifecycle.
		}
		Task::none()
	}

	pub fn view_tile(&self, colors: Colors) -> NeoButton<'_, Message> {
		let (icon, muted) = status_icon(self.state);
		let foreground = if muted {
			colors.on_accent.scale_alpha(0.5)
		} else {
			colors.on_accent
		};

		neo_button(
			row![
				svg(icon)
					.width(Length::Shrink)
					.height(ICON_HEIGHT)
					.style(move |_, _| svg::Style {
						color: Some(foreground)
					}),
				super::module_label(
					text(format!("{:.0}%", self.state.percentage))
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
		.background(colors.decorative.yellow)
	}

	pub fn view_popup(&self, colors: Colors) -> Element<'_, Message> {
		let (icon, _) = status_icon(self.state);
		let top_row = row![
			svg(icon).style(move |_, _| svg::Style {
				color: Some(colors.text)
			}).width(24).height(24),
			text(format!("{:.0}%", self.state.percentage))
				.color(colors.text)
				.size(20)
				.weight(font::Weight::Bold),
			space::horizontal(),
			text(status_label(self.state.state)).color(colors.secondary),
		]
		.align_y(Alignment::Center)
		.spacing(12.0);

		let detail = power_detail_line(self.state);
		let detail_widget: Element<'_, Message> = detail.map_or_else(
			|| space().height(0).into(),
			|line| text(line).color(colors.secondary).size(14).into(),
		);

		let mut content = column![top_row, detail_widget].spacing(16.0);

		if self.profile.available {
			content = content.push(profile_selector(&self.profile, colors));
		}

		if self.limit.available {
			content = content.push(
				column![
					text("Charge limit").color(colors.secondary).size(14),
					row![
						neo_slider(0.0_f32..=100.0_f32, self.limit.end_threshold as f32)
							.with_colors(colors)
							.step(1.0_f32)
							.on_change(|value| Message::SetChargeLimit(value as u32)),
						text(format!("{}%", self.limit.end_threshold)).color(colors.text),
					]
					.spacing(12.0)
					.align_y(Alignment::Center),
				]
				.spacing(8.0),
			);
		}

		neo_card(content)
			.with_colors(colors)
			.width(320.0)
			.radius(MODULE_RADIUS)
			.background(colors.body)
			.into()
	}
}

const PROFILES: [&str; 3] = ["power-saver", "balanced", "performance"];

fn profile_label(profile: &str) -> &str {
	match profile {
		"power-saver" => "Power saver",
		"performance" => "Performance",
		_ => "Balanced",
	}
}

fn profile_selector(profile: &PowerProfileState, colors: Colors) -> Element<'_, Message> {
	column![
		text("Power profile").color(colors.secondary).size(14),
		row(PROFILES.iter().map(|candidate| {
			let selected = profile.active == *candidate;
			neo_button(text(profile_label(candidate)).color(colors.text))
				.with_colors(colors)
				.background(if selected {
					colors.raised
				} else {
					colors.surface
				})
				.width(Length::Fill)
				.on_press(Message::SetProfile((*candidate).to_string()))
				.into()
		}))
		.spacing(8.0),
	]
	.spacing(8.0)
	.into()
}

fn status_icon(state: BatteryState) -> (iced::widget::svg::Handle, bool) {
	if !state.present {
		return (phosphor_icon!("battery-warning"), true);
	}
	match state.state {
		1 => (phosphor_icon!("battery-charging"), false),
		4 => (phosphor_icon!("battery-full"), false),
		_ if state.percentage <= 15.0 => (phosphor_icon!("battery-low"), false),
		_ => (phosphor_icon!("battery-full"), false),
	}
}

fn status_label(state: u32) -> &'static str {
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

fn power_detail_line(state: BatteryState) -> Option<String> {
	if state.time_to_full > 0 {
		Some(format!(
			"{} until full ({:.1} W)",
			format_duration(state.time_to_full),
			state.energy_rate
		))
	} else if state.time_to_empty > 0 {
		Some(format!(
			"{} remaining ({:.1} W)",
			format_duration(state.time_to_empty),
			state.energy_rate
		))
	} else if state.energy_rate > 0.0 {
		Some(format!("{:.1} W", state.energy_rate))
	} else {
		None
	}
}

fn format_duration(seconds: u64) -> String {
	let hours = seconds / 3600;
	let minutes = (seconds % 3600) / 60;
	if hours > 0 {
		format!("{hours}h {minutes}m")
	} else {
		format!("{minutes}m")
	}
}

fn battery_events(resources: Hashable<Resources>) -> impl futures::Stream<Item = Message> {
	async_stream::stream! {
		let mut state_changes = resources.battery.receive_state_changed().await;
		let mut profile_changes = resources.power_profile.receive_state_changed().await;
		let mut limit_changes = resources.charge_limit.receive_state_changed().await;

		let error = loop {
			tokio::select! {
				() = resources.connection.closed() => {
					break "Battery session disconnected".to_string();
				}
				change = state_changes.next() => {
					let Some(change) = change else { break "Battery property stream ended".to_string(); };
					match change.get().await {
						Ok(state) => yield Message::StateChanged(state),
						Err(error) => break error.to_string(),
					}
				}
				change = profile_changes.next() => {
					let Some(change) = change else { break "Power profile stream ended".to_string(); };
					match change.get().await {
						Ok(state) => yield Message::ProfileChanged(state),
						Err(error) => break error.to_string(),
					}
				}
				change = limit_changes.next() => {
					let Some(change) = change else { break "Charge limit stream ended".to_string(); };
					match change.get().await {
						Ok(state) => yield Message::ChargeLimitChanged(state),
						Err(error) => break error.to_string(),
					}
				}
			}
		};
		yield Message::Disconnected(error);
	}
}

async fn request<T, E: std::fmt::Display>(
	future: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, String> {
	tokio::time::timeout(Duration::from_secs(5), future)
		.await
		.map_err(|_| "Battery request timed out".to_string())?
		.map_err(|error| error.to_string())
}
