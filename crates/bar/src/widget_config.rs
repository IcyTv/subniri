//! Serialize and coalesce menu-layout writes without blocking UI interaction.

use config::{ConfigFile, SystemMenuWidgets};
use iced::Task;

use crate::BarMessage;

#[derive(Default)]
pub(crate) struct Writer {
	running: bool,
	pending: Option<Vec<SystemMenuWidgets>>,
	latest: Option<Vec<SystemMenuWidgets>>,
}

impl Writer {
	pub(crate) fn queue(&mut self, widgets: Vec<SystemMenuWidgets>) -> Task<BarMessage> {
		self.latest = Some(widgets.clone());
		self.pending = Some(widgets);
		if self.running {
			Task::none()
		} else {
			self.start_next()
		}
	}

	pub(crate) fn latest(&self) -> Option<&[SystemMenuWidgets]> {
		self.latest.as_deref()
	}

	pub(crate) fn completed(&mut self) -> Task<BarMessage> {
		self.running = false;
		if self.pending.is_some() {
			self.start_next()
		} else {
			self.latest = None;
			// Our config watcher suppresses this process's own writes, so reload
			// explicitly to synchronize the bar's config and other menu instances.
			Task::done(BarMessage::ConfigUpdated)
		}
	}

	fn start_next(&mut self) -> Task<BarMessage> {
		let Some(widgets) = self.pending.take() else {
			return Task::none();
		};
		self.running = true;
		Task::perform(
			async move {
				tokio::task::spawn_blocking(move || {
					// Read afresh and change only the menu layout: don't overwrite
					// unrelated settings with an old UI configuration snapshot.
					let (mut doc, mut config) =
						ConfigFile::load().map_err(|error| error.to_string())?;
					config.system_menu.widgets = widgets;
					config.write(&mut doc).map_err(|error| error.to_string())
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

	#[test]
	fn coalesces_edits_while_a_write_is_in_flight() {
		let mut writer = Writer::default();
		// Tasks are deliberately not run: this tests scheduling without touching config files.
		drop(writer.queue(vec![SystemMenuWidgets::Wifi]));
		drop(writer.queue(vec![SystemMenuWidgets::Bluetooth]));
		drop(writer.queue(vec![SystemMenuWidgets::Nightlight]));
		assert!(writer.running);
		assert_eq!(
			writer.latest(),
			Some([SystemMenuWidgets::Nightlight].as_slice())
		);
		assert_eq!(writer.pending, Some(vec![SystemMenuWidgets::Nightlight]));
		drop(writer.completed());
		assert!(writer.running);
		assert!(writer.pending.is_none());
		drop(writer.completed());
		assert!(!writer.running);
		assert!(writer.latest().is_none());
	}
}
