//! Row-oriented quick-tile configuration and integer-grid placement, without UI or I/O.
use config_traits::{Config, ConfigError, ConfigSerialize, ConfigValue};
use kdl::{KdlDocument, KdlEntry, KdlNode};

use crate::{SystemMenu, SystemMenuWidgets};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuTile {
	pub widget: SystemMenuWidgets,
	pub width: u32,
	pub height: u32,
}

impl MenuTile {
	pub fn new(widget: SystemMenuWidgets) -> Self {
		Self {
			widget,
			width: 2,
			height: 1,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
	Widget(MenuTile),
	Gap(u32),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MenuRow(pub Vec<MenuEntry>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedTile {
	pub tile: MenuTile,
	pub column: u32,
	pub row: u32,
}

impl PlacedTile {
	pub fn overlaps(&self, other: &Self) -> bool {
		u64::from(self.column) < u64::from(other.column) + u64::from(other.tile.width)
			&& u64::from(other.column) < u64::from(self.column) + u64::from(self.tile.width)
			&& u64::from(self.row) < u64::from(other.row) + u64::from(other.tile.height)
			&& u64::from(other.row) < u64::from(self.row) + u64::from(self.tile.height)
	}
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResolvedMenu {
	pub tiles: Vec<PlacedTile>,
	pub columns: u32,
	pub rows: u32,
	/// Tile indices displaced by implicit obstruction resolution.
	pub displaced: Vec<usize>,
}

impl SystemMenu {
	#[must_use]
	pub fn from_widgets(widgets: Vec<SystemMenuWidgets>) -> Self {
		Self {
			widgets,
			rows: Vec::new(),
		}
	}

	pub fn effective_rows(&self) -> Vec<MenuRow> {
		if !self.rows.is_empty() {
			return self.rows.clone();
		}
		self.widgets
			.chunks(2)
			.map(|widgets| {
				MenuRow(
					widgets
						.iter()
						.cloned()
						.map(|widget| MenuEntry::Widget(MenuTile::new(widget)))
						.collect(),
				)
			})
			.collect()
	}

	pub fn from_rows(rows: Vec<MenuRow>) -> Self {
		Self {
			widgets: Vec::new(),
			rows,
		}
	}

	pub fn resolve(&self) -> ResolvedMenu {
		resolve_rows(&self.effective_rows())
	}
}

/// Resolve each declared row from the left, advancing over whole rectangular obstructions.
/// Work is proportional to tile collisions, never to the number of occupied cells.
pub fn resolve_rows(rows: &[MenuRow]) -> ResolvedMenu {
	let mut result = ResolvedMenu {
		rows: u32::try_from(rows.len()).unwrap_or(u32::MAX),
		..Default::default()
	};
	for (row, entries) in rows.iter().enumerate() {
		let row = u32::try_from(row).unwrap_or(u32::MAX);
		let mut column = 0_u32;
		for entry in &entries.0 {
			match entry {
				MenuEntry::Gap(width) => column = column.saturating_add(*width),
				MenuEntry::Widget(tile) => {
					let mut placed = PlacedTile {
						tile: tile.clone(),
						column,
						row,
					};
					while let Some(end) = result
						.tiles
						.iter()
						.filter(|other| placed.overlaps(other))
						.map(|other| other.column.saturating_add(other.tile.width))
						.max()
					{
						if end <= placed.column {
							break;
						}
						placed.column = end;
					}
					if placed.column != column {
						result.displaced.push(result.tiles.len());
					}
					column = placed.column.saturating_add(tile.width);
					result.rows = result.rows.max(row.saturating_add(tile.height));
					result.tiles.push(placed);
				}
			}
			result.columns = result.columns.max(column);
		}
	}
	result
}

/// Serialize a spatial edit with explicit gaps, retaining deliberate trailing space.
pub fn rows_from_tiles(tiles: &[PlacedTile], row_count: u32, trailing: &[u32]) -> Vec<MenuRow> {
	let count = tiles
		.iter()
		.map(|tile| tile.row.saturating_add(1))
		.max()
		.unwrap_or(0)
		.max(row_count);
	(0..count)
		.map(|row| {
			let mut entries = Vec::new();
			let mut column = 0;
			let mut members = tiles
				.iter()
				.filter(|tile| tile.row == row)
				.collect::<Vec<_>>();
			members.sort_by_key(|tile| tile.column);
			for tile in members {
				if tile.column > column {
					entries.push(MenuEntry::Gap(tile.column - column));
				}
				entries.push(MenuEntry::Widget(tile.tile.clone()));
				column = tile.column.saturating_add(tile.tile.width);
			}
			if let Some(end) = trailing.get(row as usize).filter(|end| **end > column) {
				entries.push(MenuEntry::Gap(*end - column));
			}
			MenuRow(entries)
		})
		.collect()
}

fn dimension(node: &KdlNode, name: &str, default: u32) -> Result<u32, ConfigError> {
	node.get(name).map_or(Ok(default), |value| {
		let invalid = || {
			ConfigError::invalid_value(
				format!(
					"{} {}: {name} must be a positive integer no greater than {}",
					node.name().value(),
					node.get(0).map(ToString::to_string).unwrap_or_default(),
					u32::MAX
				),
				Some(node.span()),
			)
		};
		let value = u32::from_kdl_value(value).map_err(|_| invalid())?;
		if value == 0 {
			Err(invalid())
		} else {
			Ok(value)
		}
	})
}

fn no_arguments(node: &KdlNode) -> Result<(), ConfigError> {
	if node.entries().is_empty() {
		Ok(())
	} else {
		Err(ConfigError::wrong_type(Some(node.span())).expected("no arguments or properties"))
	}
}

impl Config for SystemMenu {
	fn from_kdl_node(node: &KdlNode) -> Result<Self, ConfigError> {
		no_arguments(node)?;
		let mut result = Self::default();
		let mut legacy = false;
		for child in node.children().iter().flat_map(|doc| doc.nodes()) {
			match child.name().value() {
				"widgets" => {
					if legacy {
						return Err(ConfigError::duplicate_field("widgets", Some(child.span())));
					}
					legacy = true;
					if let Some(children) = child.children() {
						no_arguments(child)?;
						for item in children.nodes() {
							if item.name().value() != "-" {
								return Err(ConfigError::unknown_field(
									item.name().value(),
									Some(item.span()),
								));
							}
							result.widgets.push(SystemMenuWidgets::from_kdl_node(item)?);
						}
					} else {
						for entry in child.entries() {
							if entry.name().is_some() {
								return Err(ConfigError::wrong_type(Some(entry.span()))
									.expected("widget argument"));
							}
							result
								.widgets
								.push(SystemMenuWidgets::from_kdl_value(entry.value())?);
						}
					}
				}
				"row" => {
					no_arguments(child)?;
					let mut row = MenuRow::default();
					for item in child.children().iter().flat_map(|doc| doc.nodes()) {
						let kind = item.name().value();
						if kind != "widget" && kind != "gap" {
							return Err(ConfigError::unknown_field(kind, Some(item.span())));
						}
						if item.children().is_some() {
							return Err(
								ConfigError::wrong_type(Some(item.span())).expected("no children")
							);
						}
						let mut properties = std::collections::HashSet::new();
						for entry in item.entries() {
							if let Some(name) = entry.name() {
								if !properties.insert(name.value()) {
									return Err(ConfigError::duplicate_field(
										name.value(),
										Some(entry.span()),
									));
								}
								if name.value() != "width"
									&& !(kind == "widget" && name.value() == "height")
								{
									return Err(ConfigError::unknown_field(
										name.value(),
										Some(entry.span()),
									));
								}
							}
						}
						let arguments = item
							.entries()
							.iter()
							.filter(|entry| entry.name().is_none())
							.count();
						if arguments != usize::from(kind == "widget") {
							return Err(ConfigError::wrong_type(Some(item.span())).expected(
								if kind == "widget" {
									"one widget identity"
								} else {
									"no arguments"
								},
							));
						}
						let width = dimension(item, "width", if kind == "widget" { 2 } else { 1 })?;
						row.0.push(if kind == "gap" {
							MenuEntry::Gap(width)
						} else {
							let identity = item.get(0).ok_or_else(|| {
								ConfigError::missing_field("widget identity", Some(item.span()))
							})?;
							MenuEntry::Widget(MenuTile {
								widget: SystemMenuWidgets::from_kdl_value(identity)?,
								width,
								height: dimension(item, "height", 1)?,
							})
						});
					}
					result.rows.push(row);
				}
				name => return Err(ConfigError::unknown_field(name, Some(child.span()))),
			}
		}
		if legacy && !result.rows.is_empty() {
			return Err(ConfigError::duplicate_field(
				"layout (use either widgets or rows)",
				Some(node.span()),
			));
		}
		if result.resolve().tiles.iter().any(|tile| {
			tile.column.checked_add(tile.tile.width).is_none()
				|| tile.row.checked_add(tile.tile.height).is_none()
		}) {
			return Err(ConfigError::out_of_range(Some(node.span())));
		}
		Ok(result)
	}
}

impl ConfigSerialize for SystemMenu {
	fn apply_to_kdl_node(&self, node: &mut KdlNode) -> Result<(), ConfigError> {
		let mut children = KdlDocument::new();
		for row in self.effective_rows() {
			let mut row_node = KdlNode::new("row");
			let mut entries = KdlDocument::new();
			for entry in row.0 {
				let mut item = KdlNode::new(match entry {
					MenuEntry::Widget(_) => "widget",
					MenuEntry::Gap(_) => "gap",
				});
				let (width, default) = match entry {
					MenuEntry::Widget(tile) => {
						item.push(tile.widget.to_kdl_value());
						if tile.height != 1 {
							item.push(KdlEntry::new_prop("height", i128::from(tile.height)));
						}
						(tile.width, 2)
					}
					MenuEntry::Gap(width) => (width, 1),
				};
				if width != default {
					item.push(KdlEntry::new_prop("width", i128::from(width)));
				}
				entries.nodes_mut().push(item);
			}
			row_node.set_children(entries);
			children.nodes_mut().push(row_node);
		}
		node.set_children(children);
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	fn parse(text: &str) -> Result<SystemMenu, ConfigError> {
		let doc: KdlDocument = text.parse()?;
		SystemMenu::from_kdl_node(
			doc.nodes()
				.first()
				.ok_or_else(|| ConfigError::missing_field("system_menu", None))?,
		)
	}

	#[test]
	fn rows_gaps_and_tall_obstructions_round_trip() -> Result<(), ConfigError> {
		let menu = parse(
			"system_menu { row { widget \"nightlight\" width=2 height=2; }; row { gap width=2; widget \"wifi\" width=1; }; row {}; }",
		)?;
		let resolved = menu.resolve();
		assert_eq!((resolved.columns, resolved.rows), (3, 3));
		assert!(resolved.displaced.is_empty());
		assert_eq!(
			resolved.tiles.get(1).map(|tile| (tile.column, tile.row)),
			Some((2, 1))
		);
		let mut node = KdlNode::new("system_menu");
		menu.apply_to_kdl_node(&mut node)?;
		assert_eq!(SystemMenu::from_kdl_node(&node)?, menu);
		Ok(())
	}

	#[test]
	fn implicit_gap_displaces_without_overlapping() -> Result<(), ConfigError> {
		let menu = parse(
			"system_menu { row { widget \"nightlight\" width=2 height=2; }; row { widget \"wifi\" width=1; }; }",
		)?;
		let resolved = menu.resolve();
		assert_eq!(resolved.displaced, vec![1]);
		assert_eq!(resolved.tiles.get(1).map(|tile| tile.column), Some(2));
		let normalized = SystemMenu::from_rows(rows_from_tiles(&resolved.tiles, 2, &[]));
		assert_eq!(normalized.resolve().tiles, resolved.tiles);
		assert!(normalized.resolve().displaced.is_empty());
		Ok(())
	}

	#[test]
	fn legacy_and_validation() -> Result<(), ConfigError> {
		let menu = parse("system_menu { widgets wifi bluetooth nightlight; }")?;
		assert_eq!(menu.effective_rows().len(), 2);
		assert_eq!(menu.resolve().columns, 4);
		for bad in ["width=0", "height=-1", "width=1.5", "width=2 unknown=3"] {
			assert!(
				parse(&format!(
					"system_menu {{ row {{ widget \"wifi\" {bad}; }}; }}"
				))
				.is_err()
			);
		}
		assert!(parse("system_menu { row { widget \"wifi\" width=10 height=8; }; }").is_ok());
		Ok(())
	}
}
