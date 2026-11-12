//! Shared retained tile scene for normal/edit modes, including reversible resize previews.
use super::{Message, spatial};
use config::{MenuRow, PlacedTile, SystemMenuWidgets};
use iced::{
	Border, Element, Event, Length, Point, Rectangle, Size, Transformation, Vector,
	advanced::{
		Layout, Shell, Widget, layout, mouse, overlay, renderer,
		widget::{Tree, tree},
	},
	keyboard::{self, key::Named},
	window,
};
use neo_widgets::{animation::geometry::RectangleMotion, style::Colors};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key(SystemMenuWidgets, usize);

pub(super) fn grid<'a>(
	editing: bool, rows: &[MenuRow], baseline: &[PlacedTile], enabled: &[bool],
	entries: Vec<(SystemMenuWidgets, Option<PlacedTile>, Element<'a, Message>)>, size: Size,
	maximum: Size, ready: bool, colors: Colors,
) -> Element<'a, Message> {
	let mut keys = Vec::new();
	let mut targets = Vec::new();
	let mut children = Vec::new();
	for (widget, tile, child) in entries {
		let occurrence = keys.iter().filter(|key: &&Key| key.0 == widget).count();
		keys.push(Key(widget, occurrence));
		targets.push(tile);
		children.push(child);
	}
	Element::new(Tiles {
		editing,
		rows: rows.to_vec(),
		baseline: baseline.to_vec(),
		keys,
		targets,
		children,
		size,
		maximum,
		ready,
		colors,
		enabled: enabled.to_vec(),
	})
}

struct Tiles<'a, Renderer: iced::advanced::Renderer = iced::Renderer> {
	editing: bool,
	rows: Vec<MenuRow>,
	baseline: Vec<PlacedTile>,
	keys: Vec<Key>,
	targets: Vec<Option<PlacedTile>>,
	children: Vec<Element<'a, Message, iced::Theme, Renderer>>,
	size: Size,
	maximum: Size,
	ready: bool,
	colors: Colors,
	enabled: Vec<bool>,
}

// NOTE: Gesture anchors use popup-local coordinates and assume the visible popup
// origin stays fixed during editing, as in the current left-edge system menu.
// If movable module placement is introduced, revisit anchors across popup moves;
// supporting that is out of scope for the current setup.
struct Press {
	source: usize,
	start: Point,
	cursor: Point,
	resize: bool,
	moved: bool,
	baseline: Vec<PlacedTile>,
	desired: PlacedTile,
	grid_origin: Point,
	insert_row: Option<u32>,
}

impl Press {
	fn dragged_bounds(&self) -> Option<Rectangle> {
		if self.resize || !self.moved {
			return None;
		}
		self.baseline
			.get(self.source)
			.map(|tile| spatial::bounds(tile) + (self.cursor - self.start))
	}
}

struct State {
	initialized: bool,
	keys: Vec<Key>,
	targets: Vec<Option<PlacedTile>>,
	baseline: Vec<PlacedTile>,
	motion: Vec<RectangleMotion>,
	press: Option<Press>,
	now: Instant,
	paused: bool,
}
impl Default for State {
	fn default() -> Self {
		Self {
			initialized: false,
			keys: Vec::new(),
			targets: Vec::new(),
			baseline: Vec::new(),
			motion: Vec::new(),
			press: None,
			now: Instant::now(),
			paused: false,
		}
	}
}

impl State {
	fn handoff_drag(&mut self, press: &Press) {
		if let Some(bounds) = press.dragged_bounds()
			&& let Some(motion) = self.motion.get_mut(press.source)
		{
			// draw() follows the pointer independently of snapped reflow. On
			// release/cancel that exact rectangle becomes the next motion's origin.
			self.now = Instant::now();
			*motion = RectangleMotion::new(bounds, self.now);
		}
	}
}

fn handle(bounds: Rectangle) -> Rectangle {
	Rectangle::new(
		Point::new(
			bounds.x + bounds.width - 20.0,
			bounds.y + bounds.height - 20.0,
		),
		Size::new(20.0, 20.0),
	)
}

fn snap(pixels: f32, previous: u32) -> u32 {
	let cells = ((pixels + spatial::SPACING) / spatial::PITCH).max(1.0);
	if (cells - previous as f32).abs() < 0.62 {
		previous
	} else {
		cells.round().max(1.0) as u32
	}
}

impl<Renderer: iced::advanced::Renderer> Tiles<'_, Renderer> {
	fn move_press(&self, press: &mut Press, position: Point) -> Vec<PlacedTile> {
		press.cursor = position;
		press.moved |= position.distance(press.start) > 2.0;
		let delta = position - press.start;
		let Some(original) = press.baseline.get(press.source) else {
			return press.baseline.clone();
		};
		let mut desired = original.clone();
		if press.resize {
			desired.tile.width = snap(
				spatial::span(original.tile.width) + delta.x,
				press.desired.tile.width,
			);
			desired.tile.height = snap(
				spatial::span(original.tile.height) + delta.y,
				press.desired.tile.height,
			);
			let columns = ((self.maximum.width + spatial::SPACING) / spatial::PITCH)
				.floor()
				.max(1.0) as u32;
			let rows = ((self.maximum.height + spatial::SPACING) / spatial::PITCH)
				.floor()
				.max(1.0) as u32;
			desired.tile.width = desired
				.tile
				.width
				.min(columns.saturating_sub(original.column).max(1));
			desired.tile.height = desired
				.tile
				.height
				.min(rows.saturating_sub(original.row).max(1));
		} else {
			desired.column = (original.column as f32 + delta.x / spatial::PITCH)
				.round()
				.max(0.0) as u32;
			desired.row = (original.row as f32 + delta.y / spatial::PITCH)
				.round()
				.max(0.0) as u32;
		}
		let local_y = position.y - press.grid_origin.y;
		let boundary = ((local_y + spatial::SPACING / 2.0) / spatial::PITCH)
			.round()
			.max(0.0) as u32;
		let row_count = ((spatial::extent(&press.baseline, &self.rows).height + spatial::SPACING)
			/ spatial::PITCH)
			.round() as u32;
		let insertion = (!press.resize
			&& press.moved
			&& boundary > 0
			&& boundary <= row_count
			&& (local_y - (boundary as f32 * spatial::PITCH - spatial::SPACING / 2.0)).abs()
				<= spatial::SPACING / 2.0)
			.then_some(boundary);
		loop {
			let result = if let Some(row) = insertion {
				spatial::insert_row_preview(&press.baseline, press.source, desired.clone(), row)
			} else {
				spatial::preview(&press.baseline, press.source, desired.clone())
			};
			let extent = spatial::extent(&result, &[]);
			if extent.width <= self.maximum.width && extent.height <= self.maximum.height {
				press.desired = desired;
				press.insert_row = insertion;
				return result;
			}
			// Fast pointer moves can skip several valid spans. Stop at the largest
			// reachable footprint instead of refusing the entire pointer jump.
			if press.resize
				&& extent.width > self.maximum.width
				&& desired.tile.width > press.desired.tile.width
			{
				desired.tile.width -= 1;
			} else if press.resize && desired.tile.height > press.desired.tile.height {
				desired.tile.height -= 1;
			} else {
				return if let Some(row) = press.insert_row {
					spatial::insert_row_preview(
						&press.baseline,
						press.source,
						press.desired.clone(),
						row,
					)
				} else {
					spatial::preview(&press.baseline, press.source, press.desired.clone())
				};
			}
		}
	}
}

impl<Renderer: iced::advanced::Renderer> Widget<Message, iced::Theme, Renderer>
	for Tiles<'_, Renderer>
{
	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<State>()
	}

	fn state(&self) -> tree::State {
		tree::State::new(State::default())
	}

	fn diff(&mut self, tree: &mut Tree) {
		let state = tree.state.downcast_mut::<State>();
		let committed_order = self
			.baseline
			.iter()
			.zip(&self.targets)
			.all(|(tile, target)| target.as_ref() == Some(tile));
		if state.keys != self.keys
			|| state.baseline != self.baseline
			|| (committed_order && state.targets != self.targets)
		{
			let mut previous = state
				.keys
				.iter()
				.cloned()
				.zip(std::mem::take(&mut tree.children))
				.zip(std::mem::take(&mut state.motion))
				.zip(std::mem::take(&mut state.targets))
				.map(|(((key, tree), motion), target)| (key, target, Some((tree, motion))))
				.collect::<Vec<_>>();
			let mut motions = Vec::new();
			tree.children = self
				.keys
				.iter()
				.zip(&self.targets)
				.map(|(key, tile)| {
					// A committed row sort may reorder identical widget occurrences.
					// Match the resolved preview footprint before occurrence order, so
					// duplicates keep their own subtrees through a move/resize commit.
					let exact = committed_order
						.then(|| {
							previous.iter().position(|(old, target, item)| {
								old.0 == key.0 && tile.is_some() && target == tile && item.is_some()
							})
						})
						.flatten();
					let matched = exact.or_else(|| {
						previous
							.iter()
							.position(|(old, _, item)| old == key && item.is_some())
					});
					if let Some((old, motion)) = matched
						.and_then(|index| previous.get_mut(index))
						.and_then(|(_, _, item)| item.take())
					{
						motions.push(motion);
						old
					} else {
						let bounds = tile.as_ref().map(spatial::bounds).unwrap_or_default();
						motions.push(RectangleMotion::new(
							if state.initialized {
								Rectangle::new(bounds.center(), Size::ZERO)
							} else {
								bounds
							},
							state.now,
						));
						Tree::empty()
					}
				})
				.collect();
			state.motion = motions;
			state.keys.clone_from(&self.keys);
		}
		state.targets.clone_from(&self.targets);
		state.baseline.clone_from(&self.baseline);
		// Opening motion belongs to the popup. Only changes to an existing
		// scene (including catalog entries revealed during editing) animate tiles.
		state.initialized = true;
		tree.diff_children(&mut self.children);
	}

	fn size(&self) -> Size<Length> {
		Size::new(
			Length::Fixed(self.size.width),
			Length::Fixed(self.size.height),
		)
	}

	fn layout(
		&mut self, tree: &mut Tree, renderer: &Renderer, _limits: &layout::Limits,
	) -> layout::Node {
		let state = tree.state.downcast_mut::<State>();
		if !self.ready && !state.paused {
			for motion in &mut state.motion {
				motion.freeze(state.now);
			}
			state.paused = true;
		} else if self.ready && state.paused {
			state.now = Instant::now();
			state.paused = false;
		}
		let nodes = self
			.children
			.iter_mut()
			.zip(&mut tree.children)
			.zip(&self.targets)
			.zip(&mut state.motion)
			.map(|(((child, tree), tile), motion)| {
				let target = tile
					.as_ref()
					.map(spatial::bounds)
					.unwrap_or_else(|| Rectangle::new(motion.target().center(), Size::ZERO));
				if self.ready {
					motion.retarget(target, state.now);
				}
				let bounds = motion.value(state.now);
				let size = Size::new(bounds.width.max(1.0), bounds.height.max(1.0));
				child
					.as_widget_mut()
					.layout(tree, renderer, &layout::Limits::new(size, size))
					.move_to(bounds.position())
			})
			.collect();
		layout::Node::with_children(self.size, nodes)
	}

	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &Renderer, shell: &mut Shell<'_, Message>, viewport: &Rectangle,
	) {
		let state = tree.state.downcast_mut::<State>();
		let redraw = matches!(event, Event::Window(window::Event::RedrawRequested(_)));
		if let Event::Window(window::Event::RedrawRequested(now)) = event {
			let was_active = state.motion.iter().any(|motion| motion.active(state.now));
			if self.ready {
				state.now = *now;
			}
			let active = state.motion.iter().any(|motion| motion.active(*now));
			if self.ready && (was_active || active) {
				shell.invalidate_layout();
			}
			if self.ready && active {
				shell.request_redraw();
			}
		}
		if self.editing && !redraw {
			let point = cursor.position();
			let inside = point
				.is_some_and(|point| viewport.contains(point) && layout.bounds().contains(point));
			match event {
				Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
					if inside && !shell.is_event_captured() =>
				{
					if let Some(point) = point {
						let local = point - Vector::new(layout.bounds().x, layout.bounds().y);
						if let Some((source, target)) = self
							.targets
							.iter()
							.enumerate()
							.filter_map(|(index, tile)| tile.as_ref().map(|tile| (index, tile)))
							.find(|(_, tile)| spatial::bounds(tile).contains(local))
						{
							if source < self.baseline.len() {
								state.press = Some(Press {
									source,
									start: point,
									cursor: point,
									resize: handle(spatial::bounds(target)).contains(local),
									moved: false,
									baseline: self.baseline.to_vec(),
									desired: target.clone(),
									grid_origin: layout.position(),
									insert_row: None,
								});
							}
							shell.capture_event();
						}
					}
				}
				Event::Mouse(mouse::Event::CursorMoved { position }) if state.press.is_some() => {
					if let Some(press) = &mut state.press {
						let preview = self.move_press(press, point.unwrap_or(*position));
						shell.publish(Message::PreviewTiles(Some(preview)));
					}
					shell.capture_event();
					shell.request_redraw();
				}
				Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
					if state.press.is_some() =>
				{
					if let Some(mut press) = state.press.take() {
						let position = point.unwrap_or(press.cursor);
						let preview = self.move_press(&mut press, position);
						state.handoff_drag(&press);
						if press.moved {
							let rows = press.insert_row.map_or_else(
								|| self.rows.clone(),
								|row| spatial::insert_authored_row(&self.rows, row),
							);
							shell.publish(Message::TilesEdited(preview, rows));
						} else if !press.resize {
							shell.publish(Message::ToggleWidget(press.source));
						}
					}
					shell.publish(Message::PreviewTiles(None));
					shell.capture_event();
					shell.request_redraw();
				}
				Event::Keyboard(keyboard::Event::KeyPressed {
					key: keyboard::Key::Named(Named::Escape),
					..
				}) if state.press.is_some() => {
					if let Some(press) = state.press.take() {
						state.handoff_drag(&press);
					}
					shell.publish(Message::PreviewTiles(None));
					shell.capture_event();
					shell.request_redraw();
				}
				Event::Window(window::Event::Unfocused) if state.press.is_some() => {
					if let Some(press) = state.press.take() {
						state.handoff_drag(&press);
					}
					shell.publish(Message::PreviewTiles(None));
					shell.request_redraw();
				}
				Event::Mouse(mouse::Event::CursorLeft) if state.press.is_some() => {
					if let Some(press) = state.press.take() {
						state.handoff_drag(&press);
					}
					shell.publish(Message::PreviewTiles(None));
					shell.request_redraw();
				}
				_ => {}
			}
			return;
		}
		for (index, ((child, tree), child_layout)) in self
			.children
			.iter_mut()
			.zip(&mut tree.children)
			.zip(layout.children())
			.enumerate()
		{
			let shown = self.targets.get(index).is_some_and(Option::is_some);
			if redraw
				|| (shown
					&& !self.editing
					&& self.ready && !state
					.motion
					.get(index)
					.is_some_and(|motion| motion.active(state.now)))
			{
				child.as_widget_mut().update(
					tree,
					event,
					child_layout,
					if self.editing {
						mouse::Cursor::Unavailable
					} else {
						cursor
					},
					renderer,
					shell,
					viewport,
				);
			}
		}
	}

	fn draw(
		&self, tree: &Tree, renderer: &mut Renderer, theme: &iced::Theme, style: &renderer::Style,
		layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		let state = tree.state.downcast_ref::<State>();
		let colors = self.colors;
		let source = state
			.press
			.as_ref()
			.filter(|press| press.moved && !press.resize)
			.map(|press| press.source);
		let origin = Vector::new(layout.bounds().x, layout.bounds().y);
		for index in (0..self.children.len())
			.filter(|index| Some(*index) != source)
			.chain(source)
		{
			let (Some(child), Some(child_tree), Some(child_layout), Some(motion)) = (
				self.children.get(index),
				tree.children.get(index),
				layout.children().nth(index),
				state.motion.get(index),
			) else {
				continue;
			};
			let mut visual = motion.value(state.now) + origin;
			if let Some(dragged) = state
				.press
				.as_ref()
				.filter(|press| press.source == index)
				.and_then(Press::dragged_bounds)
			{
				visual = dragged + origin;
			}
			if visual.width < 0.5 || visual.height < 0.5 {
				continue;
			}
			let scale = (visual.width / child_layout.bounds().width)
				.min(visual.height / child_layout.bounds().height)
				.max(0.001);
			let offset = visual.center()
				- Point::new(
					child_layout.bounds().width * scale / 2.0,
					child_layout.bounds().height * scale / 2.0,
				);
			let transform = Transformation::translate(offset.x, offset.y)
				* Transformation::scale(scale)
				* Transformation::translate(-child_layout.bounds().x, -child_layout.bounds().y);
			let inverse = transform.inverse();
			// The target grid may already be smaller than an exiting tile. The
			// popup's animated visual clip, not target geometry, bounds this frame.
			let clip = *viewport;
			renderer.with_layer(clip, |renderer| {
				renderer.with_transformation(transform, |renderer| {
					child.as_widget().draw(
						child_tree,
						renderer,
						theme,
						style,
						child_layout,
						if self.editing {
							mouse::Cursor::Unavailable
						} else {
							cursor * inverse
						},
						&(clip * inverse),
					)
				})
			});
			if self.editing && index < self.baseline.len() {
				renderer.with_layer(clip, |renderer| {
					let action = Rectangle::new(
						Point::new(visual.x + visual.width - 17.0, visual.y + 4.0),
						Size::new(12.0, 12.0),
					);
					renderer.fill_quad(
						renderer::Quad {
							bounds: action,
							border: Border {
								color: colors.black,
								width: 1.0,
								..Default::default()
							},
							..Default::default()
						},
						colors.surface,
					);
					renderer.fill_quad(
						renderer::Quad {
							bounds: Rectangle::new(
								Point::new(action.x + 3.0, action.y + 5.0),
								Size::new(6.0, 2.0),
							),
							..Default::default()
						},
						colors.text,
					);
					if self.enabled.get(index) == Some(&false) {
						renderer.fill_quad(
							renderer::Quad {
								bounds: Rectangle::new(
									Point::new(action.x + 5.0, action.y + 3.0),
									Size::new(2.0, 6.0),
								),
								..Default::default()
							},
							colors.text,
						);
					}
					let h = handle(visual);
					// Connected diagonal strokes, like a browser's textarea grip.
					for length in [4, 8, 12] {
						for step in 0..length {
							renderer.fill_quad(
								renderer::Quad {
									bounds: Rectangle::new(
										Point::new(
											h.x + 13.0 - length as f32 + step as f32,
											h.y + 13.0 - step as f32,
										),
										Size::new(1.5, 1.5),
									),
									border: Border::default(),
									..Default::default()
								},
								colors.text,
							);
						}
					}
				});
			}
		}
		if let Some(row) = state.press.as_ref().and_then(|press| press.insert_row) {
			renderer.with_layer(*viewport, |renderer| {
				let y = layout.bounds().y + row as f32 * spatial::PITCH - spatial::SPACING;
				renderer.fill_quad(
					renderer::Quad {
						bounds: Rectangle::new(
							Point::new(layout.bounds().x, y),
							Size::new(layout.bounds().width, spatial::SPACING),
						),
						border: Border {
							color: colors.black,
							width: 1.0,
							..Default::default()
						},
						..Default::default()
					},
					colors.decorative.purple90,
				);
				let x = layout.bounds().center_x();
				for bounds in [
					Rectangle::new(Point::new(x - 3.0, y + 3.0), Size::new(6.0, 2.0)),
					Rectangle::new(Point::new(x - 1.0, y + 1.0), Size::new(2.0, 6.0)),
				] {
					renderer.fill_quad(
						renderer::Quad {
							bounds,
							..Default::default()
						},
						colors.on_accent,
					);
				}
			});
		}
	}

	fn mouse_interaction(
		&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
		renderer: &Renderer,
	) -> mouse::Interaction {
		let state = tree.state.downcast_ref::<State>();
		if self.editing {
			if let Some(press) = &state.press {
				return if press.resize {
					mouse::Interaction::ResizingDiagonallyDown
				} else {
					mouse::Interaction::Grabbing
				};
			}
			if let Some(point) = cursor.position().filter(|point| viewport.contains(*point)) {
				let local = point - Vector::new(layout.bounds().x, layout.bounds().y);
				if let Some((index, tile)) = self
					.targets
					.iter()
					.enumerate()
					.filter_map(|(index, tile)| tile.as_ref().map(|tile| (index, tile)))
					.find(|(_, tile)| spatial::bounds(tile).contains(local))
				{
					if index >= self.baseline.len() {
						return mouse::Interaction::Pointer;
					}
					return if handle(spatial::bounds(tile)).contains(local) {
						mouse::Interaction::ResizingDiagonallyDown
					} else {
						mouse::Interaction::Grab
					};
				}
			}
			return mouse::Interaction::None;
		}
		self.children
			.iter()
			.zip(&tree.children)
			.zip(layout.children())
			.enumerate()
			.filter(|(index, _)| self.targets.get(*index).is_some_and(Option::is_some))
			.map(|(_, ((child, tree), layout))| {
				child
					.as_widget()
					.mouse_interaction(tree, layout, cursor, viewport, renderer)
			})
			.max()
			.unwrap_or_default()
	}

	fn operate(
		&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer,
		operation: &mut dyn iced::advanced::widget::Operation,
	) {
		if self.editing {
			return;
		}
		for (index, ((child, tree), layout)) in self
			.children
			.iter_mut()
			.zip(&mut tree.children)
			.zip(layout.children())
			.enumerate()
		{
			if self.targets.get(index).is_some_and(Option::is_some) {
				child
					.as_widget_mut()
					.operate(tree, layout, renderer, operation);
			}
		}
	}

	fn overlay<'a>(
		&'a mut self, tree: &'a mut Tree, layout: Layout<'a>, renderer: &Renderer,
		viewport: &Rectangle, offset: Vector,
	) -> Vec<overlay::Element<'a, Message, iced::Theme, Renderer>> {
		if self.editing {
			return Vec::new();
		}
		self.children
			.iter_mut()
			.zip(&mut tree.children)
			.zip(layout.children())
			.enumerate()
			.filter(|(index, _)| self.targets.get(*index).is_some_and(Option::is_some))
			.flat_map(|(_, ((child, tree), layout))| {
				child
					.as_widget_mut()
					.overlay(tree, layout, renderer, viewport, offset)
			})
			.collect()
	}
}

#[cfg(test)]
mod interaction_tests;

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn snap_hysteresis_keeps_threshold_stable() {
		assert_eq!(snap(spatial::span(2) + spatial::PITCH * 0.55, 2), 2);
		assert_eq!(snap(spatial::span(2) + spatial::PITCH * 0.7, 2), 3);
		assert_eq!(snap(-100.0, 1), 1);
	}
}
