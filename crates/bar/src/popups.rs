//! Popup ancestry, focus and closing order. No window effects or widget state.
use iced::window::Id;

use super::Section;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Route {
	pub section: Section,
	pub index: usize,
	pub depth: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct Popup {
	id: Id,
	parent: Option<Id>,
	route: Route,
}

#[derive(Default)]
pub(super) struct Popups {
	root: Option<Popup>,
	contexts: Vec<Popup>,
}

#[derive(Clone, Copy)]
pub(super) enum Event {
	Toggle {
		id: Id,
		section: Section,
		index: usize,
	},
	Context {
		id: Id,
		parent: Id,
		section: Section,
		index: usize,
	},
	Closed(Id),
	Unfocused(Id),
	CloseAll,
	CloseContexts,
	CloseDescendants {
		parent: Id,
		expected_child: Id,
	},
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Effect {
	Close(Id),
	NotifyClosed(Section, usize),
}

#[derive(Default)]
pub(super) struct Change {
	pub opened: Option<Route>,
	pub effects: Vec<Effect>,
}

impl Popups {
	pub fn context_children(&self) -> Vec<(Id, Id)> {
		self.contexts
			.iter()
			.filter_map(|popup| popup.parent.map(|parent| (parent, popup.id)))
			.collect()
	}

	pub fn route(&self, id: Id) -> Option<Route> {
		self.contexts
			.iter()
			.find(|popup| popup.id == id)
			.or_else(|| self.root.as_ref().filter(|popup| popup.id == id))
			.map(|popup| popup.route)
	}

	pub fn update(&mut self, event: Event) -> Change {
		let mut change = Change::default();
		match event {
			Event::Toggle { id, section, index } => {
				let same = self.root.is_some_and(|popup| {
					popup.route.section == section && popup.route.index == index
				});
				self.close_all(&mut change.effects, true);
				if !same {
					let route = Route {
						section,
						index,
						depth: None,
					};
					self.root = Some(Popup {
						id,
						parent: None,
						route,
					});
					change.opened = Some(route);
				}
			}
			Event::Context {
				id,
				parent,
				section,
				index,
			} => {
				let Some(route) = self
					.route(parent)
					.filter(|route| route.section == section && route.index == index)
				else {
					return change;
				};
				let depth = route.depth.map_or(0, |depth| depth + 1);
				self.close_contexts(depth, &mut change.effects);
				let route = Route {
					section,
					index,
					depth: Some(depth),
				};
				self.contexts.push(Popup {
					id,
					parent: Some(parent),
					route,
				});
				change.opened = Some(route);
			}
			Event::Closed(id) => {
				if let Some(depth) = self.contexts.iter().position(|popup| popup.id == id) {
					self.close_contexts(depth + 1, &mut change.effects);
					self.contexts.remove(depth);
				} else if self.root.is_some_and(|popup| popup.id == id) {
					self.close_all(&mut change.effects, false);
				}
			}
			Event::Unfocused(id) => {
				if let Some(depth) = self.contexts.iter().position(|popup| popup.id == id) {
					if depth + 1 == self.contexts.len() {
						self.close_contexts(depth, &mut change.effects);
					}
				} else if self.contexts.is_empty() && self.root.is_some_and(|popup| popup.id == id)
				{
					self.close_all(&mut change.effects, true);
				}
			}
			Event::CloseAll => self.close_all(&mut change.effects, true),
			Event::CloseContexts => self.close_contexts(0, &mut change.effects),
			Event::CloseDescendants {
				parent,
				expected_child,
			} => {
				if let Some(depth) = self
					.contexts
					.iter()
					.position(|popup| popup.parent == Some(parent) && popup.id == expected_child)
				{
					self.close_contexts(depth, &mut change.effects);
				}
			}
		}
		change
	}

	fn close_contexts(&mut self, depth: usize, effects: &mut Vec<Effect>) {
		effects.extend(
			self.contexts
				.drain(depth..)
				.rev()
				.map(|popup| Effect::Close(popup.id)),
		);
	}

	fn close_all(&mut self, effects: &mut Vec<Effect>, close_root: bool) {
		if let Some(popup) = self.root.take() {
			self.close_contexts(0, effects);
			if close_root {
				effects.push(Effect::Close(popup.id));
			}
			effects.push(Effect::NotifyClosed(popup.route.section, popup.route.index));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn open(popups: &mut Popups, id: Id) {
		popups.update(Event::Toggle {
			id,
			section: Section::Left,
			index: 0,
		});
	}
	fn context(popups: &mut Popups, id: Id, parent: Id) -> Change {
		popups.update(Event::Context {
			id,
			parent,
			section: Section::Left,
			index: 0,
		})
	}

	#[test]
	fn replacement_closes_deepest_first_and_notifies_before_opening_new_owner() {
		let mut popups = Popups::default();
		let (root, child, leaf, next) = (Id::unique(), Id::unique(), Id::unique(), Id::unique());
		open(&mut popups, root);
		context(&mut popups, child, root);
		context(&mut popups, leaf, child);
		let change = popups.update(Event::Toggle {
			id: next,
			section: Section::Right,
			index: 2,
		});
		assert_eq!(
			change.effects,
			vec![
				Effect::Close(leaf),
				Effect::Close(child),
				Effect::Close(root),
				Effect::NotifyClosed(Section::Left, 0)
			]
		);
		assert_eq!(
			change.opened,
			Some(Route {
				section: Section::Right,
				index: 2,
				depth: None
			})
		);
		assert!(popups.route(child).is_none());
		assert!(popups.route(next).is_some());
	}

	#[test]
	fn same_owner_toggles_closed_and_duplicate_closes_do_nothing() {
		let mut popups = Popups::default();
		let root = Id::unique();
		open(&mut popups, root);
		let change = popups.update(Event::Toggle {
			id: Id::unique(),
			section: Section::Left,
			index: 0,
		});
		assert!(change.opened.is_none());
		assert_eq!(
			change.effects,
			vec![Effect::Close(root), Effect::NotifyClosed(Section::Left, 0)]
		);
		assert!(popups.update(Event::Closed(root)).effects.is_empty());
	}

	#[test]
	fn focus_parentage_and_stale_requests_preserve_the_current_chain() {
		let mut popups = Popups::default();
		let (root, child, leaf) = (Id::unique(), Id::unique(), Id::unique());
		open(&mut popups, root);
		context(&mut popups, child, root);
		context(&mut popups, leaf, child);
		for id in [root, child] {
			assert!(popups.update(Event::Unfocused(id)).effects.is_empty());
		}
		assert!(
			context(&mut popups, Id::unique(), Id::unique())
				.opened
				.is_none()
		);
		assert!(
			popups
				.update(Event::CloseDescendants {
					parent: root,
					expected_child: Id::unique()
				})
				.effects
				.is_empty()
		);
		assert_eq!(
			popups.update(Event::Unfocused(leaf)).effects,
			vec![Effect::Close(leaf)]
		);
		assert_eq!(
			popups.update(Event::Closed(root)).effects,
			vec![Effect::Close(child), Effect::NotifyClosed(Section::Left, 0)]
		);
		assert!(popups.route(root).is_none());
	}
}
