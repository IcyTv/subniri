//! Exercise the actual retained grid with Iced's null renderer, without a compositor/GPU.
use super::*;
use config::{
	MenuEntry, MenuTile,
	SystemMenuWidgets::{Bluetooth, Wifi},
};
use iced::advanced::shell::{Bus, Waker};
use std::{cell::RefCell, rc::Rc};

type Events = Rc<RefCell<Vec<Event>>>;
struct Probe(Events);
impl Widget<Message, iced::Theme, ()> for Probe {
	fn size(&self) -> Size<Length> {
		Size::new(Length::Fill, Length::Fill)
	}
	fn layout(&mut self, _: &mut Tree, _: &(), limits: &layout::Limits) -> layout::Node {
		layout::Node::new(limits.max())
	}
	fn update(
		&mut self, _: &mut Tree, event: &Event, _: Layout<'_>, _: mouse::Cursor, _: &(),
		_: &mut Shell<'_, Message>, _: &Rectangle,
	) {
		self.0.borrow_mut().push(event.clone());
	}
	fn draw(
		&self, _: &Tree, _: &mut (), _: &iced::Theme, _: &renderer::Style, _: Layout<'_>,
		_: mouse::Cursor, _: &Rectangle,
	) {
	}
}

struct Harness {
	widget: Tiles<'static, ()>,
	tree: Tree,
	node: layout::Node,
	viewport: Rectangle,
	events: Events,
}
impl Harness {
	fn new() -> Self {
		let events = Events::default();
		let rows = vec![
			MenuRow(vec![MenuEntry::Widget(MenuTile::new(Wifi))]),
			MenuRow(vec![MenuEntry::Widget(MenuTile::new(Bluetooth))]),
		];
		let baseline = config::resolve_rows(&rows).tiles;
		let keys = vec![Key(Wifi, 0), Key(Bluetooth, 0)];
		let targets = baseline.iter().cloned().map(Some).collect();
		let mut widget = Tiles {
			editing: true,
			rows,
			baseline,
			keys,
			targets,
			children: vec![
				Element::new(Probe(events.clone())),
				Element::new(Probe(events.clone())),
			],
			size: Size::new(600.0, 500.0),
			maximum: Size::new(600.0, 500.0),
			ready: true,
			enabled: vec![true; 2],
		};
		let mut tree = Tree::new(&widget as &dyn Widget<_, _, _>);
		tree.diff(&mut widget as &mut dyn Widget<_, _, _>);
		let node = widget.layout(
			&mut tree,
			&(),
			&layout::Limits::new(Size::ZERO, widget.size),
		);
		Self {
			widget,
			tree,
			node,
			viewport: Rectangle::with_size(Size::new(600.0, 500.0)),
			events,
		}
	}
	fn dispatch(&mut self, event: Event, point: Point) -> Vec<Message> {
		let mut bus = Bus::new();
		let mut shell = Shell::new(&window::Headless, Waker::noop(), &mut bus);
		self.widget.update(
			&mut self.tree,
			&event,
			Layout::new(&self.node),
			mouse::Cursor::Available(point),
			&(),
			&mut shell,
			&self.viewport,
		);
		drop(shell);
		bus.into_iter().collect()
	}
	fn press(&mut self, point: Point) {
		self.dispatch(
			Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
			point,
		);
	}
	fn rest(&mut self) {
		let state = self.tree.state.downcast_mut::<State>();
		for motion in &mut state.motion {
			*motion = RectangleMotion::new(motion.target(), state.now);
		}
	}
	fn move_to(&mut self, point: Point) -> Vec<Message> {
		self.dispatch(
			Event::Mouse(mouse::Event::CursorMoved { position: point }),
			point,
		)
	}
	fn release(&mut self, point: Point) -> Vec<Message> {
		self.dispatch(
			Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
			point,
		)
	}
}

#[test]
fn opening_tiles_start_at_rest_even_while_waiting_for_popup_capacity() {
	for editing in [false, true] {
		for ready in [false, true] {
			let mut harness = Harness::new();
			harness.widget.editing = editing;
			harness.widget.ready = ready;
			harness.tree = Tree::new(&harness.widget as &dyn Widget<_, _, _>);
			harness
				.tree
				.diff(&mut harness.widget as &mut dyn Widget<_, _, _>);
			harness.node = harness.widget.layout(
				&mut harness.tree,
				&(),
				&layout::Limits::new(Size::ZERO, harness.widget.size),
			);
			let state = harness.tree.state.downcast_ref::<State>();
			assert_eq!(state.motion.len(), harness.widget.baseline.len());
			for (motion, tile) in state.motion.iter().zip(&harness.widget.baseline) {
				assert_eq!(
					motion.value(state.now),
					spatial::bounds(tile),
					"opening tile must not grow from zero"
				);
				assert!(!motion.active(state.now));
			}
		}
	}
}

#[test]
fn subsequent_tile_layout_changes_still_animate() {
	let mut harness = Harness::new();
	harness.rest();
	let before = spatial::bounds(&harness.widget.baseline[0]);
	harness.widget.baseline[0].column += 1;
	harness.widget.targets = harness.widget.baseline.iter().cloned().map(Some).collect();
	harness
		.tree
		.diff(&mut harness.widget as &mut dyn Widget<_, _, _>);
	harness.node = harness.widget.layout(
		&mut harness.tree,
		&(),
		&layout::Limits::new(Size::ZERO, harness.widget.size),
	);
	let state = harness.tree.state.downcast_ref::<State>();
	assert_eq!(state.motion[0].value(state.now), before);
	assert_eq!(
		state.motion[0].target(),
		spatial::bounds(&harness.widget.baseline[0])
	);
	assert!(state.motion[0].active(state.now));
}

#[test]
fn nightlight_can_grow_tall_when_collision_chain_needs_more_than_reserved_gutter() {
	let mut harness = Harness::new();
	harness.widget.baseline = vec![
		config::PlacedTile {
			tile: MenuTile::new(config::SystemMenuWidgets::Nightlight),
			row: 0,
			column: 0,
		},
		config::PlacedTile {
			tile: MenuTile::new(Wifi),
			row: 1,
			column: 0,
		},
		config::PlacedTile {
			tile: MenuTile::new(Bluetooth),
			row: 1,
			column: 2,
		},
		config::PlacedTile {
			tile: MenuTile::new(Wifi),
			row: 1,
			column: 4,
		},
	];
	let original = harness.widget.baseline[0].clone();
	let mut press = Press {
		source: 0,
		start: Point::ORIGIN,
		cursor: Point::ORIGIN,
		resize: true,
		moved: false,
		baseline: harness.widget.baseline.clone(),
		desired: original,
		grid_origin: Point::ORIGIN,
		insert_row: None,
	};
	// The physical output has room, even though the initial edit envelope has
	// only 128px of slack. Exercise the production maximum supplied to the grid.
	let mut config = config::ConfigFile::default();
	config.system_menu =
		config::SystemMenu::from_widgets(vec![config::SystemMenuWidgets::Nightlight]);
	let mut menu = crate::modules::system_menu::SystemMenu::new(&config);
	menu.popup_opened(window::Id::unique(), Size::new(1000.0, 800.0));
	let _ = menu.update(Message::EditingChanged(true));
	harness.widget.maximum = menu.editor_maximum();
	let result = harness.widget.move_press(&mut press, Point::new(0.0, 72.0));
	assert_eq!(result[0].tile.height, 2);
}

#[test]
fn gutter_release_commits_but_outer_departure_cancels() {
	let mut harness = Harness::new();
	harness.rest();
	harness.press(Point::new(20.0, 20.0));
	harness.move_to(Point::new(300.0, 20.0));
	assert!(
		harness
			.release(Point::new(300.0, 20.0))
			.iter()
			.any(|message| matches!(message, Message::TilesEdited(..)))
	);
	harness.press(Point::new(20.0, 20.0));
	harness.move_to(Point::new(300.0, 20.0));
	let messages = harness.dispatch(
		Event::Mouse(mouse::Event::CursorLeft),
		Point::new(700.0, 20.0),
	);
	assert!(
		messages
			.iter()
			.any(|message| matches!(message, Message::PreviewTiles(None)))
	);
	assert!(harness.tree.state.downcast_ref::<State>().press.is_none());
	assert!(
		!harness
			.release(Point::new(300.0, 20.0))
			.iter()
			.any(|message| matches!(message, Message::TilesEdited(..)))
	);
}

#[test]
fn between_row_drop_inserts_once_and_retreat_restores_baseline()
-> Result<(), Box<dyn std::error::Error>> {
	let mut harness = Harness::new();
	harness.rest();
	harness.press(Point::new(20.0, 20.0));
	harness.move_to(Point::new(180.0, 68.0));
	assert_eq!(
		harness
			.tree
			.state
			.downcast_ref::<State>()
			.press
			.as_ref()
			.and_then(|press| press.insert_row),
		Some(1)
	);
	let messages = harness.release(Point::new(180.0, 68.0));
	let (tiles, rows) = messages
		.iter()
		.find_map(|message| {
			if let Message::TilesEdited(tiles, rows) = message {
				Some((tiles, rows))
			} else {
				None
			}
		})
		.ok_or("no insertion commit")?;
	assert_eq!(rows.len(), 3);
	assert_eq!(tiles[0].row, 1);
	assert_eq!(tiles[1].row, 2);
	Ok(())
}

#[test]
fn released_drag_settles_from_pointer_position_instead_of_jumping_to_grid()
-> Result<(), Box<dyn std::error::Error>> {
	let mut harness = Harness::new();
	harness.rest();
	let initial = spatial::bounds(harness.widget.baseline.first().ok_or("missing tile")?);
	harness.press(Point::new(20.0, 20.0));
	harness.move_to(Point::new(59.0, 20.0));
	let messages = harness.release(Point::new(59.0, 20.0));
	let rows = messages
		.iter()
		.find_map(|message| {
			if let Message::TilesEdited(tiles, rows) = message {
				Some(spatial::commit(tiles, rows))
			} else {
				None
			}
		})
		.ok_or("missing commit")?;
	let pointer_bounds = initial + Vector::new(39.0, 0.0);
	let state = harness.tree.state.downcast_ref::<State>();
	assert_eq!(
		state
			.motion
			.first()
			.ok_or("missing motion")?
			.value(state.now),
		pointer_bounds
	);
	harness.widget.rows = rows;
	harness.widget.baseline = config::resolve_rows(&harness.widget.rows).tiles;
	harness.widget.targets = harness.widget.baseline.iter().cloned().map(Some).collect();
	harness
		.tree
		.diff(&mut harness.widget as &mut dyn Widget<_, _, _>);
	harness.node = harness.widget.layout(
		&mut harness.tree,
		&(),
		&layout::Limits::new(Size::ZERO, harness.widget.size),
	);
	let state = harness.tree.state.downcast_ref::<State>();
	let motion = state.motion.first().ok_or("missing motion")?;
	assert_eq!(motion.value(state.now), pointer_bounds);
	let snapped = spatial::bounds(
		harness
			.widget
			.baseline
			.first()
			.ok_or("missing committed tile")?,
	);
	assert_ne!(pointer_bounds, snapped);
	assert_eq!(
		motion.value(state.now + neo_widgets::animation::geometry::LAYOUT_DURATION),
		snapped
	);
	Ok(())
}

#[test]
fn cancelled_drag_settles_from_pointer_position_without_saving()
-> Result<(), Box<dyn std::error::Error>> {
	for escape in [false, true] {
		let mut harness = Harness::new();
		harness.rest();
		let original = spatial::bounds(harness.widget.baseline.first().ok_or("missing tile")?);
		harness.press(Point::new(20.0, 20.0));
		harness.move_to(Point::new(59.0, 20.0));
		let event = if escape {
			Event::Keyboard(keyboard::Event::KeyPressed {
				key: keyboard::Key::Named(Named::Escape),
				modified_key: keyboard::Key::Named(Named::Escape),
				physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
				location: keyboard::Location::Standard,
				modifiers: keyboard::Modifiers::default(),
				text: None,
				repeat: false,
			})
		} else {
			Event::Window(window::Event::Unfocused)
		};
		let messages = harness.dispatch(event, Point::new(59.0, 20.0));
		assert!(
			messages
				.iter()
				.all(|message| !matches!(message, Message::TilesEdited(..)))
		);
		harness.node = harness.widget.layout(
			&mut harness.tree,
			&(),
			&layout::Limits::new(Size::ZERO, harness.widget.size),
		);
		let state = harness.tree.state.downcast_ref::<State>();
		let motion = state.motion.first().ok_or("missing motion")?;
		assert_eq!(motion.value(state.now), original + Vector::new(39.0, 0.0));
		assert_eq!(
			motion.value(state.now + neo_widgets::animation::geometry::LAYOUT_DURATION),
			original
		);
		assert!(harness.release(Point::new(59.0, 20.0)).is_empty());
	}
	Ok(())
}

#[test]
fn corner_resize_previews_both_dimensions_and_only_commits_on_release()
-> Result<(), Box<dyn std::error::Error>> {
	let mut harness = Harness::new();
	harness.press(Point::new(130.0, 58.0));
	let messages = harness.move_to(Point::new(202.0, 130.0));
	let preview = messages
		.iter()
		.find_map(|message| {
			if let Message::PreviewTiles(Some(tiles)) = message {
				Some(tiles)
			} else {
				None
			}
		})
		.ok_or("missing preview")?;
	assert_eq!(
		preview
			.first()
			.map(|tile| (tile.tile.width, tile.tile.height)),
		Some((3, 2))
	);
	assert_eq!(preview.get(1).map(|tile| tile.column), Some(3));
	assert!(
		messages
			.iter()
			.all(|message| !matches!(message, Message::TilesEdited(..)))
	);
	let committed = harness.release(Point::new(202.0, 130.0));
	let rows = committed
		.iter()
		.find_map(|message| {
			if let Message::TilesEdited(tiles, rows) = message {
				Some(spatial::commit(tiles, rows))
			} else {
				None
			}
		})
		.ok_or("missing commit")?;
	assert_eq!(config::resolve_rows(&rows).tiles, *preview);
	assert!(
		harness.events.borrow().is_empty(),
		"edit input must not reach normal controls"
	);
	Ok(())
}

#[test]
fn retreat_restores_neighbors_and_unfocus_cancels_without_commit() {
	let mut harness = Harness::new();
	harness.press(Point::new(130.0, 58.0));
	harness.move_to(Point::new(202.0, 130.0));
	let messages = harness.move_to(Point::new(130.0, 58.0));
	assert!(messages.iter().any(|message| matches!(message, Message::PreviewTiles(Some(tiles)) if *tiles == harness.widget.baseline)));
	let cancelled = harness.dispatch(Event::Window(window::Event::Unfocused), Point::ORIGIN);
	assert!(
		cancelled
			.iter()
			.any(|message| matches!(message, Message::PreviewTiles(None)))
	);
	assert!(harness.release(Point::new(130.0, 58.0)).is_empty());
}

#[test]
fn escape_cancels_resize_without_saving_or_late_release_commit() {
	let mut harness = Harness::new();
	harness.press(Point::new(130.0, 58.0));
	harness.move_to(Point::new(202.0, 130.0));
	let cancelled = harness.dispatch(
		Event::Keyboard(keyboard::Event::KeyPressed {
			key: keyboard::Key::Named(Named::Escape),
			modified_key: keyboard::Key::Named(Named::Escape),
			physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
			location: keyboard::Location::Standard,
			modifiers: keyboard::Modifiers::default(),
			text: None,
			repeat: false,
		}),
		Point::new(202.0, 130.0),
	);
	assert!(
		cancelled
			.iter()
			.any(|message| matches!(message, Message::PreviewTiles(None)))
	);
	assert!(
		cancelled
			.iter()
			.all(|message| !matches!(message, Message::TilesEdited(..)))
	);
	assert!(harness.release(Point::new(202.0, 130.0)).is_empty());
}

#[test]
fn viewport_clipping_and_resize_limits_are_enforced() {
	let mut harness = Harness::new();
	harness.viewport = Rectangle::with_size(Size::new(100.0, 40.0));
	harness.press(Point::new(130.0, 58.0));
	assert!(harness.tree.state.downcast_ref::<State>().press.is_none());
	harness.viewport = Rectangle::with_size(Size::new(600.0, 500.0));
	harness.press(Point::new(130.0, 58.0));
	let messages = harness.move_to(Point::new(5000.0, 5000.0));
	assert!(messages.iter().any(|message| matches!(message, Message::PreviewTiles(Some(tiles)) if spatial::extent(tiles, &[]).width <= 600.0 && spatial::extent(tiles, &[]).height <= 500.0)));
}

#[test]
fn mode_changes_keep_keyed_child_state_and_redraws_reach_children() {
	let mut harness = Harness::new();
	harness.dispatch(
		Event::Window(window::Event::RedrawRequested(Instant::now())),
		Point::ORIGIN,
	);
	assert_eq!(harness.events.borrow().len(), 2);
	let before = harness.tree.children.len();
	harness.widget.editing = false;
	harness
		.tree
		.diff(&mut harness.widget as &mut dyn Widget<_, _, _>);
	assert_eq!(harness.tree.children.len(), before);
	harness.widget.editing = true;
	harness
		.tree
		.diff(&mut harness.widget as &mut dyn Widget<_, _, _>);
	assert_eq!(harness.tree.children.len(), before);
}

#[test]
fn duplicate_occurrences_keep_their_subtrees_when_preview_order_is_committed()
-> Result<(), Box<dyn std::error::Error>> {
	let mut harness = Harness::new();
	for tile in &mut harness.widget.baseline {
		tile.tile.widget = Wifi;
	}
	harness.widget.keys = vec![Key(Wifi, 0), Key(Wifi, 1)];
	harness.widget.targets = harness.widget.baseline.iter().cloned().map(Some).collect();
	harness
		.tree
		.diff(&mut harness.widget as &mut dyn Widget<_, _, _>);
	harness
		.tree
		.children
		.first_mut()
		.ok_or("missing first child")?
		.state = tree::State::new(11_u32);
	harness
		.tree
		.children
		.get_mut(1)
		.ok_or("missing second child")?
		.state = tree::State::new(22_u32);
	let mut preview = harness.widget.baseline.clone();
	preview.first_mut().ok_or("missing tile")?.row = 1;
	preview.get_mut(1).ok_or("missing tile")?.row = 0;
	harness.widget.targets = preview.iter().cloned().map(Some).collect();
	harness
		.tree
		.diff(&mut harness.widget as &mut dyn Widget<_, _, _>);
	preview.sort_by_key(|tile| (tile.row, tile.column));
	harness.widget.baseline.clone_from(&preview);
	harness.widget.targets = preview.into_iter().map(Some).collect();
	harness
		.tree
		.diff(&mut harness.widget as &mut dyn Widget<_, _, _>);
	assert_eq!(
		*harness
			.tree
			.children
			.first()
			.ok_or("missing first child")?
			.state
			.downcast_ref::<u32>(),
		22
	);
	assert_eq!(
		*harness
			.tree
			.children
			.get(1)
			.ok_or("missing second child")?
			.state
			.downcast_ref::<u32>(),
		11
	);
	Ok(())
}
