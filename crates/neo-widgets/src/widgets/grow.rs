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

pub fn grow<'a, Message, Theme, Renderer>(
	content: impl Into<Element<'a, Message, Theme, Renderer>>, duration: Duration,
	easing: animation::Easing, from: GrowFrom,
) -> Grow<'a, Message, Theme, Renderer> {
	Grow::new(content, duration, easing, from)
}

#[must_use]
pub struct Grow<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
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
			if let Event::Window(window::Event::RedrawRequested(now)) = event {
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
