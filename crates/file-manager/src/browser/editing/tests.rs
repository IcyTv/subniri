#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use super::*;
use crate::{Args, ViewMode, test_support::TestDir};

fn app(names: &[&str]) -> (TestDir, FileManager) {
	let dir = TestDir::new();
	for name in names {
		std::fs::write(dir.path().join(name), "keep").unwrap();
	}

	let (mut app, _) = FileManager::new(Args {
		path: dir.path().into(),
	});
	load(&mut app);

	(dir, app)
}

fn load(app: &mut FileManager) {
	let _ = app.update(Message::Loaded(
		app.request_id,
		entry::read_directory(&app.path),
	));
}

// Execute the same worker operation synchronously, then deliver its completion.
// State-machine tests deliberately avoid running unrelated app startup tasks.
fn complete(app: &mut FileManager) {
	let editor = app.editing.editor.as_ref().unwrap();
	let result = editor.operation.run(&editor.name);

	let _ = app.update(Message::EditFinished(result));
}

#[test]
fn filename_selection_keeps_extensions_and_utf8_boundaries() {
	for (name, folder, selected) in [
		("notes.txt", false, "notes"),
		("archive.tar.gz", false, "archive.tar"),
		("日本語.txt", false, "日本語"),
		(".bashrc", false, ".bashrc"),
		(".config.json", false, ".config.json"),
		("README", false, "README"),
		("folder.ext", true, "folder.ext"),
	] {
		assert_eq!(&name[..selection_end(name, folder)], selected);
	}
}

#[test]
fn drafts_are_unique_sorted_pinned_and_never_create_before_save() {
	for mode in [ViewMode::Grid, ViewMode::List] {
		for ascending in [true, false] {
			let (dir, mut app) = app(&["A", "New File", "Z"]);
			app.view_mode = mode;
			app.ascending = ascending;

			let _ = app.update(Message::NewFile);

			let editor = app.editing.editor.as_ref().unwrap();
			assert_eq!(editor.name, "New File (2)");
			let position = editor.position;
			assert_eq!(app.visible_entries()[position].name, "New File (2)");
			assert!(!editor.entry.path.exists());

			let _ = app.update(Message::EditName("000.txt".into()));

			assert_eq!(app.visible_entries()[position].name, "New File (2)");

			let _ = app.update(Message::CancelEdit);

			assert!(app.editing.editor.is_none());
			assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
		}
	}
}

#[test]
fn creation_draft_bypasses_search_and_tag_filters_without_clearing_them() {
	let (_dir, mut app) = app(&["existing"]);
	app.search = "unmatched".into();
	app.active_tag = Some(123);

	let _ = app.update(Message::NewFolder);

	assert_eq!(app.visible_entries().len(), 1);
	assert_eq!(app.visible_entries()[0].name, "New Folder");
	assert!(app.visible_entries()[0].is_dir);
	assert_eq!(app.search, "unmatched");
	assert_eq!(app.active_tag, Some(123));
}

#[test]
fn empty_file_and_folder_save_select_reveal_and_sort_without_opening() {
	for folder in [false, true] {
		let (dir, mut app) = app(&["A", "Z"]);

		let _ = app.update(if folder {
			Message::NewFolder
		} else {
			Message::NewFile
		});
		let _ = app.update(Message::EditName("Saved".into()));
		let _ = app.update(Message::SaveEdit);

		assert!(app.editing.busy);
		assert_eq!(
			app.update(Message::SaveEdit).units(),
			0,
			"duplicate submit starts no worker"
		);

		complete(&mut app);
		load(&mut app);

		assert_eq!(app.path, dir.path());
		assert_eq!(app.selected, Some(dir.path().join("Saved")));
		assert_eq!(dir.path().join("Saved").is_dir(), folder);
		assert_eq!(
			app.visible_entries()
				.iter()
				.map(|e| e.name.as_str())
				.collect::<Vec<_>>(),
			if folder {
				vec!["Saved", "A", "Z"]
			} else {
				vec!["A", "Saved", "Z"]
			}
		);
		assert!(app.editing.editor.is_none());
	}
}

#[test]
fn invalid_and_collision_errors_keep_text_and_block_navigation() {
	let (dir, mut app) = app(&["taken"]);

	let _ = app.update(Message::NewFile);
	let _ = app.update(Message::EditName("../outside".into()));
	let _ = app.update(Message::GoUp);

	assert!(!app.editing.busy);
	assert!(app.editing.pending.is_none());
	assert_eq!(app.path, dir.path());
	assert!(app.editing.editor.as_ref().unwrap().error.is_some());

	let _ = app.update(Message::EditName("taken".into()));
	let _ = app.update(Message::GoUp);

	assert!(app.editing.busy);

	complete(&mut app);

	assert_eq!(app.path, dir.path());
	assert!(!app.editing.busy);
	assert!(app.editing.pending.is_none());
	let editor = app.editing.editor.as_ref().unwrap();
	assert_eq!(editor.name, "taken");
	assert!(editor.error.is_some());
	assert_eq!(
		std::fs::read_to_string(dir.path().join("taken")).unwrap(),
		"keep"
	);
}

#[test]
fn navigation_waits_for_success_and_listing_before_mutating_history() {
	let (dir, mut app) = app(&[]);
	let child = dir.path().join("child");
	std::fs::create_dir(&child).unwrap();
	load(&mut app);

	let _ = app.update(Message::GoTo(child.clone()));
	load(&mut app);

	let _ = app.update(Message::NewFolder);
	let _ = app.update(Message::GoBack);

	assert_eq!(app.path, child);

	complete(&mut app);

	assert_eq!(app.path, child);

	load(&mut app);

	assert_eq!(app.path, dir.path());
	assert!(child.join("New Folder").is_dir());

	let _ = app.update(Message::GoForward);

	assert_eq!(app.path, child);
}

#[test]
fn latest_navigation_wins_while_save_and_reload_are_pending() {
	let (dir, mut app) = app(&[]);
	let first = dir.path().join("first");
	let last = dir.path().join("last");
	std::fs::create_dir(&first).unwrap();
	std::fs::create_dir(&last).unwrap();
	load(&mut app);

	let _ = app.update(Message::NewFile);
	let _ = app.update(Message::ClearSelection);
	let _ = app.update(Message::GoTo(first));

	assert!(app.editing.busy);

	complete(&mut app);
	let _ = app.update(Message::GoTo(last.clone()));

	assert_eq!(
		app.path,
		dir.path(),
		"navigation must still wait for the saved listing"
	);

	load(&mut app);

	assert_eq!(app.path, last);
	assert!(app.editing.pending.is_none());
	assert!(dir.path().join("New File").is_file());
}

#[test]
fn rename_retains_original_until_save_and_can_open_the_renamed_folder() {
	let (dir, mut app) = app(&[]);
	let folder = dir.path().join("old");
	std::fs::create_dir(&folder).unwrap();
	load(&mut app);

	let _ = app.update(Message::Clicked(folder.clone()));
	let _ = app.update(Message::Rename);
	let _ = app.update(Message::EditName("new".into()));

	assert!(folder.is_dir());

	let _ = app.update(Message::OpenEntry(folder.clone()));
	complete(&mut app);
	load(&mut app);

	assert_eq!(app.path, dir.path().join("new"));
	assert!(!folder.exists());
}

#[test]
fn page_switches_save_first_and_close_discards_unsubmitted_text() {
	let (dir, mut app) = app(&[]);

	let _ = app.update(Message::NewFile);
	let _ = app.update(crate::transfers::Message::ShowHistory(true));

	assert_eq!(app.page, super::super::Page::Files);

	complete(&mut app);
	load(&mut app);

	assert_eq!(app.page, super::super::Page::History);
	assert!(dir.path().join("New File").is_file());

	let _ = app.update(crate::transfers::Message::ShowHistory(false));
	let _ = app.update(Message::NewFile);
	let _ = app.update(Message::EditName("discard".into()));
	let _ = app.update(crate::Message::CloseRequested(app.host));

	assert!(app.editing.editor.is_none());
	assert!(!dir.path().join("discard").exists());
}

#[test]
fn hidden_result_toast_preserves_filters_and_show_item_clears_only_blockers() {
	let (dir, mut app) = app(&[]);
	app.search = "other".into();
	app.active_tag = Some(999);

	let _ = app.update(Message::NewFile);
	let _ = app.update(Message::SaveEdit);
	complete(&mut app);
	load(&mut app);

	assert_eq!(app.search, "other");
	assert_eq!(app.active_tag, Some(999));
	assert!(app.visible_entries().is_empty());
	assert!(
		app.editing
			.toast
			.as_ref()
			.unwrap()
			.text
			.contains("New File hidden because search")
	);

	// User changed search to a non-blocking filter before using the action.
	let _ = app.update(Message::Search("new".into()));
	let _ = app.update(Message::ShowEditedItem);

	assert_eq!(app.search, "new");
	assert!(app.active_tag.is_none());
	assert_eq!(app.selected, Some(dir.path().join("New File")));
	assert_eq!(app.visible_entries().len(), 1);
	assert!(app.editing.toast.is_none());
}

#[test]
fn toast_timeout_pauses_on_hover_and_navigation_dismisses_it() {
	let (_dir, mut app) = app(&[]);
	app.search = "hidden".into();

	let _ = app.update(Message::NewFolder);
	let _ = app.update(Message::SaveEdit);
	complete(&mut app);
	load(&mut app);

	let _ = app.update(Message::ToastHover(true));
	let now = app.editing.toast.as_ref().unwrap().last_tick;
	let _ = app.update(Message::ToastTick(now + Duration::from_secs(20)));

	assert_eq!(
		app.editing.toast.as_ref().unwrap().remaining,
		Duration::from_secs(8)
	);

	let _ = app.update(Message::ToastHover(false));
	let now = app.editing.toast.as_ref().unwrap().last_tick;
	let _ = app.update(Message::ToastTick(now + Duration::from_secs(9)));

	assert!(app.editing.toast.is_none());

	app.notify_hidden_edit(&app.path.join("New Folder"));
	let _ = app.update(Message::GoUp);

	assert!(app.editing.toast.is_none());
}

#[test]
fn renamed_items_hidden_by_search_also_get_actionable_toast() {
	let (dir, mut app) = app(&["old.txt"]);
	app.search = "old".into();

	let _ = app.update(Message::Clicked(dir.path().join("old.txt")));
	let _ = app.update(Message::Rename);
	let _ = app.update(Message::EditName("new.txt".into()));
	let _ = app.update(Message::SaveEdit);
	complete(&mut app);
	load(&mut app);

	assert_eq!(app.search, "old");
	assert!(app.visible_entries().is_empty());
	assert!(
		app.editing
			.toast
			.as_ref()
			.unwrap()
			.text
			.contains("new.txt hidden")
	);

	let _ = app.update(Message::ShowEditedItem);

	assert!(app.search.is_empty());
	assert_eq!(app.selected, Some(dir.path().join("new.txt")));
}
