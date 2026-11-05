use std::time::{Duration, Instant};

use iced::{
	Element, Event, Length, Point, Rectangle, Size, Transformation, Vector,
	advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer, widget},
	animation, window,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrowFrom {
	TopCenter,
}

/// The standard opening animation for root bar popups. Child layout stays at
/// rest; one transform scales the entire rendered popup, including its contents.
pub fn popup_grow<'a, Message, Theme, Renderer>(
	content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> Grow<'a, Message, Theme, Renderer> {
	let mut widget = grow(
		content,
		Duration::from_millis(50),
		animation::Easing::EaseOutQuart,
		GrowFrom::TopCenter,
	);
	widget.popup = true;
	widget
}

/// Read-only snapshot exposed to widget operations for rendering verification.
#[derive(Debug, Clone, Copy)]
pub struct GrowFrame {
	pub bounds: Rectangle,
	pub transformation: Transformation,
}

pub fn grow<'a, Message, Theme, Renderer>(
	content: impl Into<Element<'a, Message, Theme, Renderer>>, duration: Duration,
	easing: animation::Easing, from: GrowFrom,
) -> Grow<'a, Message, Theme, Renderer> {
	Grow::new(content, duration, easing, from)
}

#[must_use]
pub struct Grow<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
	popup: bool,
	content: Element<'a, Message, Theme, Renderer>,
	duration: Duration,
	easing: animation::Easing,
	from: GrowFrom,
}

impl<'a, Message, Theme, Renderer> Grow<'a, Message, Theme, Renderer> {
	pub fn new(
		content: impl Into<Element<'a, Message, Theme, Renderer>>, duration: Duration,
		easing: animation::Easing, from: GrowFrom,
	) -> Self {
		Self {
			popup: false,
			content: content.into(),
			duration,
			easing,
			from,
		}
	}

	fn transformation(&self, bounds: Rectangle, progress: f32) -> Transformation {
		let pivot = match self.from {
			GrowFrom::TopCenter => Point::new(bounds.center_x(), bounds.y),
		};
		let scale = 0.8 + 0.2 * self.easing.value(progress);

		Transformation::translate(pivot.x, pivot.y)
			* Transformation::scale(scale)
			* Transformation::translate(-pivot.x, -pivot.y)
	}
}

#[derive(Debug, Default)]
struct State {
	backend: bool,
	started_at: Option<Instant>,
	progress: f32,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
	for Grow<'_, Message, Theme, Renderer>
where
	Renderer: renderer::Renderer,
{
	fn tag(&self) -> widget::tree::Tag {
		widget::tree::Tag::of::<State>()
	}

	fn state(&self) -> widget::tree::State {
		widget::tree::State::new(State::default())
	}

	fn diff(&mut self, tree: &mut widget::Tree) {
		tree.diff_children(std::slice::from_mut(&mut self.content));
	}

	fn size(&self) -> Size<Length> {
		self.content.as_widget().size()
	}

	fn layout(
		&mut self, tree: &mut widget::Tree, renderer: &Renderer, limits: &layout::Limits,
	) -> layout::Node {
		#[allow(clippy::indexing_slicing)]
		self.content
			.as_widget_mut()
			.layout(&mut tree.children[0], renderer, limits)
	}

	fn update(
		&mut self, tree: &mut widget::Tree, event: &Event, layout: Layout<'_>,
		cursor: mouse::Cursor, renderer: &Renderer, shell: &mut Shell<'_, Message>,
		viewport: &Rectangle,
	) {
		let progress = {
			let state = tree.state.downcast_mut::<State>();
			if state.backend {
				state.progress = 1.0;
			} else if let Event::Window(window::Event::RedrawRequested(now)) = event {
				let started_at = state.started_at.get_or_insert(*now);
				state.progress = if self.duration.is_zero() {
					1.0
				} else {
					(now.duration_since(*started_at).as_secs_f32() / self.duration.as_secs_f32())
						.min(1.0)
				};

				if state.progress < 1.0 {
					shell.request_redraw();
				}
			}
			state.progress
		};
		let inverse = self.transformation(layout.bounds(), progress).inverse();
		#[allow(clippy::indexing_slicing)]
		self.content.as_widget_mut().update(
			&mut tree.children[0],
			event,
			layout,
			cursor * inverse,
			renderer,
			shell,
			&(*viewport * inverse),
		);
	}

	fn draw(
		&self, tree: &widget::Tree, renderer: &mut Renderer, theme: &Theme,
		style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		let bounds = layout.bounds();
		let progress = tree.state.downcast_ref::<State>().progress;
		let transformation = self.transformation(bounds, progress);
		let inverse = transformation.inverse();

		renderer.with_layer(bounds, |renderer| {
			renderer.with_transformation(transformation, |renderer| {
				#[allow(clippy::indexing_slicing)]
				self.content.as_widget().draw(
					&tree.children[0],
					renderer,
					theme,
					style,
					layout,
					cursor * inverse,
					&(*viewport * inverse),
				);
			});
		});
	}

	fn mouse_interaction(
		&self, tree: &widget::Tree, layout: Layout<'_>, cursor: mouse::Cursor,
		viewport: &Rectangle, renderer: &Renderer,
	) -> mouse::Interaction {
		let progress = tree.state.downcast_ref::<State>().progress;
		let inverse = self.transformation(layout.bounds(), progress).inverse();
		#[allow(clippy::indexing_slicing)]
		self.content.as_widget().mouse_interaction(
			&tree.children[0],
			layout,
			cursor * inverse,
			&(*viewport * inverse),
			renderer,
		)
	}

	fn operate(
		&mut self, tree: &mut widget::Tree, layout: Layout<'_>, renderer: &Renderer,
		operation: &mut dyn widget::Operation,
	) {
		let state = tree.state.downcast_mut::<State>();
		if self.popup {
			operation.custom(
				Some(&widget::Id::new("exwlshell.popup.grow")),
				layout.bounds(),
				&mut state.backend,
			);
			if state.backend {
				state.progress = 1.0;
			}
		}
		let mut frame = GrowFrame {
			bounds: layout.bounds(),
			transformation: self
				.transformation(layout.bounds(), tree.state.downcast_ref::<State>().progress),
		};
		operation.custom(None, layout.bounds(), &mut frame);
		#[allow(clippy::indexing_slicing)]
		self.content
			.as_widget_mut()
			.operate(&mut tree.children[0], layout, renderer, operation);
	}

	fn overlay<'a>(
		&'a mut self, tree: &'a mut widget::Tree, layout: Layout<'a>, renderer: &Renderer,
		viewport: &Rectangle, offset: Vector,
	) -> Vec<overlay::Element<'a, Message, Theme, Renderer>> {
		#[allow(clippy::indexing_slicing)]
		self.content.as_widget_mut().overlay(
			&mut tree.children[0],
			layout,
			renderer,
			viewport,
			offset,
		)
	}
}

impl<'a, Message, Theme, Renderer> From<Grow<'a, Message, Theme, Renderer>>
	for Element<'a, Message, Theme, Renderer>
where
	Message: 'a,
	Theme: 'a,
	Renderer: renderer::Renderer + 'a,
{
	fn from(grow: Grow<'a, Message, Theme, Renderer>) -> Self {
		Element::new(grow)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use iced::advanced::shell::{Bus, Waker};
	use std::{cell::RefCell, rc::Rc};
	type Draws = Rc<RefCell<Vec<Rectangle>>>;
	struct Probe(Draws);
	impl Widget<Point, iced::Theme, ()> for Probe {
		fn size(&self) -> Size<Length> {
			Size::new(Length::Fixed(400.0), Length::Fixed(300.0))
		}
		fn layout(&mut self, _: &mut widget::Tree, _: &(), _: &layout::Limits) -> layout::Node {
			layout::Node::new(Size::new(400.0, 300.0))
		}
		fn update(
			&mut self, _: &mut widget::Tree, event: &Event, _: Layout<'_>, cursor: mouse::Cursor,
			_: &(), shell: &mut Shell<'_, Point>, _: &Rectangle,
		) {
			if matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_))) {
				if let Some(point) = cursor.position() {
					shell.publish(point);
				}
			}
		}
		fn draw(
			&self, _: &widget::Tree, _: &mut (), _: &iced::Theme, _: &renderer::Style,
			layout: Layout<'_>, _: mouse::Cursor, _: &Rectangle,
		) {
			self.0.borrow_mut().push(layout.bounds());
		}
	}
	fn dispatch(
		grow: &mut Grow<'_, Point, iced::Theme, ()>, tree: &mut widget::Tree, node: &layout::Node,
		event: Event, point: Point,
	) -> (window::RedrawRequest, Vec<Point>) {
		let mut bus = Bus::new();
		let mut shell = Shell::new(&window::Headless, Waker::noop(), &mut bus);
		grow.update(
			tree,
			&event,
			Layout::new(node),
			mouse::Cursor::Available(point),
			&(),
			&mut shell,
			&Rectangle::with_size(node.size()),
		);
		let redraw = shell.redraw_request();
		drop(shell);
		(redraw, bus.into_iter().collect())
	}
	#[test]
	fn standard_popup_scales_as_one_image_without_changing_child_layout() {
		let draws = Draws::default();
		let mut grow = popup_grow(Element::new(Probe(draws.clone())));
		assert_eq!(grow.duration, Duration::from_millis(50));
		let mut tree = widget::Tree::new(&grow as &dyn Widget<_, _, _>);
		tree.diff(&mut grow as &mut dyn Widget<_, _, _>);
		let node = grow.layout(
			&mut tree,
			&(),
			&layout::Limits::new(Size::ZERO, Size::new(400.0, 300.0)),
		);
		let bounds = Rectangle::with_size(node.size());
		let now = Instant::now();
		let mut scales = Vec::new();
		for (ms, request) in [
			(0, window::RedrawRequest::NextFrame),
			(20, window::RedrawRequest::NextFrame),
			(50, window::RedrawRequest::Wait),
			(100, window::RedrawRequest::Wait),
		] {
			assert_eq!(
				dispatch(
					&mut grow,
					&mut tree,
					&node,
					Event::Window(window::Event::RedrawRequested(
						now + Duration::from_millis(ms)
					)),
					Point::ORIGIN
				)
				.0,
				request
			);
			let transform =
				grow.transformation(bounds, tree.state.downcast_ref::<State>().progress);
			let rendered = bounds * transform;
			let pivot = Point::new(bounds.center_x(), bounds.y);
			assert!(pivot.distance(pivot * transform) < 0.001);
			assert!((rendered.width - bounds.width * transform.scale_factor()).abs() < 0.001);
			scales.push(transform.scale_factor());
			grow.draw(
				&tree,
				&mut (),
				&iced::Theme::Light,
				&renderer::Style::default(),
				Layout::new(&node),
				mouse::Cursor::Unavailable,
				&bounds,
			);
		}
		assert!((scales[0] - 0.8).abs() < 0.001);
		assert!(scales[1] > 0.8 && scales[1] < 1.0);
		assert_eq!(&scales[2..], &[1.0, 1.0]);
		assert!(draws.borrow().iter().all(|draw| *draw == bounds));
		tree.diff(&mut grow as &mut dyn Widget<_, _, _>);
		assert_eq!(
			tree.state.downcast_ref::<State>().progress,
			1.0,
			"view updates do not replay opening"
		);
		let reopened = widget::Tree::new(&grow as &dyn Widget<_, _, _>);
		assert_eq!(reopened.state.downcast_ref::<State>().progress, 0.0);
	}
	#[test]
	fn pointer_coordinates_follow_the_whole_popup_transform() {
		let mut grow = popup_grow(Element::new(Probe(Draws::default())));
		let mut tree = widget::Tree::new(&grow as &dyn Widget<_, _, _>);
		tree.diff(&mut grow as &mut dyn Widget<_, _, _>);
		let node = grow.layout(
			&mut tree,
			&(),
			&layout::Limits::new(Size::ZERO, Size::new(400.0, 300.0)),
		);
		let logical = Point::new(120.0, 80.0);
		let physical = logical * grow.transformation(Rectangle::with_size(node.size()), 0.0);
		let messages = dispatch(
			&mut grow,
			&mut tree,
			&node,
			Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
			physical,
		)
		.1;
		assert_eq!(messages.len(), 1);
		assert!(messages[0].distance(logical) < 0.001);
	}
}
