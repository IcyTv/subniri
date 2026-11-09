use iced::{
	Element, Length,
	advanced::Renderer,
	widget::{Row, container},
};

pub const TILE_WIDTH: f32 = 160.0;
pub const TILE_HEIGHT: f32 = 155.0;
const GAP: f32 = 12.0;

/// Tiles never expand to consume leftover width. Wrapping is decided from the
/// actual content viewport, not window-size breakpoints or a guessed column count.
pub fn fixed_grid<'a, Message: 'a, R: Renderer + 'a>(
	tiles: impl IntoIterator<Item = Element<'a, Message, iced::Theme, R>>,
) -> Element<'a, Message, iced::Theme, R> {
	Row::with_children(
		tiles
			.into_iter()
			.map(|tile| container(tile).width(TILE_WIDTH).height(TILE_HEIGHT).into()),
	)
	.width(Length::Fill)
	.spacing(GAP)
	.wrap()
	.into()
}

#[cfg(test)]
mod tests {
	use super::*;
	use iced::{
		Size,
		advanced::{layout, widget::Tree},
		widget::Space,
	};

	#[test]
	fn resizing_adds_columns_without_stretching_tiles() {
		for (width, columns) in [(331.0, 1), (332.0, 2), (503.0, 2), (504.0, 3), (676.0, 4)] {
			let tiles = (0..9).map(|_| {
				Element::<(), iced::Theme, ()>::from(
					Space::new().width(Length::Fill).height(Length::Fill),
				)
			});
			let mut grid = fixed_grid(tiles);
			let mut tree = Tree::new(grid.as_widget());
			grid.as_widget_mut().diff(&mut tree);
			let node = grid.as_widget_mut().layout(
				&mut tree,
				&(),
				&layout::Limits::new(Size::ZERO, Size::new(width, f32::INFINITY)),
			);
			assert_eq!(
				node.children()
					.iter()
					.filter(|n| n.bounds().y == 0.0)
					.count(),
				columns
			);
			for child in node.children() {
				assert_eq!(child.size(), Size::new(TILE_WIDTH, TILE_HEIGHT));
				assert!(child.bounds().x + TILE_WIDTH <= width);
			}
		}
	}
}
