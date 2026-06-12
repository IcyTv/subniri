use std::{
	rc::Rc,
	time::{Duration, Instant},
};

use iced::{
	Color, Element, Event, Length, Padding, Rectangle, Size, Vector,
	advanced::{
		Layout, Shell, Widget, layout, mouse, renderer,
		widget::{self, Tree, operation, tree},
	},
	widget::Id,
};

use crate::{
	style::COLORS,
	widgets::{NeoContentSurfaceStyle, NeoSurfaceStyle, neo_surface},
};

const SCROLL_EPSILON: f32 = 0.5;

pub fn neo_scrollable<'a, Message, Theme, Renderer>(
	content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> NeoScrollable<'a, Message, Theme, Renderer> {
	NeoScrollable::new(content)
}

pub struct NeoScrollable<'a, Message, Theme, Renderer> {
	content: Element<'a, Message, Theme, Renderer>,
	id: Id,
	width: Length,
	height: Length,
	track_width: f32,
	spacing: f32,
	on_scroll: Option<Rc<dyn Fn() -> Message + 'a>>,
	smooth_scroll: bool,
	scroll_duration: Duration,
	content_style: NeoContentSurfaceStyle,
	track_style: NeoSurfaceStyle,
	handle_style: NeoSurfaceStyle,
}

impl<'a, Message, Theme, Renderer> NeoScrollable<'a, Message, Theme, Renderer> {
	pub fn new(content: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
		Self {
			content: content.into(),
			id: Id::unique(),
			width: Length::Fill,
			height: Length::Fill,
			track_width: 6.0,
			spacing: 4.0,
			on_scroll: None,
			smooth_scroll: true,
			scroll_duration: Duration::from_millis(100),
			content_style: NeoContentSurfaceStyle {
				surface: NeoSurfaceStyle {
					shadow_width: 0.0,
					border_width: 0.0,
					..Default::default()
				},
				padding: 8.0.into(),
			},
			// TODO: make this look better with the border (and/or shadow) actually enabled...
			track_style: NeoSurfaceStyle {
				background: COLORS.white,
				border: COLORS.black,
				radius: 2.0,
				border_width: 0.0,
				shadow_width: 0.0,
			},
			handle_style: NeoSurfaceStyle {
				background: COLORS.decorative.yellow,
				border: COLORS.black,
				radius: 2.0,
				border_width: 2.0,
				shadow_width: 4.0,
			},
		}
	}

	pub fn id(mut self, id: impl Into<Id>) -> Self {
		self.id = id.into();
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

	pub fn track_width(mut self, width: f32) -> Self {
		self.track_width = width;
		self
	}

	pub fn spacing(mut self, spacing: f32) -> Self {
		self.spacing = spacing;
		self
	}

	pub fn on_scroll<F: Fn() -> Message + 'a>(mut self, func: F) -> Self {
		self.on_scroll = Some(Rc::new(func));
		self
	}

	pub fn smooth_scroll(mut self, smooth_scroll: bool) -> Self {
		self.smooth_scroll = smooth_scroll;
		self
	}

	pub fn scroll_duration(mut self, duration: Duration) -> Self {
		self.scroll_duration = duration;
		self
	}

	pub fn very_quick(mut self) -> Self {
		self.scroll_duration = Duration::from_millis(100);
		self
	}

	pub fn quick(mut self) -> Self {
		self.scroll_duration = Duration::from_millis(200);
		self
	}

	pub fn slow(mut self) -> Self {
		self.scroll_duration = Duration::from_millis(400);
		self
	}

	pub fn very_slow(mut self) -> Self {
		self.scroll_duration = Duration::from_millis(500);
		self
	}

	pub fn content_style(mut self, style: NeoContentSurfaceStyle) -> Self {
		self.content_style = style;
		self
	}

	pub fn track_style(mut self, style: NeoSurfaceStyle) -> Self {
		self.track_style = style;
		self
	}

	pub fn handle_style(mut self, style: NeoSurfaceStyle) -> Self {
		self.handle_style = style;
		self
	}

	pub fn radius(mut self, radius: f32) -> Self {
		self.track_style.radius = radius;
		self
	}

	pub fn background(mut self, color: Color) -> Self {
		self.track_style.background = color;
		self
	}

	pub fn handle(mut self, color: Color) -> Self {
		self.handle_style.background = color;
		self
	}

	pub fn shadow_width(mut self, shadow_width: f32) -> Self {
		self.track_style.shadow_width = shadow_width;
		self
	}

	fn scrollbar_width(&self) -> f32 {
		let handle_width = self.track_width + 8.0;

		handle_width.max(self.track_width) + self.handle_style.shadow_width
	}

	fn viewport_bounds(&self, bounds: Rectangle) -> Rectangle {
		bounds.shrink(Padding {
			top: 0.0,
			bottom: 0.0,
			left: 0.0,
			right: self.scrollbar_width() + self.spacing,
		})
	}

	fn scrollbar_geometry(
		&self, bounds: Rectangle, content_bounds: Rectangle, scroll: Vector,
	) -> ScrollbarGeometry {
		let viewport_bounds = self.viewport_bounds(bounds);
		let track_bounds = Rectangle {
			y: bounds.y,
			height: bounds.height,
			x: bounds.x
				+ viewport_bounds.width
				+ self.spacing
				+ (self.scrollbar_width() - self.track_width) / 2.0,
			width: self.track_width,
		};

		let max_scroll_y = (content_bounds.height - viewport_bounds.height).max(0.0);
		let scroll_ratio = if max_scroll_y > 0.0 {
			(scroll.y / max_scroll_y).clamp(0.0, 1.0)
		} else {
			0.0
		};

		let visible_ratio = if content_bounds.height > 0.0 {
			(viewport_bounds.height / content_bounds.height).clamp(0.0, 1.0)
		} else {
			1.0
		};

		let min_handle_height = 28.0;
		let handle_height = (track_bounds.height * visible_ratio)
			.max(min_handle_height)
			.min(track_bounds.height);
		let handle_travel = (track_bounds.height - handle_height).max(0.0);
		let handle_width = track_bounds.width + 8.0;
		let handle_bounds = Rectangle {
			x: track_bounds.x + (track_bounds.width - handle_width) / 2.0,
			y: track_bounds.y + handle_travel * scroll_ratio,
			width: handle_width,
			height: handle_height,
		};

		ScrollbarGeometry {
			viewport_bounds,
			track_bounds,
			handle_bounds,
			handle_travel,
			max_scroll_y,
		}
	}
}

#[derive(Debug, Clone, Copy)]
struct ScrollbarGeometry {
	viewport_bounds: Rectangle,
	track_bounds: Rectangle,
	handle_bounds: Rectangle,
	handle_travel: f32,
	max_scroll_y: f32,
}

impl ScrollbarGeometry {
	fn scroll_for_handle_y(&self, y: f32) -> f32 {
		if self.max_scroll_y <= 0.0 || self.handle_travel <= 0.0 {
			return 0.0;
		}

		let handle_y = y.clamp(
			self.track_bounds.y,
			self.track_bounds.y + self.handle_travel,
		);
		let ratio = (handle_y - self.track_bounds.y) / self.handle_travel;

		self.max_scroll_y * ratio.clamp(0.0, 1.0)
	}
}

#[derive(Debug, Clone, Copy, Default)]
enum ScrollbarInteraction {
	#[default]
	None,
	Dragging {
		grab_offset: f32,
	},
}

#[derive(Debug, Clone, Copy)]
enum Offset {
	Absolute(f32),
	Relative(f32),
}

impl Default for Offset {
	fn default() -> Self {
		Self::Absolute(0.0)
	}
}

impl Offset {
	pub fn absolute(self, viewport: f32, content: f32) -> f32 {
		let max = (content - viewport).max(0.0);

		match self {
			Offset::Absolute(px) => px.clamp(0.0, max),
			Offset::Relative(pct) => (pct.clamp(0.0, 1.0) * max).clamp(0.0, max),
		}
	}
}

#[derive(Clone, Default)]
struct State {
	current: Vector,
	target: (Offset, Offset),
	last_frame: Option<Instant>,
	interaction: ScrollbarInteraction,
	handle_hovered: bool,
	handle_clicked: bool,
}

impl State {
	fn target(&self, bounds: Rectangle, content_bounds: Rectangle) -> Vector {
		Vector::new(
			self.target.0.absolute(bounds.width, content_bounds.width),
			self.target.1.absolute(bounds.height, content_bounds.height),
		)
	}

	fn is_at(&self, target: Vector) -> bool {
		(self.current.x - target.x).abs() <= SCROLL_EPSILON
			&& (self.current.y - target.y).abs() <= SCROLL_EPSILON
	}

	fn set_target_y(&mut self, y: f32) {
		self.target.1 = Offset::Absolute(y.max(0.0));
		self.last_frame = None;
	}

	fn sync_current(&mut self, bounds: Rectangle, content_bounds: Rectangle) {
		self.current = self.target(bounds, content_bounds);
		self.last_frame = None;
	}

	fn animate_towards(&mut self, target: Vector, now: Instant, duration: Duration) -> bool {
		if self.is_at(target) {
			self.current = target;
			self.last_frame = None;

			return false;
		}

		let last_frame = self.last_frame.replace(now).unwrap_or(now);
		let dt = now.duration_since(last_frame).as_secs_f32();
		let duration = duration.as_secs_f32().max(0.001);
		let progress = (dt / duration).clamp(0.0, 1.0);
		let progress = 1.0 - (1.0 - progress).powi(3);

		self.current = Vector::new(
			self.current.x + (target.x - self.current.x) * progress,
			self.current.y + (target.y - self.current.y) * progress,
		);

		if self.is_at(target) {
			self.current = target;
			self.last_frame = None;

			false
		} else {
			true
		}
	}
}

impl operation::Scrollable for State {
	fn snap_to(&mut self, offset: operation::scrollable::RelativeOffset<Option<f32>>) {
		if let Some(x) = offset.x {
			self.target.0 = Offset::Relative(x.clamp(0.0, 1.0));
		}

		if let Some(y) = offset.y {
			self.target.1 = Offset::Relative(y.clamp(0.0, 1.0));
		}
	}

	fn scroll_to(&mut self, offset: operation::scrollable::AbsoluteOffset<Option<f32>>) {
		if let Some(x) = offset.x {
			self.target.0 = Offset::Absolute(x.max(0.0));
		}

		if let Some(y) = offset.y {
			self.target.1 = Offset::Absolute(y.max(0.0));
		}
	}

	fn scroll_by(
		&mut self, offset: operation::scrollable::AbsoluteOffset, bounds: Rectangle,
		content_bounds: Rectangle,
	) {
		let target = self.target(bounds, content_bounds);

		let max = max_scroll(bounds, content_bounds);

		self.target = (
			Offset::Absolute((target.x + offset.x).clamp(0.0, max.x)),
			Offset::Absolute((target.y + offset.y).clamp(0.0, max.y)),
		);
	}
}

fn max_scroll(bounds: Rectangle, content_bounds: Rectangle) -> Vector {
	Vector::new(
		(content_bounds.width - bounds.width).max(0.0),
		(content_bounds.height - bounds.height).max(0.0),
	)
}

fn wheel_delta(delta: mouse::ScrollDelta) -> Vector {
	match delta {
		mouse::ScrollDelta::Lines { x, y } => -Vector::new(x, y) * 60.0,
		mouse::ScrollDelta::Pixels { x, y } => -Vector::new(x, y),
	}
}

impl<'a, Message, Theme: 'a, Renderer: iced::advanced::Renderer + 'a>
	From<NeoScrollable<'a, Message, Theme, Renderer>> for Element<'a, Message, Theme, Renderer>
where
	Message: Clone + 'a,
{
	fn from(button: NeoScrollable<'a, Message, Theme, Renderer>) -> Self {
		Self::new(button)
	}
}

impl<'a, Message, Theme: 'a, Renderer: 'a> Widget<Message, Theme, Renderer>
	for NeoScrollable<'a, Message, Theme, Renderer>
where
	Message: Clone,
	Renderer: renderer::Renderer,
{
	fn size(&self) -> iced::Size<Length> {
		neo_surface::size(self.width, self.height)
	}

	fn children(&self) -> Vec<Tree> {
		vec![Tree::new(&self.content)]
	}

	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<State>()
	}

	fn diff(&self, tree: &mut Tree) {
		tree.diff_children(std::slice::from_ref(&self.content));
	}

	fn state(&self) -> tree::State {
		tree::State::new(State::default())
	}

	fn operate(
		&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer,
		operation: &mut dyn widget::Operation,
	) {
		let state = tree.state.downcast_mut::<State>();
		let bounds = self.viewport_bounds(layout.bounds());
		let content_bounds = layout.child(0).bounds();
		let translation = state.current;

		operation.scrollable(Some(&self.id), bounds, content_bounds, translation, state);

		operation.traverse(&mut |operation| {
			#[allow(clippy::indexing_slicing)]
			self.content.as_widget_mut().operate(
				&mut tree.children[0],
				layout.child(0),
				renderer,
				operation,
			);
		});
	}

	fn layout(
		&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits,
	) -> layout::Node {
		let scrollbar_width = self.scrollbar_width();

		layout::padded(
			limits,
			self.width,
			self.height,
			Padding {
				right: scrollbar_width + self.spacing,
				..Padding::ZERO
			},
			|limits| {
				let child_limits = layout::Limits::with_compression(
					limits.min(),
					Size::new(limits.max().width, f32::INFINITY),
					Size::new(false, true),
				);

				self.content
					.as_widget_mut()
					.layout(&mut tree.children[0], renderer, &child_limits)
			},
		)
	}

	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &Renderer, shell: &mut Shell<'_, Message>, _viewport: &Rectangle,
	) {
		let state = tree.state.downcast_mut::<State>();
		let bounds = self.viewport_bounds(layout.bounds());
		let content_bounds = layout.child(0).bounds();
		let target = state.target(bounds, content_bounds);

		if !self.smooth_scroll {
			state.current = target;
			state.last_frame = None;
		} else if let Event::Window(iced::window::Event::RedrawRequested(now)) = event {
			if state.animate_towards(target, *now, self.scroll_duration) {
				shell.request_redraw();
			}
		} else if !state.is_at(target) {
			shell.request_redraw();
		}

		let geometry = self.scrollbar_geometry(layout.bounds(), content_bounds, state.current);
		state.handle_hovered = cursor.is_over(geometry.handle_bounds);

		let is_over_viewport = cursor.is_over(bounds);
		let is_over_track = cursor.is_over(geometry.track_bounds);
		let mut handled = false;

		match event {
			Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
				if geometry.max_scroll_y > 0.0 && state.handle_hovered =>
			{
				if let Some(position) = cursor.position() {
					state.interaction = ScrollbarInteraction::Dragging {
						grab_offset: position.y - geometry.handle_bounds.y,
					};
					state.handle_clicked = true;
					handled = true;
					shell.request_redraw();
					shell.capture_event();
				}
			}
			Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
				if geometry.max_scroll_y > 0.0 && is_over_track =>
			{
				if let Some(position) = cursor.position() {
					state.handle_clicked = true;
					state.set_target_y(
						geometry
							.scroll_for_handle_y(position.y - geometry.handle_bounds.height / 2.0),
					);
					if !self.smooth_scroll {
						state.sync_current(bounds, content_bounds);
					}
					handled = true;
					shell.request_redraw();
					shell.capture_event();

					if let Some(on_scroll) = &self.on_scroll {
						shell.publish(on_scroll());
					}
				}
			}
			Event::Mouse(mouse::Event::CursorMoved { position })
				if matches!(state.interaction, ScrollbarInteraction::Dragging { .. }) =>
			{
				let ScrollbarInteraction::Dragging { grab_offset } = state.interaction else {
					unreachable!();
				};
				state.set_target_y(geometry.scroll_for_handle_y(position.y - grab_offset));
				state.sync_current(bounds, content_bounds);
				handled = true;
				shell.request_redraw();
				shell.capture_event();

				if let Some(on_scroll) = &self.on_scroll {
					shell.publish(on_scroll());
				}
			}
			Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
				if state.handle_clicked =>
			{
				state.interaction = ScrollbarInteraction::None;
				state.handle_clicked = false;
				handled = true;
				shell.request_redraw();
				shell.capture_event();
			}
			Event::Mouse(mouse::Event::CursorLeft)
				if matches!(state.interaction, ScrollbarInteraction::Dragging { .. })
					|| state.handle_clicked =>
			{
				state.interaction = ScrollbarInteraction::None;
				state.handle_clicked = false;
				handled = true;
				shell.request_redraw();
				shell.capture_event();
			}
			Event::Mouse(mouse::Event::WheelScrolled { delta })
				if is_over_viewport || is_over_track =>
			{
				let delta = wheel_delta(*delta);
				operation::Scrollable::scroll_by(
					state,
					operation::scrollable::AbsoluteOffset {
						x: delta.x,
						y: delta.y,
					},
					bounds,
					content_bounds,
				);
				if !self.smooth_scroll {
					state.sync_current(bounds, content_bounds);
				}
				handled = true;
				shell.request_redraw();
				shell.capture_event();

				if let Some(on_scroll) = &self.on_scroll {
					shell.publish(on_scroll());
				}
			}
			_ => (),
		}

		if !handled {
			let child_viewport = Rectangle {
				x: bounds.x + state.current.x,
				y: bounds.y + state.current.y,
				..bounds
			};

			Widget::update(
				self.content.as_widget_mut(),
				&mut tree.children[0],
				event,
				layout.child(0),
				cursor + state.current,
				renderer,
				shell,
				&child_viewport,
			);
		}
	}

	fn mouse_interaction(
		&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
		renderer: &Renderer,
	) -> mouse::Interaction {
		let state = tree.state.downcast_ref::<State>();
		let geometry =
			self.scrollbar_geometry(layout.bounds(), layout.child(0).bounds(), state.current);

		if state.handle_clicked {
			mouse::Interaction::Grabbing
		} else if geometry.max_scroll_y > 0.0
			&& (cursor.is_over(geometry.handle_bounds) || cursor.is_over(geometry.track_bounds))
		{
			mouse::Interaction::Pointer
		} else {
			self.content.as_widget().mouse_interaction(
				&tree.children[0],
				layout.child(0),
				cursor + state.current,
				viewport,
				renderer,
			)
		}
	}

	fn draw(
		&self, tree: &Tree, renderer: &mut Renderer, theme: &Theme, style: &renderer::Style,
		layout: Layout<'_>, cursor: mouse::Cursor, _viewport: &Rectangle,
	) {
		let state = tree.state.downcast_ref::<State>();
		let bounds = layout.bounds();

		let content_bounds = self.viewport_bounds(bounds);
		let full_content_bounds = layout.child(0).bounds();
		let geometry = self.scrollbar_geometry(bounds, full_content_bounds, state.current);

		neo_surface::draw(
			renderer,
			content_bounds,
			self.content_style.surface,
			self.content_style.surface.shadow_width,
			|_renderer| {},
		);

		let viewport = Rectangle {
			x: content_bounds.x + state.current.x,
			y: content_bounds.y + state.current.y,
			..content_bounds
		};

		renderer.with_layer(content_bounds, |renderer| {
			renderer.with_translation(-state.current, |renderer| {
				self.content.as_widget().draw(
					&tree.children[0],
					renderer,
					theme,
					style,
					layout.child(0),
					cursor,
					&viewport,
				);
			});
		});

		neo_surface::draw(
			renderer,
			geometry.track_bounds,
			self.track_style,
			0.0,
			|_renderer| {},
		);

		if geometry.max_scroll_y > 0.0 {
			let handle = self.handle_style;
			let shadow_offset = if state.handle_clicked {
				handle.shadow_width / 2.0
			} else {
				handle.shadow_width
			};

			let handle_shadow = Rectangle {
				x: geometry.handle_bounds.x + shadow_offset,
				y: geometry.handle_bounds.y + shadow_offset,
				width: geometry.handle_bounds.width,
				height: geometry.handle_bounds.height,
			};
			let handle_surface = Rectangle {
				x: geometry.handle_bounds.x,
				y: geometry.handle_bounds.y,
				width: geometry.handle_bounds.width,
				height: geometry.handle_bounds.height,
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
					},
					snap: true,
					..Default::default()
				},
				handle.background,
			);
		}
	}
}
