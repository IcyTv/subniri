//! Grid dimensions and reversible spatial edits. No widget state, clocks or I/O.
use config::{MenuEntry, MenuRow, PlacedTile};
use iced::{Point, Rectangle, Size};

pub const WIDGET_SIZE: f32 = 64.0;
pub const SPACING: f32 = 8.0;
pub const PITCH: f32 = WIDGET_SIZE + SPACING;

pub fn span(cells: u32) -> f32 {
	if cells == 0 {
		0.0
	} else {
		cells as f32 * PITCH - SPACING
	}
}
pub fn bounds(tile: &PlacedTile) -> Rectangle {
	Rectangle::new(
		Point::new(tile.column as f32 * PITCH, tile.row as f32 * PITCH),
		Size::new(span(tile.tile.width), span(tile.tile.height)),
	)
}
pub fn extent(tiles: &[PlacedTile], rows: &[MenuRow]) -> Size {
	let columns = tiles
		.iter()
		.map(|tile| tile.column.saturating_add(tile.tile.width))
		.chain(trailing_ends(tiles, rows))
		.max()
		.unwrap_or(0);
	let height = tiles
		.iter()
		.map(|tile| tile.row.saturating_add(tile.tile.height))
		.max()
		.unwrap_or(0)
		.max(rows.len() as u32);
	Size::new(span(columns), span(height))
}

/// Tiles retain occurrence indices during a preview; the active tile has collision priority.
/// Every call starts from the baseline, so retreat/cancel never accumulates empty cells.
pub fn preview(baseline: &[PlacedTile], source: usize, desired: PlacedTile) -> Vec<PlacedTile> {
	let mut output = baseline.to_vec();
	let Some(active) = output.get_mut(source) else {
		return output;
	};
	*active = desired;
	let mut accepted = vec![source];
	let mut order = (0..baseline.len())
		.filter(|index| *index != source)
		.collect::<Vec<_>>();
	order.sort_by_key(|index| baseline.get(*index).map(|tile| (tile.row, tile.column)));
	for index in order {
		loop {
			let Some(tile) = output.get(index) else {
				break;
			};
			let end = accepted
				.iter()
				.filter_map(|other| output.get(*other))
				.filter(|other| tile.overlaps(other))
				.map(|other| other.column.saturating_add(other.tile.width))
				.max();
			let Some(end) = end else {
				break;
			};
			let Some(tile) = output.get_mut(index) else {
				break;
			};
			if end <= tile.column {
				break;
			}
			tile.column = end;
		}
		accepted.push(index);
	}
	output
}

pub fn insert_row_preview(
	baseline: &[PlacedTile], source: usize, mut desired: PlacedTile, row: u32,
) -> Vec<PlacedTile> {
	let shifted = baseline
		.iter()
		.cloned()
		.map(|mut tile| {
			if tile.row >= row {
				tile.row = tile.row.saturating_add(1);
			}
			tile
		})
		.collect::<Vec<_>>();
	desired.row = row;
	preview(&shifted, source, desired)
}

pub fn insert_authored_row(rows: &[MenuRow], row: u32) -> Vec<MenuRow> {
	let mut rows = rows.to_vec();
	rows.resize(rows.len().max(row as usize), MenuRow::default());
	rows.insert(row as usize, MenuRow::default());
	rows
}

pub fn trailing_ends(tiles: &[PlacedTile], rows: &[MenuRow]) -> Vec<u32> {
	rows.iter()
		.enumerate()
		.map(|(index, row)| {
			let end = tiles
				.iter()
				.filter(|tile| tile.row == index as u32)
				.map(|tile| tile.column.saturating_add(tile.tile.width))
				.max();
			if end.is_none() {
				return row
					.0
					.iter()
					.filter_map(|entry| {
						if let MenuEntry::Gap(width) = entry {
							Some(*width)
						} else {
							None
						}
					})
					.fold(0_u32, u32::saturating_add);
			}
			if !matches!(row.0.last(), Some(MenuEntry::Gap(_))) {
				return 0;
			}
			let last = row
				.0
				.iter()
				.rev()
				.take_while(|entry| matches!(entry, MenuEntry::Gap(_)))
				.filter_map(|entry| {
					if let MenuEntry::Gap(width) = entry {
						Some(*width)
					} else {
						None
					}
				})
				.fold(0_u32, u32::saturating_add);
			end.unwrap_or(0).saturating_add(last)
		})
		.collect()
}

#[cfg(test)]
pub fn commit(tiles: &[PlacedTile], baseline_rows: &[MenuRow]) -> Vec<MenuRow> {
	let intentional_rows = baseline_rows
		.iter()
		.enumerate()
		.filter(|(_, row)| {
			row.0.is_empty() || row.0.iter().any(|entry| matches!(entry, MenuEntry::Gap(_)))
		})
		.map(|(index, _)| index as u32 + 1)
		.max()
		.unwrap_or(0);
	config::rows_from_tiles(
		tiles,
		intentional_rows,
		&trailing_ends(tiles, baseline_rows),
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use config::{
		MenuTile,
		SystemMenuWidgets::{Bluetooth, Wifi},
	};
	#[test]
	fn insertion_keeps_crossing_tile_height_and_shifts_only_lower_starts() {
		let tiles = vec![
			PlacedTile {
				tile: MenuTile {
					widget: Wifi,
					width: 2,
					height: 3,
				},
				row: 0,
				column: 0,
			},
			PlacedTile {
				tile: MenuTile::new(Bluetooth),
				row: 1,
				column: 2,
			},
			PlacedTile {
				tile: MenuTile::new(Wifi),
				row: 3,
				column: 0,
			},
		];
		let inserted = insert_row_preview(
			&tiles,
			2,
			PlacedTile {
				tile: MenuTile::new(Wifi),
				row: 1,
				column: 2,
			},
			1,
		);
		assert_eq!(inserted[0], tiles[0]);
		assert_eq!(inserted[1].row, 2);
		assert_eq!(inserted[1].column, 2);
		assert_eq!(inserted[2].row, 1);
		assert_eq!(
			preview(&tiles, 2, tiles[2].clone()),
			tiles,
			"retreat uses the original baseline"
		);
		let rows = vec![MenuRow::default(), MenuRow(vec![MenuEntry::Gap(3)])];
		assert_eq!(
			insert_authored_row(&rows, 1),
			vec![rows[0].clone(), MenuRow::default(), rows[1].clone()]
		);
	}

	#[test]
	fn removing_tiles_preserves_authored_spacers_but_not_automatic_trailing_rows() {
		let rows = vec![
			MenuRow(vec![
				MenuEntry::Gap(2),
				MenuEntry::Widget(MenuTile::new(Wifi)),
			]),
			MenuRow(vec![MenuEntry::Widget(MenuTile::new(Bluetooth))]),
		];
		let remaining = commit(&[], &rows);
		assert_eq!(remaining, vec![MenuRow(vec![MenuEntry::Gap(2)])]);
		assert_eq!(spatial_size(&remaining), Size::new(span(2), span(1)));
	}

	fn spatial_size(rows: &[MenuRow]) -> Size {
		extent(&config::resolve_rows(rows).tiles, rows)
	}
	#[test]
	fn resize_pushes_only_affected_tiles_and_retreat_restores_them()
	-> Result<(), Box<dyn std::error::Error>> {
		let original = vec![
			PlacedTile {
				tile: MenuTile::new(Wifi),
				row: 0,
				column: 0,
			},
			PlacedTile {
				tile: MenuTile::new(Bluetooth),
				row: 1,
				column: 0,
			},
		];
		let first = original.first().ok_or("missing tile")?;
		let mut large = first.clone();
		large.tile.height = 2;
		let expanded = preview(&original, 0, large);
		assert_eq!(expanded.get(1).map(|tile| tile.column), Some(2));
		assert!(
			expanded
				.first()
				.zip(expanded.get(1))
				.is_some_and(|(a, b)| !a.overlaps(b))
		);
		assert_eq!(preview(&original, 0, first.clone()), original);
		assert_eq!(span(1), WIDGET_SIZE);
		assert_eq!(span(2), WIDGET_SIZE * 2.0 + SPACING);
		Ok(())
	}
	#[test]
	fn tall_collision_chains_round_trip_with_explicit_gaps() {
		let rows = vec![
			MenuRow(vec![MenuEntry::Widget(MenuTile {
				widget: Wifi,
				width: 2,
				height: 2,
			})]),
			MenuRow(vec![MenuEntry::Widget(MenuTile {
				widget: Bluetooth,
				width: 2,
				height: 2,
			})]),
			MenuRow::default(),
		];
		let resolved = config::resolve_rows(&rows);
		let written = commit(&resolved.tiles, &rows);
		assert_eq!(config::resolve_rows(&written).tiles, resolved.tiles);
		assert!(config::resolve_rows(&written).displaced.is_empty());
		assert_eq!(written.len(), 3);
	}
}
