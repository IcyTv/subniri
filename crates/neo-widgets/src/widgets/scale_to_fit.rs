//! Lay out content at its preferred dimensions and scale it uniformly into a changing region.
//! Useful inside an animated surface: the surface changes shape, its contents never stretch.
use iced::{
	Element, Event, Length, Rectangle, Size, Transformation, Vector,
	advanced::{
		Layout, Shell, Widget, layout, mouse, overlay, renderer,
		widget::{self, Tree},
	},
};

pub fn scale_to_fit<'a, Message: 'a, Renderer: renderer::Renderer + 'a>(
	content: impl Into<Element<'a, Message, iced::Theme, Renderer>>, natural: Size,
) -> Element<'a, Message, iced::Theme, Renderer> {
	Element::new(Fit {
		content: content.into(),
		natural,
	})
}
struct Fit<'a, Message, Renderer> {
	content: Element<'a, Message, iced::Theme, Renderer>,
	natural: Size,
}
fn transform(parent: Layout<'_>, child: Layout<'_>) -> Transformation {
	let bounds = parent.bounds();
	let source = child.bounds();
	let scale = (bounds.width / source.width.max(1.0))
		.min(bounds.height / source.height.max(1.0))
		.max(0.001);
	Transformation::translate(bounds.center_x(), bounds.center_y())
		* Transformation::scale(scale)
		* Transformation::translate(-source.center_x(), -source.center_y())
}
impl<Message, Renderer: renderer::Renderer> Widget<Message, iced::Theme, Renderer>
	for Fit<'_, Message, Renderer>
{
	fn size(&self) -> Size<Length> {
		Size::new(Length::Fill, Length::Fill)
	}
	fn diff(&mut self, tree: &mut Tree) {
		tree.diff_children(std::slice::from_mut(&mut self.content));
	}
	fn layout(
		&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits,
	) -> layout::Node {
		let Some(child_tree) = tree.children.first_mut() else {
			return layout::Node::new(limits.max());
		};
		let natural = Size::new(self.natural.width.max(1.0), self.natural.height.max(1.0));
		let child = self.content.as_widget_mut().layout(
			child_tree,
			renderer,
			&layout::Limits::new(natural, natural),
		);
		layout::Node::with_children(limits.max(), vec![child])
	}
	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &Renderer, shell: &mut Shell<'_, Message>, viewport: &Rectangle,
	) {
		let Some(child_tree) = tree.children.first_mut() else {
			return;
		};
		if let Some(child) = layout.children().next() {
			let inverse = transform(layout, child).inverse();
			self.content.as_widget_mut().update(
				child_tree,
				event,
				child,
				cursor * inverse,
				renderer,
				shell,
				&(*viewport * inverse),
			);
		}
	}
	fn draw(
		&self, tree: &Tree, renderer: &mut Renderer, theme: &iced::Theme, style: &renderer::Style,
		layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		let Some(child_tree) = tree.children.first() else {
			return;
		};
		if let Some(child) = layout.children().next() {
			let transform = transform(layout, child);
			let inverse = transform.inverse();
			let clip = layout.bounds().intersection(viewport).unwrap_or_default();
			renderer.with_layer(clip, |renderer| {
				renderer.with_transformation(transform, |renderer| {
					self.content.as_widget().draw(
						child_tree,
						renderer,
						theme,
						style,
						child,
						cursor * inverse,
						&(clip * inverse),
					)
				})
			});
		}
	}
	fn mouse_interaction(
		&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
		renderer: &Renderer,
	) -> mouse::Interaction {
		let Some(child_tree) = tree.children.first() else {
			return mouse::Interaction::None;
		};
		layout
			.children()
			.next()
			.map_or(mouse::Interaction::None, |child| {
				let inverse = transform(layout, child).inverse();
				self.content.as_widget().mouse_interaction(
					child_tree,
					child,
					cursor * inverse,
					&(*viewport * inverse),
					renderer,
				)
			})
	}
	fn operate(
		&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer,
		operation: &mut dyn widget::Operation,
	) {
		let Some(child_tree) = tree.children.first_mut() else {
			return;
		};
		if let Some(child) = layout.children().next() {
			self.content
				.as_widget_mut()
				.operate(child_tree, child, renderer, operation);
		}
	}
	fn overlay<'a>(
		&'a mut self, tree: &'a mut Tree, layout: Layout<'a>, renderer: &Renderer,
		viewport: &Rectangle, offset: Vector,
	) -> Vec<overlay::Element<'a, Message, iced::Theme, Renderer>> {
		let Some(child_tree) = tree.children.first_mut() else {
			return Vec::new();
		};
		// Transformed content must not spawn an untransformed overlay mid-transition.
		if let Some(child) = layout
			.children()
			.next()
			.filter(|child| child.bounds().size() == layout.bounds().size())
		{
			self.content
				.as_widget_mut()
				.overlay(child_tree, child, renderer, viewport, offset)
		} else {
			Vec::new()
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn resting_content_and_wider_headers_keep_unit_scale() {
		// Generic content uses its resting interior as the natural size. Expanded
		// Nightlight headers retain their natural height even when width grows.
		for (parent, natural) in [
			(Size::new(188.0, 44.0), Size::new(188.0, 44.0)),
			(Size::new(260.0, 116.0), Size::new(260.0, 116.0)),
			(Size::new(404.0, 188.0), Size::new(404.0, 188.0)),
			(Size::new(260.0, 20.0), Size::new(220.0, 20.0)),
			(Size::new(404.0, 20.0), Size::new(220.0, 20.0)),
		] {
			let node = layout::Node::with_children(parent, vec![layout::Node::new(natural)]);
			let parent = Layout::new(&node);
			let child = parent.children().next().expect("natural content");
			assert_eq!(transform(parent, child).scale_factor(), 1.0);
		}
	}
}
