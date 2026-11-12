//! Serialize and coalesce menu-layout writes without blocking UI interaction.

use config::ConfigFile;
use iced::Task;

use crate::BarMessage;

#[cfg(test)]
mod file_tests;

#[derive(Default)]
pub(crate) struct Writer {
	running: bool,
	pending_layout: Option<config::SystemMenu>,
	latest_layout: Option<config::SystemMenu>,
}

impl Writer {
	pub(crate) fn latest_layout(&self) -> Option<&config::SystemMenu> {
		self.latest_layout.as_ref()
	}

	pub(crate) fn queue_layout(&mut self, layout: config::SystemMenu) -> Task<BarMessage> {
		self.latest_layout = Some(layout.clone());
		self.pending_layout = Some(layout);
		if self.running {
			Task::none()
		} else {
			self.start_next()
		}
	}

	pub(crate) fn completed(&mut self) -> Task<BarMessage> {
		self.running = false;
		if self.pending_layout.is_some() {
			self.start_next()
		} else {
			self.latest_layout = None;
			// Our config watcher suppresses this process's own writes, so reload
			// explicitly to synchronize the bar's config and other menu instances.
			Task::done(BarMessage::ConfigUpdated)
		}
	}

	fn start_next(&mut self) -> Task<BarMessage> {
		let Some(layout) = self.pending_layout.take() else {
			return Task::none();
		};
		self.running = true;
		Task::perform(
			async move {
				tokio::task::spawn_blocking(move || {
					// Read afresh and change only the menu layout: don't overwrite
					// unrelated settings with an old UI configuration snapshot.
					ConfigFile::update(|config| config.system_menu = layout)
						.map(|_| ())
						.map_err(|error| error.to_string())
				})
				.await
				.unwrap_or_else(|error| Err(error.to_string()))
			},
			BarMessage::SystemMenuWidgetsSaved,
		)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use config::{SystemMenu, SystemMenuWidgets};

	#[test]
	fn coalesces_edits_while_a_write_is_in_flight() {
		let mut writer = Writer::default();
		// Tasks are deliberately not run: this tests scheduling without touching config files.
		drop(writer.queue_layout(SystemMenu::from_widgets(vec![SystemMenuWidgets::Wifi])));
		drop(writer.queue_layout(SystemMenu::from_widgets(vec![SystemMenuWidgets::Bluetooth])));
		drop(writer.queue_layout(SystemMenu::from_widgets(vec![
			SystemMenuWidgets::Nightlight,
		])));
		assert!(writer.running);
		assert_eq!(
			writer.latest_layout(),
			Some(&SystemMenu::from_widgets(vec![
				SystemMenuWidgets::Nightlight
			]))
		);
		assert_eq!(
			writer.pending_layout,
			Some(SystemMenu::from_widgets(vec![
				SystemMenuWidgets::Nightlight
			]))
		);
		drop(writer.completed());
		assert!(writer.running);
		assert!(writer.pending_layout.is_none());
		drop(writer.completed());
		assert!(!writer.running);
		assert!(writer.latest_layout().is_none());
	}
}
