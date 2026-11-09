//! A narrow keyboard gate: inspect real widget focus before dispatch, rather than
//! guessing from the last mouse click or consuming keystrokes after a button acts.
use crate::{Message, browser::typeahead};
use crate::{browser::Message as BrowserMessage, context_menu::Message as ContextMenuMessage};
use iced::{
	Element, Event, Length, Rectangle, Size, Vector,
	advanced::{
		Layout, Renderer, Shell, Widget, layout, mouse, overlay, renderer,
		widget::{
			Id, Operation, Tree,
			operation::{Focusable, TextInput},
		},
	},
};

pub(crate) fn gate<'a>(
	content: Element<'a, Message>, active: bool, enabled: bool,
	entries: Vec<(Id, std::path::PathBuf)>,
) -> Element<'a, Message> {
	Element::new(Gate {
		content,
		active,
		enabled,
		menu: None,
		forward_keys: false,
		blocked: false,
		entries,
	})
}

pub(crate) fn menu_gate<'a>(
	content: Element<'a, Message>, id: iced::window::Id,
) -> Element<'a, Message> {
	Element::new(Gate {
		content,
		active: false,
		enabled: false,
		menu: Some(id),
		forward_keys: false,
		blocked: false,
		entries: Vec::new(),
	})
}

pub(crate) fn host_gate<'a, R: Renderer + 'a>(
	content: Element<'a, Message, iced::Theme, R>, popup: Option<iced::window::Id>,
) -> Element<'a, Message, iced::Theme, R> {
	Element::new(Gate {
		content,
		active: false,
		enabled: false,
		menu: popup,
		forward_keys: popup.is_some(),
		blocked: false,
		entries: Vec::new(),
	})
}

/// Always retain this wrapper in the host tree, preserving scroll/widget state.
pub(crate) fn modal_gate<'a, R: Renderer + 'a>(
	content: Element<'a, Message, iced::Theme, R>, blocked: bool,
) -> Element<'a, Message, iced::Theme, R> {
	Element::new(Gate {
		content,
		active: false,
		enabled: false,
		menu: None,
		forward_keys: false,
		blocked,
		entries: Vec::new(),
	})
}

struct Gate<'a, R = iced::Renderer> {
	content: Element<'a, Message, iced::Theme, R>,
	active: bool,
	enabled: bool,
	menu: Option<iced::window::Id>,
	forward_keys: bool,
	blocked: bool,
	entries: Vec<(Id, std::path::PathBuf)>,
}

#[derive(Default)]
struct EditorFocus {
	editors: Vec<Id>,
	focused: Vec<Id>,
	bounds: Vec<(Id, Rectangle)>,
}
impl EditorFocus {
	fn is_editing(&self) -> bool {
		self.editors.iter().any(|id| self.focused.contains(id))
	}
}
impl Operation for EditorFocus {
	fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
		operate(self);
	}
	fn text_input(&mut self, id: Option<&Id>, _bounds: Rectangle, _state: &mut dyn TextInput) {
		if let Some(id) = id {
			self.editors.push(id.clone());
		}
	}
	fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
		if let Some(id) = id {
			self.bounds.push((id.clone(), bounds));
		}
		if state.is_focused()
			&& let Some(id) = id
		{
			self.focused.push(id.clone());
		}
	}
}

impl<R: Renderer> Widget<Message, iced::Theme, R> for Gate<'_, R> {
	fn size(&self) -> Size<Length> {
		self.content.as_widget().size()
	}
	fn diff(&mut self, tree: &mut Tree) {
		tree.diff_children(std::slice::from_mut(&mut self.content));
	}
	fn layout(&mut self, tree: &mut Tree, renderer: &R, limits: &layout::Limits) -> layout::Node {
		let Some(child) = tree.children.first_mut() else {
			return layout::Node::new(Size::ZERO);
		};
		self.content.as_widget_mut().layout(child, renderer, limits)
	}
	fn operate(
		&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &R, operation: &mut dyn Operation,
	) {
		if let Some(child) = tree.children.first_mut() {
			self.content
				.as_widget_mut()
				.operate(child, layout, renderer, operation);
		}
	}
	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &R, shell: &mut Shell<'_, Message>, viewport: &Rectangle,
	) {
		let Some(child) = tree.children.first_mut() else {
			return;
		};
		if self.blocked
			&& matches!(
				event,
				Event::Keyboard(_)
					| Event::Mouse(_)
					| Event::Touch(_)
					| Event::InputMethod(_)
					| Event::Clipboard(_)
			) {
			// Clear captures/pressed states without rebuilding the host tree or moving
			// its viewport. The application router independently rejects any queued
			// interaction messages, including subscription observations of this event.
			self.content.as_widget_mut().update(
				child,
				&Event::Window(iced::window::Event::Unfocused),
				layout,
				mouse::Cursor::Unavailable,
				renderer,
				shell,
				viewport,
			);
			self.content.as_widget_mut().update(
				child,
				&Event::Mouse(mouse::Event::CursorLeft),
				layout,
				mouse::Cursor::Unavailable,
				renderer,
				shell,
				viewport,
			);
			shell.capture_event();
			return;
		}
		if self.forward_keys
			&& let (Some(id), Event::Keyboard(key)) = (self.menu, event)
		{
			if let iced::keyboard::Event::KeyPressed {
				text, modifiers, ..
			} = key
			{
				if !modifiers.control()
					&& !modifiers.alt()
					&& !modifiers.logo()
					&& let Some(text) = text
						.as_ref()
						.filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
				{
					shell.publish(ContextMenuMessage::MenuText(id, text.to_string()).into());
				} else {
					shell.publish(ContextMenuMessage::MenuKey(id, key.clone(), None).into());
				}
			}
			shell.capture_event();
			return;
		}
		if !shell.is_event_captured()
			&& let Event::Keyboard(key) = event
		{
			let mut focus = EditorFocus::default();
			self.content
				.as_widget_mut()
				.operate(child, layout, renderer, &mut focus);
			if self.enabled
				&& !self.entries.is_empty()
				&& let iced::keyboard::Event::KeyPressed {
					key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab),
					modifiers,
					..
				} = key
			{
				let file_stop = self
					.entries
					.iter()
					.find(|(id, _)| focus.focused.contains(id))
					.or_else(|| self.entries.first())
					.map(|(id, _)| id);
				let mut stops = Vec::new();
				for (id, _) in &focus.bounds {
					if !stops.contains(id)
						&& (!self.entries.iter().any(|(entry, _)| entry == id)
							|| Some(id) == file_stop)
					{
						stops.push(id.clone());
					}
				}
				if !stops.is_empty() {
					let current = stops.iter().position(|id| focus.focused.contains(id));
					let next = match (current, modifiers.shift()) {
						(Some(i), true) => (i + stops.len() - 1) % stops.len(),
						(Some(i), false) => (i + 1) % stops.len(),
						(None, true) => stops.len() - 1,
						(None, false) => 0,
					};
					if let Some(id) = stops.get(next) {
						if self.active {
							shell.publish(BrowserMessage::TypeAhead(typeahead::Edit::Clear).into());
						}
						self.content.as_widget_mut().operate(
							child,
							layout,
							renderer,
							&mut iced::advanced::widget::operation::focusable::focus::<()>(
								id.clone(),
							),
						);
						shell.request_redraw();
						shell.capture_event();
						return;
					}
				}
			}
			if let Some(id) = self.menu {
				if let iced::keyboard::Event::KeyPressed {
					key,
					text,
					modifiers,
					..
				} = key
				{
					use iced::keyboard::{Key, key::Named};
					if matches!(
						key.as_ref(),
						Key::Named(
							Named::Escape
								| Named::Enter | Named::Tab
								| Named::ArrowDown | Named::ArrowUp
								| Named::ArrowRight | Named::ArrowLeft
						)
					) && !(focus.is_editing()
						&& matches!(
							key.as_ref(),
							Key::Named(Named::ArrowRight | Named::ArrowLeft)
						)) {
						shell.publish(
							ContextMenuMessage::MenuKey(
								id,
								key_event(event),
								focus.focused.first().cloned(),
							)
							.into(),
						);
						shell.capture_event();
						return;
					}
					if !focus.is_editing()
						&& !modifiers.control()
						&& !modifiers.alt()
						&& !modifiers.logo()
						&& let Some(text) = text
							.as_ref()
							.filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
					{
						shell.publish(ContextMenuMessage::MenuText(id, text.to_string()).into());
						shell.capture_event();
						return;
					}
				}
			}
			if let Some(edit) = typeahead::key_edit(key, self.active, focus.is_editing())
				&& (self.enabled || edit == typeahead::Edit::Clear)
			{
				shell.publish(BrowserMessage::TypeAhead(edit).into());
				shell.capture_event();
				return;
			}
			if self.enabled
				&& !focus.is_editing()
				&& self.menu.is_none()
				&& let iced::keyboard::Event::KeyPressed { key, modifiers, .. } = key
				&& !modifiers.alt()
				&& !modifiers.logo()
			{
				use iced::keyboard::{Key, key::Named};
				let focused = self
					.entries
					.iter()
					.find(|(id, _)| focus.focused.contains(id))
					.map(|(_, p)| p.clone());
				// An unfocused file pane must not steal navigation/activation from toolbar controls.
				if focused.is_some() || focus.focused.is_empty() {
					let geometry = self
						.entries
						.iter()
						.filter_map(|(id, path)| {
							focus
								.bounds
								.iter()
								.find(|(i, _)| i == id)
								.map(|(_, b)| (path.clone(), *b))
						})
						.collect::<Vec<_>>();
					if let Some(path) = crate::browser::view::selection::destination(
						&geometry,
						focused.as_ref(),
						key,
					) {
						shell.publish(BrowserMessage::SelectionMove(path, *modifiers).into());
						shell.capture_event();
						return;
					}
					if matches!(
						key.as_ref(),
						Key::Named(Named::Enter | Named::Space | Named::ContextMenu)
					) || (key.as_ref() == Key::Named(Named::F10) && modifiers.shift())
						|| (matches!(key.as_ref(), Key::Character("a" | "A"))
							&& modifiers.control())
					{
						shell.publish(
							BrowserMessage::SelectionKey(key.clone(), *modifiers, focused).into(),
						);
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
		renderer: &R,
	) -> mouse::Interaction {
		tree.children
			.first()
			.map_or(mouse::Interaction::None, |child| {
				self.content
					.as_widget()
					.mouse_interaction(child, layout, cursor, viewport, renderer)
			})
	}
	fn draw(
		&self, tree: &Tree, renderer: &mut R, theme: &iced::Theme, style: &renderer::Style,
		layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		if let Some(child) = tree.children.first() {
			self.content
				.as_widget()
				.draw(child, renderer, theme, style, layout, cursor, viewport);
		}
	}
	fn overlay<'b>(
		&'b mut self, tree: &'b mut Tree, layout: Layout<'b>, renderer: &R, viewport: &Rectangle,
		translation: Vector,
	) -> Vec<overlay::Element<'b, Message, iced::Theme, R>> {
		tree.children.first_mut().map_or_else(Vec::new, |child| {
			self.content
				.as_widget_mut()
				.overlay(child, layout, renderer, viewport, translation)
		})
	}
}

fn key_event(event: &Event) -> iced::keyboard::Event {
	match event {
		Event::Keyboard(key) => key.clone(),
		_ => unreachable!("called only for keyboard events"),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use iced::{
		advanced::{
			shell::{Bus, Waker},
			widget::operation,
		},
		keyboard,
		widget::{Space, text_input},
		window,
	};

	#[test]
	fn menu_open_and_action_close_preserve_host_scroll() {
		use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};
		struct ReadOffset(f32);
		impl Operation for ReadOffset {
			fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
				operate(self);
			}
			fn scrollable(
				&mut self, _: Option<&Id>, _: Rectangle, _: Rectangle, translation: Vector,
				_: &mut dyn Scrollable,
			) {
				self.0 = translation.y;
			}
		}
		let build = |popup| {
			host_gate::<()>(
				iced::widget::scrollable(Space::new().height(2000))
					.id("files")
					.height(200)
					.into(),
				popup,
			)
		};
		let mut content = build(None);
		let mut tree = Tree::new(content.as_widget());
		content.as_widget_mut().diff(&mut tree);
		let limits = layout::Limits::new(Size::ZERO, Size::new(300.0, 200.0));
		let node = content.as_widget_mut().layout(&mut tree, &(), &limits);
		content.as_widget_mut().operate(
			&mut tree,
			Layout::new(&node),
			&(),
			&mut operation::scrollable::scroll_to::<()>(
				Id::from("files"),
				AbsoluteOffset {
					x: None,
					y: Some(600.0),
				},
			),
		);
		for popup in [Some(window::Id::unique()), None] {
			content = build(popup);
			tree.diff(content.as_widget_mut());
			content.as_widget_mut().diff(&mut tree);
			let node = content.as_widget_mut().layout(&mut tree, &(), &limits);
			let mut offset = ReadOffset(-1.0);
			content
				.as_widget_mut()
				.operate(&mut tree, Layout::new(&node), &(), &mut offset);
			assert_eq!(
				offset.0, 600.0,
				"popup transition {popup:?} reset file-area scroll"
			);
		}
	}

	fn dispatch(
		content: Element<'_, Message, iced::Theme, ()>, id: &str, active: bool,
		event: keyboard::Event, menu: Option<window::Id>,
	) -> Vec<Message> {
		dispatch_files(content, id, active, event, menu, Vec::new())
	}

	fn dispatch_files(
		content: Element<'_, Message, iced::Theme, ()>, id: &str, active: bool,
		event: keyboard::Event, menu: Option<window::Id>, entries: Vec<(Id, std::path::PathBuf)>,
	) -> Vec<Message> {
		let mut gate = Gate {
			content,
			active,
			enabled: true,
			menu,
			forward_keys: false,
			blocked: false,
			entries,
		};
		let mut tree = Tree::new(&gate as &dyn Widget<Message, iced::Theme, ()>);
		gate.diff(&mut tree);
		let node = gate.layout(
			&mut tree,
			&(),
			&layout::Limits::new(Size::ZERO, Size::new(300.0, 100.0)),
		);
		gate.operate(
			&mut tree,
			Layout::new(&node),
			&(),
			&mut operation::focusable::focus::<()>(Id::from(id.to_owned())),
		);
		let mut bus = Bus::new();
		gate.update(
			&mut tree,
			&Event::Keyboard(event),
			Layout::new(&node),
			mouse::Cursor::Unavailable,
			&(),
			&mut Shell::new(&window::Headless, Waker::noop(), &mut bus),
			&Rectangle::with_size(Size::new(300.0, 100.0)),
		);
		bus.into_iter().collect()
	}
	#[test]
	fn focused_search_receives_text_without_starting_typeahead() {
		let input = text_input::<Message, iced::Theme>("Search", "")
			.id("search")
			.on_input(|text| BrowserMessage::Search(text).into());
		let key = typeahead::tests::key(
			keyboard::Key::Character("f".into()),
			Some("f"),
			keyboard::Modifiers::default(),
		);
		let messages = dispatch(input.into(), "search", false, key, None);
		// The null renderer's text editor does not retain text; the callback still
		// proves the key reached the real TextInput rather than the keyboard gate.
		assert!(
			messages
				.iter()
				.any(|m| matches!(m, Message::Browser(BrowserMessage::Search(_)))),
			"{messages:?}"
		);
		assert!(
			!messages
				.iter()
				.any(|m| matches!(m, Message::Browser(BrowserMessage::TypeAhead(_))))
		);
	}

	#[test]
	fn modal_gate_stops_parent_widgets_and_cancels_held_presses() {
		let build = |blocked| {
			let button = neo_widgets::widgets::NeoButton::<Message, iced::Theme, ()>::new(
				Space::new().width(100).height(50),
			)
			.id("modal-parent")
			.focusable(true)
			.on_press(BrowserMessage::ClearSelection.into());
			modal_gate::<()>(button.into(), blocked)
		};
		let mut content = build(false);
		let mut tree = Tree::new(content.as_widget());
		content.as_widget_mut().diff(&mut tree);
		let limits = layout::Limits::new(Size::ZERO, Size::new(200.0, 100.0));
		let node = content.as_widget_mut().layout(&mut tree, &(), &limits);
		content.as_widget_mut().operate(
			&mut tree,
			Layout::new(&node),
			&(),
			&mut operation::focusable::focus::<()>(Id::from("modal-parent")),
		);
		let mut bus = Bus::new();
		content.as_widget_mut().update(
			&mut tree,
			&Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
			Layout::new(&node),
			mouse::Cursor::Available(iced::Point::new(20.0, 20.0)),
			&(),
			&mut Shell::new(&window::Headless, Waker::noop(), &mut bus),
			&Rectangle::with_size(Size::new(200.0, 100.0)),
		);
		assert_eq!(bus.into_iter().count(), 0);
		content = build(true);
		tree.diff(content.as_widget_mut());
		let node = content.as_widget_mut().layout(&mut tree, &(), &limits);
		for event in [
			Event::Keyboard(typeahead::tests::key(
				keyboard::Key::Named(keyboard::key::Named::Enter),
				None,
				keyboard::Modifiers::empty(),
			)),
			Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
			Event::Mouse(mouse::Event::WheelScrolled {
				delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
			}),
			Event::Touch(iced::touch::Event::FingerPressed {
				id: iced::touch::Finger(1),
				position: iced::Point::new(20.0, 20.0),
			}),
		] {
			let mut bus = Bus::new();
			content.as_widget_mut().update(
				&mut tree,
				&event,
				Layout::new(&node),
				mouse::Cursor::Available(iced::Point::new(20.0, 20.0)),
				&(),
				&mut Shell::new(&window::Headless, Waker::noop(), &mut bus),
				&Rectangle::with_size(Size::new(200.0, 100.0)),
			);
			assert_eq!(bus.into_iter().count(), 0);
		}
		content = build(false);
		tree.diff(content.as_widget_mut());
		let node = content.as_widget_mut().layout(&mut tree, &(), &limits);
		let mut bus = Bus::new();
		content.as_widget_mut().update(
			&mut tree,
			&Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
			Layout::new(&node),
			mouse::Cursor::Available(iced::Point::new(20.0, 20.0)),
			&(),
			&mut Shell::new(&window::Headless, Waker::noop(), &mut bus),
			&Rectangle::with_size(Size::new(200.0, 100.0)),
		);
		assert_eq!(
			bus.into_iter().count(),
			0,
			"release after dismissal activated parent"
		);
	}

	#[test]
	fn file_keyboard_gate_handles_selection_before_buttons_can_activate() {
		let button = || {
			neo_widgets::widgets::NeoButton::<Message, iced::Theme, ()>::new(
				Space::new().width(100).height(40),
			)
			.id("file")
			.focusable(true)
			.on_press(BrowserMessage::OpenEntry("/file".into()).into())
		};
		for (key, modifiers) in [
			(
				keyboard::Key::Named(keyboard::key::Named::Space),
				keyboard::Modifiers::CTRL,
			),
			(
				keyboard::Key::Named(keyboard::key::Named::Space),
				keyboard::Modifiers::empty(),
			),
			(
				keyboard::Key::Named(keyboard::key::Named::Enter),
				keyboard::Modifiers::empty(),
			),
			(
				keyboard::Key::Named(keyboard::key::Named::ContextMenu),
				keyboard::Modifiers::empty(),
			),
			(
				keyboard::Key::Named(keyboard::key::Named::F10),
				keyboard::Modifiers::SHIFT,
			),
			(
				keyboard::Key::Character("a".into()),
				keyboard::Modifiers::CTRL,
			),
		] {
			let messages = dispatch_files(
				button().into(),
				"file",
				false,
				typeahead::tests::key(key.clone(), None, modifiers),
				None,
				vec![(Id::from("file"), "/file".into())],
			);
			assert!(
				matches!(messages.as_slice(), [Message::Browser(BrowserMessage::SelectionKey(k, m, Some(path)))] if *k == key && *m == modifiers && path == &std::path::PathBuf::from("/file")),
				"{messages:?}"
			);
		}
	}

	#[test]
	fn selection_shortcuts_do_not_escape_a_focused_editor() {
		let input = text_input::<Message, iced::Theme>("Search", "abc")
			.id("search")
			.on_input(|text| BrowserMessage::Search(text).into());
		let messages = dispatch_files(
			input.into(),
			"search",
			false,
			typeahead::tests::key(
				keyboard::Key::Character("a".into()),
				None,
				keyboard::Modifiers::CTRL,
			),
			None,
			vec![(Id::from("file"), "/file".into())],
		);
		assert!(!messages.iter().any(|m| matches!(
			m,
			Message::Browser(BrowserMessage::SelectionKey(..) | BrowserMessage::SelectionMove(..))
		)));
	}

	#[test]
	fn pointer_background_clears_only_when_no_entry_consumes_the_press() {
		use iced::widget::{column, mouse_area, scrollable};
		let build = || -> Element<'static, Message, iced::Theme, ()> {
			let entry = neo_widgets::widgets::NeoButton::<Message, iced::Theme, ()>::new(
				mouse_area(Space::new().width(100).height(40))
					.on_press(BrowserMessage::Clicked("/file".into()).into()),
			)
			.focusable(true)
			.focus_on_click(false)
			.padding(0)
			.on_press(BrowserMessage::OpenEntry("/file".into()).into());
			let content: Element<'_, Message, iced::Theme, ()> =
				scrollable(column![entry]).height(100).into();
			neo_widgets::widgets::NeoButton::new(
				mouse_area(content).on_press(BrowserMessage::ClearSelection.into()),
			)
			.padding(0)
			.focusable(false)
			.focus_on_click(false)
			.into()
		};
		for (point, background) in [
			(iced::Point::new(20.0, 20.0), false),
			(iced::Point::new(20.0, 70.0), true),
		] {
			let mut content = build();
			let mut tree = Tree::new(content.as_widget());
			content.as_widget_mut().diff(&mut tree);
			let node = content.as_widget_mut().layout(
				&mut tree,
				&(),
				&layout::Limits::new(Size::ZERO, Size::new(100.0, 100.0)),
			);
			let mut bus = Bus::new();
			content.as_widget_mut().update(
				&mut tree,
				&Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
				Layout::new(&node),
				mouse::Cursor::Available(point),
				&(),
				&mut Shell::new(&window::Headless, Waker::noop(), &mut bus),
				&Rectangle::with_size(Size::new(100.0, 100.0)),
			);
			let messages = bus.into_iter().collect::<Vec<_>>();
			assert_eq!(messages.len(), 1, "{messages:?}");
			assert_eq!(
				matches!(
					messages.first(),
					Some(Message::Browser(BrowserMessage::ClearSelection))
				),
				background,
				"{messages:?}"
			);
		}
	}

	#[test]
	fn tab_treats_files_as_one_stop_without_changing_selection() {
		let button = |id: &'static str| {
			neo_widgets::widgets::NeoButton::<Message, iced::Theme, ()>::new(
				Space::new().width(100).height(20),
			)
			.id(id)
			.focusable(true)
			.on_press(BrowserMessage::ClearSelection.into())
		};
		let mut gate = Gate {
			content: iced::widget::column![
				button("before"),
				button("file-a"),
				button("file-b"),
				button("after")
			]
			.into(),
			active: false,
			enabled: true,
			menu: None,
			forward_keys: false,
			blocked: false,
			entries: vec![
				(Id::from("file-a"), "/a".into()),
				(Id::from("file-b"), "/b".into()),
			],
		};
		let mut tree = Tree::new(&gate as &dyn Widget<Message, iced::Theme, ()>);
		gate.diff(&mut tree);
		let node = gate.layout(
			&mut tree,
			&(),
			&layout::Limits::new(Size::ZERO, Size::new(200.0, 200.0)),
		);
		for (start, modifiers, expected) in [
			("file-a", keyboard::Modifiers::empty(), "after"),
			("file-b", keyboard::Modifiers::empty(), "after"),
			("file-b", keyboard::Modifiers::SHIFT, "before"),
			("before", keyboard::Modifiers::empty(), "file-a"),
			("after", keyboard::Modifiers::SHIFT, "file-a"),
		] {
			gate.operate(
				&mut tree,
				Layout::new(&node),
				&(),
				&mut operation::focusable::focus::<()>(Id::from(start)),
			);
			let mut bus = Bus::new();
			gate.update(
				&mut tree,
				&Event::Keyboard(typeahead::tests::key(
					keyboard::Key::Named(keyboard::key::Named::Tab),
					None,
					modifiers,
				)),
				Layout::new(&node),
				mouse::Cursor::Unavailable,
				&(),
				&mut Shell::new(&window::Headless, Waker::noop(), &mut bus),
				&Rectangle::with_size(Size::new(200.0, 200.0)),
			);
			assert_eq!(bus.into_iter().count(), 0);
			let mut focus = EditorFocus::default();
			gate.operate(&mut tree, Layout::new(&node), &(), &mut focus);
			assert_eq!(focus.focused, vec![Id::from(expected)]);
		}
	}
	#[test]
	fn space_refines_an_active_prefix_instead_of_opening_a_focused_file() {
		let button = || {
			neo_widgets::widgets::NeoButton::<Message, iced::Theme, ()>::new(
				Space::new().width(100).height(40),
			)
			.id("file")
			.focusable(true)
			.on_press(BrowserMessage::OpenEntry("/file".into()).into())
		};
		let space = typeahead::tests::key(
			keyboard::Key::Named(keyboard::key::Named::Space),
			Some(" "),
			keyboard::Modifiers::default(),
		);
		let messages = dispatch(button().into(), "file", true, space.clone(), None);
		assert_eq!(messages.len(), 1);
		assert!(
			matches!(messages.first(), Some(Message::Browser(BrowserMessage::TypeAhead(typeahead::Edit::Append(text)))) if text == " ")
		);
		let messages = dispatch(button().into(), "file", false, space, None);
		assert!(matches!(
			messages.first(),
			Some(Message::Browser(BrowserMessage::OpenEntry(_)))
		));
	}

	#[test]
	fn focused_menu_search_keeps_horizontal_caret_keys() {
		for named in [
			keyboard::key::Named::ArrowLeft,
			keyboard::key::Named::ArrowRight,
		] {
			let input = text_input::<Message, iced::Theme>("Search", "open")
				.id("menu-search")
				.on_input(|text| BrowserMessage::Search(text).into());
			let key = typeahead::tests::key(
				keyboard::Key::Named(named),
				None,
				keyboard::Modifiers::default(),
			);
			let messages = dispatch(
				input.into(),
				"menu-search",
				false,
				key,
				Some(window::Id::unique()),
			);
			assert!(!messages.iter().any(|m| matches!(
				m,
				Message::ContextMenu(ContextMenuMessage::MenuKey(..))
					| Message::Browser(BrowserMessage::TypeAhead(_))
			)));
		}
	}
}
