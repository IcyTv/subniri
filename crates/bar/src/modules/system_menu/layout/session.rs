//! Stable edit-session geometry. Membership changes never move an interaction target.
use config::{MenuRow, MenuTile, PlacedTile, SystemMenu, SystemMenuWidgets};

#[derive(Debug, Clone)]
pub(super) struct Session {
	pub tiles: Vec<PlacedTile>,
	pub enabled: Vec<bool>,
	pub rows: Vec<MenuRow>,
}

impl Session {
	pub fn new(layout: &SystemMenu, available: &[SystemMenuWidgets], columns: u32) -> Self {
		let resolved = layout.resolve();
		let mut tiles = resolved.tiles;
		let mut enabled = vec![true; tiles.len()];
		let selected = tiles
			.iter()
			.map(|tile| &tile.tile.widget)
			.collect::<Vec<_>>();
		let missing = available
			.iter()
			.filter(|widget| !selected.contains(widget))
			.cloned()
			.collect::<Vec<_>>();
		let mut row = resolved.rows;
		let mut column = 0_u32;
		for widget in missing {
			if column > 0 && column.saturating_add(2) > columns {
				row += 1;
				column = 0;
			}
			tiles.push(PlacedTile {
				tile: MenuTile::new(widget),
				column,
				row,
			});
			enabled.push(false);
			column += 2;
		}
		Self {
			tiles,
			enabled,
			rows: layout.effective_rows(),
		}
	}

	pub fn replace(&mut self, tiles: Vec<PlacedTile>, rows: Vec<MenuRow>) -> bool {
		if tiles.len() != self.tiles.len()
			|| !tiles
				.iter()
				.zip(&self.tiles)
				.all(|(a, b)| a.tile.widget == b.tile.widget)
		{
			return false;
		}
		if self.tiles == tiles && self.rows == rows {
			return false;
		}
		self.tiles = tiles;
		self.rows = rows;
		true
	}

	pub fn toggle(&mut self, index: usize) -> bool {
		if let Some(enabled) = self.enabled.get_mut(index) {
			*enabled = !*enabled;
			true
		} else {
			false
		}
	}

	/// Union of disabled footprints on this row, clipped before the supplied column.
	/// Authored gaps are not in this set and therefore never disappear on exit.
	fn removed_before(&self, row: u32, column: u32) -> u32 {
		let mut intervals =
			self.tiles
				.iter()
				.zip(&self.enabled)
				.filter(|(tile, enabled)| {
					!**enabled
						&& tile.row <= row && row < tile.row.saturating_add(tile.tile.height)
						&& tile.column < column
				})
				.map(|(tile, _)| {
					(
						tile.column,
						tile.column.saturating_add(tile.tile.width).min(column),
					)
				})
				.collect::<Vec<_>>();
		intervals.sort_unstable();
		let mut end = 0;
		let mut removed = 0;
		for (start, next) in intervals {
			removed += next.saturating_sub(end.max(start));
			end = end.max(next);
		}
		removed
	}

	pub fn configuration(&self, compact: bool) -> SystemMenu {
		let tiles = self
			.tiles
			.iter()
			.zip(&self.enabled)
			.filter(|(_, enabled)| **enabled)
			.map(|(tile, _)| {
				let mut tile = tile.clone();
				if compact {
					tile.column = tile
						.column
						.saturating_sub(self.removed_before(tile.row, tile.column));
				}
				tile
			})
			.collect::<Vec<_>>();
		let trailing = super::super::spatial::trailing_ends(&self.tiles, &self.rows)
			.into_iter()
			.enumerate()
			.map(|(row, end)| {
				if compact {
					end.saturating_sub(self.removed_before(row as u32, end))
				} else {
					end
				}
			})
			.collect::<Vec<_>>();
		// The row resolver still owns tall-tile collision rules after local compaction.
		let rows = config::rows_from_tiles(&tiles, self.rows.len() as u32, &trailing);
		let resolved = config::resolve_rows(&rows);
		if compact {
			let occupied = (0..resolved.rows)
				.filter(|row| {
					resolved.tiles.iter().any(|tile| {
						tile.row <= *row && *row < tile.row.saturating_add(tile.tile.height)
					})
				})
				.collect::<Vec<_>>();
			let tiles = resolved
				.tiles
				.into_iter()
				.map(|mut tile| {
					tile.row = occupied.partition_point(|row| *row < tile.row) as u32;
					tile
				})
				.collect::<Vec<_>>();
			let trailing = occupied
				.iter()
				.map(|row| trailing.get(*row as usize).copied().unwrap_or(0))
				.collect::<Vec<_>>();
			return SystemMenu::from_rows(config::rows_from_tiles(
				&tiles,
				occupied.len() as u32,
				&trailing,
			));
		}
		SystemMenu::from_rows(config::rows_from_tiles(
			&resolved.tiles,
			self.rows.len() as u32,
			&trailing,
		))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use config::{
		MenuEntry,
		SystemMenuWidgets::{Bluetooth, Nightlight, Wifi},
	};
	#[test]
	fn repeated_edit_exit_does_not_accumulate_empty_catalog_rows() {
		let mut layout = SystemMenu::from_rows(vec![
			MenuRow(vec![MenuEntry::Widget(MenuTile::new(Nightlight))]),
			MenuRow::default(),
			MenuRow(vec![MenuEntry::Gap(2)]),
		]);
		for _ in 0..5 {
			let session = Session::new(&layout, &[Wifi, Bluetooth, Nightlight], 6);
			layout = session.configuration(true);
			assert_eq!(layout.effective_rows().len(), 1);
		}
	}
	#[test]
	fn empty_row_compaction_preserves_rows_covered_by_tall_tiles() {
		let layout = SystemMenu::from_rows(vec![
			MenuRow(vec![MenuEntry::Widget(MenuTile {
				widget: Nightlight,
				width: 2,
				height: 3,
			})]),
			MenuRow::default(),
			MenuRow::default(),
			MenuRow::default(),
			MenuRow(vec![
				MenuEntry::Gap(2),
				MenuEntry::Widget(MenuTile::new(Wifi)),
			]),
		]);
		let compact = Session::new(&layout, &[], 6).configuration(true);
		assert_eq!(
			compact
				.resolve()
				.tiles
				.iter()
				.map(|tile| (tile.row, tile.column, tile.tile.height))
				.collect::<Vec<_>>(),
			vec![(0, 0, 3), (3, 2, 1)]
		);
		assert_eq!(compact.effective_rows().len(), 4);
	}
	#[test]
	fn duplicate_occurrences_keep_membership_when_moved() {
		let mut session = Session::new(&SystemMenu::from_widgets(vec![Wifi, Wifi]), &[], 6);
		session.toggle(0);
		let mut moved = session.tiles.clone();
		moved[0].column = 6;
		moved[1].column = 2;
		assert!(session.replace(moved, session.rows.clone()));
		let written = session.configuration(true).resolve();
		assert_eq!(written.tiles.len(), 1);
		assert_eq!(written.tiles[0].column, 2);
		assert_eq!(session.enabled, vec![false, true]);
	}
	#[test]
	fn disabled_tall_footprints_compact_each_intersected_row_without_merging_rows() {
		let rows = vec![
			MenuRow(vec![
				MenuEntry::Widget(MenuTile {
					widget: Wifi,
					width: 2,
					height: 2,
				}),
				MenuEntry::Gap(1),
				MenuEntry::Widget(MenuTile::new(Bluetooth)),
			]),
			MenuRow(vec![
				MenuEntry::Gap(3),
				MenuEntry::Widget(MenuTile::new(Nightlight)),
				MenuEntry::Gap(2),
			]),
		];
		let mut session = Session::new(&SystemMenu::from_rows(rows), &[], 6);
		session.toggle(0);
		let compact = session.configuration(true);
		let placed = compact.resolve();
		assert_eq!(
			placed
				.tiles
				.iter()
				.map(|tile| (tile.row, tile.column))
				.collect::<Vec<_>>(),
			vec![(0, 1), (1, 1)]
		);
		assert!(matches!(
			compact.effective_rows()[1].0.last(),
			Some(MenuEntry::Gap(2))
		));
	}
	#[test]
	fn toggles_keep_all_session_slots_still() {
		let layout = SystemMenu::from_widgets(vec![Wifi]);
		let mut session = Session::new(&layout, &[Wifi, Bluetooth, Nightlight], 6);
		let before = session.tiles.clone();
		assert!(session.toggle(1));
		assert_eq!(session.tiles, before);
		assert!(session.toggle(0));
		assert_eq!(session.tiles, before);
	}
	#[test]
	fn exit_removes_disabled_footprints_and_empty_rows_but_preserves_horizontal_gaps() {
		let layout = SystemMenu::from_rows(vec![
			MenuRow(vec![
				MenuEntry::Gap(1),
				MenuEntry::Widget(MenuTile::new(Wifi)),
				MenuEntry::Gap(2),
				MenuEntry::Widget(MenuTile::new(Bluetooth)),
			]),
			MenuRow::default(),
			MenuRow(vec![MenuEntry::Widget(MenuTile::new(Nightlight))]),
		]);
		let mut session = Session::new(&layout, &[], 6);
		assert!(session.toggle(0));
		let before = session.configuration(false).resolve();
		assert_eq!(before.tiles.first().map(|tile| tile.column), Some(5));
		let after = session.configuration(true);
		let placed = after.resolve();
		assert_eq!(placed.tiles.first().map(|tile| tile.column), Some(3));
		assert_eq!(placed.tiles.get(1).map(|tile| tile.row), Some(1));
		assert_eq!(after.effective_rows().len(), 2);
		assert_eq!(placed.tiles.first().map(|tile| tile.tile.width), Some(2));
	}
}
