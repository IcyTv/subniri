//! Records the production whole-popup transform on actual WGPU draw calls.
use iced::{
	Element, Event, Length, Rectangle, Size, Vector,
	advanced::{
		Layout, Shell, Widget, layout, mouse, overlay, renderer,
		widget::{self, Tree, tree},
	},
};
use neo_widgets::widgets::GrowFrame;
use serde_json::json;
use std::any::Any;
use std::io::Write;

pub fn audit<'a, Message: 'a>(
	content: Element<'a, Message>, name: &'static str,
) -> Element<'a, Message> {
	Element::new(Audit { content, name })
}
struct Audit<'a, Message> {
	content: Element<'a, Message>,
	name: &'static str,
}
#[derive(Default)]
struct Snapshot {
	frame: Option<GrowFrame>,
	grow_count: usize,
	texts: Vec<(String, Rectangle)>,
}
impl widget::Operation for Snapshot {
	fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn widget::Operation)) {
		operate(self);
	}
	fn custom(&mut self, _: Option<&widget::Id>, _: Rectangle, state: &mut dyn Any) {
		if let Some(frame) = state.downcast_ref::<GrowFrame>() {
			self.frame = Some(*frame);
			self.grow_count += 1;
		}
	}
	fn text(&mut self, _: Option<&widget::Id>, bounds: Rectangle, text: &str) {
		self.texts.push((text.to_owned(), bounds));
	}
}
impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Audit<'_, Message> {
	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<Snapshot>()
	}
	fn state(&self) -> tree::State {
		tree::State::new(Snapshot::default())
	}
	fn diff(&mut self, tree: &mut Tree) {
		tree.diff_children(std::slice::from_mut(&mut self.content));
	}
	fn size(&self) -> Size<Length> {
		self.content.as_widget().size()
	}
	fn layout(
		&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits,
	) -> layout::Node {
		let child = self
			.content
			.as_widget_mut()
			.layout(&mut tree.children[0], renderer, limits);
		let mut snapshot = Snapshot::default();
		self.content.as_widget_mut().operate(
			&mut tree.children[0],
			Layout::new(&child),
			renderer,
			&mut snapshot,
		);
		*tree.state.downcast_mut::<Snapshot>() = snapshot;
		layout::Node::with_children(child.size(), vec![child])
	}
	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &iced::Renderer, shell: &mut Shell<'_, Message>, viewport: &Rectangle,
	) {
		let node = layout.children().next().expect("audit child");
		self.content.as_widget_mut().update(
			&mut tree.children[0],
			event,
			node,
			cursor,
			renderer,
			shell,
			viewport,
		);
		if matches!(
			event,
			Event::Window(iced::window::Event::RedrawRequested(_))
		) {
			let mut snapshot = Snapshot::default();
			self.content.as_widget_mut().operate(
				&mut tree.children[0],
				node,
				renderer,
				&mut snapshot,
			);
			*tree.state.downcast_mut::<Snapshot>() = snapshot;
		}
	}
	fn draw(
		&self, tree: &Tree, renderer: &mut iced::Renderer, theme: &iced::Theme,
		style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		self.content.as_widget().draw(
			&tree.children[0],
			renderer,
			theme,
			style,
			layout.children().next().expect("audit child"),
			cursor,
			viewport,
		);
		let snapshot = tree.state.downcast_ref::<Snapshot>();
		let rect = |rect: Rectangle| [rect.x, rect.y, rect.width, rect.height];
		let line = format!(
			"OPENING {}\n",
			json!({"module":self.name,"grow_count":snapshot.grow_count,"frame":snapshot.frame.map(|frame|json!({"bounds":rect(frame.bounds),"scaled_bounds":rect(frame.bounds * frame.transformation),"scale":frame.transformation.scale_factor()})),"texts":snapshot.texts.iter().map(|(text,bounds)|json!({"text":text,"bounds":rect(*bounds),"scaled_bounds":snapshot.frame.map(|frame|rect(*bounds * frame.transformation))})).collect::<Vec<_>>() })
		);
		std::io::stderr()
			.lock()
			.write_all(line.as_bytes())
			.expect("opening audit log");
	}
	fn mouse_interaction(
		&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
		renderer: &iced::Renderer,
	) -> mouse::Interaction {
		self.content.as_widget().mouse_interaction(
			&tree.children[0],
			layout.children().next().expect("audit child"),
			cursor,
			viewport,
			renderer,
		)
	}
	fn operate(
		&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &iced::Renderer,
		operation: &mut dyn widget::Operation,
	) {
		self.content.as_widget_mut().operate(
			&mut tree.children[0],
			layout.children().next().expect("audit child"),
			renderer,
			operation,
		);
	}
	fn overlay<'a>(
		&'a mut self, tree: &'a mut Tree, layout: Layout<'a>, renderer: &iced::Renderer,
		viewport: &Rectangle, offset: Vector,
	) -> Vec<overlay::Element<'a, Message, iced::Theme, iced::Renderer>> {
		self.content.as_widget_mut().overlay(
			&mut tree.children[0],
			layout.children().next().expect("audit child"),
			renderer,
			viewport,
			offset,
		)
	}
}
