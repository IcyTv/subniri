#![allow(clippy::unwrap_used)]

use super::*;
use crate::test_support::TestDir;
use std::os::unix::fs::symlink;

fn request(sources: Vec<PathBuf>, destination: &Path) -> Request {
	Request {
		sources,
		destination: destination.into(),
		operation: Operation::Copy,
		durability: Durability::Buffered,
		acknowledge_volatile: true,
	}
}

fn assert_forward_progress(events: &[Event], expected: u64) {
	assert!(matches!(
		events.first(),
		Some(Event::Progress {
			phase: Phase::Preparing,
			bytes: 0,
			total: None,
		})
	));

	let mut previous = 0;
	let mut known_total = false;
	for event in events {
		if let Event::Progress {
			phase,
			bytes,
			total,
		} = event
		{
			assert_ne!(*phase, Phase::RollingBack);
			assert!(*bytes >= previous);
			if let Some(total) = total {
				known_total = true;
				assert_eq!(*total, expected);
				assert!(*bytes <= *total);
			} else {
				assert!(!known_total);
			}
			previous = *bytes;
		}
	}
	assert!(known_total);
	assert_eq!(previous, expected);
}

#[test]
fn metadata_total_counts_flat_nested_and_multiple_roots_without_following_links() {
	let root = TestDir::new();
	let flat = root.path().join("flat");
	let tree = root.path().join("tree");
	let outside = root.path().join("outside");
	stdfs::write(&flat, b"flat").unwrap();
	stdfs::create_dir_all(tree.join("nested")).unwrap();
	stdfs::write(tree.join("one"), b"123").unwrap();
	stdfs::write(tree.join("nested/two"), b"12345").unwrap();
	stdfs::write(&outside, vec![1; 4096]).unwrap();
	symlink(&outside, tree.join("external")).unwrap();
	symlink(&tree, tree.join("cycle")).unwrap();
	symlink(root.path().join("missing"), tree.join("dangling")).unwrap();
	let cancelled = AtomicBool::new(false);

	assert_eq!(
		transfer_total(std::slice::from_ref(&flat), &cancelled).unwrap(),
		4
	);
	assert_eq!(
		transfer_total(std::slice::from_ref(&tree), &cancelled).unwrap(),
		8
	);
	assert_eq!(transfer_total(&[flat, tree], &cancelled).unwrap(), 12);
	assert_eq!(
		transfer_total(&[root.path().join("tree/external")], &cancelled).unwrap(),
		0
	);
}

#[test]
fn empty_trees_and_symlink_only_selection_have_known_zero_total() {
	let root = TestDir::new();
	let empty = root.path().join("empty");
	let link = root.path().join("link");
	let destination = root.path().join("destination");
	stdfs::create_dir(&empty).unwrap();
	stdfs::create_dir(&destination).unwrap();
	symlink(&empty, &link).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut events = Vec::new();

	let result = execute(
		&store,
		request(vec![empty, link], &destination),
		&AtomicBool::new(false),
		|_| Choice::Cancel,
		|event| events.push(event),
	)
	.unwrap();

	assert_eq!(result.status, Status::Complete, "{:?}", result.problems);
	assert_forward_progress(&events, 0);
	assert!(
		stdfs::symlink_metadata(destination.join("link"))
			.unwrap()
			.is_symlink()
	);
}

#[test]
fn copy_progress_is_cumulative_across_files_roots_and_directory_merges() {
	let root = TestDir::new();
	let tree = root.path().join("tree");
	let flat = root.path().join("flat");
	let destination = root.path().join("destination");
	stdfs::create_dir_all(tree.join("nested")).unwrap();
	stdfs::create_dir_all(destination.join("tree/nested")).unwrap();
	stdfs::write(tree.join("one"), vec![1; 150_003]).unwrap();
	stdfs::write(tree.join("nested/two"), vec![2; 170_009]).unwrap();
	stdfs::write(&flat, vec![3; 90_007]).unwrap();
	symlink("one", tree.join("link")).unwrap();
	let store = Store::open(root.path().join("state")).unwrap();
	let mut events = Vec::new();

	let result = execute(
		&store,
		request(vec![tree, flat], &destination),
		&AtomicBool::new(false),
		|conflict| {
			assert!(conflict.directories);
			Choice::Merge
		},
		|event| events.push(event),
	)
	.unwrap();

	assert_eq!(result.status, Status::Complete, "{:?}", result.problems);
	assert_forward_progress(&events, 410_019);
	assert!(events.iter().any(|event| matches!(
		event,
		Event::Progress { phase: Phase::Copying, bytes, .. } if *bytes > 0 && *bytes < 410_019
	)));
	assert_eq!(
		stdfs::read(destination.join("tree/one")).unwrap(),
		vec![1; 150_003]
	);
	assert_eq!(
		stdfs::read(destination.join("tree/nested/two")).unwrap(),
		vec![2; 170_009]
	);
	assert_eq!(
		stdfs::read(destination.join("flat")).unwrap(),
		vec![3; 90_007]
	);
}

#[test]
fn replacement_and_undo_do_not_add_forward_bytes_or_claim_an_undo_total() {
	let root = TestDir::new();
	let source = root.path().join("file");
	let destination = root.path().join("destination");
	stdfs::create_dir(&destination).unwrap();
	stdfs::write(&source, b"new").unwrap();
	stdfs::write(destination.join("file"), vec![7; 8192]).unwrap();
	let store =
		Store::open_with_recovery(root.path().join("state"), root.path().join("recovery")).unwrap();
	let mut events = Vec::new();

	let result = execute(
		&store,
		request(vec![source], &destination),
		&AtomicBool::new(false),
		|_| Choice::ReplaceCentral,
		|event| events.push(event),
	)
	.unwrap();

	assert_eq!(result.status, Status::Complete, "{:?}", result.problems);
	assert_forward_progress(&events, 3);
	events.clear();
	let result = undo(&store, &result.id, |event| events.push(event)).unwrap();
	assert_eq!(result.status, Status::RolledBack, "{:?}", result.problems);
	assert!(
		events
			.iter()
			.any(|event| matches!(event, Event::Progress { .. }))
	);
	assert!(
		events
			.iter()
			.all(|event| !matches!(event, Event::Progress { total: Some(_), .. }))
	);
	assert_eq!(
		stdfs::read(destination.join("file")).unwrap(),
		vec![7; 8192]
	);
}

#[test]
fn cancellation_before_totals_leaves_sources_destinations_and_journals_untouched() {
	let root = TestDir::new();
	let source = root.path().join("file");
	let destination = root.path().join("destination");
	let state = root.path().join("state");
	stdfs::write(&source, b"untouched").unwrap();
	stdfs::create_dir(&destination).unwrap();
	let store = Store::open(state.clone()).unwrap();
	let cancelled = AtomicBool::new(false);
	let mut events = Vec::new();
	let state_entries = stdfs::read_dir(&state).unwrap().count();

	let result = execute(
		&store,
		request(vec![source.clone()], &destination),
		&cancelled,
		|_| panic!("preparing must not ask for a conflict"),
		|event| {
			events.push(event);
			cancelled.store(true, Ordering::Release);
		},
	);

	assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
	assert_eq!(events.len(), 1);
	assert!(matches!(events[0], Event::Progress { total: None, .. }));
	assert_eq!(stdfs::read(&source).unwrap(), b"untouched");
	assert_eq!(stdfs::read_dir(&destination).unwrap().count(), 0);
	assert_eq!(stdfs::read_dir(&state).unwrap().count(), state_entries);
}

#[test]
fn source_size_changes_after_totals_fail_without_exceeding_total() {
	for changed in [b"".as_slice(), b"longer than before".as_slice()] {
		let root = TestDir::new();
		let source = root.path().join("file");
		let destination = root.path().join("destination");
		stdfs::write(&source, b"old").unwrap();
		stdfs::create_dir(&destination).unwrap();
		let store = Store::open(root.path().join("state")).unwrap();
		let mut events = Vec::new();
		let mut changed_source = false;

		let result = execute(
			&store,
			request(vec![source.clone()], &destination),
			&AtomicBool::new(false),
			|_| Choice::Cancel,
			|event| {
				if !changed_source && matches!(event, Event::Progress { total: Some(_), .. }) {
					stdfs::write(&source, changed).unwrap();
					changed_source = true;
				}
				events.push(event);
			},
		)
		.unwrap();

		assert_eq!(result.status, Status::RolledBack);
		assert!(!result.problems.is_empty());
		assert_eq!(stdfs::read(&source).unwrap(), changed);
		assert_eq!(stdfs::read_dir(&destination).unwrap().count(), 0);
		for event in events {
			if let Event::Progress {
				bytes,
				total: Some(total),
				..
			} = event
			{
				assert_eq!(total, 3);
				assert!(bytes <= total);
			}
		}
	}
}

#[test]
fn copy_tree_reports_cumulative_bytes_and_rejects_source_growth() {
	let root = TestDir::new();
	let source = root.path().join("tree");
	stdfs::create_dir(&source).unwrap();
	stdfs::write(source.join("a"), vec![1; 90_001]).unwrap();
	stdfs::write(source.join("b"), vec![2; 90_003]).unwrap();
	let expected = fs::snapshot(&source).unwrap();
	let mut observations = Vec::new();

	copy_tree(
		&source,
		&root.path().join("copied"),
		&expected,
		false,
		&AtomicBool::new(false),
		&mut |bytes| observations.push(bytes),
	)
	.unwrap();

	assert!(observations.windows(2).all(|pair| pair[0] <= pair[1]));
	assert_eq!(observations.last(), Some(&180_004));
	stdfs::write(source.join("b"), vec![2; 190_003]).unwrap();
	observations.clear();
	let result = copy_tree(
		&source,
		&root.path().join("changed-copy"),
		&expected,
		false,
		&AtomicBool::new(false),
		&mut |bytes| observations.push(bytes),
	);
	assert!(result.is_err());
	assert!(observations.iter().all(|bytes| *bytes <= 180_004));
}

#[test]
fn checked_progress_rejects_overflow_and_exceeding_total() {
	assert_eq!(checked_progress(7, 3, 10).unwrap(), 10);
	assert!(checked_progress(7, 4, 10).is_err());
	assert!(checked_progress(u64::MAX, 1, u64::MAX).is_err());
}

#[test]
fn metadata_total_rejects_special_nodes_without_opening_them() {
	let root = TestDir::new();
	let socket = root.path().join("socket");
	let link = root.path().join("link");
	let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
	symlink(&socket, &link).unwrap();
	let cancelled = AtomicBool::new(false);

	assert!(transfer_total(&[socket], &cancelled).is_err());
	assert_eq!(transfer_total(&[link], &cancelled).unwrap(), 0);
}

#[test]
fn snapshot_byte_sum_rejects_overflow() {
	let root = TestDir::new();
	stdfs::write(root.path().join("one"), b"1").unwrap();
	stdfs::write(root.path().join("two"), b"2").unwrap();
	let mut snapshot = fs::snapshot(root.path()).unwrap();
	let file = snapshot
		.0
		.iter_mut()
		.find(|entry| entry.kind == Kind::File)
		.unwrap();
	file.length = u64::MAX;

	assert!(snapshot_bytes(&snapshot).is_err());
}
