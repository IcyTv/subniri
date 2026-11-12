//! Geometry-aware drag interception, without replacing the retained child tree.
use super::{Area, Hit, Message, Scroll};
use crate::Message as AppMessage;
use iced::{
	Element, Event, Length, Point, Rectangle, Size, Task, Vector,
	advanced::{
		Layout, Shell, Widget, layout, mouse, overlay, renderer,
		shell::Bus,
		widget::{Id, Operation, Tree, operation, tree},
	},
};
use std::any::Any;

pub(crate) fn zone<'a>(content: Element<'a, AppMessage>, area: Area) -> Element<'a, AppMessage> {
	Element::new(Wrapper {
		content,
		area: Some(area),
		enabled: false,
		gesture: false,
	})
}

/// Keep this gate mounted even when gestures are disabled.
pub(crate) fn gate<'a>(
	content: Element<'a, AppMessage>, enabled: bool, gesture: bool,
) -> Element<'a, AppMessage> {
	Element::new(Wrapper {
		content,
		area: None,
		enabled,
		gesture,
	})
}

pub(crate) fn hit_test(point: Point) -> Task<Hit> {
	iced::advanced::widget::operate(HitTest::new(point))
}

#[derive(Clone, Copy, Default)]
struct Context {
	translation: Vector,
	clip: Option<Rectangle>,
	clipped: bool,
}

struct HitTest {
	point: Point,
	hit: Hit,
	context: Context,
	pending: Option<Context>,
	priority: u8,
	area: f32,
}

impl HitTest {
	fn new(point: Point) -> Self {
		Self {
			point,
			hit: Hit::default(),
			context: Context::default(),
			pending: None,
			priority: 0,
			area: f32::INFINITY,
		}
	}

	fn visible(&self, bounds: Rectangle) -> Option<Rectangle> {
		if self.context.clipped {
			return None;
		}

		let bounds = bounds - self.context.translation;
		match self.context.clip {
			Some(clip) => bounds.intersection(&clip),
			None => Some(bounds),
		}
	}

	fn observe_scroll(
		&mut self, id: Option<&Id>, bounds: Rectangle, content: Rectangle, translation: Vector,
	) {
		let maximum = (content.height - bounds.height).max(0.0);
		let visible = self.visible(bounds);
		self.pending = Some(Context {
			translation: self.context.translation + translation,
			clip: visible,
			clipped: visible.is_none(),
		});

		if let Some(bounds) = visible.filter(|bounds| bounds.contains(self.point))
			&& let Some(id) = id
		{
			self.hit.scroll = Some(Scroll {
				id: id.clone(),
				bounds,
				offset: translation.y,
				maximum,
			});
		}
	}

	fn observe_area(&mut self, bounds: Rectangle, area: &Area) {
		let Some(visible) = self.visible(bounds) else {
			return;
		};
		if !visible.contains(self.point) {
			return;
		}

		let priority = if matches!(area, Area::Background(_)) {
			1
		} else {
			2
		};
		let size = bounds.width * bounds.height;
		if priority < self.priority || (priority == self.priority && size > self.area) {
			return;
		}

		self.priority = priority;
		self.area = size;
		self.hit.entry = None;
		self.hit.navigation = false;
		self.hit.target = match area {
			Area::Entry { path, directory } => {
				self.hit.entry = Some(path.clone());
				self.hit.navigation = *directory;
				directory.then(|| path.clone())
			}
			Area::Background(path) => Some(path.clone()),
			Area::Sidebar(path) => {
				self.hit.navigation = true;
				Some(path.clone())
			}
		};
	}
}

impl Operation<Hit> for HitTest {
	fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Hit>)) {
		// Scrollable reports itself immediately before traversing its content.
		// Restore the parent context before the next sibling is visited.
		let parent = self.context;
		if let Some(context) = self.pending.take() {
			self.context = context;
		}
		operate(self);
		self.context = parent;
	}

	fn scrollable(
		&mut self, id: Option<&Id>, bounds: Rectangle, content: Rectangle, translation: Vector,
		_: &mut dyn operation::Scrollable,
	) {
		self.observe_scroll(id, bounds, content, translation);
	}

	fn custom(&mut self, _: Option<&Id>, bounds: Rectangle, state: &mut dyn Any) {
		if let Some(area) = state.downcast_ref::<Area>() {
			self.observe_area(bounds, area);
		}
	}

	fn finish(&self) -> operation::Outcome<Hit> {
		operation::Outcome::Some(self.hit.clone())
	}
}

struct Wrapper<'a> {
	content: Element<'a, AppMessage>,
	area: Option<Area>,
	enabled: bool,
	gesture: bool,
}

#[derive(Default)]
struct State {
	pressed: bool,
	point: Point,
}

impl Wrapper<'_> {
	fn clear_child(
		&mut self, child: &mut Tree, layout: Layout<'_>, renderer: &iced::Renderer,
		shell: &Shell<'_, AppMessage>, viewport: &Rectangle,
	) {
		// Reset button/mouse-area captures without leaking synthetic messages.
		let mut bus = Bus::<AppMessage>::new();
		let mut silent = shell.local(&mut bus);
		for event in [
			Event::Window(iced::window::Event::Unfocused),
			Event::Mouse(mouse::Event::CursorLeft),
		] {
			self.content.as_widget_mut().update(
				child,
				&event,
				layout,
				mouse::Cursor::Unavailable,
				renderer,
				&mut silent,
				viewport,
			);
		}
	}
}

impl Widget<AppMessage, iced::Theme, iced::Renderer> for Wrapper<'_> {
	fn size(&self) -> Size<Length> {
		self.content.as_widget().size()
	}

	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<State>()
	}

	fn state(&self) -> tree::State {
		tree::State::new(State::default())
	}

	fn diff(&mut self, tree: &mut Tree) {
		if !self.enabled || !self.gesture {
			tree.state.downcast_mut::<State>().pressed = false;
		}
		tree.diff_children(std::slice::from_mut(&mut self.content));
	}

	fn layout(
		&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits,
	) -> layout::Node {
		self.content
			.as_widget_mut()
			.layout(&mut tree.children[0], renderer, limits)
	}

	fn operate(
		&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &iced::Renderer,
		operation: &mut dyn Operation,
	) {
		if let Some(area) = &mut self.area {
			operation.custom(None, layout.bounds(), area);
		}
		self.content
			.as_widget_mut()
			.operate(&mut tree.children[0], layout, renderer, operation);
	}

	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &iced::Renderer, shell: &mut Shell<'_, AppMessage>, viewport: &Rectangle,
	) {
		let state = tree.state.downcast_mut::<State>();
		let child = &mut tree.children[0];
		if self.area.is_none() {
			if state.pressed && (!self.enabled || !self.gesture) {
				state.pressed = false;
				self.clear_child(child, layout, renderer, shell, viewport);
			}

			if state.pressed {
				let cancel = matches!(event, Event::Window(iced::window::Event::Unfocused))
					|| matches!(
						event,
						Event::Keyboard(iced::keyboard::Event::KeyPressed {
							key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
							..
						})
					);
				if cancel {
					state.pressed = false;
					self.clear_child(child, layout, renderer, shell, viewport);
					shell.publish(Message::Cancel.into());
					shell.capture_event();
					return;
				}
			}

			let motion = matches!(event, Event::Mouse(mouse::Event::CursorMoved { .. }));
			let release = matches!(
				event,
				Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
			);
			let press = self.enabled
				&& !shell.is_event_captured()
				&& matches!(
					event,
					Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
				);
			if press || (state.pressed && (motion || release)) {
				let point = match event {
					Event::Mouse(mouse::Event::CursorMoved { position }) => Some(*position),
					_ => cursor.position(),
				};
				if point.is_some() || state.pressed {
					let point = point.unwrap_or(state.point);
					let mut query = HitTest::new(point);
					query.context.clip = Some(*viewport);
					self.content.as_widget_mut().operate(
						child,
						layout,
						renderer,
						&mut operation::black_box(&mut query),
					);
					if state.pressed {
						state.point = point;
						if release {
							state.pressed = false;
							shell.publish(Message::Released(point, query.hit).into());
						} else {
							shell.publish(Message::Motion(point, query.hit).into());
						}
						shell.capture_event();
						return;
					} else if let Some(path) = query.hit.entry {
						state.pressed = true;
						state.point = point;
						self.clear_child(child, layout, renderer, shell, viewport);
						shell.publish(Message::Pressed(path, point).into());
						shell.capture_event();
						return;
					}
				}
			}
		}

		self.content
			.as_widget_mut()
			.update(child, event, layout, cursor, renderer, shell, viewport);
	}

	fn mouse_interaction(
		&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
		renderer: &iced::Renderer,
	) -> mouse::Interaction {
		self.content.as_widget().mouse_interaction(
			&tree.children[0],
			layout,
			cursor,
			viewport,
			renderer,
		)
	}

	fn draw(
		&self, tree: &Tree, renderer: &mut iced::Renderer, theme: &iced::Theme,
		style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		self.content.as_widget().draw(
			&tree.children[0],
			renderer,
			theme,
			style,
			layout,
			cursor,
			viewport,
		);
	}

	fn overlay<'b>(
		&'b mut self, tree: &'b mut Tree, layout: Layout<'b>, renderer: &iced::Renderer,
		viewport: &Rectangle, translation: Vector,
	) -> Vec<overlay::Element<'b, AppMessage, iced::Theme, iced::Renderer>> {
		self.content.as_widget_mut().overlay(
			&mut tree.children[0],
			layout,
			renderer,
			viewport,
			translation,
		)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn bounds(y: f32, height: f32) -> Rectangle {
		Rectangle {
			x: 0.0,
			y,
			width: 100.0,
			height,
		}
	}

	#[test]
	fn translated_entries_respect_viewport_and_restore_siblings() {
		let mut hit = HitTest::new(Point::new(10.0, 20.0));
		hit.observe_scroll(
			Some(&Id::from("files")),
			bounds(0.0, 100.0),
			bounds(0.0, 400.0),
			Vector::new(0.0, 100.0),
		);
		hit.traverse(&mut |operation| {
			operation.custom(
				None,
				bounds(100.0, 100.0),
				&mut Area::Background("/root".into()),
			);
			operation.custom(
				None,
				bounds(110.0, 30.0),
				&mut Area::Entry {
					path: "/root/file".into(),
					directory: false,
				},
			);
			operation.custom(
				None,
				bounds(0.0, 30.0),
				&mut Area::Entry {
					path: "/offscreen".into(),
					directory: true,
				},
			);
		});

		assert_eq!(hit.hit.entry, Some("/root/file".into()));
		assert!(hit.hit.target.is_none());
		assert!(!hit.hit.navigation);
		assert_eq!(hit.context.translation, Vector::ZERO);
		assert_eq!(
			hit.hit.scroll.as_ref().map(|scroll| scroll.offset),
			Some(100.0)
		);

		let mut sibling = HitTest::new(Point::new(10.0, 20.0));
		sibling.observe_scroll(
			None,
			bounds(0.0, 100.0),
			bounds(0.0, 400.0),
			Vector::new(0.0, 100.0),
		);
		sibling.traverse(&mut |_| {});
		sibling.observe_area(bounds(10.0, 30.0), &Area::Sidebar("/sidebar".into()));
		assert_eq!(sibling.hit.target, Some("/sidebar".into()));
		assert!(sibling.hit.entry.is_none());
		assert!(sibling.hit.navigation);
	}

	#[test]
	fn offscreen_child_cannot_hit_outside_its_scroll_viewport() {
		let mut hit = HitTest::new(Point::new(10.0, 150.0));
		hit.observe_scroll(
			Some(&Id::from("files")),
			bounds(0.0, 100.0),
			bounds(0.0, 400.0),
			Vector::new(0.0, 100.0),
		);
		hit.traverse(&mut |operation| {
			operation.custom(
				None,
				bounds(240.0, 30.0),
				&mut Area::Entry {
					path: "/clipped".into(),
					directory: true,
				},
			);
		});

		assert!(hit.hit.entry.is_none());
		assert!(hit.hit.target.is_none());
		assert!(hit.hit.scroll.is_none());
	}

	#[test]
	fn nested_scrolls_intersect_clips_and_accumulate_translation() {
		let mut hit = HitTest::new(Point::new(10.0, 60.0));
		hit.observe_scroll(
			None,
			bounds(0.0, 100.0),
			bounds(0.0, 500.0),
			Vector::new(0.0, 100.0),
		);
		hit.context = hit.pending.take().unwrap_or_default();
		hit.observe_scroll(
			Some(&Id::from("nested")),
			bounds(150.0, 100.0),
			bounds(150.0, 300.0),
			Vector::new(0.0, 40.0),
		);
		hit.traverse(&mut |operation| {
			operation.custom(
				None,
				bounds(190.0, 30.0),
				&mut Area::Entry {
					path: "/nested".into(),
					directory: true,
				},
			);
		});

		assert_eq!(hit.hit.target, Some("/nested".into()));
		assert_eq!(
			hit.hit.scroll.as_ref().map(|scroll| scroll.bounds),
			Some(bounds(50.0, 50.0))
		);
	}
}
