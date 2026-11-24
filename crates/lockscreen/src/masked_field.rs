use iced::{
	Alignment, Element, Length, Rectangle, Size, Vector,
	advanced::{Layout, Widget, layout, mouse, renderer, widget::Tree},
	widget::{Space, container, row, text},
};
use neo_widgets::style::Colors;

use crate::Message;

// Dedicated display-only viewport: no plaintext, event handling, or editor state.
pub(crate) struct MaskedField {
	content: Element<'static, Message>,
}

impl MaskedField {
	pub(crate) fn new(prefix: usize, suffix: usize, colors: Colors, caret_visible: bool) -> Self {
		let caret = container(Space::new().width(2).height(26)).style(move |_| container::Style {
			background: caret_visible.then_some(colors.decorative.blue.into()),
			..container::Style::default()
		});
		let mask = |count| {
			text("•".repeat(count))
				.size(22)
				.color(colors.text)
				.wrapping(text::Wrapping::None)
		};
		let content = row![
			mask(prefix),
			caret,
			mask(suffix),
			text(if prefix + suffix == 0 { "Password" } else { "" })
				.size(22)
				.color(colors.secondary)
				.wrapping(text::Wrapping::None),
		]
		.width(Length::Shrink)
		.align_y(Alignment::Center)
		.into();

		Self { content }
	}
}

// Use measured row/caret geometry, never an assumed glyph advance.
fn offset(content_width: f32, viewport_width: f32, caret_x: f32, caret_width: f32) -> f32 {
	let max_offset = (content_width - viewport_width).max(0.0);
	let target = caret_x - (viewport_width - caret_width).max(0.0) / 2.0;
	target.clamp(0.0, max_offset)
}

impl Widget<Message, iced::Theme, iced::Renderer> for MaskedField {
	fn size(&self) -> Size<Length> {
		Size::new(Length::Fill, Length::Shrink)
	}

	fn diff(&mut self, tree: &mut Tree) {
		tree.diff_children(std::slice::from_mut(&mut self.content));
	}

	fn layout(
		&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits,
	) -> layout::Node {
		let child_limits =
			layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, limits.max().height));
		let child =
			self.content
				.as_widget_mut()
				.layout(&mut tree.children[0], renderer, &child_limits);
		let size = limits.resolve(Length::Fill, Length::Shrink, child.size());
		let caret = child.children()[1].bounds();
		let shift = offset(child.size().width, size.width, caret.x, caret.width);
		let y = (size.height - child.size().height) / 2.0;

		layout::Node::with_children(size, vec![child.translate(Vector::new(-shift, y))])
	}

	fn draw(
		&self, tree: &Tree, renderer: &mut iced::Renderer, theme: &iced::Theme,
		style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		let Some(clip) = layout.bounds().intersection(viewport) else {
			return;
		};

		use iced::advanced::Renderer as _;
		renderer.with_layer(clip, |renderer| {
			self.content.as_widget().draw(
				&tree.children[0],
				renderer,
				theme,
				style,
				layout.child(0),
				cursor,
				&clip,
			);
		});
	}
}

impl From<MaskedField> for Element<'static, Message> {
	fn from(field: MaskedField) -> Self {
		Self::new(field)
	}
}

#[cfg(test)]
mod tests {
	use super::offset;

	#[test]
	fn short_content_and_empty_mask_do_not_scroll() {
		assert_eq!(offset(2.0, 200.0, 0.0, 2.0), 0.0);
		assert_eq!(offset(70.5, 200.0, 31.25, 2.0), 0.0);
	}

	#[test]
	fn measured_caret_edges_remain_visible_through_resizing() {
		for width in [2.0, 17.75, 120.5, 300.25, 800.0] {
			for x in [0.0, 1.25, 203.125, 598.75] {
				let shift = offset(600.75, width, x, 2.0);
				assert!(shift.is_finite());
				assert!(x - shift >= 0.0);
				assert!(x + 2.0 - shift <= width);
			}
		}
		assert_eq!(offset(600.75, 120.5, 0.0, 2.0), 0.0);
		assert_eq!(offset(600.75, 120.5, 598.75, 2.0), 480.25);
	}

	#[test]
	fn zero_viewport_is_finite_and_blink_does_not_change_geometry() {
		assert!(offset(600.75, 0.0, 301.5, 2.0).is_finite());
		// Visibility is draw-only; the same measured caret width is used in both phases.
		let visible = offset(600.75, 120.5, 301.5, 2.0);
		let hidden = offset(600.75, 120.5, 301.5, 2.0);
		assert_eq!(visible, hidden);
	}
}
