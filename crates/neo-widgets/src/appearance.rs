use std::time::{Duration, Instant};

use daemon_common::{APPEARANCE_OBJECT_PATH, AppearanceProxy, AppearanceState, DEFAULT_BUS_NAME};
use iced::{
	Subscription, Task,
	futures::{SinkExt, StreamExt},
};

pub use config::AppearanceMode;

#[derive(Debug, Clone)]
pub enum Event {
	State(AppearanceState),
	Disconnected,
	RequestFinished(Result<(), String>),
	Frame(Instant),
}

/// Per-application presentation of the daemon's effective appearance. The saved
/// configuration is only a startup fallback, never an independent write path.
pub struct Appearance {
	pub theme: crate::theme::ThemeContext,
	pub connected: bool,
	pub saving: bool,
	pub error: String,
	seen_state: bool,
}

impl Default for Appearance {
	fn default() -> Self {
		let theme = config::ConfigFile::load()
			.map(|(_, config)| config.appearance.theme)
			.unwrap_or_default();

		Self::new(theme)
	}
}

impl Appearance {
	pub fn new(theme: AppearanceMode) -> Self {
		Self {
			theme: crate::theme::ThemeContext::new(theme),
			connected: false,
			saving: false,
			error: String::new(),
			seen_state: false,
		}
	}
	pub fn colors(&self) -> crate::style::Colors {
		self.theme.colors()
	}

	pub fn subscription(&self) -> Subscription<Event> {
		let frames = if self.theme.is_animating(Instant::now()) {
			iced::window::frames().map(Event::Frame)
		} else {
			Subscription::none()
		};

		Subscription::batch([subscription(), frames])
	}

	pub fn update(&mut self, event: Event) {
		match event {
			Event::State(state) => {
				let Some(theme) = AppearanceMode::from_name(&state.theme) else {
					log::warn!("Ignoring unknown appearance theme: {}", state.theme);
					return;
				};

				if self.seen_state {
					self.theme.set_theme(theme, Instant::now());
				} else {
					self.theme = crate::theme::ThemeContext::new(theme);
					self.seen_state = true;
				}
				self.connected = true;
				self.saving = state.saving;
				self.error = state.error;
			}
			Event::Disconnected => self.connected = false,
			Event::RequestFinished(Err(error)) => {
				log::warn!("Appearance request failed: {error}");
			}
			Event::RequestFinished(Ok(())) | Event::Frame(_) => {}
		}
	}
}

pub fn subscription() -> Subscription<Event> {
	Subscription::run(|| {
		iced::stream::channel(16, |mut output| async move {
			loop {
				if let Err(error) = observe(&mut output).await {
					log::debug!("Appearance connection unavailable: {error}");
				}
				if output.send(Event::Disconnected).await.is_err() {
					return;
				}
				tokio::time::sleep(Duration::from_secs(2)).await;
			}
		})
	})
}

async fn proxy(connection: &zbus::Connection) -> zbus::Result<AppearanceProxy<'_>> {
	AppearanceProxy::builder(connection)
		.destination(DEFAULT_BUS_NAME)?
		.path(APPEARANCE_OBJECT_PATH)?
		.build()
		.await
}

async fn observe(output: &mut iced::futures::channel::mpsc::Sender<Event>) -> zbus::Result<()> {
	let connection = zbus::Connection::session().await?;
	let proxy = proxy(&connection).await?;

	// Install the signal match first so a change during the initial read is not lost.
	let mut changes = proxy.receive_changed().await?;
	let mut owner_changes = proxy.inner().receive_owner_changed().await?;
	let mut latest = proxy.state().await?;
	let _ = output.send(Event::State(latest.clone())).await;

	loop {
		tokio::select! {
			change = changes.next() => {
				let Some(change) = change else { return Ok(()); };
				let snapshot = change.args()?.state;

				if snapshot.revision > latest.revision {
					latest = snapshot;
					if output.send(Event::State(latest.clone())).await.is_err() {
						return Ok(());
					}
				}
			}
			_ = owner_changes.next() => return Ok(()),
		}
	}
}

pub fn select(theme: AppearanceMode) -> Task<Event> {
	request(Some(theme))
}

pub fn retry_saving() -> Task<Event> {
	request(None)
}

fn request(theme: Option<AppearanceMode>) -> Task<Event> {
	Task::perform(
		async move {
			let connection = zbus::Connection::session().await?;
			let proxy = proxy(&connection).await?;

			match theme {
				Some(theme) => proxy.select(theme.as_str()).await?,
				None => proxy.retry_saving().await?,
			}

			Ok::<_, zbus::Error>(())
		},
		|result| Event::RequestFinished(result.map_err(|error| error.to_string())),
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn client(theme: AppearanceMode) -> Appearance {
		Appearance {
			theme: crate::theme::ThemeContext::new(theme),
			connected: false,
			saving: false,
			error: String::new(),
			seen_state: false,
		}
	}

	fn state(theme: &str) -> Event {
		Event::State(AppearanceState {
			theme: theme.into(),
			saving: false,
			error: String::new(),
			revision: 0,
		})
	}

	#[test]
	fn initial_state_does_not_fade_and_disconnect_retains_appearance() {
		let mut appearance = client(AppearanceMode::Light);

		appearance.update(state("dark"));

		assert!(appearance.connected);
		assert_eq!(appearance.colors(), crate::style::DARK);
		assert!(!appearance.theme.is_animating(Instant::now()));

		appearance.update(Event::Disconnected);

		assert!(!appearance.connected);
		assert_eq!(appearance.colors(), crate::style::DARK);

		appearance.update(state("light"));

		assert!(appearance.connected);
		assert_eq!(appearance.theme.selected(), AppearanceMode::Light);
	}

	#[test]
	fn unsupported_theme_does_not_replace_last_valid_state() {
		let mut appearance = client(AppearanceMode::Dark);

		appearance.update(state("future-theme"));

		assert!(!appearance.connected);
		assert_eq!(appearance.colors(), crate::style::DARK);
	}
}
