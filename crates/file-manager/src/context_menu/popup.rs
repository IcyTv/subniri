//! Adapter for the shared popup lifecycle and native shell actions.
use super::Message as ContextMenuMessage;
use crate::{
	FileManager, Message,
	context_menu::{Action, Context, Menu, Page, Target},
};
use iced::{
	Rectangle, Task,
	advanced::widget::{Id, Operation, operation::Outcome},
	keyboard::{self, Key, key::Named},
	window,
};
use iced_exwlshell::{
	actions::IcedNewPopupSettings,
	reexport::{PixelSize, PopupAnchor, PopupGravity},
};
use neo_widgets::popup::{Effect, Event};

impl FileManager {
	pub fn menu_lifecycle(&mut self, event: Event<()>) -> Task<Message> {
		let change = self.popups.update(event);
		let mut task = Task::none();
		for effect in change.effects {
			match effect {
				Effect::Close(id) => task = task.chain(iced::window::close(id)),
				Effect::NotifyClosed(()) => {
					self.menu = None;
					self.drives.menu = None;
				}
			}
		}
		if let Some(menu) = &mut self.menu {
			menu.pages.retain(|p| self.popups.route(p.id).is_some());
		}
		task
	}

	pub fn open_menu(
		&mut self, path: Option<std::path::PathBuf>, position: iced::Point,
	) -> Task<Message> {
		let (path, context) = match path {
			Some(path) => {
				let Some(entry) = self.entries.iter().find(|e| e.path == path) else {
					return Task::none();
				};
				let context = if entry.is_dir {
					Context::Folder
				} else {
					Context::File
				};
				if !self.selection.paths.contains(&path) {
					self.select_only(path.clone());
				}
				self.selected = Some(path.clone());
				self.history.remember_selection(self.selected.clone());
				(path, context)
			}
			None => (self.path.clone(), Context::Background),
		};
		let mut target = match Target::capture(&path, context) {
			Ok(t) => t,
			Err(e) => {
				self.error = Some(e);
				return Task::none();
			}
		};
		if context != Context::Background {
			for path in self.selection.paths.iter().filter(|p| *p != &target.path) {
				let context = if self.entries.iter().any(|e| &e.path == path && e.is_dir) {
					Context::Folder
				} else {
					Context::File
				};
				match Target::capture(path, context) {
					Ok(other) => target.others.push(other),
					Err(error) => {
						self.error = Some(error);
						return Task::none();
					}
				}
			}
		}
		self.typeahead.clear();
		let id = window::Id::unique();
		let close = self.menu_lifecycle(Event::Open {
			id,
			parent: Some(self.host),
			owner: (),
		});
		self.menu = Some(Menu {
			target,
			available: true,
			pages: vec![Page::new(id, None)],
			drag: None,
		});
		self.highlight_first_menu_row(id);
		let position = iced::Point::new(
			position.x.clamp(0.0, (self.host_size.width - 1.0).max(0.0)),
			position
				.y
				.clamp(0.0, (self.host_size.height - 1.0).max(0.0)),
		);
		log::debug!(?id, ?context, "Context menu opened");
		let focus = if context != Context::Background {
			self.entries
				.iter()
				.find(|e| e.path == path)
				.map_or_else(Task::none, |e| iced::widget::operation::focus(e.id.clone()))
		} else {
			Task::none()
		};
		close.chain(focus).chain(Task::done(Message::NewPopUp {
			id,
			settings: IcedNewPopupSettings::at_position(
				self.host,
				PixelSize::px(324, 480),
				(position.x as i32, position.y as i32),
			),
		}))
	}

	pub fn menu_window_event(&mut self, id: window::Id, event: window::Event) -> Task<Message> {
		match event {
			window::Event::Opened { .. } => self
				.menu
				.as_ref()
				.and_then(|m| m.page(id))
				.map_or_else(Task::none, |p| {
					iced::widget::operation::focus(p.search_id())
				}),
			window::Event::Closed => self.menu_lifecycle(Event::Closed(id)),
			window::Event::CloseRequested => self.menu_lifecycle(Event::CloseAll),
			// Focus changes during submenu creation are not dismissal. The compositor
			// reports PopupDone through Closed; do not close on a background update.
			_ => Task::none(),
		}
	}

	pub fn menu_input(&mut self, id: window::Id, query: String) -> Task<Message> {
		if let Some(menu) = &mut self.menu {
			menu.drag = None;
		}
		let Some(page) = self.menu.as_mut().and_then(|m| m.page_mut(id)) else {
			return Task::none();
		};
		page.query = query;
		let focus = page.search_id();
		self.highlight_first_menu_row(id);
		log::debug!(?id,query=%self.menu.as_ref().and_then(|m|m.page(id)).map_or("",|p|p.query.as_str()),"Context query changed");
		iced::widget::operation::focus(focus)
	}

	fn highlight_first_menu_row(&mut self, id: window::Id) {
		let action = self
			.menu_rows(id)
			.iter()
			.find(|r| r.disabled.is_none())
			.map(|r| r.action);
		if let Some(p) = self.menu.as_mut().and_then(|m| m.page_mut(id)) {
			p.highlighted = action;
		}
	}

	pub fn reconcile_menu_highlight(&mut self) {
		let Some(menu) = &self.menu else {
			return;
		};
		let stale: Vec<_> = menu
			.pages
			.iter()
			.filter(|p| {
				p.highlighted.is_some_and(|a| {
					!self
						.menu_rows(p.id)
						.iter()
						.any(|r| r.action == a && r.disabled.is_none())
				})
			})
			.map(|p| p.id)
			.collect();
		for id in stale {
			if let Some(p) = self.menu.as_mut().and_then(|m| m.page_mut(id)) {
				p.highlighted = None;
			}
		}
	}

	pub fn menu_key(
		&mut self, id: window::Id, event: keyboard::Event, focused: Option<Id>,
	) -> Task<Message> {
		let keyboard::Event::KeyPressed { key, modifiers, .. } = &event else {
			return Task::none();
		};
		if self.menu.is_none() {
			if id == self.host
				&& (key.as_ref() == Key::Named(Named::ContextMenu)
					|| (key.as_ref() == Key::Named(Named::F10) && modifiers.shift()))
			{
				return self.keyboard_menu();
			}
			if id == self.host {
				return self.update(crate::browser::Message::Keyboard(event));
			}
			return Task::none();
		}
		if key.as_ref() == Key::Named(Named::Escape) {
			return self.menu_lifecycle(Event::CloseAll);
		}
		if id == self.host {
			return Task::none();
		}
		let rows = self.menu_rows(id);
		let Some(page) = self.menu.as_ref().and_then(|m| m.page(id)) else {
			return Task::none();
		};
		match key.as_ref() {
			Key::Named(Named::ArrowDown | Named::ArrowUp) => {
				let enabled: Vec<_> = rows.iter().filter(|r| r.disabled.is_none()).collect();
				if enabled.is_empty() {
					return Task::none();
				}
				let previous = page
					.highlighted
					.and_then(|a| enabled.iter().position(|r| r.action == a));
				let next = if key.as_ref() == Key::Named(Named::ArrowDown) {
					previous.map_or(0, |i| (i + 1) % enabled.len())
				} else {
					previous.map_or(enabled.len() - 1, |i| {
						(i + enabled.len() - 1) % enabled.len()
					})
				};
				let Some(row) = enabled.get(next) else {
					return Task::none();
				};
				let row_id = page.row_id(row.action);
				let scroll = page.scroll_id();
				if let Some(page) = self.menu.as_mut().and_then(|m| m.page_mut(id)) {
					page.highlighted = Some(row.action);
				}
				iced::widget::operation::focus(row_id).chain(iced::widget::operation::snap_to(
					scroll,
					iced::widget::operation::RelativeOffset {
						x: 0.0,
						y: next as f32 / (enabled.len().saturating_sub(1).max(1)) as f32,
					},
				))
			}
			Key::Named(Named::Enter | Named::ArrowRight) => {
				if key.as_ref() == Key::Named(Named::Enter)
					&& let Some(command) = rows
						.iter()
						.filter_map(|r| match r.action {
							Action::Command(c) => Some(c),
							_ => None,
						})
						.find(|c| {
							focused.as_ref() == Some(&Id::from(page.pin_id(Action::Command(*c))))
						}) {
					return self.menu_pin(id, command);
				}
				let action = page.highlighted;
				if let Some(action) = action {
					if key.as_ref() == Key::Named(Named::ArrowRight)
						&& !matches!(action,Action::Command(c) if c.submenu())
					{
						return Task::none();
					}
					return self.menu_activate(id, action, None);
				}
				Task::none()
			}
			Key::Named(Named::ArrowLeft) if page.parent_action.is_some() => {
				let parent = self
					.menu
					.as_ref()
					.and_then(|m| {
						m.pages
							.iter()
							.position(|p| p.id == id)
							.and_then(|i| i.checked_sub(1).and_then(|j| m.pages.get(j)))
					})
					.map(|p| p.search_id());
				self.menu_lifecycle(Event::Unfocused(id))
					.chain(parent.map_or_else(Task::none, iced::widget::operation::focus))
			}
			Key::Named(Named::Tab) => {
				let mut ids = vec![Id::from(page.search_id())];
				for row in rows.iter().filter(|r| r.disabled.is_none()) {
					ids.push(Id::from(page.row_id(row.action)));
					if matches!(row.action, Action::Command(_)) && self.favorites.loaded {
						ids.push(Id::from(page.pin_id(row.action)));
					}
				}
				let current = focused
					.and_then(|f| ids.iter().position(|i| *i == f))
					.unwrap_or(0);
				let next = if modifiers.shift() {
					(current + ids.len() - 1) % ids.len()
				} else {
					(current + 1) % ids.len()
				};
				let Some(target) = ids.get(next).cloned() else {
					return Task::none();
				};
				if let Some(row) = rows.iter().find(|r| {
					Id::from(page.row_id(r.action)) == target
						|| Id::from(page.pin_id(r.action)) == target
				}) {
					if let Some(page) = self.menu.as_mut().and_then(|m| m.page_mut(id)) {
						page.highlighted = Some(row.action);
					}
				}
				iced::widget::operation::focus(target)
			}
			_ => Task::none(),
		}
	}

	pub fn menu_activate(
		&mut self, id: window::Id, action: Action, bounds: Option<Rectangle>,
	) -> Task<Message> {
		if !self
			.menu_rows(id)
			.iter()
			.any(|r| r.action == action && r.disabled.is_none())
		{
			return Task::none();
		}
		let Some(menu) = self.menu.as_ref() else {
			return Task::none();
		};
		if !menu.available || !menu.target.available() {
			if let Some(m) = &mut self.menu {
				m.available = false;
			}
			return Task::none();
		}
		if let Action::Command(command) = action
			&& command.submenu()
		{
			if bounds.is_none() {
				let Some(page) = menu.page(id) else {
					return Task::none();
				};
				return iced_runtime::task::widget(Bounds::new(Id::from(format!(
					"{}-bounds",
					page.row_id(action)
				))))
				.map(move |bounds| {
					ContextMenuMessage::MenuActivate(id, action, Some(bounds)).into()
				});
			}
			let child = window::Id::unique();
			let change = self.popups.update(Event::Context {
				id: child,
				parent: id,
				owner: (),
			});
			let Some(route) = change.opened else {
				return Task::none();
			};
			let mut close = Task::none();
			for effect in change.effects {
				if let Effect::Close(id) = effect {
					close = close.chain(iced::window::close(id));
				}
			}
			let Some(menu) = self.menu.as_mut() else {
				return Task::none();
			};
			menu.pages.truncate(route.depth.unwrap_or(0) + 1);
			menu.pages.push(Page::new(child, Some(command)));
			self.highlight_first_menu_row(child);
			let Some(bounds) = bounds else {
				return Task::none();
			};
			return close.chain(Task::done(Message::NewPopUp {
				id: child,
				settings: IcedNewPopupSettings::new(
					id,
					PixelSize::px(324, 360),
					(bounds.x as i32, bounds.y as i32),
					PixelSize::px(bounds.width.max(1.0) as u32, bounds.height.max(1.0) as u32),
				)
				.anchor(PopupAnchor::Right)
				.gravity(PopupGravity::Right),
			}));
		}
		let target = menu.target.clone();
		self.menu_lifecycle(Event::CloseAll).chain(Task::done(
			ContextMenuMessage::MenuDispatch(target, action).into(),
		))
	}

	pub fn menu_dispatch(&mut self, target: Target, action: Action) -> Task<Message> {
		action.execute(self, target)
	}

	pub(super) fn restore_menu_selection(&mut self, target: &Target) {
		self.select_only(target.path.clone());
		self.selection
			.paths
			.extend(target.others.iter().map(|t| t.path.clone()));
	}

	pub(crate) fn keyboard_menu(&mut self) -> Task<Message> {
		if let Some(entry) = self
			.entries
			.iter()
			.find(|e| Some(&e.path) == self.selected.as_ref())
		{
			let path = entry.path.clone();
			return self.reveal_selection().chain(
				iced_runtime::task::widget(Bounds::new(entry.id.clone())).map(move |bounds| {
					ContextMenuMessage::MenuOpen(
						Some(path.clone()),
						iced::Point::new(bounds.x, bounds.y + bounds.height),
					)
					.into()
				}),
			);
		}
		self.open_menu(None, iced::Point::new(260.0, 180.0))
	}
}

struct Bounds {
	id: Id,
	bounds: Option<Rectangle>,
	translation: iced::Vector,
	pending_translation: iced::Vector,
}
impl Bounds {
	fn new(id: Id) -> Self {
		Self {
			id,
			bounds: None,
			translation: iced::Vector::ZERO,
			pending_translation: iced::Vector::ZERO,
		}
	}
}
impl Operation<Rectangle> for Bounds {
	fn focusable(
		&mut self, id: Option<&Id>, bounds: Rectangle,
		_state: &mut dyn iced::advanced::widget::operation::Focusable,
	) {
		if id == Some(&self.id) {
			self.bounds = Some(bounds - self.translation);
		}
	}
	fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Rectangle>)) {
		let previous = self.translation;
		self.translation += std::mem::replace(&mut self.pending_translation, iced::Vector::ZERO);
		operate(self);
		self.translation = previous;
	}
	fn scrollable(
		&mut self, _: Option<&Id>, _: Rectangle, _: Rectangle, translation: iced::Vector,
		_: &mut dyn iced::advanced::widget::operation::Scrollable,
	) {
		self.pending_translation = translation;
	}
	fn container(&mut self, id: Option<&Id>, bounds: Rectangle) {
		if id == Some(&self.id) {
			self.bounds = Some(bounds - self.translation);
		}
	}
	fn finish(&self) -> Outcome<Rectangle> {
		self.bounds.map_or(Outcome::None, Outcome::Some)
	}
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
	use super::*;
	use crate::context_menu::Command;
	use crate::context_menu::Message;
	use crate::{Args, test_support::TestDir};

	#[test]
	fn anchor_bounds_use_window_space_without_leaking_scroll_to_siblings() {
		use iced::{
			Element, Length, Size,
			advanced::{
				Layout, layout,
				widget::{Tree, operation},
			},
			widget::{Space, column, container, scrollable},
		};
		let mut widget: Element<'_, (), iced::Theme, ()> = column![
			scrollable(column![
				Space::new().height(700),
				container(
					neo_widgets::widgets::NeoButton::new(
						Space::new().height(30).width(Length::Fill)
					)
					.padding(0)
					.focusable(true)
					.id("entry")
				)
				.id("row")
				.width(Length::Fill),
				Space::new().height(400),
			])
			.id("files")
			.height(200),
			container(Space::new().height(30)).id("sibling"),
		]
		.into();
		let mut tree = Tree::new(widget.as_widget());
		widget.as_widget_mut().diff(&mut tree);
		let node = widget.as_widget_mut().layout(
			&mut tree,
			&(),
			&layout::Limits::new(Size::ZERO, Size::new(300.0, 300.0)),
		);
		widget.as_widget_mut().operate(
			&mut tree,
			Layout::new(&node),
			&(),
			&mut operation::scrollable::scroll_to::<()>(
				Id::from("files"),
				operation::scrollable::AbsoluteOffset {
					x: None,
					y: Some(600.0),
				},
			),
		);
		for (id, y) in [("entry", 100.0), ("row", 100.0), ("sibling", 200.0)] {
			let mut bounds = Bounds::new(Id::from(id));
			widget.as_widget_mut().operate(
				&mut tree,
				Layout::new(&node),
				&(),
				&mut operation::black_box(&mut bounds),
			);
			assert_eq!(bounds.bounds.unwrap().y, y, "{id}");
		}
	}
	fn app() -> (TestDir, FileManager, window::Id) {
		let dir = TestDir::new();
		std::fs::write(dir.path().join("file"), "unchanged").unwrap();
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		app.entries = crate::filesystem::entry::read_directory(dir.path()).unwrap();
		app.loading = false;
		let _ = app.open_menu(
			Some(dir.path().join("file")),
			iced::Point::new(300.0, 300.0),
		);
		let id = app.menu.as_ref().unwrap().pages[0].id;
		(dir, app, id)
	}
	#[test]
	fn logging_stubs_do_not_modify_the_original_or_directory() {
		let (dir, mut app, _) = app();
		let target = app.menu.as_ref().unwrap().target.clone();
		for command in [
			Command::Cut,
			Command::Copy,
			Command::Duplicate,
			Command::Rename,
			Command::OpenWindow,
		] {
			let _ = app.menu_dispatch(target.clone(), Action::Command(command));
		}
		assert_eq!(
			std::fs::read_to_string(dir.path().join("file")).unwrap(),
			"unchanged"
		);
		assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
	}
	#[test]
	fn active_menu_blocks_typeahead_and_background_reload_preserves_query_and_target() {
		let (dir, mut app, id) = app();
		let _ = app.menu_input(id, "dup".into());
		let original = app.menu.as_ref().unwrap().target.path.clone();
		let _ = app.update(crate::browser::Message::TypeAhead(
			crate::browser::typeahead::Edit::Append("f".into()),
		));
		assert!(!app.typeahead.is_active());
		let _ = app.update(crate::browser::Message::Loaded(
			app.request_id,
			crate::filesystem::entry::read_directory(dir.path()),
		));
		assert_eq!(app.menu.as_ref().unwrap().page(id).unwrap().query, "dup");
		assert_eq!(app.menu.as_ref().unwrap().target.path, original);
		let _ = app.menu_lifecycle(Event::CloseAll);
		assert!(app.menu.is_none());
		assert_eq!(app.selected, Some(original));
	}
	#[test]
	fn lost_highlight_never_falls_through_to_another_enter_action() {
		let (_dir, mut app, id) = app();
		let _ = app.menu_input(id, "duplicate".into());
		let action = app.menu.as_ref().unwrap().page(id).unwrap().highlighted;
		assert_eq!(action, Some(Action::Command(Command::Duplicate)));
		let _ = app.update(Message::MenuStatus(id, false));
		assert!(
			app.menu
				.as_ref()
				.unwrap()
				.page(id)
				.unwrap()
				.highlighted
				.is_none()
		);
		let key = crate::browser::typeahead::tests::key(
			Key::Named(Named::Enter),
			None,
			keyboard::Modifiers::default(),
		);
		let _ = app.menu_key(id, key, None);
		assert!(app.menu.is_some());
		let _ = app.update(Message::MenuStatus(id, true));
		assert!(!app.menu.as_ref().unwrap().available);
	}
	#[test]
	fn pinning_is_unique_and_repinning_appends_without_closing() {
		let (_dir, mut app, id) = app();
		app.favorites
			.observe(Ok(vec!["copy".into(), "rename".into()]));
		assert_eq!(
			app.menu_rows(id)
				.iter()
				.filter(|r| r.action == Action::Command(Command::Copy))
				.count(),
			1
		);
		let _ = app.menu_pin(id, Command::Copy);
		let _ = app.menu_pin(id, Command::Copy);
		assert_eq!(app.favorites.order, vec![Command::Rename, Command::Copy]);
		assert!(app.menu.is_some());
	}
	#[test]
	fn favorite_gesture_threshold_distinguishes_click_reorder_and_search() {
		let (_dir, mut app, id) = app();
		app.favorites
			.observe(Ok(vec!["copy".into(), "rename".into()]));
		app.menu_favorite_press(id, Command::Copy, iced::Point::new(10.0, 10.0));
		let _ = app.menu_pointer(
			id,
			iced::mouse::Event::CursorMoved {
				position: iced::Point::new(10.0, 13.0),
			},
		);
		assert!(!app.menu.as_ref().unwrap().drag.as_ref().unwrap().dragging);
		app.menu.as_mut().unwrap().page_mut(id).unwrap().hovered =
			Some(Action::Command(Command::Rename));
		let _ = app.menu_pointer(
			id,
			iced::mouse::Event::CursorMoved {
				position: iced::Point::new(10.0, 50.0),
			},
		);
		app.menu.as_mut().unwrap().drag.as_mut().unwrap().over = Some((Command::Rename, true));
		let _ = app.menu_pointer(
			id,
			iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left),
		);
		assert_eq!(app.favorites.order, vec![Command::Rename, Command::Copy]);
		assert!(app.menu.is_some());
		let _ = app.menu_input(id, "copy".into());
		app.menu_favorite_press(id, Command::Copy, iced::Point::ORIGIN);
		assert!(app.menu.as_ref().unwrap().drag.is_none());
		let _ = app.menu_input(id, String::new());
		app.menu.as_mut().unwrap().page_mut(id).unwrap().hovered =
			Some(Action::Command(Command::Copy));
		app.menu_favorite_press(id, Command::Copy, iced::Point::ORIGIN);
		let _ = app.menu_pointer(
			id,
			iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left),
		);
		assert!(app.menu.is_none());
	}

	#[test]
	fn external_config_reload_is_deferred_through_favorite_writes() {
		let (_dir, mut app, id) = app();
		app.favorites.observe(Ok(vec!["copy".into()]));
		let _ = app.menu_pin(id, Command::Rename);
		assert!(app.favorites.running());
		let _ = app.update(Message::FavoritesReload);
		assert!(app.favorites.reload_pending);
		let _ = app.update(Message::FavoritesSaved(
			vec![Command::Copy, Command::Rename],
			Ok(()),
		));
		assert!(!app.favorites.running());
		assert!(!app.favorites.reload_pending);
	}
}
