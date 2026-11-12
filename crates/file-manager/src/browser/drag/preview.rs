//! A draw-only drag overlay that leaves the content's retained tree untouched.
use crate::Message;
use iced::{
	Element, Event, Length, Point, Rectangle, Size, Vector,
	advanced::{
		Layout, Shell, Widget, layout, mouse, overlay, renderer,
		widget::{Operation, Tree, tree},
	},
};

pub(crate) fn with_preview<'a>(
	content: Element<'a, Message>, icon: Element<'a, Message>, point: Point,
) -> Element<'a, Message> {
	let mut icon_tree = Tree::new(icon.as_widget());
	let mut icon = icon;
	icon_tree.diff(icon.as_widget_mut());

	Element::new(Preview {
		content,
		icon,
		icon_tree,
		point,
	})
}

struct Preview<'a> {
	content: Element<'a, Message>,
	icon: Element<'a, Message>,
	icon_tree: Tree,
	point: Point,
}

impl Widget<Message, iced::Theme, iced::Renderer> for Preview<'_> {
	fn size(&self) -> Size<Length> {
		self.content.as_widget().size()
	}

	fn tag(&self) -> tree::Tag {
		self.content.as_widget().tag()
	}

	fn state(&self) -> tree::State {
		self.content.as_widget().state()
	}

	fn diff(&mut self, tree: &mut Tree) {
		// Share the exact content tree, including the gate's pressed state, even
		// when this conditional wrapper is mounted or removed during a gesture.
		self.content.as_widget_mut().diff(tree);
	}

	fn layout(
		&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits,
	) -> layout::Node {
		self.content.as_widget_mut().layout(tree, renderer, limits)
	}

	fn operate(
		&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &iced::Renderer,
		operation: &mut dyn Operation,
	) {
		self.content
			.as_widget_mut()
			.operate(tree, layout, renderer, operation);
	}

	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &iced::Renderer, shell: &mut Shell<'_, Message>, viewport: &Rectangle,
	) {
		self.content
			.as_widget_mut()
			.update(tree, event, layout, cursor, renderer, shell, viewport);
	}

	fn mouse_interaction(
		&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
		renderer: &iced::Renderer,
	) -> mouse::Interaction {
		self.content
			.as_widget()
			.mouse_interaction(tree, layout, cursor, viewport, renderer)
	}

	fn draw(
		&self, tree: &Tree, renderer: &mut iced::Renderer, theme: &iced::Theme,
		style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		self.content
			.as_widget()
			.draw(tree, renderer, theme, style, layout, cursor, viewport);
	}

	fn overlay<'b>(
		&'b mut self, tree: &'b mut Tree, layout: Layout<'b>, renderer: &iced::Renderer,
		viewport: &Rectangle, translation: Vector,
	) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
		let mut overlays =
			self.content
				.as_widget_mut()
				.overlay(tree, layout, renderer, viewport, translation);
		overlays.push(overlay::Element::new(Box::new(Icon {
			icon: &mut self.icon,
			tree: &mut self.icon_tree,
			point: self.point,
		})));
		overlays
	}
}

struct Icon<'a, 'b> {
	icon: &'b mut Element<'a, Message>,
	tree: &'b mut Tree,
	point: Point,
}

impl overlay::Overlay<Message, iced::Theme, iced::Renderer> for Icon<'_, '_> {
	fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
		let node = self.icon.as_widget_mut().layout(
			self.tree,
			renderer,
			&layout::Limits::new(Size::ZERO, bounds),
		);
		let position = preview_position(self.point, node.size(), bounds);

		node.move_to(position)
	}

	fn draw(
		&self, renderer: &mut iced::Renderer, theme: &iced::Theme, style: &renderer::Style,
		layout: Layout<'_>, _: mouse::Cursor,
	) {
		self.icon.as_widget().draw(
			self.tree,
			renderer,
			theme,
			style,
			layout,
			mouse::Cursor::Unavailable,
			&layout.bounds(),
		);
	}

	// Keep Overlay's no-op update/operate/overlay and None mouse interaction.
	// In particular, never expose even a container to hit-test operations.
}

fn preview_position(point: Point, icon: Size, viewport: Size) -> Point {
	// Cursor coordinates already use window logical pixels. Overlay bounds are
	// the host viewport, not the content's possibly scrolled local rectangle.
	Point::new(
		(point.x + 12.0).clamp(0.0, (viewport.width - icon.width).max(0.0)),
		(point.y + 12.0).clamp(0.0, (viewport.height - icon.height).max(0.0)),
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn follows_cursor_and_clamps_all_viewport_edges() {
		let icon = Size::new(80.0, 60.0);
		let viewport = Size::new(300.0, 200.0);

		assert_eq!(
			preview_position(Point::new(20.0, 30.0), icon, viewport),
			Point::new(32.0, 42.0)
		);
		assert_eq!(
			preview_position(Point::new(299.0, 199.0), icon, viewport),
			Point::new(220.0, 140.0)
		);
		assert_eq!(
			preview_position(Point::new(-100.0, -100.0), icon, viewport),
			Point::ORIGIN
		);
		assert_eq!(
			preview_position(Point::ORIGIN, icon, Size::ZERO),
			Point::ORIGIN
		);

		for point in [
			Point::ORIGIN,
			Point::new(299.0, 199.0),
			Point::new(-20.0, -20.0),
		] {
			let node = layout::Node::new(icon).move_to(preview_position(point, icon, viewport));
			let bounds = node.bounds();
			assert_eq!(bounds.size(), icon);
			assert!(bounds.x >= 0.0 && bounds.y >= 0.0);
			assert!(bounds.x + bounds.width <= viewport.width);
			assert!(bounds.y + bounds.height <= viewport.height);
		}
	}

	#[test]
	fn conditional_preview_preserves_content_tree_and_size() {
		#[derive(Default)]
		struct Retained(u32);

		struct Content;

		impl Widget<Message, iced::Theme, iced::Renderer> for Content {
			fn size(&self) -> Size<Length> {
				Size::new(Length::Fill, Length::Fixed(100.0))
			}

			fn tag(&self) -> tree::Tag {
				tree::Tag::of::<Retained>()
			}

			fn state(&self) -> tree::State {
				tree::State::new(Retained::default())
			}

			fn layout(
				&mut self, _: &mut Tree, _: &iced::Renderer, _: &layout::Limits,
			) -> layout::Node {
				layout::Node::new(Size::new(300.0, 100.0))
			}

			fn draw(
				&self, _: &Tree, _: &mut iced::Renderer, _: &iced::Theme, _: &renderer::Style,
				_: Layout<'_>, _: mouse::Cursor, _: &Rectangle,
			) {
			}
		}

		let mut content: Element<'_, Message> = Element::new(Content);
		let size = content.as_widget().size();
		let mut tree = Tree::new(content.as_widget());
		tree.state.downcast_mut::<Retained>().0 = 42;
		let mut wrapped = with_preview(content, Element::new(Content), Point::ORIGIN);

		tree.diff(wrapped.as_widget_mut());
		assert_eq!(tree.state.downcast_ref::<Retained>().0, 42);
		assert_eq!(wrapped.as_widget().size(), size);
		assert!(tree.children.is_empty());

		content = Element::new(Content);
		tree.diff(content.as_widget_mut());
		assert_eq!(tree.state.downcast_ref::<Retained>().0, 42);
	}
}
