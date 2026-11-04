//! Edit-only input and layout around the normal widget visuals.
//!
//! Children receive redraws, but never input or focus operations. The dragged
//! child is drawn once, above its siblings; its destination is an empty slot.

use config::SystemMenuWidgets;
use iced::{
	Border, Element, Event, Length, Point, Rectangle, Size, Vector,
	advanced::{
		Layout, Shell, Widget, layout, mouse, renderer,
		widget::{Tree, tree},
	},
	keyboard::{self, key::Named},
	window,
};
use neo_widgets::style::COLORS;

use super::Message;

const COLUMNS: usize = 2;
const SPACING: f32 = 8.0;
const CELL_HEIGHT: f32 = 64.0;

pub(super) fn grid<'a>(
	active: usize, entries: Vec<(SystemMenuWidgets, Element<'a, Message>)>,
) -> Element<'a, Message> {
	let (keys, children) = entries.into_iter().unzip();
	Element::new(Editor {
		active,
		keys,
		children,
	})
}

struct Editor<'a, Renderer: iced::advanced::Renderer = iced::Renderer> {
	active: usize,
	keys: Vec<SystemMenuWidgets>,
	children: Vec<Element<'a, Message, iced::Theme, Renderer>>,
}

#[derive(Default)]
struct State {
	keys: Vec<SystemMenuWidgets>,
	press: Option<Press>,
	viewport: Option<Rectangle>,
}

struct Press {
	source: usize,
	start: Point,
	cursor: Point,
	target: usize,
	moved: bool,
}

impl Press {
	fn dragging(&self, active: usize) -> bool {
		self.source < active && self.moved
	}

	fn move_to(&mut self, position: Point, bounds: Rectangle, active: usize) {
		self.cursor = position;
		self.moved |= position != self.start;
		if self.source < active {
			let center = slot_bounds(bounds, self.source).center() + (position - self.start);
			self.target = (0..active)
				.min_by(|left, right| {
					center
						.distance(slot_bounds(bounds, *left).center())
						.total_cmp(&center.distance(slot_bounds(bounds, *right).center()))
				})
				.unwrap_or(self.source);
		}
	}
}

// Remove the source, then make room for it at the insertion slot.
fn display_slot(index: usize, source: usize, target: usize) -> usize {
	if index == source {
		return source;
	}
	let compacted = if index > source { index - 1 } else { index };
	if compacted >= target {
		compacted + 1
	} else {
		compacted
	}
}

fn slot_bounds(bounds: Rectangle, index: usize) -> Rectangle {
	let width = ((bounds.width - SPACING) / COLUMNS as f32).max(0.0);
	Rectangle {
		x: bounds.x + (index % COLUMNS) as f32 * (width + SPACING),
		y: bounds.y + (index / COLUMNS) as f32 * (CELL_HEIGHT + SPACING),
		width,
		height: CELL_HEIGHT,
	}
}

impl<Renderer: iced::advanced::Renderer> Editor<'_, Renderer> {
	fn changed_widgets(&self, press: &Press) -> Vec<SystemMenuWidgets> {
		super::layout::changed_widgets(
			&self.keys,
			self.active,
			press.source,
			press.dragging(self.active).then_some(press.target),
		)
	}
}

impl<Renderer: iced::advanced::Renderer> Widget<Message, iced::Theme, Renderer>
	for Editor<'_, Renderer>
{
	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<State>()
	}

	fn state(&self) -> tree::State {
		tree::State::new(State::default())
	}

	fn diff(&mut self, tree: &mut Tree) {
		let state = tree.state.downcast_mut::<State>();
		if state.keys != self.keys {
			// Retained animations belong to the card, not its former grid slot.
			// Move entire subtrees before diffing so nested toggles keep their state.
			let mut previous = state
				.keys
				.iter()
				.zip(std::mem::take(&mut tree.children))
				.map(|(key, tree)| (key, Some(tree)))
				.collect::<Vec<_>>();
			tree.children = self
				.keys
				.iter()
				.map(|key| {
					previous
						.iter_mut()
						.find(|(old_key, tree)| *old_key == key && tree.is_some())
						.and_then(|(_, tree)| tree.take())
						.unwrap_or_else(Tree::empty)
				})
				.collect();
			state.keys.clone_from(&self.keys);
			state.press = None;
		}
		tree.diff_children(&mut self.children);
	}

	fn size(&self) -> Size<Length> {
		Size::new(Length::Fill, Length::Shrink)
	}

	fn layout(
		&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits,
	) -> layout::Node {
		let width = limits.max().width;
		let rows = self.children.len().div_ceil(COLUMNS);
		let height = rows as f32 * (CELL_HEIGHT + SPACING) - if rows > 0 { SPACING } else { 0.0 };
		let bounds = Rectangle::new(Point::ORIGIN, Size::new(width, height));
		let press = tree
			.state
			.downcast_ref::<State>()
			.press
			.as_ref()
			.filter(|press| press.dragging(self.active));
		let nodes = self
			.children
			.iter_mut()
			.zip(&mut tree.children)
			.enumerate()
			.map(|(index, (child, tree))| {
				let slot = match press {
					Some(press) if index < self.active => {
						display_slot(index, press.source, press.target)
					}
					_ => index,
				};
				let cell = slot_bounds(bounds, slot);
				child
					.as_widget_mut()
					.layout(
						tree,
						renderer,
						&layout::Limits::new(cell.size(), cell.size()),
					)
					.move_to(cell.position())
			})
			.collect();
		layout::Node::with_children(bounds.size(), nodes)
	}

	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &Renderer, shell: &mut Shell<'_, Message>, viewport: &Rectangle,
	) {
		let state = tree.state.downcast_mut::<State>();
		state.viewport = Some(*viewport);
		// Keep animated visuals alive, but do not forward clicks, keyboard input,
		// wheel events, or context-menu requests to the normal controls.
		if matches!(event, Event::Window(window::Event::RedrawRequested(_))) {
			// Scrolling changes content-space coordinates even with a stationary
			// pointer. Keep the dragged card under that pointer during scrolling.
			if let Some(press) = &mut state.press
				&& press.dragging(self.active)
				&& let Some(position) = cursor
					.position()
					.filter(|position| *position != press.cursor)
			{
				press.move_to(position, layout.bounds(), self.active);
				shell.invalidate_layout();
				shell.request_redraw();
			}
			for ((child, tree), layout) in self
				.children
				.iter_mut()
				.zip(&mut tree.children)
				.zip(layout.children())
			{
				child.as_widget_mut().update(
					tree,
					event,
					layout,
					mouse::Cursor::Unavailable,
					renderer,
					shell,
					viewport,
				);
			}
			return;
		}

		let inside = cursor.position().is_some_and(|position| {
			layout.bounds().contains(position) && viewport.contains(position)
		});
		match event {
			Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
				if inside && !shell.is_event_captured() =>
			{
				if let Some(position) = cursor.position()
					&& let Some(source) = layout
						.children()
						.position(|cell| cell.bounds().contains(position))
				{
					state.press = Some(Press {
						source,
						start: position,
						cursor: position,
						target: source,
						moved: false,
					});
					shell.capture_event();
				}
			}
			Event::Mouse(mouse::Event::CursorMoved { position }) if state.press.is_some() => {
				if let Some(press) = &mut state.press {
					// The cursor (unlike the raw event) includes parent scroll offsets.
					press.move_to(
						cursor.position().unwrap_or(*position),
						layout.bounds(),
						self.active,
					);
				}
				shell.invalidate_layout();
				shell.request_redraw();
				shell.capture_event();
			}
			Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
				if state.press.is_some() =>
			{
				if let Some(mut press) = state.press.take() {
					if press.dragging(self.active)
						&& let Some(position) = cursor.position()
					{
						press.move_to(position, layout.bounds(), self.active);
					}
					let over_source = cursor.position().is_some_and(|position| {
						slot_bounds(layout.bounds(), press.source).contains(position)
					});
					if inside && (press.dragging(self.active) || over_source) {
						shell.publish(Message::WidgetsEdited(self.changed_widgets(&press)));
					}
				}
				shell.invalidate_layout();
				shell.request_redraw();
				shell.capture_event();
			}
			Event::Keyboard(keyboard::Event::KeyPressed {
				key: keyboard::Key::Named(Named::Escape),
				..
			}) if state.press.is_some() => {
				state.press = None;
				shell.invalidate_layout();
				shell.request_redraw();
				shell.capture_event();
			}
			Event::Mouse(mouse::Event::CursorLeft) | Event::Window(window::Event::Unfocused) => {
				if state.press.take().is_some() {
					shell.invalidate_layout();
					shell.request_redraw();
				}
			}
			Event::Mouse(mouse::Event::ButtonPressed(_) | mouse::Event::ButtonReleased(_))
				if inside =>
			{
				shell.capture_event()
			}
			_ => {}
		}
	}

	fn mouse_interaction(
		&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
		_renderer: &Renderer,
	) -> mouse::Interaction {
		let state = tree.state.downcast_ref::<State>();
		if state
			.press
			.as_ref()
			.is_some_and(|press| press.dragging(self.active))
		{
			return mouse::Interaction::Grabbing;
		}
		if let Some(position) = cursor
			.position()
			.filter(|position| state.viewport.unwrap_or(*viewport).contains(*position))
			&& let Some(index) = layout
				.children()
				.position(|cell| cell.bounds().contains(position))
		{
			return if index < self.active {
				mouse::Interaction::Grab
			} else {
				mouse::Interaction::Pointer
			};
		}
		mouse::Interaction::None
	}

	fn draw(
		&self, tree: &Tree, renderer: &mut Renderer, theme: &iced::Theme, style: &renderer::Style,
		layout: Layout<'_>, _cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		let dragging = tree
			.state
			.downcast_ref::<State>()
			.press
			.as_ref()
			.filter(|press| press.dragging(self.active));
		if let Some(press) = dragging {
			renderer.fill_quad(
				renderer::Quad {
					bounds: slot_bounds(layout.bounds(), press.target),
					border: Border {
						color: COLORS.decorative.blue,
						width: 2.0,
						radius: 4.0.into(),
					},
					..Default::default()
				},
				COLORS.decorative.blue90,
			);
		}

		// Draw the same children as normal mode, with only an inactive wash and
		// a translation for the dragged child. Never construct a second card.
		let source = dragging.map(|press| press.source);
		let indices = (0..self.children.len())
			.filter(|index| Some(*index) != source)
			.chain(source);
		for index in indices {
			let (Some(child), Some(tree), Some(cell)) = (
				self.children.get(index),
				tree.children.get(index),
				layout.children().nth(index),
			) else {
				continue;
			};
			let translation = dragging
				.filter(|press| press.source == index)
				.map_or(Vector::ZERO, |press| press.cursor - press.start);
			renderer.with_layer(*viewport, |renderer| {
				renderer.with_translation(translation, |renderer| {
					child.as_widget().draw(
						tree,
						renderer,
						theme,
						style,
						cell,
						mouse::Cursor::Unavailable,
						&(*viewport - translation),
					);
					if index >= self.active {
						renderer.fill_quad(
							renderer::Quad {
								bounds: cell.bounds(),
								border: Border {
									radius: 4.0.into(),
									..Default::default()
								},
								..Default::default()
							},
							COLORS.disabled_background.scale_alpha(0.4),
						);
					}
				})
			});
		}
	}
}

#[cfg(test)]
mod interaction_tests;

#[cfg(test)]
mod tests {
	use super::*;
	use iced::Animation;
	use std::time::Instant;

	// Expose the same retained animation/reconciliation used by NeoToggle, with
	// a fixed clock so a brief off frame cannot disappear before we measure it.
	struct ToggleProbe {
		enabled: bool,
		at: Instant,
	}

	impl Widget<Message, iced::Theme, iced::Renderer> for ToggleProbe {
		fn size(&self) -> Size<Length> {
			Size::new(Length::Shrink, Length::Shrink)
		}

		fn tag(&self) -> tree::Tag {
			tree::Tag::of::<Animation<bool>>()
		}

		fn state(&self) -> tree::State {
			tree::State::new(Animation::new(self.enabled).very_quick())
		}

		fn diff(&mut self, tree: &mut Tree) {
			let animation = tree.state.downcast_mut::<Animation<bool>>();
			if animation.value() != self.enabled {
				animation.go_mut(self.enabled, self.at);
			}
		}

		fn layout(
			&mut self, _tree: &mut Tree, _renderer: &iced::Renderer, _limits: &layout::Limits,
		) -> layout::Node {
			layout::Node::new(Size::ZERO)
		}

		fn draw(
			&self, _tree: &Tree, _renderer: &mut iced::Renderer, _theme: &iced::Theme,
			_style: &renderer::Style, _layout: Layout<'_>, _cursor: mouse::Cursor,
			_viewport: &Rectangle,
		) {
		}
	}

	#[test]
	fn reordering_preserves_toggle_animation_state() {
		use SystemMenuWidgets::{Homeassistant, Wifi};
		let at = Instant::now();
		let light = Homeassistant("light.office".to_string());
		let card = |enabled| Element::new(ToggleProbe { enabled, at });
		let mut before = grid(2, vec![(Wifi, card(false)), (light.clone(), card(true))]);
		let mut tree = Tree::new(before.as_widget());
		tree.diff(before.as_widget_mut());

		let mut after = grid(2, vec![(light, card(true)), (Wifi, card(false))]);
		tree.diff(after.as_widget_mut());

		let visuals = tree
			.children
			.iter()
			.map(|child| {
				child
					.state
					.downcast_ref::<Animation<bool>>()
					.interpolate(0.0_f32, 1.0_f32, at)
			})
			.collect::<Vec<_>>();
		assert_eq!(
			visuals,
			vec![1.0, 0.0],
			"moving cards must not restart their toggle animations"
		);
	}

	#[test]
	fn preview_slots_match_committed_order_in_both_directions() {
		for source in 0..6 {
			for target in 0..6 {
				let mut order = (0..6).collect::<Vec<_>>();
				order.remove(source);
				order.insert(target, source);
				for (slot, index) in order
					.into_iter()
					.enumerate()
					.filter(|(_, index)| *index != source)
				{
					assert_eq!(display_slot(index, source, target), slot);
				}
			}
		}
		let editor: Editor<'_> = Editor {
			active: 2,
			keys: vec![
				SystemMenuWidgets::Wifi,
				SystemMenuWidgets::Bluetooth,
				SystemMenuWidgets::Nightlight,
			],
			children: Vec::new(),
		};
		let mut press = Press {
			source: 0,
			start: Point::ORIGIN,
			cursor: Point::ORIGIN,
			target: 0,
			moved: false,
		};
		assert_eq!(
			editor.changed_widgets(&press),
			vec![SystemMenuWidgets::Bluetooth]
		);
		// Even the smallest movement starts a drag instead of removing the card.
		press.move_to(
			Point::new(0.01, 0.0),
			Rectangle::new(Point::ORIGIN, Size::new(400.0, 300.0)),
			editor.active,
		);
		assert!(press.dragging(editor.active));
		assert_eq!(
			editor.changed_widgets(&press),
			vec![SystemMenuWidgets::Wifi, SystemMenuWidgets::Bluetooth]
		);
		// Moving on an inactive card never turns it into a draggable card.
		press.source = 2;
		assert!(!press.dragging(editor.active));
		assert_eq!(editor.changed_widgets(&press), editor.keys);
	}
}
