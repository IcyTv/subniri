//! Shared popup ancestry and ordered lifecycle, independent of shell/runtime.
use iced::window::Id;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route<Owner> {
	pub owner: Owner,
	pub depth: Option<usize>,
}

#[derive(Clone, Copy)]
struct Popup<Owner> {
	id: Id,
	parent: Option<Id>,
	route: Route<Owner>,
}

pub struct Popups<Owner> {
	root: Option<Popup<Owner>>,
	contexts: Vec<Popup<Owner>>,
}

impl<Owner> Default for Popups<Owner> {
	fn default() -> Self {
		Self {
			root: None,
			contexts: Vec::new(),
		}
	}
}

#[derive(Debug, Clone, Copy)]
pub enum Event<Owner> {
	Toggle {
		id: Id,
		owner: Owner,
	},
	Open {
		id: Id,
		parent: Option<Id>,
		owner: Owner,
	},
	Context {
		id: Id,
		parent: Id,
		owner: Owner,
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
pub enum Effect<Owner> {
	Close(Id),
	NotifyClosed(Owner),
}

pub struct Change<Owner> {
	pub opened: Option<Route<Owner>>,
	pub effects: Vec<Effect<Owner>>,
}

impl<Owner> Default for Change<Owner> {
	fn default() -> Self {
		Self {
			opened: None,
			effects: Vec::new(),
		}
	}
}

impl<Owner: Copy + Eq> Popups<Owner> {
	pub fn topmost(&self) -> Option<Id> {
		self.contexts.last().or(self.root.as_ref()).map(|p| p.id)
	}
	pub fn context_children(&self) -> Vec<(Id, Id)> {
		self.root
			.iter()
			.chain(self.contexts.iter())
			.filter_map(|p| p.parent.map(|parent| (parent, p.id)))
			.collect()
	}
	pub fn route(&self, id: Id) -> Option<Route<Owner>> {
		self.contexts
			.iter()
			.chain(self.root.iter())
			.find(|p| p.id == id)
			.map(|p| p.route)
	}
	pub fn update(&mut self, event: Event<Owner>) -> Change<Owner> {
		let mut change = Change::default();
		match event {
			Event::Toggle { id, owner } => {
				let same = self.root.is_some_and(|p| p.route.owner == owner);
				self.close_all(&mut change.effects, true);
				if !same {
					self.open(id, None, owner, &mut change);
				}
			}
			Event::Open { id, parent, owner } => {
				self.close_all(&mut change.effects, true);
				self.open(id, parent, owner, &mut change);
			}
			Event::Context { id, parent, owner } => {
				let Some(route) = self.route(parent).filter(|r| r.owner == owner) else {
					return change;
				};
				let depth = route.depth.map_or(0, |d| d + 1);
				self.close_contexts(depth, &mut change.effects);
				let route = Route {
					owner,
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
				if let Some(depth) = self.contexts.iter().position(|p| p.id == id) {
					self.close_contexts(depth + 1, &mut change.effects);
					self.contexts.remove(depth);
				} else if self.root.is_some_and(|p| p.id == id) {
					self.close_all(&mut change.effects, false);
				} else if self.root.is_some_and(|p| p.parent == Some(id)) {
					self.close_all(&mut change.effects, true);
				}
			}
			Event::Unfocused(id) => {
				if let Some(depth) = self.contexts.iter().position(|p| p.id == id) {
					if depth + 1 == self.contexts.len() {
						self.close_contexts(depth, &mut change.effects);
					}
				} else if self.contexts.is_empty() && self.root.is_some_and(|p| p.id == id) {
					self.close_all(&mut change.effects, true);
				}
			}
			Event::CloseAll => self.close_all(&mut change.effects, true),
			Event::CloseContexts => self.close_contexts(0, &mut change.effects),
			Event::CloseDescendants {
				parent,
				expected_child,
			} => {
				if self
					.root
					.is_some_and(|p| p.parent == Some(parent) && p.id == expected_child)
				{
					self.close_all(&mut change.effects, true);
				} else if let Some(depth) = self
					.contexts
					.iter()
					.position(|p| p.parent == Some(parent) && p.id == expected_child)
				{
					self.close_contexts(depth, &mut change.effects);
				}
			}
		}
		change
	}
	fn open(&mut self, id: Id, parent: Option<Id>, owner: Owner, change: &mut Change<Owner>) {
		let route = Route { owner, depth: None };
		self.root = Some(Popup { id, parent, route });
		change.opened = Some(route);
	}
	fn close_contexts(&mut self, depth: usize, effects: &mut Vec<Effect<Owner>>) {
		effects.extend(
			self.contexts
				.drain(depth..)
				.rev()
				.map(|p| Effect::Close(p.id)),
		);
	}
	fn close_all(&mut self, effects: &mut Vec<Effect<Owner>>, close_root: bool) {
		if let Some(popup) = self.root.take() {
			self.close_contexts(0, effects);
			if close_root {
				effects.push(Effect::Close(popup.id));
			}
			effects.push(Effect::NotifyClosed(popup.route.owner));
		}
	}
}

/// Observe parent presses even if a widget captured them. Carry the expected
/// child so a queued click cannot close a replacement popup.
pub fn parent_clicks<Owner: Copy + Eq + Send + 'static>(
	edges: Vec<(Id, Id)>,
) -> iced::Subscription<Event<Owner>> {
	iced::Subscription::batch(edges.into_iter().map(|(parent, child)| {
		iced::event::listen_with(|event, _, id| match event {
			iced::Event::Mouse(iced::mouse::Event::ButtonPressed(_)) => Some(id),
			_ => None,
		})
		.with((parent, child))
		.filter_map(|((parent, expected_child), source)| {
			(source == parent).then_some(Event::CloseDescendants {
				parent,
				expected_child,
			})
		})
	}))
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn host_close_and_stale_clicks_respect_application_ancestry() {
		let (host, root, child) = (Id::unique(), Id::unique(), Id::unique());
		let mut p = Popups::default();
		p.update(Event::Open {
			id: root,
			parent: Some(host),
			owner: 7,
		});
		p.update(Event::Context {
			id: child,
			parent: root,
			owner: 7,
		});
		assert_eq!(p.context_children(), vec![(host, root), (root, child)]);
		assert!(p.update(Event::Unfocused(root)).effects.is_empty());
		assert!(
			p.update(Event::CloseDescendants {
				parent: host,
				expected_child: Id::unique()
			})
			.effects
			.is_empty()
		);
		assert_eq!(
			p.update(Event::Closed(host)).effects,
			vec![
				Effect::Close(child),
				Effect::Close(root),
				Effect::NotifyClosed(7)
			]
		);
		assert!(p.update(Event::Closed(root)).effects.is_empty());
	}
}
