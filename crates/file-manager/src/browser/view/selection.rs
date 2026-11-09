use crate::Message;
use iced::{
	Rectangle, Task, Vector,
	advanced::widget::{
		Id, Operation,
		operation::{
			Outcome,
			focusable::Focusable,
			scrollable::{self, AbsoluteOffset, Scrollable},
		},
	},
};

pub const FILES_ID: &str = "files";

/// Use post-layout rows, so grid navigation follows wrapping after every resize.
pub(crate) fn destination(
	entries: &[(std::path::PathBuf, Rectangle)], focused: Option<&std::path::PathBuf>,
	key: &iced::keyboard::Key,
) -> Option<std::path::PathBuf> {
	use iced::keyboard::{Key, key::Named};
	let Key::Named(
		named @ (Named::ArrowLeft
		| Named::ArrowRight
		| Named::ArrowUp
		| Named::ArrowDown
		| Named::Home
		| Named::End),
	) = key
	else {
		return None;
	};
	if *named == Named::Home {
		return entries.first().map(|e| e.0.clone());
	}
	if *named == Named::End {
		return entries.last().map(|e| e.0.clone());
	}
	let Some(index) = entries.iter().position(|(p, _)| Some(p) == focused) else {
		return entries.first().map(|e| e.0.clone());
	};
	let (_, current) = entries.get(index)?;
	let target = match named {
		Named::ArrowLeft => entries.get(index.saturating_sub(1)),
		Named::ArrowRight => entries.get(index + 1).or_else(|| entries.get(index)),
		Named::ArrowUp | Named::ArrowDown => entries
			.iter()
			.filter(|(_, b)| {
				if *named == Named::ArrowUp {
					b.y < current.y - 1.0
				} else {
					b.y > current.y + 1.0
				}
			})
			.min_by(|(_, a), (_, b)| {
				(a.y - current.y)
					.abs()
					.total_cmp(&(b.y - current.y).abs())
					.then_with(|| {
						(a.center_x() - current.center_x())
							.abs()
							.total_cmp(&(b.center_x() - current.center_x()).abs())
					})
			})
			.or_else(|| entries.get(index)),
		_ => None,
	};
	target.map(|e| e.0.clone())
}

/// Descendants of a scrollable receive content-space cursor positions. Native
/// popup anchors are relative to the parent window, not that scrolled content.
pub(crate) fn window_point(point: iced::Point) -> Task<iced::Point> {
	iced_runtime::task::widget(WindowPoint {
		point,
		translation: Vector::ZERO,
	})
}

struct WindowPoint {
	point: iced::Point,
	translation: Vector,
}

impl Operation<iced::Point> for WindowPoint {
	fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<iced::Point>)) {
		operate(self);
	}
	fn scrollable(
		&mut self, id: Option<&Id>, _: Rectangle, _: Rectangle, translation: Vector,
		_: &mut dyn Scrollable,
	) {
		if id == Some(&Id::from(FILES_ID)) {
			self.translation = translation;
		}
	}
	fn finish(&self) -> Outcome<iced::Point> {
		Outcome::Some(self.point - self.translation)
	}
}

/// Focus and reveal a restored entry using its real post-layout bounds, so it
/// works for both list rows and wrapped tiles without guessing row/column counts.
pub(crate) fn reveal_selection(target: Id) -> Task<Message> {
	iced::advanced::widget::operate(Reveal {
		target,
		viewport: None,
		selected: None,
	})
}

struct Reveal {
	target: Id,
	viewport: Option<(Rectangle, Vector)>,
	selected: Option<Rectangle>,
}

impl Operation<Message> for Reveal {
	fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Message>)) {
		operate(self);
	}
	fn scrollable(
		&mut self, id: Option<&Id>, bounds: Rectangle, _content: Rectangle, translation: Vector,
		_state: &mut dyn Scrollable,
	) {
		if id == Some(&Id::from(FILES_ID)) {
			self.viewport = Some((bounds, translation));
		}
	}
	fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
		if id == Some(&self.target) {
			self.selected = Some(bounds);
			state.focus();
		} else {
			state.unfocus();
		}
	}
	fn finish(&self) -> Outcome<Message> {
		if let (Some((viewport, translation)), Some(selected)) = (self.viewport, self.selected) {
			let y = reveal_offset(viewport, translation.y, selected);
			return Outcome::Chain(Box::new(scrollable::scroll_to(
				Id::from(FILES_ID),
				AbsoluteOffset {
					x: None,
					y: Some(y),
				},
			)));
		}
		Outcome::None
	}
}

fn reveal_offset(viewport: Rectangle, current: f32, selected: Rectangle) -> f32 {
	let top = selected.y - viewport.y;
	let bottom = top + selected.height;
	if top < current {
		top.max(0.0)
	} else if bottom > current + viewport.height {
		(bottom - viewport.height).max(0.0)
	} else {
		current
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn navigation_uses_real_grid_rows_and_clamps_ragged_edges() {
		use iced::keyboard::{Key, key::Named};
		for columns in [1, 2, 3] {
			let entries = (0..5)
				.map(|i| {
					(
						std::path::PathBuf::from(i.to_string()),
						Rectangle {
							x: (i % columns) as f32 * 172.0,
							y: (i / columns) as f32 * 167.0,
							width: 160.0,
							height: 155.0,
						},
					)
				})
				.collect::<Vec<_>>();
			let first = std::path::PathBuf::from("0");
			assert_eq!(
				destination(&entries, Some(&first), &Key::Named(Named::ArrowDown)),
				Some(std::path::PathBuf::from(columns.to_string()))
			);
			assert_eq!(
				destination(&entries, Some(&first), &Key::Named(Named::ArrowUp)),
				Some(first.clone())
			);
			assert_eq!(
				destination(&entries, None, &Key::Named(Named::Home)),
				Some(first)
			);
			assert_eq!(
				destination(&entries, None, &Key::Named(Named::End)),
				Some("4".into())
			);
			assert_eq!(
				destination(&entries, Some(&"4".into()), &Key::Named(Named::ArrowRight)),
				Some("4".into())
			);
		}
	}

	#[test]
	fn pointer_anchor_subtracts_the_file_scroll_offset_once() {
		use iced::{
			Element, Size,
			advanced::{
				Layout, layout,
				widget::{Tree, operation},
			},
			widget::{Space, scrollable},
		};
		let mut widget: Element<'_, (), iced::Theme, ()> = scrollable(Space::new().height(2000))
			.id(FILES_ID)
			.height(200)
			.into();
		let mut tree = Tree::new(widget.as_widget());
		widget.as_widget_mut().diff(&mut tree);
		let node = widget.as_widget_mut().layout(
			&mut tree,
			&(),
			&layout::Limits::new(Size::ZERO, Size::new(300.0, 200.0)),
		);
		widget.as_widget_mut().operate(
			&mut tree,
			Layout::new(&node),
			&(),
			&mut operation::scrollable::scroll_to::<()>(
				Id::from(FILES_ID),
				AbsoluteOffset {
					x: None,
					y: Some(500.0),
				},
			),
		);
		let mut anchor = WindowPoint {
			point: iced::Point::new(100.0, 780.0),
			translation: Vector::ZERO,
		};
		widget.as_widget_mut().operate(
			&mut tree,
			Layout::new(&node),
			&(),
			&mut operation::black_box(&mut anchor),
		);
		assert!(
			matches!(anchor.finish(), Outcome::Some(point) if point == iced::Point::new(100.0, 280.0))
		);
	}
	#[test]
	fn restored_entries_are_revealed_without_moving_already_visible_rows() {
		let viewport = Rectangle {
			x: 0.0,
			y: 100.0,
			width: 600.0,
			height: 300.0,
		};
		let item = |y| Rectangle {
			x: 0.0,
			y,
			width: 160.0,
			height: 155.0,
		};
		assert_eq!(reveal_offset(viewport, 0.0, item(120.0)), 0.0);
		assert_eq!(reveal_offset(viewport, 0.0, item(700.0)), 455.0);
		assert_eq!(reveal_offset(viewport, 500.0, item(120.0)), 20.0);
	}
}
