//! Keep a compositor-sized transparent envelope around an independently sized visual popup.
use iced::{
	Element, Event, Length, Rectangle, Size, Vector,
	advanced::{
		Layout, Shell, Widget, layout, mouse, overlay, renderer,
		widget::{self, Tree},
	},
};

pub fn popup_surface<'a, Message: 'a>(
	content: impl Into<Element<'a, Message>>, envelope: Size, visual: Size,
) -> Element<'a, Message> {
	Element::new(PopupSurface {
		content: content.into(),
		envelope,
		visual,
		origin: Vector::ZERO,
		on_outside: None,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use iced::{
		Point,
		advanced::shell::{Bus, Waker},
		window,
	};
	#[derive(Debug, PartialEq)]
	enum Message {
		Dismiss,
		Pointer(Option<Point>),
		Left,
	}
	struct Probe;
	impl Widget<Message, iced::Theme, ()> for Probe {
		fn size(&self) -> Size<Length> {
			Size::new(Length::Fill, Length::Fill)
		}
		fn layout(&mut self, _: &mut Tree, _: &(), limits: &layout::Limits) -> layout::Node {
			layout::Node::new(limits.max())
		}
		fn update(
			&mut self, _: &mut Tree, event: &Event, _: Layout<'_>, cursor: mouse::Cursor, _: &(),
			shell: &mut Shell<'_, Message>, _: &Rectangle,
		) {
			if matches!(event, Event::Mouse(mouse::Event::CursorLeft)) {
				shell.publish(Message::Left);
			} else {
				shell.publish(Message::Pointer(cursor.position()));
			}
		}
		fn draw(
			&self, _: &Tree, _: &mut (), _: &iced::Theme, _: &renderer::Style, _: Layout<'_>,
			_: mouse::Cursor, _: &Rectangle,
		) {
		}
	}
	#[test]
	fn gutter_dismisses_idle_clicks_but_keeps_gestures_until_outer_departure() {
		let mut widget = PopupSurface {
			content: Element::new(Probe),
			envelope: Size::new(456.0, 356.0),
			visual: Size::new(200.0, 100.0),
			origin: Vector::new(128.0, 128.0),
			on_outside: Some(Box::new(|| Message::Dismiss)),
		};
		let mut tree = Tree::new(&widget as &dyn Widget<_, _, _>);
		tree.diff(&mut widget as &mut dyn Widget<_, _, _>);
		let node = widget.layout(
			&mut tree,
			&(),
			&layout::Limits::new(Size::ZERO, widget.envelope),
		);
		assert_eq!(
			Layout::new(&node)
				.children()
				.next()
				.map(|child| child.position()),
			Some(Point::new(128.0, 128.0))
		);
		let mut dispatch = |event, point| {
			let mut bus = Bus::new();
			let mut shell = Shell::new(&window::Headless, Waker::noop(), &mut bus);
			widget.update(
				&mut tree,
				&Event::Mouse(event),
				Layout::new(&node),
				mouse::Cursor::Available(point),
				&(),
				&mut shell,
				&Rectangle::with_size(widget.envelope),
			);
			drop(shell);
			bus.into_iter().collect::<Vec<_>>()
		};
		let gutter = Point::new(400.0, 150.0);
		assert_eq!(
			dispatch(mouse::Event::ButtonPressed(mouse::Button::Left), gutter),
			vec![Message::Dismiss]
		);
		let content = Point::new(150.0, 150.0);
		assert_eq!(
			dispatch(mouse::Event::ButtonPressed(mouse::Button::Left), content),
			vec![Message::Pointer(Some(content))]
		);
		assert_eq!(
			dispatch(mouse::Event::CursorMoved { position: gutter }, gutter),
			vec![Message::Pointer(Some(gutter))]
		);
		assert_eq!(
			dispatch(mouse::Event::ButtonReleased(mouse::Button::Left), gutter),
			vec![Message::Pointer(Some(gutter))]
		);
		assert_eq!(
			dispatch(mouse::Event::ButtonPressed(mouse::Button::Right), gutter),
			vec![Message::Dismiss]
		);
		dispatch(mouse::Event::ButtonPressed(mouse::Button::Left), content);
		let outside = Point::new(457.0, 150.0);
		assert_eq!(
			dispatch(mouse::Event::CursorMoved { position: outside }, outside),
			vec![Message::Left]
		);
		assert_eq!(
			dispatch(mouse::Event::ButtonReleased(mouse::Button::Left), gutter),
			vec![Message::Pointer(None)]
		);
	}
}

/// A transparent input envelope. Idle gutter presses dismiss; active left-button
/// gestures retain real pointer coordinates until they leave the outer surface.
pub fn popup_gutter<'a, Message: 'a>(
	content: impl Into<Element<'a, Message>>, envelope: Size, visual: Size, origin: Vector,
	on_outside: impl Fn() -> Message + 'a,
) -> Element<'a, Message> {
	Element::new(PopupSurface {
		content: content.into(),
		envelope,
		visual,
		origin,
		on_outside: Some(Box::new(on_outside)),
	})
}

struct PopupSurface<'a, Message, Renderer = iced::Renderer> {
	content: Element<'a, Message, iced::Theme, Renderer>,
	envelope: Size,
	visual: Size,
	origin: Vector,
	on_outside: Option<Box<dyn Fn() -> Message + 'a>>,
}

#[derive(Default)]
struct State {
	pressed: bool,
}

impl<Message, Renderer: iced::advanced::Renderer> Widget<Message, iced::Theme, Renderer>
	for PopupSurface<'_, Message, Renderer>
{
	fn tag(&self) -> widget::tree::Tag {
		widget::tree::Tag::of::<State>()
	}
	fn state(&self) -> widget::tree::State {
		widget::tree::State::new(State::default())
	}
	fn size(&self) -> Size<Length> {
		Size::new(
			Length::Fixed(self.envelope.width),
			Length::Fixed(self.envelope.height),
		)
	}
	fn diff(&mut self, tree: &mut Tree) {
		tree.diff_children(std::slice::from_mut(&mut self.content));
	}
	fn layout(
		&mut self, tree: &mut Tree, renderer: &Renderer, _limits: &layout::Limits,
	) -> layout::Node {
		let Some(child_tree) = tree.children.first_mut() else {
			return layout::Node::new(self.envelope);
		};
		let visual = Size::new(self.visual.width.max(1.0), self.visual.height.max(1.0));
		let child = self.content.as_widget_mut().layout(
			child_tree,
			renderer,
			&layout::Limits::new(visual, visual),
		);
		layout::Node::with_children(self.envelope, vec![child.translate(self.origin)])
	}
	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &Renderer, shell: &mut Shell<'_, Message>, viewport: &Rectangle,
	) {
		let state = tree.state.downcast_mut::<State>();
		let Some(child_tree) = tree.children.first_mut() else {
			return;
		};
		if let Some(child) = layout.children().next() {
			let clip = Rectangle::new(layout.position() + self.origin, self.visual)
				.intersection(viewport)
				.unwrap_or_default();
			let point = cursor.position();
			if let Some(on_outside) = &self.on_outside {
				if !state.pressed
					&& matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_)))
					&& point.is_some_and(|point| {
						layout.bounds().contains(point) && !clip.contains(point)
					}) {
					shell.publish(on_outside());
					shell.capture_event();
					return;
				}
				if matches!(
					event,
					Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
				) && point.is_some_and(|point| clip.contains(point))
				{
					state.pressed = true;
				}
				let left = matches!(
					event,
					Event::Mouse(mouse::Event::CursorLeft)
						| Event::Window(iced::window::Event::Unfocused)
				) || matches!(event,Event::Mouse(mouse::Event::CursorMoved { position }) if !layout.bounds().contains(*position));
				if state.pressed && left {
					state.pressed = false;
					self.content.as_widget_mut().update(
						child_tree,
						&Event::Mouse(mouse::Event::CursorLeft),
						child,
						mouse::Cursor::Unavailable,
						renderer,
						shell,
						&clip,
					);
					return;
				}
			}
			let cursor = if state.pressed || point.is_some_and(|point| clip.contains(point)) {
				cursor
			} else {
				mouse::Cursor::Unavailable
			};
			self.content
				.as_widget_mut()
				.update(child_tree, event, child, cursor, renderer, shell, &clip);
			if matches!(
				event,
				Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
			) {
				state.pressed = false;
			}
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
			let clip = Rectangle::new(layout.position() + self.origin, self.visual)
				.intersection(viewport)
				.unwrap_or_default();
			renderer.with_layer(clip, |renderer| {
				self.content
					.as_widget()
					.draw(child_tree, renderer, theme, style, child, cursor, &clip);
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
		if let Some(child) = layout.children().next() {
			let clip = Rectangle::new(layout.position() + self.origin, self.visual)
				.intersection(viewport)
				.unwrap_or_default();
			if tree.state.downcast_ref::<State>().pressed
				|| cursor.position().is_some_and(|point| clip.contains(point))
			{
				return self
					.content
					.as_widget()
					.mouse_interaction(child_tree, child, cursor, &clip, renderer);
			}
		}
		mouse::Interaction::None
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
			if self.on_outside.is_some() {
				// Backend operation marker: exclude transparent margins from xdg placement.
				operation.container(
					Some(&widget::Id::new("exwlshell.popup.window-geometry")),
					child.bounds(),
				);
			}
		}
	}
	fn overlay<'a>(
		&'a mut self, tree: &'a mut Tree, layout: Layout<'a>, renderer: &Renderer,
		viewport: &Rectangle, offset: Vector,
	) -> Vec<overlay::Element<'a, Message, iced::Theme, Renderer>> {
		let Some(child_tree) = tree.children.first_mut() else {
			return Vec::new();
		};
		layout.children().next().map_or_else(Vec::new, |child| {
			self.content
				.as_widget_mut()
				.overlay(child_tree, child, renderer, viewport, offset)
		})
	}
}
