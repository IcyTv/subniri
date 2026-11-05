//! `SystemMenuWidgets` selection, editing and reload rules; no widgets or I/O.
use config::{ConfigFile, SystemMenuWidgets};
mod session;
use session::Session;

#[derive(Debug, Clone)]
pub(super) struct Layout {
	selected: Vec<SystemMenuWidgets>,
	available: Vec<SystemMenuWidgets>,
	editing: bool,
	rows: Vec<config::MenuRow>,
	session: Option<Session>,
}

#[cfg(test)]
pub(super) enum Event {
	Editing(bool),
	Closed,
}

impl Layout {
	pub fn new(config: &ConfigFile) -> Self {
		let rows = config.system_menu.effective_rows();
		let selected = config
			.system_menu
			.resolve()
			.tiles
			.into_iter()
			.map(|tile| tile.tile.widget)
			.collect::<Vec<_>>();
		let available = catalog(&selected, &config.homeassistant.tracked_devices);
		Self {
			selected,
			available,
			editing: false,
			rows,
			session: None,
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

	pub fn configuration(&self) -> config::SystemMenu {
		config::SystemMenu::from_rows(self.rows.clone())
	}

	pub fn begin_edit(&mut self, columns: u32) {
		if self.session.is_none() {
			self.session = Some(Session::new(
				&self.configuration(),
				&self.available,
				columns,
			));
		}
		self.editing = true;
	}

	pub fn tiles(&self) -> Vec<config::PlacedTile> {
		self.session.as_ref().map_or_else(
			|| self.configuration().resolve().tiles,
			|session| session.tiles.clone(),
		)
	}
	pub fn edit_baseline_rows(&self) -> Vec<config::MenuRow> {
		self.session
			.as_ref()
			.map_or_else(|| self.rows.clone(), |session| session.rows.clone())
	}
	pub fn enabled(&self) -> Vec<bool> {
		self.session.as_ref().map_or_else(
			|| vec![true; self.selected.len()],
			|session| session.enabled.clone(),
		)
	}

	fn refresh_session_configuration(&mut self) {
		if let Some(session) = &self.session {
			self.rows = session.configuration(false).effective_rows();
			self.selected = self
				.configuration()
				.resolve()
				.tiles
				.into_iter()
				.map(|tile| tile.tile.widget)
				.collect();
		}
	}

	pub fn edit_tiles(
		&mut self, tiles: Vec<config::PlacedTile>, rows: Vec<config::MenuRow>,
	) -> bool {
		let changed = self
			.session
			.as_mut()
			.is_some_and(|session| session.replace(tiles, rows));
		if changed {
			self.refresh_session_configuration();
		}
		changed
	}

	pub fn toggle(&mut self, index: usize) -> bool {
		let changed = self
			.session
			.as_mut()
			.is_some_and(|session| session.toggle(index));
		if changed {
			self.refresh_session_configuration();
		}
		changed
	}

	pub fn finish_edit(&mut self) -> bool {
		let before = self.configuration();
		if let Some(session) = self.session.take() {
			self.rows = session.configuration(true).effective_rows();
			self.selected = self
				.configuration()
				.resolve()
				.tiles
				.into_iter()
				.map(|tile| tile.tile.widget)
				.collect();
		}
		self.editing = false;
		before != self.configuration()
	}

	#[cfg(test)]
	pub fn edit_rows(&mut self, rows: Vec<config::MenuRow>) -> bool {
		if !self.editing || self.rows == rows {
			return false;
		}
		self.rows = rows;
		self.selected = self
			.configuration()
			.resolve()
			.tiles
			.into_iter()
			.map(|tile| tile.tile.widget)
			.collect();
		if self.session.is_some() {
			self.session = Some(Session::new(&self.configuration(), &self.available, 6));
		}
		true
	}

	pub fn reload(&mut self, config: &ConfigFile) {
		let widgets = config
			.system_menu
			.resolve()
			.tiles
			.into_iter()
			.map(|tile| tile.tile.widget)
			.collect::<Vec<_>>();
		self.available = catalog(&widgets, &config.homeassistant.tracked_devices);
		if !self.editing {
			self.selected = widgets;
			self.rows = config.system_menu.effective_rows();
		}
		// Reload the catalog without invalidating in-progress cards or their observers.
		for widget in &self.selected {
			if !self.available.contains(widget) {
				self.available.push(widget.clone());
			}
		}
	}

	pub fn homeassistant_ids(&self) -> Vec<String> {
		self.available()
			.iter()
			.filter_map(|widget| match widget {
				SystemMenuWidgets::Homeassistant(id) => Some(id.clone()),
				_ => None,
			})
			.collect()
	}

	#[cfg(test)]
	pub fn update(&mut self, event: Event) {
		match event {
			Event::Editing(true) => self.begin_edit(6),
			Event::Editing(false) | Event::Closed => {
				self.finish_edit();
			}
		}
	}
}

fn catalog(widgets: &[SystemMenuWidgets], tracked_devices: &[String]) -> Vec<SystemMenuWidgets> {
	let mut available = SystemMenuWidgets::VARIANTS.to_vec();
	available.extend(
		tracked_devices
			.iter()
			.cloned()
			.map(SystemMenuWidgets::Homeassistant),
	);
	for widget in widgets {
		if !available.contains(widget) {
			available.push(widget.clone());
		}
	}
	available
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
		let rows = config::SystemMenu::from_widgets(vec![Bluetooth]).effective_rows();
		assert!(!layout.edit_rows(rows.clone()));
		layout.update(Event::Editing(true));
		layout.reload(&ConfigFile::default());
		assert_eq!(layout.selected(), config.system_menu.widgets);
		assert!(layout.edit_rows(rows.clone()));
		assert_eq!(layout.selected(), &[Bluetooth]);
		assert!(!layout.edit_rows(rows));
		layout.update(Event::Closed);
		assert!(!layout.editing());
		assert!(!layout.edit_rows(Vec::new()));
		layout.reload(&ConfigFile::default());
		assert!(layout.selected().is_empty());
	}
}
