use std::time::{Duration, Instant};

use iced::{
	Animation, Color, Element, Event, Length, Padding, Pixels, Rectangle, Vector,
	advanced::{
		Layout, Shell, Widget, layout, mouse, overlay, renderer,
		widget::{Operation, Tree, tree},
	},
	widget::{Id, TextInput, text_input},
	window,
};

use crate::style::COLORS;

use super::neo_surface::{self, NeoContentSurfaceStyle};

pub fn neo_text_input<'a, Message: Clone + 'a>(
	placeholder: impl iced::widget::text::IntoFragment<'a>,
	value: impl iced::widget::text::IntoFragment<'a>,
) -> NeoTextInput<'a, Message> {
	NeoTextInput::new(placeholder, value)
}

/// Native Iced editing, selection, clipboard, and focus behavior with neo chrome.
#[must_use]
pub struct NeoTextInput<'a, Message> {
	input: TextInput<'a, Message>,
	width: Length,
}

impl<'a, Message: Clone + 'a> NeoTextInput<'a, Message> {
	pub fn new(
		placeholder: impl iced::widget::text::IntoFragment<'a>,
		value: impl iced::widget::text::IntoFragment<'a>,
	) -> Self {
		Self {
			input: TextInput::new(placeholder, value).style(input_style),
			width: Length::Fill,
		}
	}

	pub fn id(mut self, id: impl Into<Id>) -> Self {
		self.input = self.input.id(id);
		self
	}

	pub fn secure(mut self, secure: bool) -> Self {
		self.input = self.input.secure(secure);
		self
	}

	pub fn on_input(mut self, on_input: impl Fn(String) -> Message + 'a) -> Self {
		self.input = self.input.on_input(on_input);
		self
	}

	pub fn on_submit(mut self, message: Message) -> Self {
		self.input = self.input.on_submit(message);
		self
	}

	pub fn width(mut self, width: impl Into<Length>) -> Self {
		self.width = width.into();
		self
	}

	pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
		self.input = self.input.padding(padding);
		self
	}

	pub fn size(mut self, size: impl Into<Pixels>) -> Self {
		self.input = self.input.size(size);
		self
	}
}

impl<'a, Message: Clone + 'a> From<NeoTextInput<'a, Message>> for Element<'a, Message> {
	fn from(input: NeoTextInput<'a, Message>) -> Self {
		Element::new(NeoField::new(input.input, input.width, false))
	}
}

fn input_style(_theme: &iced::Theme, _status: text_input::Status) -> text_input::Style {
	text_input::Style {
		background: Color::TRANSPARENT.into(),
		border: iced::Border {
			color: COLORS.black,
			width: 0.0,
			radius: 0.0.into(),
		},
		placeholder: COLORS.secondary,
		value: COLORS.black,
		selection: COLORS.decorative.blue90,
	}
}

/// Adds chrome without owning the child's focus, input state, or overlays.
pub(crate) struct NeoField<'a, Message> {
	content: Element<'a, Message>,
	width: Length,
	animated: bool,
	style: NeoContentSurfaceStyle,
}

impl<'a, Message: 'a> NeoField<'a, Message> {
	pub(crate) fn new(
		content: impl Into<Element<'a, Message>>, width: Length, animated: bool,
	) -> Self {
		Self {
			content: content.into(),
			width,
			animated,
			style: NeoContentSurfaceStyle {
				padding: 2.0.into(),
				..Default::default()
			},
		}
	}
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for NeoField<'_, Message> {
	fn size(&self) -> iced::Size<Length> {
		neo_surface::size(self.width, self.content.as_widget().size().height)
	}

	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<FieldState>()
	}

	fn state(&self) -> tree::State {
		tree::State::new(FieldState::default())
	}

	fn diff(&mut self, tree: &mut Tree) {
		tree.diff_children(std::slice::from_mut(&mut self.content));
	}

	fn layout(
		&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits,
	) -> layout::Node {
		neo_surface::layout(
			&mut self.content,
			tree,
			renderer,
			limits,
			self.width,
			Length::Shrink,
			self.style,
		)
	}

	fn operate(
		&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &iced::Renderer,
		operation: &mut dyn Operation,
	) {
		operation.traverse(&mut |operation| {
			self.content.as_widget_mut().operate(
				&mut tree.children[0],
				layout.child(0),
				renderer,
				operation,
			);
		});
	}

	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &iced::Renderer, shell: &mut Shell<'_, Message>, viewport: &Rectangle,
	) {
		// Observe before delegation: the native opener captures its own press.
		if self.animated {
			let state = tree.state.downcast_mut::<FieldState>();
			let over = cursor.is_over(layout.child(0).bounds());

			if state.observe(event, over, Instant::now()) {
				shell.request_redraw();
			}
		}

		self.content.as_widget_mut().update(
			&mut tree.children[0],
			event,
			layout.child(0),
			cursor,
			renderer,
			shell,
			viewport,
		);
	}

	fn mouse_interaction(
		&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
		renderer: &iced::Renderer,
	) -> mouse::Interaction {
		self.content.as_widget().mouse_interaction(
			&tree.children[0],
			layout.child(0),
			cursor,
			viewport,
			renderer,
		)
	}

	fn draw(
		&self, tree: &Tree, renderer: &mut iced::Renderer, theme: &iced::Theme,
		style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		let state = tree.state.downcast_ref::<FieldState>();
		let offset =
			state
				.pressed
				.interpolate(0.0, self.style.surface.shadow_width, Instant::now());

		neo_surface::draw(
			renderer,
			layout.bounds(),
			self.style.surface,
			offset,
			|renderer| {
				self.content.as_widget().draw(
					&tree.children[0],
					renderer,
					theme,
					style,
					layout.child(0),
					cursor,
					viewport,
				);
			},
		);
	}

	fn overlay<'b>(
		&'b mut self, tree: &'b mut Tree, layout: Layout<'b>, renderer: &iced::Renderer,
		viewport: &Rectangle, translation: Vector,
	) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
		// Keep native placement anchored to the stable child layout, not the pressed drawing.
		let style = self.style.surface;
		let overlays = self.content.as_widget_mut().overlay(
			&mut tree.children[0],
			layout.child(0),
			renderer,
			viewport,
			translation,
		);

		if self.animated {
			overlays
				.into_iter()
				.map(|content| overlay::Element::new(Box::new(MenuShadow { content, style })))
				.collect()
		} else {
			overlays
		}
	}
}

/// Draw a real opaque offset surface; zero-blur native shadows vary by renderer.
struct MenuShadow<'a, Message> {
	content: overlay::Element<'a, Message, iced::Theme, iced::Renderer>,
	style: super::NeoSurfaceStyle,
}

impl<Message> overlay::Overlay<Message, iced::Theme, iced::Renderer> for MenuShadow<'_, Message> {
	fn layout(&mut self, renderer: &iced::Renderer, bounds: iced::Size) -> layout::Node {
		let content = self.content.as_overlay_mut().layout(renderer, bounds);

		menu_layout(content, self.style.shadow_width)
	}

	fn draw(
		&self, renderer: &mut iced::Renderer, theme: &iced::Theme, defaults: &renderer::Style,
		layout: Layout<'_>, cursor: mouse::Cursor,
	) {
		use iced::advanced::Renderer as _;

		let content_layout = layout.child(0);
		let mut shadow = content_layout.bounds();
		shadow.x += self.style.shadow_width;
		shadow.y += self.style.shadow_width;

		renderer.fill_quad(
			renderer::Quad {
				bounds: shadow,
				border: iced::Border {
					radius: self.style.radius.into(),
					..Default::default()
				},
				snap: true,
				..Default::default()
			},
			self.style.border,
		);

		self.content
			.as_overlay()
			.draw(renderer, theme, defaults, content_layout, cursor);

		// Native selected-row backgrounds can paint over the menu's own border.
		renderer.fill_quad(
			renderer::Quad {
				bounds: content_layout.bounds(),
				border: iced::Border {
					color: self.style.border,
					width: self.style.border_width,
					radius: self.style.radius.into(),
				},
				snap: true,
				..Default::default()
			},
			Color::TRANSPARENT,
		);
	}

	fn operate(
		&mut self, layout: Layout<'_>, renderer: &iced::Renderer, operation: &mut dyn Operation,
	) {
		self.content
			.as_overlay_mut()
			.operate(layout.child(0), renderer, operation);
	}

	fn update(
		&mut self, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &iced::Renderer, shell: &mut Shell<'_, Message>,
	) {
		self.content
			.as_overlay_mut()
			.update(event, layout.child(0), cursor, renderer, shell);
	}

	fn mouse_interaction(
		&self, layout: Layout<'_>, cursor: mouse::Cursor, renderer: &iced::Renderer,
	) -> mouse::Interaction {
		self.content
			.as_overlay()
			.mouse_interaction(layout.child(0), cursor, renderer)
	}

	fn overlay<'a>(
		&'a mut self, layout: Layout<'a>, renderer: &iced::Renderer,
	) -> Vec<overlay::Element<'a, Message, iced::Theme, iced::Renderer>> {
		self.content
			.as_overlay_mut()
			.overlay(layout.child(0), renderer)
	}

	fn index(&self) -> f32 {
		self.content.as_overlay().index()
	}
}

fn menu_layout(content: layout::Node, shadow: f32) -> layout::Node {
	// Iced clips each overlay to its layout bounds. Reserve shadow pixels in
	// the wrapper while retaining the native menu's original position/hit boxes.
	let position = content.bounds().position();
	let size = content.size();

	layout::Node::with_children(
		iced::Size::new(size.width + shadow, size.height + shadow),
		vec![content.move_to(iced::Point::ORIGIN)],
	)
	.move_to(position)
}

#[derive(Debug)]
struct FieldState {
	pressed: Animation<bool>,
}

impl Default for FieldState {
	fn default() -> Self {
		Self {
			pressed: Animation::new(false)
				.delay(Duration::ZERO)
				.duration(Duration::from_millis(50)),
		}
	}
}

impl FieldState {
	fn observe(&mut self, event: &Event, over: bool, now: Instant) -> bool {
		let pressed = match event {
			Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if over => true,
			Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
			| Event::Mouse(mouse::Event::CursorLeft)
			| Event::Window(window::Event::Unfocused) => false,
			Event::Window(window::Event::RedrawRequested(now)) => {
				return self.pressed.is_animating(*now);
			}
			_ => return false,
		};

		if self.pressed.value() == pressed {
			return false;
		}

		self.pressed = self.pressed.clone().go(pressed, now);
		true
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn overlay_reserves_shadow_without_moving_native_menu_hit_boxes() {
		let original =
			layout::Node::new(iced::Size::new(100.0, 60.0)).move_to(iced::Point::new(20.0, 30.0));
		let original_bounds = original.bounds();

		let wrapped = menu_layout(original, 4.0);
		let layout = Layout::new(&wrapped);

		assert_eq!(layout.bounds().position(), original_bounds.position());
		assert_eq!(layout.bounds().size(), iced::Size::new(104.0, 64.0));
		assert_eq!(layout.child(0).bounds(), original_bounds);
	}

	#[test]
	fn input_chrome_is_neutral_in_every_native_state() {
		for status in [
			text_input::Status::Active,
			text_input::Status::Hovered,
			text_input::Status::Focused { is_hovered: false },
			text_input::Status::Disabled,
		] {
			let style = input_style(&iced::Theme::Light, status);

			assert_eq!(style.background, Color::TRANSPARENT.into());
			assert_eq!(style.border.width, 0.0);
			assert_eq!(style.value, COLORS.black);
			assert_eq!(style.selection, COLORS.decorative.blue90);
		}
	}

	#[test]
	fn press_finishes_in_button_time_and_releases_outside() {
		let mut state = FieldState::default();
		let now = Instant::now();
		let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
		let release = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));

		assert!(!state.observe(&press, false, now));
		assert!(state.observe(&press, true, now));
		assert!(state.pressed.value());
		assert_eq!(
			state
				.pressed
				.interpolate(0.0, 4.0, now + Duration::from_millis(51)),
			4.0
		);

		assert!(state.observe(&release, false, now + Duration::from_millis(60)));
		assert!(!state.pressed.value());
		assert_eq!(
			state
				.pressed
				.interpolate(0.0, 4.0, now + Duration::from_millis(111)),
			0.0
		);
	}

	#[test]
	fn losing_window_focus_releases_the_surface() {
		let mut state = FieldState::default();
		let now = Instant::now();

		state.observe(
			&Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
			true,
			now,
		);
		assert!(state.observe(&Event::Window(window::Event::Unfocused), false, now));
		assert!(!state.pressed.value());
	}
}
