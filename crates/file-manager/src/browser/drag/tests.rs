#![allow(clippy::unwrap_used)]

use super::*;
use crate::{Args, filesystem::entry, test_support::TestDir};
use std::{collections::BTreeSet, fs, path::Path};

fn fixture() -> (TestDir, FileManager) {
	let root = TestDir::new();
	fs::write(root.path().join("first"), "first source").unwrap();
	fs::write(root.path().join("second"), "second source").unwrap();
	fs::create_dir(root.path().join("destination")).unwrap();
	fs::create_dir(root.path().join("other")).unwrap();

	// Dropping these tasks must not poll configuration, D-Bus, or engine work.
	let (mut app, _) = FileManager::new(Args {
		path: root.path().into(),
	});
	load_listing(&mut app);
	app.transfers.loaded = true;
	(root, app)
}

fn load_listing(app: &mut FileManager) {
	let entries = entry::read_directory(&app.path);
	let _ = app.update(BrowserMessage::Loaded(app.request_id, entries));
	assert!(!app.loading);
	assert!(app.error.is_none(), "{:?}", app.error);
}

fn select_pair(app: &mut FileManager, root: &Path) -> BTreeSet<PathBuf> {
	let first = root.join("first");
	let second = root.join("second");
	let _ = app.update(BrowserMessage::Clicked(first));
	let _ = app.update(BrowserMessage::Modifiers(keyboard::Modifiers::CTRL));
	let _ = app.update(BrowserMessage::Clicked(second));
	let _ = app.update(BrowserMessage::Modifiers(keyboard::Modifiers::empty()));
	let expected = [root.join("first"), root.join("second")].into();
	assert_eq!(app.selection.paths, expected);
	expected
}

fn target_hit(path: &Path, navigation: bool) -> Hit {
	Hit {
		target: Some(path.into()),
		navigation,
		..Hit::default()
	}
}

fn start_drag(app: &mut FileManager, source: &Path, destination: &Path) {
	let _ = app.update(Message::Pressed(source.into(), Point::ORIGIN));
	let _ = app.update(Message::Motion(
		Point::new(6.0, 0.0),
		target_hit(destination, false),
	));
	assert!(app.drag.active);
	assert!(app.drag.valid);
	assert!(app.drag.guard.is_none());
}

#[test]
fn selected_member_press_preserves_multiple_sources_through_drag_threshold() {
	let (root, mut app) = fixture();
	let expected = select_pair(&mut app, root.path());
	let first = root.path().join("first");
	let sources: Vec<_> = expected.iter().cloned().collect();

	let _ = app.update(Message::Pressed(first, Point::ORIGIN));
	assert_eq!(app.selection.paths, expected);
	assert_eq!(app.drag.sources, sources);
	assert!(!app.drag.origin.as_ref().unwrap().applied);
	assert!(!app.drag.active);

	let _ = app.update(Message::Motion(Point::new(5.0, 0.0), Hit::default()));
	assert!(!app.drag.active);
	assert_eq!(app.selection.paths, expected);

	let _ = app.update(Message::Motion(Point::new(6.0, 0.0), Hit::default()));
	assert!(app.drag.active);
	assert_eq!(app.selection.paths, expected);
	assert_eq!(app.drag.sources, sources);
}

#[test]
fn control_click_toggles_membership_once_on_press_not_again_on_release() {
	let (root, mut app) = fixture();
	let first = root.path().join("first");
	let second = root.path().join("second");
	let _ = app.update(BrowserMessage::Clicked(first.clone()));
	let _ = app.update(BrowserMessage::Modifiers(keyboard::Modifiers::CTRL));

	for selected in [true, false] {
		let _ = app.update(Message::Pressed(second.clone(), Point::ORIGIN));
		assert_eq!(app.selection.paths.contains(&second), selected);
		assert!(app.selection.paths.contains(&first));
		assert!(app.drag.origin.as_ref().unwrap().applied);
		let after_press = app.selection.paths.clone();

		let _ = app.update(Message::Released(Point::ORIGIN, Hit::default()));
		assert_eq!(app.selection.paths, after_press);
		assert!(!app.drag.active);
		assert!(app.drag.origin.is_none());
		assert_eq!(app.path, root.path());
	}
}

#[test]
fn hover_and_explicit_navigation_clear_selection_without_replacing_drag_sources() {
	let (root, mut app) = fixture();
	let expected = select_pair(&mut app, root.path());
	let sources: Vec<_> = expected.iter().cloned().collect();
	let destination = root.path().join("destination");
	start_drag(&mut app, &root.path().join("first"), &destination);
	let generation = app.drag.generation;

	let _ = app.update(Message::Motion(
		Point::new(7.0, 0.0),
		target_hit(&destination, true),
	));
	let since = app.drag.hover.as_ref().unwrap().1;
	let _ = app.update(Message::Tick(since + Duration::from_millis(649)));
	assert_eq!(app.path, root.path());
	assert_eq!(app.selection.paths, expected);

	let _ = app.update(Message::Tick(since + Duration::from_millis(650)));
	assert_eq!(app.path, destination);
	assert!(app.selection.paths.is_empty());
	assert!(app.selected.is_none());
	assert!(app.drag.active);
	assert_eq!(app.drag.sources, sources);
	assert_eq!(app.drag.generation, generation);
	load_listing(&mut app);

	let other = root.path().join("other");
	let _ = app.update(BrowserMessage::GoTo(other.clone()));
	load_listing(&mut app);
	assert_eq!(app.path, other);
	assert!(app.selection.paths.is_empty());
	assert!(app.drag.active);
	assert_eq!(app.drag.sources, sources);
	assert_eq!(app.drag.generation, generation);

	let mut guard = Target::capture(&sources[0], Context::File).unwrap();
	guard.others = vec![Target::capture(&sources[1], Context::File).unwrap()];
	let _ = app.update(Message::Prepared(generation, Ok(guard)));
	assert_eq!(app.drag.guard.as_ref().unwrap().paths(), sources);
}

#[test]
fn prepared_success_and_failure_after_cancel_are_ignored() {
	let (root, mut app) = fixture();
	let source = root.path().join("first");
	let destination = root.path().join("destination");
	start_drag(&mut app, &source, &destination);
	let generation = app.drag.generation;
	let _ = app.update(Message::Released(
		Point::new(6.0, 0.0),
		target_hit(&destination, false),
	));
	assert!(app.drag.pending.is_some());

	let _ = app.update(Message::Cancel);
	let cancelled_generation = app.drag.generation;
	assert!(cancelled_generation > generation);
	let guard = Target::capture(&source, Context::File).unwrap();
	let _ = app.update(Message::Prepared(generation, Ok(guard)));
	let _ = app.update(Message::Prepared(
		generation,
		Err("stale preparation failure".into()),
	));

	assert_eq!(app.drag.generation, cancelled_generation);
	assert!(!app.drag.active);
	assert!(app.drag.origin.is_none());
	assert!(app.drag.guard.is_none());
	assert!(app.drag.pending.is_none());
	assert!(app.drag.target.is_none());
	assert!(app.error.is_none());
	assert!(!app.transfers.busy);
	assert_eq!(app.transfers.generation, 0);
}

#[test]
fn release_before_prepared_freezes_destination_and_release_modifiers() {
	let cases = [
		(
			keyboard::Modifiers::CTRL,
			keyboard::Modifiers::SHIFT,
			Operation::Copy,
		),
		(
			keyboard::Modifiers::SHIFT,
			keyboard::Modifiers::CTRL,
			Operation::Move,
		),
		(
			keyboard::Modifiers::empty(),
			keyboard::Modifiers::CTRL,
			Operation::Automatic,
		),
		(
			keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT,
			keyboard::Modifiers::empty(),
			Operation::Copy,
		),
	];

	for (released_modifiers, later_modifiers, operation) in cases {
		let (root, mut app) = fixture();
		let source = root.path().join("first");
		let destination = root.path().join("destination");
		start_drag(&mut app, &source, &destination);
		let generation = app.drag.generation;
		let _ = app.update(BrowserMessage::Modifiers(released_modifiers));

		let _ = app.update(Message::Released(
			Point::new(6.0, 0.0),
			target_hit(&destination, false),
		));
		assert!(!app.drag.active);
		assert!(app.drag.origin.is_none());
		assert!(app.drag.guard.is_none());
		assert_eq!(app.drag.pending, Some((destination.clone(), operation)));
		assert!(!app.transfers.busy);

		let _ = app.update(BrowserMessage::Modifiers(later_modifiers));
		let _ = app.update(BrowserMessage::GoTo(root.path().join("other")));
		assert_eq!(app.drag.pending, Some((destination.clone(), operation)));
		assert_eq!(app.drag.sources, vec![source.clone()]);

		let guard = Target::capture(&source, Context::File).unwrap();
		let _ = app.update(Message::Prepared(generation, Ok(guard)));
		assert!(app.drag.pending.is_none());
		assert!(app.drag.guard.is_none());
		assert!(app.transfers.busy);
		assert_eq!(app.transfers.generation, 1);
		assert_eq!(fs::read(&source).unwrap(), b"first source");
		assert!(!destination.join("first").exists());
	}
}

#[test]
fn release_below_threshold_collapses_selection_as_an_ordinary_click() {
	let (root, mut app) = fixture();
	let expected = select_pair(&mut app, root.path());
	let first = root.path().join("first");
	let _ = app.update(Message::Pressed(first.clone(), Point::ORIGIN));
	let _ = app.update(Message::Motion(Point::new(5.0, 0.0), Hit::default()));
	assert_eq!(app.selection.paths, expected);

	let _ = app.update(Message::Released(Point::new(5.0, 0.0), Hit::default()));
	assert_eq!(app.selection.paths, [first.clone()].into());
	assert_eq!(app.selected, Some(first.clone()));
	assert_eq!(
		app.drag.last_click.as_ref().map(|(path, _)| path),
		Some(&first)
	);
	assert!(app.drag.origin.is_none());
	assert!(!app.drag.active);
	assert!(!app.transfers.busy);
	assert_eq!(app.path, root.path());
}

#[test]
fn two_non_drag_clicks_open_a_directory_on_the_second_release() {
	let (root, mut app) = fixture();
	let directory = root.path().join("destination");
	let _ = app.update(Message::Pressed(directory.clone(), Point::ORIGIN));
	assert_eq!(app.selection.paths, [directory.clone()].into());
	let _ = app.update(Message::Released(Point::ORIGIN, Hit::default()));
	assert_eq!(app.path, root.path());
	assert_eq!(
		app.drag.last_click.as_ref().map(|(path, _)| path),
		Some(&directory)
	);

	// A future timestamp makes elapsed saturate at zero, without a timed sleep
	// or depending on how quickly the test thread receives its next timeslice.
	app.drag.last_click.as_mut().unwrap().1 = Instant::now() + Duration::from_secs(3600);
	let _ = app.update(Message::Pressed(directory.clone(), Point::ORIGIN));
	assert_eq!(app.path, root.path());
	let _ = app.update(Message::Released(Point::ORIGIN, Hit::default()));

	assert_eq!(app.path, directory);
	assert!(app.loading);
	assert!(app.selection.paths.is_empty());
	assert!(app.drag.last_click.is_none());
	assert!(app.drag.origin.is_none());
	assert!(!app.drag.active);
	assert!(!app.transfers.busy);
}
