//! Popup ancestry, focus and closing order. No window effects or widget state.
use iced::window::Id;

use super::Section;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Route {
	pub section: Section,
	pub index: usize,
	pub depth: Option<usize>,
}

#[derive(Default)]
pub(super) struct Popups {
	inner: neo_widgets::popup::Popups<(Section, usize)>,
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
	pub fn topmost(&self) -> Option<Id> {
		self.inner.topmost()
	}

	pub fn context_children(&self) -> Vec<(Id, Id)> {
		self.inner.context_children()
	}

	pub fn route(&self, id: Id) -> Option<Route> {
		self.inner.route(id).map(Self::route_from_shared)
	}

	pub fn update(&mut self, event: Event) -> Change {
		use neo_widgets::popup as shared;
		let event = match event {
			Event::Toggle { id, section, index } => shared::Event::Toggle {
				id,
				owner: (section, index),
			},
			Event::Context {
				id,
				parent,
				section,
				index,
			} => shared::Event::Context {
				id,
				parent,
				owner: (section, index),
			},
			Event::Closed(id) => shared::Event::Closed(id),
			Event::Unfocused(id) => shared::Event::Unfocused(id),
			Event::CloseAll => shared::Event::CloseAll,
			Event::CloseContexts => shared::Event::CloseContexts,
			Event::CloseDescendants {
				parent,
				expected_child,
			} => shared::Event::CloseDescendants {
				parent,
				expected_child,
			},
		};
		let change = self.inner.update(event);
		Change {
			opened: change.opened.map(Self::route_from_shared),
			effects: change
				.effects
				.into_iter()
				.map(|effect| match effect {
					shared::Effect::Close(id) => Effect::Close(id),
					shared::Effect::NotifyClosed((section, index)) => {
						Effect::NotifyClosed(section, index)
					}
				})
				.collect(),
		}
	}

	fn route_from_shared(route: neo_widgets::popup::Route<(Section, usize)>) -> Route {
		Route {
			section: route.owner.0,
			index: route.owner.1,
			depth: route.depth,
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
	fn escape_target_tracks_the_topmost_live_popup() {
		let mut popups = Popups::default();
		let (root, child) = (Id::unique(), Id::unique());
		assert_eq!(popups.topmost(), None);
		open(&mut popups, root);
		assert_eq!(popups.topmost(), Some(root));
		context(&mut popups, child, root);
		assert_eq!(popups.topmost(), Some(child));
		popups.update(Event::Closed(child));
		assert_eq!(popups.topmost(), Some(root));
		popups.update(Event::Closed(root));
		assert_eq!(popups.topmost(), None);
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
