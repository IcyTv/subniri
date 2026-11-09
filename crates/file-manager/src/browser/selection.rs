//! Selection membership is independent of the keyboard cursor (`FileManager::selected`).
use iced::keyboard::Modifiers;
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Default)]
pub struct Selection {
	pub paths: BTreeSet<PathBuf>,
	anchor: Option<PathBuf>,
	base: BTreeSet<PathBuf>,
	extending: bool,
}

impl Selection {
	pub fn only(&mut self, path: PathBuf) {
		self.paths = [path.clone()].into();
		self.anchor = Some(path);
		self.base = self.paths.clone();
		self.extending = false;
	}

	pub fn clear(&mut self) {
		*self = Self::default();
	}

	pub fn apply(
		&mut self, order: &[PathBuf], cursor: Option<&PathBuf>, target: PathBuf,
		modifiers: Modifiers, pointer: bool,
	) {
		if modifiers.shift() {
			if !self.extending {
				self.base = self.paths.clone();
			}
			self.extending = true;
			let anchor = self
				.anchor
				.clone()
				.or_else(|| cursor.cloned())
				.unwrap_or_else(|| target.clone());
			let start = order.iter().position(|p| p == &anchor);
			let end = order.iter().position(|p| p == &target);
			if let (Some(start), Some(end)) = (start, end) {
				self.anchor = Some(anchor);
				self.paths = if modifiers.control() {
					self.base.clone()
				} else {
					BTreeSet::new()
				};
				self.paths
					.extend(order[start.min(end)..=start.max(end)].iter().cloned());
			}
		} else if modifiers.control() {
			self.extending = false;
			if pointer {
				if !self.paths.remove(&target) {
					self.paths.insert(target.clone());
				}
				self.anchor = Some(target);
				self.base = self.paths.clone();
			}
		} else {
			self.only(target);
		}
	}

	pub fn end_range(&mut self) {
		self.extending = false;
	}

	pub fn retain(&mut self, order: &[PathBuf]) {
		self.paths.retain(|p| order.contains(p));
		self.base.retain(|p| order.contains(p));
		if self.anchor.as_ref().is_some_and(|p| !order.contains(p)) {
			self.anchor = None;
		}
	}
}

impl crate::FileManager {
	pub(crate) fn select_only(&mut self, path: PathBuf) {
		self.selection.only(path.clone());
		self.selected = Some(path);
		self.pending_selection = None;
	}

	pub(super) fn apply_selection(&mut self, path: PathBuf, modifiers: Modifiers, pointer: bool) {
		let order = self
			.visible_entries()
			.iter()
			.map(|e| e.path.clone())
			.collect::<Vec<_>>();
		if !order.contains(&path) {
			return;
		}
		self.selection.apply(
			&order,
			self.selected.as_ref(),
			path.clone(),
			modifiers,
			pointer,
		);
		self.selected = Some(path);
		self.pending_selection = None;
	}

	pub(super) fn selection_key(
		&mut self, key: iced::keyboard::Key, modifiers: Modifiers, focused: Option<PathBuf>,
	) -> iced::Task<crate::Message> {
		use iced::keyboard::{Key, key::Named};
		self.typeahead.clear();
		if let Some(path) = focused {
			self.selected = Some(path);
		}
		match key.as_ref() {
			Key::Character("a" | "A") if modifiers.control() => {
				self.selection.paths = self
					.visible_entries()
					.iter()
					.map(|e| e.path.clone())
					.collect();
				if self.selected.is_none() {
					self.selected = self.visible_entries().first().map(|e| e.path.clone());
				}
				self.selection.end_range();
				return self.reveal_selection();
			}
			Key::Named(Named::Space) => {
				if let Some(path) = self.selected.clone() {
					self.apply_selection(path, modifiers, true);
				}
			}
			Key::Named(Named::Enter) => return self.open_selected(),
			Key::Named(Named::ContextMenu | Named::F10) => {
				return self.keyboard_menu();
			}
			_ => {}
		}
		iced::Task::none()
	}
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
	use super::*;
	use crate::{
		Args, FileManager,
		browser::Message,
		context_menu::{Action, Command},
		filesystem::entry,
		test_support::TestDir,
	};
	use iced::{
		Point,
		keyboard::{Key, key::Named},
	};

	fn app() -> (TestDir, FileManager, Vec<PathBuf>) {
		let dir = TestDir::new();
		for name in ["a", "b", "c", "d", "e", "f"] {
			std::fs::create_dir(dir.path().join(name)).unwrap();
		}
		let (mut app, _) = FileManager::new(Args {
			path: dir.path().into(),
		});
		let _ = app.update(Message::Loaded(
			app.request_id,
			entry::read_directory(dir.path()),
		));
		let paths = app
			.visible_entries()
			.iter()
			.map(|e| e.path.clone())
			.collect();
		(dir, app, paths)
	}

	fn click(app: &mut FileManager, path: &PathBuf, modifiers: Modifiers) {
		let _ = app.update(Message::Modifiers(modifiers));
		let _ = app.update(Message::Clicked(path.clone()));
	}
	fn members(app: &FileManager, paths: &[PathBuf]) {
		assert_eq!(app.selection.paths, paths.iter().cloned().collect());
	}

	#[test]
	fn clicks_toggle_and_shift_ranges_shrink_and_reverse_around_a_stable_anchor() {
		let (_dir, mut app, p) = app();
		click(&mut app, &p[2], Modifiers::empty());
		click(&mut app, &p[5], Modifiers::SHIFT);
		members(&app, &p[2..=5]);
		click(&mut app, &p[3], Modifiers::SHIFT);
		members(&app, &p[2..=3]);
		click(&mut app, &p[0], Modifiers::SHIFT);
		members(&app, &p[..=2]);
		click(&mut app, &p[1], Modifiers::CTRL);
		members(&app, &[p[0].clone(), p[2].clone()]);
		assert_eq!(
			app.selected,
			Some(p[1].clone()),
			"deselected item retains cursor"
		);
		click(&mut app, &p[4], Modifiers::empty());
		members(&app, &p[4..=4]);
	}

	#[test]
	fn additive_ranges_preserve_other_members_and_shrink_without_leaving_a_trail() {
		let (_dir, mut app, p) = app();
		click(&mut app, &p[0], Modifiers::empty());
		click(&mut app, &p[3], Modifiers::CTRL);
		click(&mut app, &p[5], Modifiers::CTRL | Modifiers::SHIFT);
		members(
			&app,
			&[p[0].clone(), p[3].clone(), p[4].clone(), p[5].clone()],
		);
		click(&mut app, &p[4], Modifiers::CTRL | Modifiers::SHIFT);
		members(&app, &[p[0].clone(), p[3].clone(), p[4].clone()]);
	}

	#[test]
	fn keyboard_cursor_and_space_are_independent_of_membership() {
		let (_dir, mut app, p) = app();
		click(&mut app, &p[0], Modifiers::empty());
		let _ = app.update(Message::SelectionMove(p[3].clone(), Modifiers::CTRL));
		members(&app, &p[..=0]);
		assert_eq!(app.selected, Some(p[3].clone()));
		let _ = app.update(Message::SelectionKey(
			Key::Named(Named::Space),
			Modifiers::CTRL,
			Some(p[3].clone()),
		));
		members(&app, &[p[0].clone(), p[3].clone()]);
		let _ = app.update(Message::SelectionKey(
			Key::Named(Named::Space),
			Modifiers::CTRL,
			Some(p[3].clone()),
		));
		members(&app, &p[..=0]);
		// The only selected directory opens, even with the cursor elsewhere.
		let _ = app.update(Message::OpenSelected);
		assert_eq!(app.path, p[0]);
	}

	#[test]
	fn select_all_uses_visible_order_and_refresh_filtering_reconcile_membership() {
		let (dir, mut app, p) = app();
		let _ = app.update(Message::SelectionKey(
			Key::Character("a".into()),
			Modifiers::CTRL,
			None,
		));
		members(&app, &p);
		let _ = app.update(Message::Sort);
		members(&app, &p);
		std::fs::remove_dir(&p[1]).unwrap();
		let _ = app.update(Message::Loaded(
			app.request_id,
			entry::read_directory(dir.path()),
		));
		assert!(!app.selection.paths.contains(&p[1]));
		assert_eq!(app.selection.paths.len(), 5);
		let _ = app.update(Message::Search("c".into()));
		members(&app, &p[2..=2]);
		let _ = app.update(Message::SelectionKey(
			Key::Character("a".into()),
			Modifiers::CTRL,
			None,
		));
		members(&app, &p[2..=2]);
	}

	#[test]
	fn ranges_follow_display_sort_not_storage_order() {
		let (_dir, mut app, p) = app();
		click(&mut app, &p[4], Modifiers::empty());
		let _ = app.update(Message::Sort);
		click(&mut app, &p[1], Modifiers::SHIFT);
		members(&app, &p[1..=4]);
	}

	#[test]
	fn escape_clears_selection_but_not_filters_and_typeahead_dismissal_takes_priority() {
		let (_dir, mut app, p) = app();
		let _ = app.update(Message::Search("a".into()));
		click(&mut app, &p[0], Modifiers::empty());
		let _ = app.update(Message::TypeAhead(crate::browser::typeahead::Edit::Append(
			"a".into(),
		)));
		let escape = crate::browser::typeahead::tests::key(
			Key::Named(Named::Escape),
			None,
			Modifiers::empty(),
		);
		let _ = app.update(Message::Keyboard(escape.clone()));
		members(&app, &p[..=0]);
		let _ = app.update(Message::Keyboard(escape));
		members(&app, &[]);
		assert_eq!(app.search, "a");
		assert!(app.pending_selection.is_none());
	}

	#[test]
	fn context_menu_preserves_groups_background_and_dismissal_but_replaces_for_unselected_item() {
		let (_dir, mut app, p) = app();
		click(&mut app, &p[0], Modifiers::empty());
		click(&mut app, &p[2], Modifiers::CTRL);
		let _ = app.open_menu(Some(p[0].clone()), Point::ORIGIN);
		members(&app, &[p[0].clone(), p[2].clone()]);
		let menu = app.menu.as_ref().unwrap();
		assert_eq!(menu.target.others.len(), 1);
		assert_eq!(menu.target.others[0].path, p[2]);
		let rows = app.menu_rows(menu.pages[0].id);
		assert!(rows.iter().all(|row| row.disabled.is_none()));
		assert!(
			!rows
				.iter()
				.any(|row| row.action == Action::Command(Command::Open))
		);
		for command in [
			Command::Cut,
			Command::Copy,
			Command::Duplicate,
			Command::Rename,
			Command::Trash,
			Command::Properties,
		] {
			assert!(
				rows.iter()
					.any(|row| row.action == Action::Command(command)),
				"missing {command:?}"
			);
		}
		let _ = app.menu_lifecycle(neo_widgets::popup::Event::CloseAll);
		members(&app, &[p[0].clone(), p[2].clone()]);
		let _ = app.open_menu(None, Point::ORIGIN);
		members(&app, &[p[0].clone(), p[2].clone()]);
		assert!(app.menu.as_ref().unwrap().target.others.is_empty());
		let _ = app.open_menu(Some(p[4].clone()), Point::ORIGIN);
		members(&app, &p[4..=4]);
	}

	#[test]
	fn context_snapshot_covers_every_identity_and_dispatch_does_not_use_live_selection() {
		let (_dir, mut app, p) = app();
		click(&mut app, &p[0], Modifiers::empty());
		click(&mut app, &p[2], Modifiers::CTRL);
		let _ = app.open_menu(Some(p[0].clone()), Point::ORIGIN);
		let snapshot = app.menu.as_ref().unwrap().target.clone();
		click(&mut app, &p[4], Modifiers::empty());
		let _ = app.menu_dispatch(snapshot.clone(), Action::Command(Command::Properties));
		members(&app, &[p[0].clone(), p[2].clone()]);
		assert!(app.show_info);
		std::fs::rename(&p[2], p[2].with_extension("old")).unwrap();
		std::fs::create_dir(&p[2]).unwrap();
		assert!(!snapshot.available());
		let _ = app.menu_dispatch(snapshot, Action::Command(Command::Open));
		assert_eq!(app.error.as_deref(), Some("Item no longer available"));
	}

	#[test]
	fn multiple_selection_blocks_open_but_edit_stubs_leave_every_target_untouched() {
		let (_dir, mut app, p) = app();
		click(&mut app, &p[0], Modifiers::empty());
		click(&mut app, &p[2], Modifiers::CTRL);
		assert_eq!(app.update(Message::OpenSelected).units(), 0);
		assert_eq!(
			app.update(Message::SelectionKey(
				Key::Named(Named::Enter),
				Modifiers::empty(),
				Some(p[0].clone())
			))
			.units(),
			0
		);
		let _ = app.open_menu(Some(p[0].clone()), Point::ORIGIN);
		let snapshot = app.menu.as_ref().unwrap().target.clone();
		for command in [
			Command::Cut,
			Command::Copy,
			Command::Duplicate,
			Command::Rename,
		] {
			assert_eq!(
				app.menu_dispatch(snapshot.clone(), Action::Command(command))
					.units(),
				0
			);
			assert!(app.error.is_none(), "stub {command:?} was rejected");
			assert!(snapshot.available());
			members(&app, &[p[0].clone(), p[2].clone()]);
		}
		assert_eq!(app.visible_entries().len(), 6);
		assert_eq!(
			app.menu_dispatch(snapshot, Action::Command(Command::Open))
				.units(),
			0
		);
		assert_eq!(
			app.error.as_deref(),
			Some("Not available for multiple items")
		);
	}

	#[test]
	fn clear_and_navigation_drop_old_membership_and_range_anchor() {
		let (_dir, mut app, p) = app();
		click(&mut app, &p[0], Modifiers::empty());
		click(&mut app, &p[3], Modifiers::SHIFT);
		let _ = app.update(Message::ClearSelection);
		members(&app, &[]);
		click(&mut app, &p[4], Modifiers::SHIFT);
		members(&app, &p[4..=4]);
		let _ = app.update(Message::GoTo(p[4].clone()));
		members(&app, &[]);
		assert!(app.selection.anchor.is_none());
	}
}
