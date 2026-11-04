//! Drive the production Widget implementation with Iced's null renderer. No GPU,
//! compositor, alternate event handler, or extracted test-only decision path.
use super::*;
use config::SystemMenuWidgets::{Bluetooth, Nightlight, Wifi};
use iced::advanced::shell::{Bus, Waker};
use std::{cell::RefCell, rc::Rc, time::Instant};

type ChildEvents = Rc<RefCell<Vec<(Event, mouse::Cursor)>>>;

struct Probe(ChildEvents);

impl Widget<Message, iced::Theme, ()> for Probe {
	fn size(&self) -> Size<Length> {
		Size::new(Length::Fill, Length::Fill)
	}

	fn layout(&mut self, _: &mut Tree, _: &(), limits: &layout::Limits) -> layout::Node {
		layout::Node::new(limits.max())
	}

	fn update(
		&mut self, _: &mut Tree, event: &Event, _: Layout<'_>, cursor: mouse::Cursor, _: &(),
		_: &mut Shell<'_, Message>, _: &Rectangle,
	) {
		self.0.borrow_mut().push((event.clone(), cursor));
	}

	fn draw(
		&self, _: &Tree, _: &mut (), _: &iced::Theme, _: &renderer::Style, _: Layout<'_>,
		_: mouse::Cursor, _: &Rectangle,
	) {
	}
}

struct Outcome {
	messages: Vec<Message>,
	captured: bool,
	relayout: bool,
}

struct Harness {
	widget: Element<'static, Message, iced::Theme, ()>,
	tree: Tree,
	node: layout::Node,
	viewport: Rectangle,
	child_events: ChildEvents,
}

impl Harness {
	fn element(
		keys: Vec<SystemMenuWidgets>, events: &ChildEvents,
	) -> Element<'static, Message, iced::Theme, ()> {
		let children = keys
			.iter()
			.map(|_| Element::new(Probe(events.clone())))
			.collect();
		Element::new(Editor {
			active: 2,
			keys,
			children,
		})
	}

	fn new() -> Self {
		let child_events = ChildEvents::default();
		let mut widget = Self::element(vec![Wifi, Bluetooth, Nightlight], &child_events);
		let mut tree = Tree::new(widget.as_widget());
		tree.diff(widget.as_widget_mut());
		let node = widget
			.as_widget_mut()
			.layout(&mut tree, &(), &Self::limits());
		Self {
			widget,
			tree,
			node,
			viewport: Rectangle::new(Point::ORIGIN, Size::new(400.0, 136.0)),
			child_events,
		}
	}

	fn limits() -> layout::Limits {
		layout::Limits::new(Size::ZERO, Size::new(400.0, 136.0))
	}

	fn dispatch(&mut self, event: Event, position: Option<Point>) -> Outcome {
		self.dispatch_with_capture(event, position, false)
	}

	fn dispatch_with_capture(
		&mut self, event: Event, position: Option<Point>, captured: bool,
	) -> Outcome {
		let cursor = position.map_or(mouse::Cursor::Unavailable, mouse::Cursor::Available);
		let mut bus = Bus::new();
		let mut shell = Shell::new(&window::Headless, Waker::noop(), &mut bus);
		if captured {
			shell.capture_event();
		}
		self.widget.as_widget_mut().update(
			&mut self.tree,
			&event,
			Layout::new(&self.node),
			cursor,
			&(),
			&mut shell,
			&self.viewport,
		);
		let outcome = Outcome {
			messages: Vec::new(),
			captured: shell.is_event_captured(),
			relayout: shell.is_layout_invalid().is_some(),
		};
		drop(shell);
		if outcome.relayout {
			self.node = self
				.widget
				.as_widget_mut()
				.layout(&mut self.tree, &(), &Self::limits());
		}
		Outcome {
			messages: bus.into_iter().collect(),
			..outcome
		}
	}

	fn press(&mut self, point: Point) -> Outcome {
		self.dispatch(
			Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
			Some(point),
		)
	}

	fn release(&mut self, point: Option<Point>) -> Outcome {
		self.dispatch(
			Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
			point,
		)
	}

	fn move_to(&mut self, point: Point) -> Outcome {
		self.dispatch(
			Event::Mouse(mouse::Event::CursorMoved { position: point }),
			Some(point),
		)
	}
}

fn escape() -> Event {
	Event::Keyboard(keyboard::Event::KeyPressed {
		key: keyboard::Key::Named(Named::Escape),
		modified_key: keyboard::Key::Named(Named::Escape),
		physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
		location: keyboard::Location::Standard,
		modifiers: keyboard::Modifiers::default(),
		text: None,
		repeat: false,
	})
}

#[test]
fn escape_cursor_leave_and_unfocus_cancel_both_clicks_and_drags() {
	for event in [
		escape(),
		Event::Mouse(mouse::Event::CursorLeft),
		Event::Window(window::Event::Unfocused),
	] {
		for drag in [false, true] {
			let mut harness = Harness::new();
			assert!(harness.press(Point::new(10.0, 10.0)).captured);
			if drag {
				harness.move_to(Point::new(220.0, 10.0));
			}
			let cancelled = harness.dispatch(event.clone(), None);
			assert!(cancelled.messages.is_empty());
			assert!(cancelled.relayout);
			assert!(
				harness
					.release(Some(Point::new(10.0, 10.0)))
					.messages
					.is_empty(),
				"cancelled gestures must not commit on later release: {event:?}, drag={drag}"
			);
		}
	}
}

#[test]
fn clipped_and_previously_captured_presses_cannot_start_an_edit() {
	for already_captured in [false, true] {
		let mut harness = Harness::new();
		harness.viewport = Rectangle::new(Point::ORIGIN, Size::new(400.0, 32.0));
		let point = if already_captured {
			Point::new(10.0, 10.0)
		} else {
			Point::new(10.0, 50.0)
		};
		harness.dispatch_with_capture(
			Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
			Some(point),
			already_captured,
		);
		assert!(
			harness
				.release(Some(Point::new(10.0, 10.0)))
				.messages
				.is_empty()
		);
	}
}

#[test]
fn release_requires_a_cursor_inside_both_layout_and_viewport() {
	for drag in [false, true] {
		for release in [
			None,
			Some(Point::new(450.0, 10.0)),
			Some(Point::new(10.0, 50.0)),
		] {
			let mut harness = Harness::new();
			harness.viewport = Rectangle::new(Point::ORIGIN, Size::new(400.0, 32.0));
			harness.press(Point::new(10.0, 10.0));
			if drag {
				harness.move_to(Point::new(220.0, 10.0));
			}
			let outcome = harness.release(release);
			assert!(outcome.captured);
			assert!(outcome.relayout);
			assert!(
				outcome.messages.is_empty(),
				"invalid release committed: {release:?}, drag={drag}"
			);
			assert!(
				harness
					.release(Some(Point::new(10.0, 10.0)))
					.messages
					.is_empty()
			);
		}
	}
}

#[test]
fn click_release_must_remain_over_the_source_but_a_tiny_drag_is_not_a_click() {
	let point = Point::new(10.0, 10.0);
	let mut harness = Harness::new();
	assert!(harness.release(Some(point)).messages.is_empty());
	harness.press(point);
	assert!(
		harness
			.release(Some(Point::new(220.0, 10.0)))
			.messages
			.is_empty()
	);
	harness.press(point);
	let click = harness.release(Some(point));
	assert!(
		matches!(click.messages.as_slice(), [Message::WidgetsEdited(widgets)] if *widgets == vec![Bluetooth])
	);
	harness.press(point);
	harness.move_to(Point::new(10.01, 10.0));
	let drag = harness.release(Some(point));
	assert!(
		matches!(drag.messages.as_slice(), [Message::WidgetsEdited(widgets)] if *widgets == vec![Wifi, Bluetooth])
	);
	let inactive = Point::new(10.0, 82.0);
	harness.press(inactive);
	harness.move_to(Point::new(220.0, 10.0));
	assert!(harness.release(Some(point)).messages.is_empty());
	harness.press(inactive);
	let add = harness.release(Some(inactive));
	assert!(
		matches!(add.messages.as_slice(), [Message::WidgetsEdited(widgets)] if *widgets == vec![Wifi, Bluetooth, Nightlight])
	);
}

#[test]
fn drag_rechecks_the_release_position_and_redraw_tracks_scroll_adjusted_cursor() {
	for redraw in [false, true] {
		let mut harness = Harness::new();
		harness.press(Point::new(10.0, 10.0));
		harness.move_to(Point::new(11.0, 10.0));
		if redraw {
			let outcome = harness.dispatch(
				Event::Window(window::Event::RedrawRequested(Instant::now())),
				Some(Point::new(220.0, 10.0)),
			);
			assert!(outcome.relayout);
		}
		let outcome = harness.release(Some(Point::new(220.0, 10.0)));
		assert!(
			matches!(outcome.messages.as_slice(), [Message::WidgetsEdited(widgets)] if *widgets == vec![Bluetooth, Wifi])
		);
	}
}

#[test]
fn retained_tree_reconciliation_cancels_a_gesture_after_keys_change() {
	let mut harness = Harness::new();
	harness.press(Point::new(10.0, 10.0));
	harness.move_to(Point::new(220.0, 10.0));
	harness.widget = Harness::element(vec![Bluetooth, Wifi, Nightlight], &harness.child_events);
	harness.tree.diff(harness.widget.as_widget_mut());
	harness.node =
		harness
			.widget
			.as_widget_mut()
			.layout(&mut harness.tree, &(), &Harness::limits());
	assert!(
		harness
			.release(Some(Point::new(220.0, 10.0)))
			.messages
			.is_empty()
	);
}

#[test]
fn editor_forwards_redraws_but_not_input_to_children() {
	let mut harness = Harness::new();
	harness.press(Point::new(10.0, 10.0));
	harness.move_to(Point::new(220.0, 10.0));
	harness.release(Some(Point::new(220.0, 10.0)));
	harness.dispatch(escape(), None);
	assert!(harness.child_events.borrow().is_empty());
	harness.dispatch(
		Event::Window(window::Event::RedrawRequested(Instant::now())),
		Some(Point::new(10.0, 10.0)),
	);
	let events = harness.child_events.borrow();
	assert_eq!(events.len(), 3);
	assert!(events.iter().all(|(event, cursor)| matches!(
		event,
		Event::Window(window::Event::RedrawRequested(_))
	) && matches!(cursor, mouse::Cursor::Unavailable)));
}
