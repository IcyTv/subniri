#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::filesystem::disks::{BlockProperties, FilesystemProperties};
use std::ffi::CString;

fn object(device: &str, paths: &[&str]) -> UDisks2Object {
	UDisks2Object {
		block: BlockProperties {
			device: CString::new(device).unwrap(),
			id_label: "Data".into(),
			id_uuid: "123".into(),
			drive: "/drives/test".try_into().unwrap(),
			..Default::default()
		},
		filesystem: Some(FilesystemProperties {
			mount_points: paths.iter().map(|p| CString::new(*p).unwrap()).collect(),
		}),
		..Default::default()
	}
}

pub(crate) fn fixture(paths: &[&str]) -> Snapshot {
	let mut objects = HashMap::new();
	objects.insert(
		"/blocks/test".try_into().unwrap(),
		object("/dev/test", paths),
	);
	objects.insert(
		"/drives/test".try_into().unwrap(),
		UDisks2Object {
			drive: DriveProperties {
				model: "Disk model".into(),
				ejectable: true,
				can_power_off: true,
				..Default::default()
			},
			..Default::default()
		},
	);
	let mountinfo = paths
		.iter()
		.map(|p| format!("10 1 8:1 / {p} rw - ext4 /dev/test rw\n"))
		.collect::<String>();

	from_objects(
		objects,
		&parse_mounts(mountinfo.as_bytes()),
		b"UUID=123 /etc/system ext4 defaults 0 0\n/dev/test /system ext4 defaults 0 0\n",
	)
}

#[test]
fn discovery_retains_identities_and_joins_separate_drive_objects() {
	let snapshot = fixture(&["/media/data"]);
	let volume = &snapshot.volumes[0];

	assert_eq!(volume.identity.object.as_str(), "/blocks/test");
	assert_eq!(volume.drive.model, "Disk model");
	assert!(volume.drive.can_power_off);
	assert_eq!(volume.name, "Data");
	assert!(volume.mounts[0].usage.is_none());
	assert!(!volume.ambiguous);
}

#[test]
fn cards_are_per_destination_and_filter_system_mounts_without_false_discovery_cards() {
	let snapshot = fixture(&["/system", "/etc/system"]);

	assert!(snapshot.cards(false).is_empty());
	assert_eq!(snapshot.cards(true).len(), 2);
	assert!(snapshot.cards(true).iter().all(|(_, m)| m.is_some()));
}

#[test]
fn root_is_visible_and_unmounted_filesystem_gets_one_discovery_card() {
	let mut snapshot = fixture(&["/", "/system"]);
	snapshot.volumes[0].mounts[0].configured = true;

	assert_eq!(snapshot.cards(false).len(), 1);
	assert_eq!(snapshot.cards(false)[0].1.unwrap().path, PathBuf::from("/"));

	let snapshot = fixture(&[]);
	assert_eq!(snapshot.cards(false).len(), 1);
	assert!(snapshot.cards(false)[0].1.is_none());
}

#[test]
fn hint_ignore_hides_cards_but_preserves_drive_safety_information() {
	let mut disk = object("/dev/test", &["/"]);
	disk.block.hint_ignore = true;
	let snapshot = from_objects(
		HashMap::from([("/blocks/test".try_into().unwrap(), disk)]),
		&[],
		b"",
	);

	assert!(snapshot.cards(true).is_empty());
	assert_eq!(snapshot.volumes.len(), 1);
}

#[test]
fn btrfs_uuid_peers_share_mount_knowledge_but_are_not_safe_operation_targets() {
	let mut a = object("/dev/a", &["/data"]);
	a.block.id_type = "btrfs".into();
	a.block.id_uuid = "same".into();
	let mut b = a.clone();
	b.block.device = CString::new("/dev/b").unwrap();
	b.filesystem.as_mut().unwrap().mount_points.clear();
	let mounts = parse_mounts(b"1 0 0:1 / /data rw - btrfs /dev/a rw\n");
	let snapshot = from_objects(
		HashMap::from([
			("/blocks/a".try_into().unwrap(), a),
			("/blocks/b".try_into().unwrap(), b),
		]),
		&mounts,
		b"",
	);

	assert_eq!(snapshot.cards(false).len(), 1);
	assert!(
		snapshot
			.volumes
			.iter()
			.all(|v| v.ambiguous && !v.mounts.is_empty())
	);
	assert!(Operation::prepare(&snapshot, &snapshot.volumes[1], Action::Mount).is_err());
}

#[test]
fn mount_table_paths_preserve_octal_escapes_and_non_utf8_bytes() {
	let mounts = parse_mounts(b"1 0 8:1 / /data\\040space\\377 rw shared:1 - ext4 /dev/test rw\n");
	let configured = parse_fstab(b"# comment\nUUID=test /data\\040space\\377 ext4 defaults 0 0\n");

	assert_eq!(mounts.len(), 1);
	assert_eq!(mounts[0].target.as_os_str().as_bytes(), b"/data space\xff");
	assert!(
		configured
			.iter()
			.any(|entry| entry.target == mounts[0].target)
	);
}

#[test]
fn operations_protect_root_multimounts_and_hidden_root_on_same_drive() {
	for paths in [&["/"][..], &["/one", "/two"][..]] {
		let snapshot = fixture(paths);

		for action in [Action::Unmount, Action::Eject, Action::PowerOff] {
			assert!(Operation::prepare(&snapshot, &snapshot.volumes[0], action).is_err());
		}
	}

	let mut snapshot = fixture(&["/data"]);
	let mut root = snapshot.volumes[0].clone();
	root.identity.object = "/blocks/root".try_into().unwrap();
	root.mounts[0].path = "/".into();
	root.visible = false;
	snapshot.volumes.push(root);

	assert!(Operation::prepare(&snapshot, &snapshot.volumes[0], Action::Eject).is_err());
	assert!(Operation::prepare(&snapshot, &snapshot.volumes[0], Action::Unmount).is_ok());
}

#[test]
fn capabilities_and_shared_physical_devices_limit_removal() {
	let mut snapshot = fixture(&[]);
	snapshot.volumes[0].drive.ejectable = false;

	assert!(Operation::prepare(&snapshot, &snapshot.volumes[0], Action::Eject).is_err());
	assert!(Operation::prepare(&snapshot, &snapshot.volumes[0], Action::Mount).is_ok());

	snapshot
		.unsafe_drives
		.push(snapshot.volumes[0].identity.drive.clone());
	assert!(Operation::prepare(&snapshot, &snapshot.volumes[0], Action::PowerOff).is_err());
}

#[test]
fn system_visibility_matches_source_and_destination_not_ancestry() {
	let mounts = parse_mounts(b"1 0 8:1 / /system rw - ext4 /dev/test rw\n2 0 8:1 / /system/child rw - ext4 /dev/test rw\n");
	let objects = HashMap::from([(
		"/blocks/test".try_into().unwrap(),
		object("/dev/test", &["/system", "/system/child"]),
	)]);
	let snapshot = from_objects(
		objects.clone(),
		&mounts,
		b"UUID=123 /system ext4 defaults 0 0\n",
	);

	assert_eq!(snapshot.cards(false).len(), 1);
	assert_eq!(
		snapshot.cards(false)[0].1.unwrap().path,
		PathBuf::from("/system/child")
	);

	let other_source = from_objects(objects, &mounts, b"UUID=other /system ext4 defaults 0 0\n");
	assert_eq!(other_source.cards(false).len(), 2);
}

#[test]
fn daemon_only_paths_are_not_navigation_targets_or_unmounted_cards() {
	let snapshot = from_objects(
		HashMap::from([(
			"/blocks/test".try_into().unwrap(),
			object("/dev/test", &["/daemon-only"]),
		)]),
		&[],
		b"",
	);

	assert!(snapshot.cards(true).is_empty());
	assert!(snapshot.volumes[0].ambiguous);
	assert!(Operation::prepare(&snapshot, &snapshot.volumes[0], Action::Unmount).is_err());
}

#[test]
fn stacked_mounts_are_not_safe_management_targets() {
	let mounts = parse_mounts(
		b"1 0 8:1 / /data rw - ext4 /dev/test rw\n2 0 8:2 / /data rw - ext4 /dev/other rw\n",
	);
	let snapshot = from_objects(
		HashMap::from([(
			"/blocks/test".try_into().unwrap(),
			object("/dev/test", &["/data"]),
		)]),
		&mounts,
		b"",
	);

	assert!(snapshot.volumes[0].ambiguous);
	assert!(Operation::prepare(&snapshot, &snapshot.volumes[0], Action::Unmount).is_err());
}

#[test]
fn shared_hardware_without_a_filesystem_still_blocks_power_off() {
	let mut objects =
		HashMap::from([("/blocks/test".try_into().unwrap(), object("/dev/test", &[]))]);
	let drive = UDisks2Object {
		drive: DriveProperties {
			can_power_off: true,
			sibling_id: "shared".into(),
			..Default::default()
		},
		..Default::default()
	};
	objects.insert("/drives/test".try_into().unwrap(), drive.clone());
	objects.insert("/drives/empty_slot".try_into().unwrap(), drive);
	let snapshot = from_objects(objects, &[], b"");

	assert!(Operation::prepare(&snapshot, &snapshot.volumes[0], Action::PowerOff).is_err());
}

#[test]
fn system_directory_reexposures_are_hidden_without_hiding_root_or_other_subvolumes() {
	let paths = [
		"/",
		"/nix",
		"/nix/store",
		"/var/lib/docker/btrfs",
		"/mnt/snapshot",
	];
	let mounts = parse_mounts(
		concat!(
			"65 1 0:32 /root / rw - btrfs /dev/test rw\n",
			"69 65 0:32 /nix /nix rw - btrfs /dev/test rw\n",
			"42 69 0:32 /nix/store /nix/store ro - btrfs /dev/test rw\n",
			"283 65 0:32 /root/var/lib/docker/btrfs /var/lib/docker/btrfs rw - btrfs /dev/test rw\n",
			"300 65 0:32 /snapshots/one /mnt/snapshot rw - btrfs /dev/test rw\n",
		)
		.as_bytes(),
	);
	let objects = HashMap::from([(
		"/blocks/test".try_into().unwrap(),
		object("/dev/test", &paths),
	)]);

	let snapshot = from_objects(
		objects,
		&mounts,
		b"UUID=123 / btrfs subvol=root 0 0\nUUID=123 /nix btrfs subvol=nix 0 0\n",
	);
	let visible: Vec<_> = snapshot
		.cards(false)
		.iter()
		.map(|(_, m)| m.unwrap().path.clone())
		.collect();

	assert_eq!(
		visible,
		[PathBuf::from("/"), PathBuf::from("/mnt/snapshot")]
	);
	assert_eq!(snapshot.cards(true).len(), paths.len());
	assert!(snapshot.cards(true).iter().all(|(_, m)| m.is_some()));
	assert_eq!(
		snapshot.volumes[0].mounts.len(),
		paths.len(),
		"visibility must not erase operation-safety evidence"
	);
}

#[test]
fn aliases_remain_visible_without_a_matching_system_configuration() {
	let mounts = parse_mounts(b"1 0 0:32 /root / rw - btrfs /dev/test rw\n2 1 0:32 /root/data /data rw - btrfs /dev/test rw\n");
	let objects = HashMap::from([(
		"/blocks/test".try_into().unwrap(),
		object("/dev/test", &["/", "/data"]),
	)]);

	for fstab in [&b""[..], &b"UUID=another / btrfs defaults 0 0\n"[..]] {
		let snapshot = from_objects(objects.clone(), &mounts, fstab);

		assert_eq!(snapshot.cards(false).len(), 2);
	}
}

#[test]
fn a_different_covering_filesystem_prevents_system_alias_inheritance() {
	let mounts = parse_mounts(
		concat!(
			"1 0 8:1 / /system rw - ext4 /dev/test rw\n",
			"2 1 8:2 / /system/external rw - ext4 /dev/other rw\n",
			"3 2 8:1 /external/child /system/external/child rw - ext4 /dev/test rw\n",
		)
		.as_bytes(),
	);
	let objects = HashMap::from([(
		"/blocks/test".try_into().unwrap(),
		object("/dev/test", &["/system", "/system/external/child"]),
	)]);

	let snapshot = from_objects(objects, &mounts, b"UUID=123 /system ext4 defaults 0 0\n");

	assert_eq!(snapshot.cards(false).len(), 1);
	assert_eq!(
		snapshot.cards(false)[0].1.unwrap().path,
		PathBuf::from("/system/external/child")
	);
}

#[test]
fn breadcrumb_root_is_the_deepest_visible_mount_not_a_path_prefix_or_system_alias() {
	let mut snapshot = fixture(&["/", "/run/media/m/Backup", "/run/media/m/Backup/nested"]);
	let path = std::path::Path::new("/run/media/m/Backup/nested/Photos");

	assert_eq!(
		snapshot
			.breadcrumb_root(path)
			.map(|(_, m)| m.path.as_path()),
		Some(std::path::Path::new("/run/media/m/Backup/nested"))
	);

	snapshot.volumes[0]
		.mounts
		.iter_mut()
		.find(|m| m.path.ends_with("nested"))
		.unwrap()
		.configured = true;

	assert_eq!(
		snapshot
			.breadcrumb_root(path)
			.map(|(v, m)| (v.name.as_str(), m.path.as_path())),
		Some(("Data", std::path::Path::new("/run/media/m/Backup")))
	);
	assert_eq!(
		snapshot
			.breadcrumb_root(std::path::Path::new("/run/media/m/Backup-other"))
			.map(|(_, m)| m.path.as_path()),
		Some(std::path::Path::new("/"))
	);
}
