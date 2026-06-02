use std::{
	rc::Rc,
	time::{Duration, Instant},
};

use iced::{
	Animation, Color, Element, Event, Length, Padding, Rectangle,
	advanced::{
		Widget, layout, mouse, renderer,
		widget::{Tree, tree},
	},
	border, window,
};
use num_traits::{AsPrimitive, Num, NumCast, ToPrimitive};

use crate::{style::COLORS, widgets::neo_surface};

use super::NeoSurfaceStyle;

const DEFAULT_INTERACTION_RADIUS: f32 = 10.0;

pub fn neo_slider<T: Num + Clone, Message>(
	range: std::ops::RangeInclusive<T>, value: T,
) -> NeoSlider<T, Message> {
	NeoSlider::new(range.start().clone(), range.end().clone(), value)
}

pub struct NeoSlider<T, Message> {
	track: NeoSurfaceStyle,
	handle: NeoSurfaceStyle,
	running_color: Color,
	on_change: Option<Rc<dyn Fn(T) -> Message>>,
	on_change_live: Option<Rc<dyn Fn(T) -> Message>>,
	width: Length,
	height: Length,
	interaction_radius: f32,
	enabled: bool,
	minimum: T,
	maximum: T,
	value: T,
	step: T,
}

impl<T: Num + Clone, Message> NeoSlider<T, Message> {
	pub fn new(minimum: T, maximum: T, value: T) -> Self {
		Self {
			track: NeoSurfaceStyle {
				background: COLORS.white,
				border: COLORS.black,
				radius: 2.0,
				border_width: 2.0,
				shadow_width: 4.0,
			},
			handle: NeoSurfaceStyle {
				background: COLORS.decorative.yellow,
				border: COLORS.black,
				radius: 2.0,
				border_width: 2.0,
				shadow_width: 4.0,
			},
			running_color: COLORS.decorative.pink70,
			on_change: None,
			on_change_live: None,
			width: Length::Fill,
			height: Length::Fixed(18.0),
			interaction_radius: DEFAULT_INTERACTION_RADIUS,
			enabled: true,
			value,
			minimum,
			maximum,
			step: T::one(),
		}
	}

	pub fn track_style(mut self, style: NeoSurfaceStyle) -> Self {
		self.track = style;
		self
	}

	pub fn handle_style(mut self, style: NeoSurfaceStyle) -> Self {
		self.handle = style;
		self
	}

	pub fn running_color(mut self, color: impl Into<Color>) -> Self {
		self.running_color = color.into();
		self
	}

	pub fn handle_color(mut self, color: impl Into<Color>) -> Self {
		self.handle.background = color.into();
		self
	}

	pub fn on_change(self, func: impl Fn(T) -> Message + 'static) -> Self {
		self.on_change_maybe(Some(func))
	}

	pub fn on_change_maybe(mut self, func: Option<impl Fn(T) -> Message + 'static>) -> Self {
		self.on_change = func.map(|func| Rc::new(func) as Rc<dyn Fn(T) -> Message + 'static>);
		self
	}

	pub fn on_change_live(mut self, func: impl Fn(T) -> Message + 'static) -> Self {
		self.on_change_live = Some(Rc::new(func));
		self
	}

	pub fn on_change_live_maybe(mut self, func: Option<impl Fn(T) -> Message + 'static>) -> Self {
		self.on_change_live = func.map(|func| Rc::new(func) as Rc<dyn Fn(T) -> Message + 'static>);
		self
	}

	pub fn width(mut self, width: impl Into<Length>) -> Self {
		self.width = width.into();
		self
	}

	pub fn height(mut self, height: impl Into<Length>) -> Self {
		self.height = height.into();
		self
	}

	pub fn interaction_radius(mut self, radius: impl Into<f32>) -> Self {
		self.interaction_radius = radius.into();
		self
	}

	pub fn minimum(mut self, minimum: impl Into<T>) -> Self {
		self.minimum = minimum.into();
		self
	}

	pub fn maximum(mut self, maximum: impl Into<T>) -> Self {
		self.maximum = maximum.into();
		self
	}

	pub fn value(mut self, value: impl Into<T>) -> Self {
		self.value = value.into();
		self
	}

	pub fn step(mut self, step: impl Into<T>) -> Self {
		self.step = step.into();
		self
	}

	pub fn enabled(mut self, enabled: bool) -> Self {
		self.enabled = enabled;
		self
	}
}

impl<T, Message> NeoSlider<T, Message>
where
	T: Num + NumCast + AsPrimitive<f32> + Clone,
{
	fn percentage_from_value(&self, value: T) -> f32 {
		let range = (self.maximum.clone() - self.minimum.clone()).as_();

		if range == 0.0 {
			return 0.0;
		}

		((value - self.minimum.clone()).as_() / range).clamp(0.0, 1.0)
	}

	fn value_from_percentage(&self, percentage: f32) -> T {
		let minimum = self.minimum.as_();
		let maximum = self.maximum.as_();
		let range = maximum - minimum;
		let step = self.step.as_();

		let value = minimum + percentage.clamp(0.0, 1.0) * range;
		let value = if step > 0.0 {
			minimum + ((value - minimum) / step).round() * step
		} else {
			value
		};

		NumCast::from(value.clamp(minimum, maximum)).unwrap()
	}

	fn stepped_percentage(&self, percentage: f32) -> f32 {
		self.percentage_from_value(self.value_from_percentage(percentage))
	}
}

#[derive(Debug)]
struct State {
	percentage: f32,
	pressed: Animation<bool>,
	hovered: bool,
}

fn expanded_bounds(bounds: Rectangle, radius: f32) -> Rectangle {
	let radius = radius.max(0.0);

	Rectangle {
		x: bounds.x - radius,
		y: bounds.y - radius,
		width: bounds.width + radius * 2.0,
		height: bounds.height + radius * 2.0,
	}
}

impl<T, Message, Theme, Renderer> Widget<Message, Theme, Renderer> for NeoSlider<T, Message>
where
	T: Num + NumCast + AsPrimitive<f32> + Clone + 'static,
	Message: Clone,
	Renderer: renderer::Renderer,
{
	fn size(&self) -> iced::Size<Length> {
		neo_surface::size(self.width, self.height)
	}

	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<State>()
	}

	fn state(&self) -> tree::State {
		tree::State::new(State {
			percentage: self.percentage_from_value(self.value.clone()),
			pressed: Animation::new(false).duration(Duration::from_millis(50)),
			hovered: false,
		})
	}

	fn update(
		&mut self, tree: &mut Tree, event: &iced::Event, layout: layout::Layout<'_>,
		cursor: mouse::Cursor, _renderer: &Renderer,
		shell: &mut iced::advanced::Shell<'_, Message>, _viewport: &Rectangle,
	) {
		if !self.enabled {
			return;
		}

		let state = tree.state.downcast_mut::<State>();
		let bounds = layout.bounds();
		let interaction_bounds = expanded_bounds(bounds, self.interaction_radius);
		let over = cursor.is_over(interaction_bounds);

		state.hovered = over;

		let percentage = self.percentage_from_value(self.value.clone());
		if !state.pressed.value() && state.percentage != percentage {
			state.percentage = percentage;
		}

		match event {
			Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if over => {
				state.pressed.go_mut(true, Instant::now());

				let cursor_pos = cursor.position().unwrap_or_default();
				let x = cursor_pos.x;
				let x = x.clamp(bounds.x, bounds.x + bounds.width) - bounds.x;
				let percentage = x / bounds.width;
				state.percentage = self.stepped_percentage(percentage);
				if let Some(on_change_live) = &self.on_change_live {
					shell.publish(on_change_live(self.value_from_percentage(state.percentage)));
				}

				shell.request_redraw();
				shell.capture_event();
			}
			Event::Mouse(mouse::Event::CursorLeft) | Event::Window(window::Event::Unfocused)
				if state.pressed.value() =>
			{
				state.pressed.go_mut(false, Instant::now());
				shell.request_redraw();
				shell.capture_event();
			}
			Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
				if state.pressed.value() =>
			{
				let value = self.value_from_percentage(state.percentage);

				if let Some(on_change) = &self.on_change.as_deref() {
					let msg = on_change(value);
					shell.publish(msg);
				}
				state.pressed.go_mut(false, Instant::now());
				shell.request_redraw();
				shell.capture_event();
			}
			Event::Mouse(mouse::Event::CursorMoved { position }) if state.pressed.value() => {
				let x = position.x;
				let x = x.clamp(bounds.x, bounds.x + bounds.width) - bounds.x;
				let percentage = x / bounds.width;
				state.percentage = self.stepped_percentage(percentage);
				if let Some(on_change_live) = &self.on_change_live {
					shell.publish(on_change_live(self.value_from_percentage(state.percentage)));
				}

				shell.request_redraw();
				shell.capture_event();
			}
			Event::Window(window::Event::RedrawRequested(now)) => {
				if state.pressed.is_animating(*now) {
					shell.request_redraw();
				}
			}
			_ => (),
		}
	}

	fn mouse_interaction(
		&self, tree: &Tree, layout: layout::Layout<'_>, cursor: mouse::Cursor,
		_viewport: &Rectangle, _renderer: &Renderer,
	) -> mouse::Interaction {
		let state = tree.state.downcast_ref::<State>();
		if self.enabled
			&& (self.on_change.is_some() || self.on_change_live.is_some())
			&& cursor.is_over(expanded_bounds(layout.bounds(), self.interaction_radius))
		{
			if state.pressed.value() {
				mouse::Interaction::Grabbing
			} else {
				mouse::Interaction::Pointer
			}
		} else {
			mouse::Interaction::None
		}
	}

	fn layout(
		&mut self, _tree: &mut Tree, _renderer: &Renderer, limits: &iced::advanced::layout::Limits,
	) -> iced::advanced::layout::Node {
		layout::padded(
			limits,
			self.width,
			self.height,
			Padding {
				top: 0.0,
				right: self.track.shadow_width,
				bottom: self.track.shadow_width,
				left: 0.0,
			},
			|limits| layout::atomic(limits, self.width, self.height),
		)
	}

	fn draw(
		&self, tree: &Tree, renderer: &mut Renderer, _theme: &Theme, _style: &renderer::Style,
		layout: iced::advanced::Layout<'_>, _cursor: iced::advanced::mouse::Cursor,
		_viewport: &iced::Rectangle,
	) {
		let state = tree.state.downcast_ref::<State>();
		let bounds = layout.bounds();
		let track = self.track;
		let handle = self.handle;
		let running_color = self.running_color;

		let s = track.shadow_width;

		let shadow = Rectangle {
			x: bounds.x + s,
			y: bounds.y + s,
			width: bounds.width - s,
			height: bounds.height - s,
		};

		renderer.fill_quad(
			renderer::Quad {
				bounds: shadow,
				border: iced::Border {
					radius: track.radius.into(),
					..Default::default()
				},
				snap: true,
				..Default::default()
			},
			track.border,
		);

		let percentage_filled =
			if state.pressed.value() && !state.pressed.is_animating(Instant::now()) {
				state.percentage
			} else {
				self.percentage_from_value(self.value.clone())
			};
		let percentage_filled = percentage_filled.clamp(0.0, 1.0);
		// debug_assert!(
		//     percentage_filled <= 1.0 && percentage_filled >= 0.0,
		//     "Percentage {percentage_filled} not between 0 and 1"
		// );

		let filled_width = (bounds.width - s) * percentage_filled;
		let unfilled_width = (bounds.width - s) * (1.0 - percentage_filled);

		let track_filled = Rectangle {
			x: bounds.x,
			y: bounds.y,
			width: filled_width,
			height: bounds.height - s,
		};
		let track_unfilled = Rectangle {
			x: bounds.x + filled_width,
			y: bounds.y,
			width: unfilled_width,
			height: bounds.height - s,
		};

		renderer.fill_quad(
			renderer::Quad {
				bounds: track_filled,
				border: iced::Border {
					radius: border::Radius {
						top_left: track.radius,
						bottom_left: track.radius,
						..Default::default()
					},
					..Default::default()
				},
				snap: true,
				..Default::default()
			},
			running_color,
		);
		renderer.fill_quad(
			renderer::Quad {
				bounds: track_unfilled,
				border: iced::Border {
					radius: border::Radius {
						top_right: track.radius,
						bottom_right: track.radius,
						..Default::default()
					},
					..Default::default()
				},
				snap: true,
				..Default::default()
			},
			track.background,
		);

		let border = Rectangle {
			x: bounds.x,
			y: bounds.y,
			width: bounds.width - s,
			height: bounds.height - s,
		};

		renderer.fill_quad(
			renderer::Quad {
				bounds: border,
				border: iced::Border {
					radius: track.radius.into(),
					color: track.border,
					width: track.border_width,
					..Default::default()
				},
				snap: true,
				..Default::default()
			},
			Color::TRANSPARENT,
		);

		let handle_size = bounds.height * 1.25;
		let track_width = bounds.width - s;
		let handle_x = bounds.x + (track_width - handle_size).max(0.0) * percentage_filled;

		let offset = state
			.pressed
			.interpolate(0.0, handle.shadow_width, Instant::now());

		let handle_y = bounds.y - (bounds.height * 0.125) - (offset / 1.5);

		let handle_shadow = Rectangle {
			x: handle_x + handle.shadow_width,
			y: handle_y + handle.shadow_width,
			width: handle_size - handle.shadow_width,
			height: handle_size - handle.shadow_width,
		};

		let handle_surface = Rectangle {
			x: handle_x + offset,
			y: handle_y + offset,
			width: handle_size - handle.shadow_width,
			height: handle_size - handle.shadow_width,
		};

		renderer.fill_quad(
			renderer::Quad {
				bounds: handle_shadow,
				border: iced::Border {
					radius: handle.radius.into(),
					..Default::default()
				},
				snap: true,
				..Default::default()
			},
			handle.border,
		);
		renderer.fill_quad(
			renderer::Quad {
				bounds: handle_surface,
				border: iced::Border {
					radius: handle.radius.into(),
					width: handle.border_width,
					color: handle.border,
					..Default::default()
				},
				snap: true,
				..Default::default()
			},
			handle.background,
		);
	}
}

impl<'a, T, Message: 'a, Theme, Renderer> From<NeoSlider<T, Message>>
	for Element<'a, Message, Theme, Renderer>
where
	T: Num + NumCast + AsPrimitive<f32> + Clone + 'static,
	Message: Clone,
	Renderer: renderer::Renderer,
{
	fn from(value: NeoSlider<T, Message>) -> Self {
		Self::new(value)
	}
}
