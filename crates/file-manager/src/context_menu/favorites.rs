//! Shared-config persistence and deterministic contextual favorites ordering.
use super::Message as ContextMenuMessage;
use crate::{Message, context_menu::Command};
use iced::Task;

#[derive(Default)]
pub struct Favorites {
	pub order: Vec<Command>,
	pub loaded: bool,
	pub error: Option<String>,
	pub revision: u64,
	pub reload_pending: bool,
	confirmed: Vec<Command>,
	active: Option<Vec<Command>>,
	pending: Option<Vec<Command>>,
}

impl Favorites {
	pub fn running(&self) -> bool {
		self.active.is_some()
	}
	pub fn observe(&mut self, result: Result<Vec<String>, String>) {
		if self.running() {
			return;
		}
		match result {
			Ok(ids) => {
				self.order = ids.iter().filter_map(|id| Command::parse(id)).collect();
				self.confirmed = self.order.clone();
				self.loaded = true;
				self.error = None;
			}
			Err(e) => {
				self.error = Some(e);
				self.loaded = false;
			}
		}
	}
	pub fn change(&mut self, order: Vec<Command>) -> Task<Message> {
		if !self.loaded || self.order == order {
			return Task::none();
		}
		self.revision += 1;
		self.order = order.clone();
		self.pending = Some(order);
		self.start_next()
	}
	pub fn complete(&mut self, saved: Vec<Command>, result: Result<(), String>) -> Task<Message> {
		self.active = None;
		match result {
			Ok(()) => {
				self.confirmed = saved;
				self.error = None;
			}
			Err(e) => {
				self.error = Some(format!("Could not save favorites: {e}"));
				if self.pending.is_none() {
					self.order = self.confirmed.clone();
				}
			}
		}
		self.start_next()
	}

	fn start_next(&mut self) -> Task<Message> {
		if self.running() {
			return Task::none();
		}
		let Some(order) = self.pending.take() else {
			return Task::none();
		};
		self.active = Some(order.clone());
		Task::perform(
			async move {
				let saved = order.clone();
				let result = tokio::task::spawn_blocking(move || {
					config::ConfigFile::update(|config| {
						config.fileberg.favorites =
							order.iter().map(|c| c.id().to_owned()).collect();
					})
					.map(|_| ())
					.map_err(|e| e.to_string())
				})
				.await
				.map_err(|e| e.to_string())
				.and_then(|r| r);
				(saved, result)
			},
			|(saved, result)| ContextMenuMessage::FavoritesSaved(saved, result).into(),
		)
	}
}

pub fn load(revision: u64) -> Task<Message> {
	Task::perform(
		async {
			tokio::task::spawn_blocking(|| {
				config::ConfigFile::load()
					.map(|(_, c)| c.fileberg.favorites)
					.map_err(|e| e.to_string())
			})
			.await
			.map_err(|e| e.to_string())
			.and_then(|r| r)
		},
		move |result| ContextMenuMessage::FavoritesLoaded(revision, result).into(),
	)
}

/// Permute only occupied visible slots. Hidden favorites retain exact positions.
pub fn reordered(
	order: &[Command], visible: &[Command], source: Command, target: Command, after: bool,
) -> Vec<Command> {
	if source == target || !visible.contains(&source) || !visible.contains(&target) {
		return order.to_vec();
	}
	let mut projection: Vec<_> = order
		.iter()
		.copied()
		.filter(|c| visible.contains(c) && *c != source)
		.collect();
	let Some(index) = projection.iter().position(|c| *c == target) else {
		return order.to_vec();
	};
	projection.insert(index + usize::from(after), source);
	let mut next = projection.into_iter();
	order
		.iter()
		.map(|c| {
			if visible.contains(c) {
				next.next().unwrap_or(*c)
			} else {
				*c
			}
		})
		.collect()
}

impl crate::FileManager {
	pub fn menu_pin(&mut self, id: iced::window::Id, command: Command) -> Task<Message> {
		if !self.menu_rows(id).iter().any(|r| {
			r.action == crate::context_menu::Action::Command(command) && r.disabled.is_none()
		}) {
			return Task::none();
		}
		let mut order = self.favorites.order.clone();
		if order.contains(&command) {
			order.retain(|c| *c != command);
		} else {
			order.push(command);
		}
		if let Some(menu) = &mut self.menu {
			menu.drag = None;
		}
		self.favorites.change(order)
	}

	pub fn menu_favorite_press(
		&mut self, id: iced::window::Id, command: Command, position: iced::Point,
	) {
		if !self.favorites.order.contains(&command) {
			return;
		}
		if let Some(menu) = &mut self.menu
			&& let Some(page) = menu.page(id)
			&& page.query.is_empty()
			&& page.parent_action.is_none()
		{
			menu.drag = Some(crate::context_menu::Drag {
				page: id,
				source: command,
				origin: Some(position),
				dragging: false,
				over: Some((command, false)),
			});
		}
	}

	pub fn menu_pointer(
		&mut self, id: iced::window::Id, event: iced::mouse::Event,
	) -> Task<Message> {
		use iced::mouse::{Button, Event};
		let Some(menu) = &mut self.menu else {
			return Task::none();
		};
		match event {
			Event::CursorMoved { position } => {
				if let Some(p) = menu.page_mut(id) {
					p.cursor = Some(position);
				}
				if let Some(drag) = &mut menu.drag
					&& drag.page == id
					&& drag
						.origin
						.is_some_and(|start| start.distance(position) >= 6.0)
				{
					drag.dragging = true;
				}
			}
			Event::CursorLeft => {
				if menu.drag.as_ref().is_some_and(|d| d.page == id) {
					menu.drag = None;
				}
			}
			Event::ButtonReleased(Button::Left) => {
				let Some(drag) = menu.drag.take() else {
					return Task::none();
				};
				if drag.page != id {
					return Task::none();
				}
				let Some(page) = menu.page(id) else {
					return Task::none();
				};
				if !page.query.is_empty() || page.parent_action.is_some() {
					return Task::none();
				}
				if !drag.dragging {
					if page.hovered != Some(crate::context_menu::Action::Command(drag.source)) {
						return Task::none();
					}
					return self.menu_activate(
						id,
						crate::context_menu::Action::Command(drag.source),
						None,
					);
				}
				let Some((target, after)) = drag.over else {
					return Task::none();
				};
				if page.hovered != Some(crate::context_menu::Action::Command(target)) {
					return Task::none();
				}
				let visible: Vec<_> = self
					.menu_rows(id)
					.into_iter()
					.filter_map(|r| match r.action {
						crate::context_menu::Action::Command(c)
							if self.favorites.order.contains(&c) =>
						{
							Some(c)
						}
						_ => None,
					})
					.collect();
				let order = reordered(&self.favorites.order, &visible, drag.source, target, after);
				log::debug!(?order, "Context favorites reordered");
				return self.favorites.change(order);
			}
			_ => {}
		}
		Task::none()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn reorder_preserves_hidden_slots_and_unrelated_context_favorites() {
		use Command::*;
		assert_eq!(
			reordered(
				&[Open, NewFolder, Copy, Paste, Trash],
				&[Open, Copy, Trash],
				Trash,
				Open,
				false
			),
			vec![Trash, NewFolder, Open, Paste, Copy]
		);
		assert_eq!(
			reordered(
				&[Open, Copy, Trash],
				&[Open, Copy, Trash],
				Open,
				Trash,
				true
			),
			vec![Copy, Trash, Open]
		);
	}
	#[test]
	fn serialized_writes_coalesce_and_failure_restores_confirmed_order() {
		let mut f = Favorites::default();
		f.observe(Ok(vec!["open".into()]));
		drop(f.change(vec![Command::Open, Command::Copy]));
		drop(f.change(vec![Command::Copy]));
		assert!(f.running());
		drop(f.complete(vec![Command::Open, Command::Copy], Ok(())));
		assert!(f.running());
		drop(f.complete(vec![Command::Copy], Err("read only".into())));
		assert_eq!(f.order, vec![Command::Open, Command::Copy]);
		assert!(f.error.is_some());
	}
}
