//! Narrow retained drag handle; adding a preview never reparents the file viewport.
use iced::{
	Color, Element, Event, Length, Rectangle, Size,
	advanced::{
		Layout, Renderer, Shell, Widget, layout, mouse, renderer,
		widget::{Tree, tree},
	},
};

use crate::Message;

pub(crate) fn divider<'a>(visible: bool, width: f32, color: Color) -> Element<'a, Message> {
	Element::new(Divider {
		visible,
		width,
		color,
	})
}

struct Divider {
	visible: bool,
	width: f32,
	color: Color,
}

#[derive(Default)]
struct Drag {
	start: Option<(f32, f32)>,
}

impl<R: Renderer> Widget<Message, iced::Theme, R> for Divider {
	fn size(&self) -> Size<Length> {
		Size::new(
			Length::Fixed(if self.visible { 8.0 } else { 0.0 }),
			Length::Fill,
		)
	}

	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<Drag>()
	}

	fn state(&self) -> tree::State {
		tree::State::new(Drag::default())
	}

	fn layout(&mut self, _: &mut Tree, _: &R, limits: &layout::Limits) -> layout::Node {
		let size = <Self as Widget<Message, iced::Theme, R>>::size(self);
		layout::Node::new(limits.resolve(size.width, size.height, Size::ZERO))
	}

	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		_: &R, shell: &mut Shell<'_, Message>, _: &Rectangle,
	) {
		let drag = tree.state.downcast_mut::<Drag>();
		if !self.visible {
			drag.start = None;
			return;
		}

		match event {
			Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
				if cursor.is_over(layout.bounds()) =>
			{
				if let Some(point) = cursor.position() {
					drag.start = Some((point.x, self.width));
					shell.capture_event();
				}
			}
			Event::Mouse(mouse::Event::CursorMoved { position }) if drag.start.is_some() => {
				if let Some((x, width)) = drag.start {
					shell.publish(super::Message::Resize(width + x - position.x).into());
					shell.capture_event();
				}
			}
			Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
				if drag.start.take().is_some() =>
			{
				shell.capture_event()
			}
			Event::Window(iced::window::Event::Unfocused)
			| Event::Mouse(mouse::Event::CursorLeft) => drag.start = None,
			_ => {}
		}
	}

	fn mouse_interaction(
		&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, _: &Rectangle, _: &R,
	) -> mouse::Interaction {
		if self.visible
			&& (cursor.is_over(layout.bounds())
				|| tree.state.downcast_ref::<Drag>().start.is_some())
		{
			mouse::Interaction::ResizingHorizontally
		} else {
			mouse::Interaction::None
		}
	}

	fn draw(
		&self, _: &Tree, renderer: &mut R, _: &iced::Theme, _: &renderer::Style,
		layout: Layout<'_>, _: mouse::Cursor, _: &Rectangle,
	) {
		if self.visible {
			let bounds = layout.bounds();
			renderer.fill_quad(
				renderer::Quad {
					bounds: Rectangle {
						x: bounds.center_x() - 1.0,
						width: 2.0,
						..bounds
					},
					..Default::default()
				},
				self.color,
			);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use iced::{
		Point,
		advanced::{
			shell::{Bus, Waker},
			widget::{Id, Operation, operation},
		},
	};

	#[test]
	fn preview_visibility_preserves_file_scroll_and_main_height() {
		use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};

		struct Offset(f32);

		impl Operation for Offset {
			fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
				operate(self);
			}

			fn scrollable(
				&mut self, id: Option<&Id>, _: Rectangle, _: Rectangle, translation: iced::Vector,
				_: &mut dyn Scrollable,
			) {
				if id == Some(&Id::from("files")) {
					self.0 = translation.y;
				}
			}
		}

		let build = |visible| -> Element<'_, Message, iced::Theme, ()> {
			iced::widget::row![
				iced::widget::scrollable(iced::widget::Space::new().height(2000))
					.id("files")
					.height(Length::Fill)
					.width(Length::Fill),
				Element::new(Divider {
					visible,
					width: 360.0,
					color: Color::BLACK
				}),
				iced::widget::Space::new().width(if visible { 360.0 } else { 0.0 }),
			]
			.height(Length::Fill)
			.into()
		};

		let mut content = build(false);
		let mut tree = Tree::new(content.as_widget());
		content.as_widget_mut().diff(&mut tree);
		let limits = layout::Limits::new(Size::ZERO, Size::new(900.0, 500.0));
		let node = content.as_widget_mut().layout(&mut tree, &(), &limits);

		content.as_widget_mut().operate(
			&mut tree,
			Layout::new(&node),
			&(),
			&mut operation::scrollable::scroll_to::<()>(
				Id::from("files"),
				AbsoluteOffset {
					x: None,
					y: Some(600.0),
				},
			),
		);

		for visible in [true, false, true] {
			content = build(visible);
			tree.diff(content.as_widget_mut());
			content.as_widget_mut().diff(&mut tree);
			let node = content.as_widget_mut().layout(&mut tree, &(), &limits);

			assert_eq!(node.size().height, 500.0);

			let mut offset = Offset(-1.0);
			content
				.as_widget_mut()
				.operate(&mut tree, Layout::new(&node), &(), &mut offset);

			assert_eq!(offset.0, 600.0);
		}
	}

	#[test]
	fn divider_drag_resizes_in_window_coordinates_and_release_stops_it() {
		let mut handle = Divider {
			visible: true,
			width: 360.0,
			color: Color::BLACK,
		};
		let widget: &dyn Widget<Message, iced::Theme, ()> = &handle;
		let mut tree = Tree::new(widget);
		let node = layout::Node::new(Size::new(8.0, 500.0)).move_to(Point::new(500.0, 0.0));
		let layout = Layout::new(&node);
		let mut bus = Bus::default();
		let viewport = Rectangle::with_size(Size::new(1000.0, 500.0));

		for (event, position) in [
			(
				mouse::Event::ButtonPressed(mouse::Button::Left),
				Point::new(504.0, 20.0),
			),
			(
				mouse::Event::CursorMoved {
					position: Point::new(474.0, 20.0),
				},
				Point::new(474.0, 20.0),
			),
			(
				mouse::Event::ButtonReleased(mouse::Button::Left),
				Point::new(474.0, 20.0),
			),
			(
				mouse::Event::CursorMoved {
					position: Point::new(400.0, 20.0),
				},
				Point::new(400.0, 20.0),
			),
		] {
			let mut shell = Shell::new(&iced::window::Headless, Waker::noop(), &mut bus);
			handle.update(
				&mut tree,
				&Event::Mouse(event),
				layout,
				mouse::Cursor::Available(position),
				&(),
				&mut shell,
				&viewport,
			);
		}

		let messages: Vec<_> = bus.into_iter().collect();
		assert!(
			matches!(messages.as_slice(), [Message::Preview(super::super::Message::Resize(width))] if *width == 390.0)
		);
	}
}
