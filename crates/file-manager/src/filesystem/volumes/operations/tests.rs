#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::filesystem::volumes::tests::fixture;
use std::collections::VecDeque;

struct Fake {
	snapshots: VecDeque<Result<Snapshot, String>>,
	calls: Vec<String>,
	mount: Result<PathBuf, MountError>,
	fail_unmount: bool,
}

impl Fake {
	fn new(snapshots: Vec<Snapshot>) -> Self {
		Self {
			snapshots: snapshots.into_iter().map(Ok).collect(),
			calls: Vec::new(),
			mount: Ok("/returned/by/udisks".into()),
			fail_unmount: false,
		}
	}
}

impl Backend for Fake {
	async fn snapshot(&mut self) -> Result<Snapshot, String> {
		self.snapshots.pop_front().expect("unexpected discovery")
	}

	async fn mount(&mut self, target: &Identity) -> Result<PathBuf, MountError> {
		self.calls.push(format!("mount:{}", target.object));

		std::mem::replace(
			&mut self.mount,
			Err(MountError::Failed("duplicate mount".into())),
		)
	}

	async fn unmount(&mut self, target: &Identity) -> Result<(), String> {
		self.calls.push(format!("unmount:{}", target.object));

		if self.fail_unmount {
			Err("DeviceBusy".into())
		} else {
			Ok(())
		}
	}

	async fn remove(&mut self, target: &Identity, action: Action) -> Result<(), String> {
		self.calls.push(format!("{action:?}:{}", target.drive));

		Ok(())
	}
}

fn operation(snapshot: &Snapshot, action: Action) -> Operation {
	Operation::prepare(snapshot, &snapshot.volumes[0], action).unwrap()
}

#[tokio::test]
async fn mount_uses_retained_object_and_returns_service_selected_path() {
	let snapshot = fixture(&[]);
	let op = operation(&snapshot, Action::Mount);
	let mut backend = Fake::new(vec![snapshot]);

	let path = op.execute_with(&mut backend).await.unwrap();

	assert_eq!(path, Some("/returned/by/udisks".into()));
	assert_eq!(backend.calls, ["mount:/blocks/test"]);
}

#[tokio::test]
async fn changed_identity_mount_id_and_new_partitions_abort_before_mutation() {
	let snapshot = fixture(&["/data"]);
	let op = operation(&snapshot, Action::Eject);

	for changed in 0..3 {
		let mut current = snapshot.clone();
		match changed {
			0 => current.volumes[0].identity.serial = "replacement".into(),
			1 => current.volumes[0].mounts[0].mount_id = Some(999),
			_ => {
				let mut partition = current.volumes[0].clone();
				partition.identity.object = "/blocks/new".try_into().unwrap();
				current.volumes.push(partition);
			}
		}
		let mut backend = Fake::new(vec![current]);

		assert!(op.clone().execute_with(&mut backend).await.is_err());
		assert!(backend.calls.is_empty());
	}
}

#[tokio::test]
async fn mount_race_opens_only_unambiguous_single_destination() {
	let snapshot = fixture(&[]);
	let op = operation(&snapshot, Action::Mount);
	let mut before = Fake::new(vec![fixture(&["/raced"])]);

	assert_eq!(
		op.clone().execute_with(&mut before).await.unwrap(),
		Some("/raced".into())
	);
	assert!(before.calls.is_empty());

	let mut during = Fake::new(vec![snapshot.clone(), fixture(&["/raced"])]);
	during.mount = Err(MountError::AlreadyMounted);

	assert_eq!(
		op.clone().execute_with(&mut during).await.unwrap(),
		Some("/raced".into())
	);
	assert_eq!(during.calls.len(), 1);

	let mut ambiguous = Fake::new(vec![snapshot, fixture(&["/one", "/two"])]);
	ambiguous.mount = Err(MountError::AlreadyMounted);

	assert!(op.execute_with(&mut ambiguous).await.is_err());
}

#[tokio::test]
async fn eject_unmounts_confirmed_partitions_then_targets_drive() {
	let first = fixture(&["/data"]);
	let op = operation(&first, Action::Eject);
	let mut backend = Fake::new(vec![first.clone(), first, fixture(&[])]);

	assert_eq!(op.execute_with(&mut backend).await.unwrap(), None);
	assert_eq!(
		backend.calls,
		["unmount:/blocks/test", "Eject:/drives/test"]
	);
}

#[tokio::test]
async fn busy_unmount_stops_removal_without_force_or_retry() {
	let snapshot = fixture(&["/data"]);
	let op = operation(&snapshot, Action::PowerOff);
	let mut backend = Fake::new(vec![snapshot.clone(), snapshot]);
	backend.fail_unmount = true;

	assert_eq!(
		op.execute_with(&mut backend).await.unwrap_err(),
		"DeviceBusy"
	);
	assert_eq!(backend.calls, ["unmount:/blocks/test"]);
}

#[tokio::test]
async fn root_or_new_mount_appearing_between_steps_stops_removal() {
	let snapshot = fixture(&["/data"]);
	let op = operation(&snapshot, Action::PowerOff);
	let mut backend = Fake::new(vec![snapshot.clone(), snapshot, fixture(&["/"])]);

	assert!(op.execute_with(&mut backend).await.is_err());
	assert_eq!(backend.calls, ["unmount:/blocks/test"]);
}

#[tokio::test]
async fn discovery_and_authorization_failures_do_not_proceed() {
	let snapshot = fixture(&[]);
	let op = operation(&snapshot, Action::Mount);
	let mut backend = Fake::new(vec![]);
	backend
		.snapshots
		.push_back(Err("Service unavailable".into()));

	assert_eq!(
		op.clone().execute_with(&mut backend).await.unwrap_err(),
		"Service unavailable"
	);
	assert!(backend.calls.is_empty());

	let mut backend = Fake::new(vec![snapshot]);
	backend.mount = Err(MountError::Failed("Not authorized".into()));

	assert_eq!(
		op.execute_with(&mut backend).await.unwrap_err(),
		"Not authorized"
	);
	assert_eq!(backend.calls.len(), 1);
}

#[tokio::test]
async fn power_off_without_mounts_calls_only_drive_operation() {
	let snapshot = fixture(&[]);
	let op = operation(&snapshot, Action::PowerOff);
	let mut backend = Fake::new(vec![snapshot.clone(), snapshot]);

	assert!(op.execute_with(&mut backend).await.is_ok());
	assert_eq!(backend.calls, ["PowerOff:/drives/test"]);
}

#[tokio::test]
async fn removal_unmounts_nested_destinations_before_parents() {
	let mut snapshot = fixture(&["/parent"]);
	let mut child = snapshot.volumes[0].clone();
	child.identity.object = "/blocks/child".try_into().unwrap();
	child.identity.device = "/dev/child".into();
	child.mounts[0].path = "/parent/child".into();
	snapshot.volumes.push(child);
	let op = operation(&snapshot, Action::Eject);
	let mut child_unmounted = snapshot.clone();
	child_unmounted.volumes[1].mounts.clear();
	let mut all_unmounted = child_unmounted.clone();
	all_unmounted.volumes[0].mounts.clear();
	let mut backend = Fake::new(vec![
		snapshot.clone(),
		snapshot,
		child_unmounted,
		all_unmounted,
	]);

	assert!(op.execute_with(&mut backend).await.is_ok());
	assert_eq!(
		backend.calls,
		[
			"unmount:/blocks/child",
			"unmount:/blocks/test",
			"Eject:/drives/test"
		]
	);
}
