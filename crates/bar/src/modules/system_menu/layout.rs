//! SystemMenuWidgets selection, editing and reload rules; no widgets or I/O.
use config::{ConfigFile, SystemMenuWidgets};

#[derive(Debug, Clone)]
pub(super) struct Layout {
	selected: Vec<SystemMenuWidgets>,
	available: Vec<SystemMenuWidgets>,
	editing: bool,
}

pub(super) enum Event {
	Editing(bool),
	Edited(Vec<SystemMenuWidgets>),
	Reload(Vec<SystemMenuWidgets>),
	Closed,
}

impl Layout {
	pub fn new(config: &ConfigFile) -> Self {
		let mut available = SystemMenuWidgets::VARIANTS.to_vec();
		available.extend(
			config
				.homeassistant
				.tracked_devices
				.iter()
				.cloned()
				.map(SystemMenuWidgets::Homeassistant),
		);
		for widget in &config.system_menu.widgets {
			if !available.contains(widget) {
				available.push(widget.clone());
			}
		}
		Self {
			selected: config.system_menu.widgets.clone(),
			available,
			editing: false,
		}
	}

	pub fn selected(&self) -> &[SystemMenuWidgets] {
		&self.selected
	}
	pub fn available(&self) -> &[SystemMenuWidgets] {
		&self.available
	}
	pub fn editing(&self) -> bool {
		self.editing
	}

	pub fn entries(&self) -> Vec<&SystemMenuWidgets> {
		let mut entries = self.selected.iter().collect::<Vec<_>>();
		if self.editing {
			entries.extend(
				self.available
					.iter()
					.filter(|widget| !self.selected.contains(widget)),
			);
		}
		entries
	}

	/// Only accepted, changed user edits request persistence. Reloads never do.
	pub fn update(&mut self, event: Event) -> Option<Vec<SystemMenuWidgets>> {
		match event {
			Event::Editing(editing) => self.editing = editing,
			Event::Closed => self.editing = false,
			Event::Edited(widgets) if self.editing && self.selected != widgets => {
				self.selected = widgets;
				return Some(self.selected.clone());
			}
			Event::Reload(widgets) if !self.editing => self.selected = widgets,
			_ => {}
		}
		None
	}
}

/// Pointer geometry stays in the editor; this is the resulting layout intent.
pub(super) fn changed_widgets(
	keys: &[SystemMenuWidgets], active: usize, source: usize, target: Option<usize>,
) -> Vec<SystemMenuWidgets> {
	let mut widgets = keys.iter().take(active).cloned().collect::<Vec<_>>();
	if source >= active {
		if let Some(widget) = keys.get(source) {
			widgets.push(widget.clone());
		}
	} else if let Some(target) = target {
		if source < widgets.len() && target < widgets.len() {
			let widget = widgets.remove(source);
			widgets.insert(target, widget);
		}
	} else if source < widgets.len() {
		widgets.remove(source);
	}
	widgets
}

#[cfg(test)]
mod tests {
	use super::*;
	use SystemMenuWidgets::{Bluetooth, Homeassistant, Wifi};

	#[test]
	fn selection_and_persistence_follow_editing_not_rendering() {
		let mut config = ConfigFile::default();
		config.system_menu.widgets = vec![Wifi, Wifi, Homeassistant("light.untracked".into())];
		let mut layout = Layout::new(&config);
		assert!(
			layout
				.available()
				.contains(&Homeassistant("light.untracked".into()))
		);
		assert_eq!(
			layout.entries(),
			layout.selected().iter().collect::<Vec<_>>()
		);
		assert!(layout.update(Event::Edited(vec![Bluetooth])).is_none());
		layout.update(Event::Editing(true));
		assert!(layout.entries().len() > layout.selected().len());
		assert!(layout.update(Event::Reload(vec![])).is_none());
		assert_eq!(layout.selected(), config.system_menu.widgets);
		assert_eq!(
			layout.update(Event::Edited(vec![Bluetooth])),
			Some(vec![Bluetooth])
		);
		assert!(layout.update(Event::Edited(vec![Bluetooth])).is_none());
		layout.update(Event::Closed);
		assert!(!layout.editing());
		assert!(layout.update(Event::Edited(vec![])).is_none());
		layout.update(Event::Reload(vec![]));
		assert!(layout.selected().is_empty());
	}

	#[test]
	fn click_and_drag_intents_preserve_order_in_both_directions() {
		let keys = vec![Wifi, Bluetooth, Homeassistant("light.test".into())];
		assert_eq!(changed_widgets(&keys, 2, 0, None), vec![Bluetooth]);
		assert_eq!(changed_widgets(&keys, 2, 2, None), keys);
		assert_eq!(changed_widgets(&keys, 2, 0, Some(1)), vec![Bluetooth, Wifi]);
		assert_eq!(changed_widgets(&keys, 2, 1, Some(0)), vec![Bluetooth, Wifi]);
		assert_eq!(changed_widgets(&keys, 2, 0, Some(0)), vec![Wifi, Bluetooth]);
	}
}
