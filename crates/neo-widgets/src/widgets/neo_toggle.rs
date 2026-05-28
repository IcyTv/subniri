use std::time::Instant;

use iced::{
	Animation, Color, Element, Event, Length, Padding, Rectangle,
	advanced::{
		Widget, layout, mouse, renderer,
		widget::{Tree, tree},
	},
	window,
};

use crate::{
	style::COLORS,
	widgets::{NeoSurfaceStyle, neo_surface},
};

pub fn neo_toggle<Message>() -> NeoToggle<Message> {
	NeoToggle::new()
}

pub struct NeoToggle<Message> {
	track: NeoSurfaceStyle,
	fill_color: Color,
	handle: NeoSurfaceStyle,
	on_toggled: Option<Box<dyn Fn(bool) -> Message>>,
	toggled: bool,
	width: Length,
	height: Length,
}

impl<Message> NeoToggle<Message> {
	pub fn new() -> Self {
		Self {
			track: Default::default(),
			fill_color: COLORS.decorative.green70,
			handle: NeoSurfaceStyle {
				background: COLORS.decorative.yellow,
				..Default::default()
			},
			on_toggled: None,
			toggled: false,
			width: Length::Shrink,
			height: Length::Shrink,
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

	pub fn fill_color(mut self, color: Color) -> Self {
		self.fill_color = color;
		self
	}

	pub fn on_toggled(mut self, f: impl Fn(bool) -> Message + 'static) -> Self {
		self.on_toggled = Some(Box::new(f));
		self
	}

	pub fn toggled(mut self, toggled: bool) -> Self {
		self.toggled = toggled;
		self
	}

	pub fn width(mut self, width: Length) -> Self {
		self.width = width;
		self
	}

	pub fn height(mut self, height: Length) -> Self {
		self.height = height;
		self
	}

	pub fn shadow_width(mut self, shadow_width: f32) -> Self {
		self.track.shadow_width = shadow_width;
		self.handle.shadow_width = shadow_width;
		self
	}
}

#[derive(Debug)]
struct State {
	toggled: Animation<bool>,
	pressed: Animation<bool>,
	hovered: bool,
}

impl Default for State {
	fn default() -> Self {
		Self::new(false)
	}
}

impl State {
	fn new(toggled: bool) -> Self {
		Self {
			toggled: Animation::new(toggled).very_quick(),
			pressed: Animation::new(false).very_quick(),
			hovered: false,
		}
	}
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for NeoToggle<Message>
where
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
		tree::State::new(State::new(self.toggled))
	}

	fn diff(&self, tree: &mut Tree) {
		let state = tree.state.downcast_mut::<State>();

		if state.toggled.value() != self.toggled {
			state.toggled.go_mut(self.toggled, Instant::now());
		}
	}

	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: layout::Layout<'_>,
		cursor: mouse::Cursor, _renderer: &Renderer,
		shell: &mut iced::advanced::Shell<'_, Message>, _viewport: &Rectangle,
	) {
		let state = tree.state.downcast_mut::<State>();
		let bounds = layout.bounds();
		let over = cursor.is_over(bounds);

		state.hovered = over;
		state.toggled.go_mut(self.toggled, Instant::now());

		match event {
			Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if over => {
				state.pressed.go_mut(true, Instant::now());

				shell.request_redraw();
				shell.capture_event();
			}
			Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
				if state.pressed.value() =>
			{
				if over {
					if let Some(on_toggled) = &self.on_toggled {
						self.toggled = !self.toggled;
						state.toggled.go_mut(self.toggled, Instant::now());
						shell.publish(on_toggled(self.toggled));
					}
				}

				state.pressed.go_mut(false, Instant::now());
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
			Event::Window(window::Event::RedrawRequested(now)) => {
				if state.pressed.is_animating(*now) || state.toggled.is_animating(*now) {
					shell.request_redraw();
				}
			}
			_ => (),
		}
	}

	fn mouse_interaction(
		&self, _tree: &Tree, layout: layout::Layout<'_>, cursor: mouse::Cursor,
		_viewport: &Rectangle, _renderer: &Renderer,
	) -> mouse::Interaction {
		if cursor.is_over(layout.bounds()) {
			mouse::Interaction::Pointer
		} else {
			mouse::Interaction::None
		}
	}

	fn layout(
		&mut self, _tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits,
	) -> layout::Node {
		let shadow = self.track.shadow_width;

		layout::padded(
			limits,
			self.width,
			self.height,
			Padding {
				top: 0.0,
				right: shadow,
				bottom: shadow,
				left: 0.0,
			},
			|limits| layout::atomic(limits, self.width, self.height),
		)
	}

	fn draw(
		&self, tree: &Tree, renderer: &mut Renderer, _theme: &Theme, _style: &renderer::Style,
		layout: layout::Layout<'_>, _cursor: mouse::Cursor, _viewport: &Rectangle,
	) {
		let state = tree.state.downcast_ref::<State>();
		let bounds = layout.bounds();
		let track_style = self.track;
		let handle_style = self.handle;
		let fill_color = self.fill_color;

		let s = track_style.shadow_width;

		let offset = state.pressed.interpolate(0.0, s, Instant::now());

		if s > 0.0 {
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
						radius: track_style.radius.into(),
						..Default::default()
					},
					snap: true,
					..Default::default()
				},
				track_style.border,
			);
		}

		let track = Rectangle {
			x: bounds.x + offset,
			y: bounds.y + offset,
			width: bounds.width - s,
			height: bounds.height - s,
		};

		let track_fill_color =
			state
				.toggled
				.interpolate(track_style.background, fill_color, Instant::now());

		renderer.fill_quad(
			renderer::Quad {
				bounds: track,
				border: iced::Border {
					radius: track_style.radius.into(),
					color: track_style.border,
					width: track_style.border_width,
					..Default::default()
				},
				snap: true,
				..Default::default()
			},
			track_fill_color,
		);

		let handle_size = bounds.height * 1.25;
		let handle_x = state.toggled.interpolate(
			bounds.x,
			bounds.x + bounds.width - handle_size,
			Instant::now(),
		);

		let handle = Rectangle {
			x: handle_x + offset,
			y: bounds.y - (bounds.height * 0.125) + offset,
			width: handle_size,
			height: handle_size,
		};

		renderer.fill_quad(
			renderer::Quad {
				bounds: handle,
				border: iced::Border {
					radius: handle_style.radius.into(),
					width: handle_style.border_width,
					color: handle_style.border,
					..Default::default()
				},
				snap: true,
				..Default::default()
			},
			handle_style.background,
		);
	}
}

impl<'a, Message, Theme, Renderer> Into<Element<'a, Message, Theme, Renderer>>
	for NeoToggle<Message>
where
	Message: Clone + 'a,
	Theme: 'a,
	Renderer: renderer::Renderer,
{
	fn into(self) -> Element<'a, Message, Theme, Renderer> {
		Element::new(self)
	}
}
