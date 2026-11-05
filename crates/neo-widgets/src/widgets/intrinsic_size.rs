//! Report a shrink-sized probe's natural geometry while rendering a separate fill-sized view.
//! Useful when model-driven surfaces must round intrinsic constraints to grid units.
use iced::{
	Element, Event, Length, Rectangle, Size, Vector,
	advanced::{
		Layout, Shell, Widget, layout, mouse, overlay, renderer,
		widget::{self, Tree, tree},
	},
};

pub fn intrinsic_size<'a, Message: 'a>(
	probe: impl Into<Element<'a, Message>>, content: impl Into<Element<'a, Message>>,
	on_change: impl Fn(Size) -> Message + 'a,
) -> Element<'a, Message> {
	Element::new(Intrinsic {
		probe: probe.into(),
		content: content.into(),
		on_change: Box::new(on_change),
	})
}
struct Intrinsic<'a, Message> {
	probe: Element<'a, Message>,
	content: Element<'a, Message>,
	on_change: Box<dyn Fn(Size) -> Message + 'a>,
}
#[derive(Default)]
struct State {
	measured: Size,
	reported: Option<Size>,
}
impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Intrinsic<'_, Message> {
	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<State>()
	}
	fn state(&self) -> tree::State {
		tree::State::new(State::default())
	}
	fn size(&self) -> Size<Length> {
		self.content.as_widget().size()
	}
	fn diff(&mut self, tree: &mut Tree) {
		tree.diff_children(&mut [&mut self.probe, &mut self.content]);
	}
	fn layout(
		&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits,
	) -> layout::Node {
		let [probe, content] = tree.children.as_mut_slice() else {
			return layout::Node::new(Size::ZERO);
		};
		let measured = self.probe.as_widget_mut().layout(
			probe,
			renderer,
			&layout::Limits::new(Size::ZERO, Size::new(4096.0, 4096.0)),
		);
		tree.state.downcast_mut::<State>().measured = measured.size();
		let child = self
			.content
			.as_widget_mut()
			.layout(content, renderer, limits);
		layout::Node::with_children(child.size(), vec![child])
	}
	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &iced::Renderer, shell: &mut Shell<'_, Message>, viewport: &Rectangle,
	) {
		let state = tree.state.downcast_mut::<State>();
		if matches!(
			event,
			Event::Window(iced::window::Event::RedrawRequested(_))
		) && state.reported != Some(state.measured)
		{
			state.reported = Some(state.measured);
			shell.publish((self.on_change)(state.measured));
		}
		if let (Some(child), Some(node)) = (tree.children.get_mut(1), layout.children().next()) {
			self.content
				.as_widget_mut()
				.update(child, event, node, cursor, renderer, shell, viewport);
		}
	}
	fn draw(
		&self, tree: &Tree, renderer: &mut iced::Renderer, theme: &iced::Theme,
		style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		if let (Some(child), Some(node)) = (tree.children.get(1), layout.children().next()) {
			self.content
				.as_widget()
				.draw(child, renderer, theme, style, node, cursor, viewport);
		}
	}
	fn operate(
		&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &iced::Renderer,
		operation: &mut dyn widget::Operation,
	) {
		if let (Some(child), Some(node)) = (tree.children.get_mut(1), layout.children().next()) {
			self.content
				.as_widget_mut()
				.operate(child, node, renderer, operation);
		}
	}
	fn mouse_interaction(
		&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
		renderer: &iced::Renderer,
	) -> mouse::Interaction {
		if let (Some(child), Some(node)) = (tree.children.get(1), layout.children().next()) {
			self.content
				.as_widget()
				.mouse_interaction(child, node, cursor, viewport, renderer)
		} else {
			mouse::Interaction::None
		}
	}
	fn overlay<'a>(
		&'a mut self, tree: &'a mut Tree, layout: Layout<'a>, renderer: &iced::Renderer,
		viewport: &Rectangle, offset: Vector,
	) -> Vec<overlay::Element<'a, Message, iced::Theme, iced::Renderer>> {
		if let (Some(child), Some(node)) = (tree.children.get_mut(1), layout.children().next()) {
			self.content
				.as_widget_mut()
				.overlay(child, node, renderer, viewport, offset)
		} else {
			Vec::new()
		}
	}
}
