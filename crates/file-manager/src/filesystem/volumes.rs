//! Discovery and operation policy. UI consumers never choose D-Bus targets by label.
mod mounts;
mod operations;
#[cfg(test)]
pub(crate) mod tests;

use std::{collections::HashMap, os::unix::ffi::OsStrExt, path::PathBuf};

use zbus::zvariant::OwnedObjectPath;

use super::{
	disks::{Disks, DriveProperties, UDisks2Object},
	storage::Usage,
};
use mounts::{Mount, parse_fstab, parse_mounts};
pub(crate) use operations::{Action, Operation};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Identity {
	pub object: OwnedObjectPath,
	pub device: PathBuf,
	pub number: u64,
	pub uuid: String,
	pub drive: OwnedObjectPath,
	pub serial: String,
	pub detected: u64,
	pub media_detected: u64,
	pub size: u64,
	pub fs_type: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Destination {
	pub path: PathBuf,
	pub mount_id: Option<u64>,
	pub root: Option<PathBuf>,
	pub configured: bool,
	pub usage: Option<Usage>,
}

#[derive(Debug, Clone)]
pub(crate) struct Volume {
	pub identity: Identity,
	pub name: String,
	pub drive: DriveProperties,
	pub mounts: Vec<Destination>,
	pub visible: bool,
	pub ambiguous: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Snapshot {
	pub volumes: Vec<Volume>,
	// Include hidden/non-filesystem blocks when deciding drive-wide safety.
	pub unsafe_drives: Vec<OwnedObjectPath>,
	pub shared_drives: Vec<OwnedObjectPath>,
}

impl Snapshot {
	pub(crate) fn volume(&self, object: &OwnedObjectPath) -> Option<&Volume> {
		self.volumes.iter().find(|v| &v.identity.object == object)
	}

	pub(crate) fn breadcrumb_root(
		&self, path: &std::path::Path,
	) -> Option<(&Volume, &Destination)> {
		self.cards(false)
			.into_iter()
			.filter_map(|(volume, destination)| destination.map(|mount| (volume, mount)))
			.filter(|(_, mount)| path.starts_with(&mount.path))
			.max_by_key(|(_, mount)| mount.path.components().count())
	}

	pub(crate) fn cards(&self, show_system: bool) -> Vec<(&Volume, Option<&Destination>)> {
		let mut cards = Vec::new();
		let mut seen = std::collections::HashSet::new();

		for volume in self.volumes.iter().filter(|v| v.visible) {
			if volume.mounts.is_empty() {
				cards.push((volume, None));
			}

			for mount in &volume.mounts {
				// A daemon-only path may refer to a different filesystem here.
				if mount.mount_id.is_none() {
					continue;
				}
				if (show_system || !mount.configured || mount.path == std::path::Path::new("/"))
					&& seen.insert(&mount.path)
				{
					cards.push((volume, Some(mount)));
				}
			}
		}

		cards.sort_by(|(a, am), (b, bm)| {
			let ap = am.map(|m| &m.path);
			let bp = bm.map(|m| &m.path);
			let a_root = ap.is_some_and(|p| p == std::path::Path::new("/"));
			let b_root = bp.is_some_and(|p| p == std::path::Path::new("/"));
			b_root
				.cmp(&a_root)
				.then_with(|| a.name.cmp(&b.name))
				.then_with(|| ap.cmp(&bp))
				.then_with(|| a.identity.object.cmp(&b.identity.object))
		});

		cards
	}
}

pub(crate) async fn discover() -> Result<Snapshot, String> {
	discover_with_usage(true).await
}

async fn discover_with_usage(usage: bool) -> Result<Snapshot, String> {
	let objects = Disks::new()
		.await
		.map_err(|e| e.to_string())?
		.objects()
		.await
		.map_err(|e| e.to_string())?;

	tokio::task::spawn_blocking(move || {
		let mounts = std::fs::read("/proc/self/mountinfo").map_err(|e| e.to_string())?;
		let fstab = match std::fs::read("/etc/fstab") {
			Ok(bytes) => bytes,
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
			Err(e) => return Err(format!("Cannot read fstab: {e}")),
		};
		let mut snapshot = from_objects(objects, &parse_mounts(&mounts), &fstab);

		if usage {
			for volume in &mut snapshot.volumes {
				for mount in &mut volume.mounts {
					if mount.mount_id.is_some() {
						mount.usage = Usage::read(&mount.path);
					}
				}
			}
		}

		Ok(snapshot)
	})
	.await
	.map_err(|e| e.to_string())?
}

fn from_objects(
	objects: HashMap<OwnedObjectPath, UDisks2Object>, mounts: &[Mount], fstab: &[u8],
) -> Snapshot {
	let configured = parse_fstab(fstab);
	let mut snapshot = Snapshot::default();

	for (path, object) in &objects {
		if !object.block.id_usage.is_empty() && object.block.id_usage != "filesystem" {
			snapshot.unsafe_drives.push(object.block.drive.clone());
		}
		if !object.drive.sibling_id.is_empty()
			&& objects.iter().any(|(other_path, other)| {
				other_path != path && other.drive.sibling_id == object.drive.sibling_id
			}) {
			snapshot.shared_drives.push(path.clone());
		}

		let Some(filesystem) = &object.filesystem else {
			continue;
		};
		let drive = objects
			.get(&object.block.drive)
			.map(|o| o.drive.clone())
			.unwrap_or_default();
		let device = PathBuf::from(std::ffi::OsStr::from_bytes(object.block.device.as_bytes()));
		let configured_paths: Vec<_> = configured
			.iter()
			.filter(|entry| entry.matches(&device, &object.block.id_uuid, &object.block.id_label))
			.map(|entry| &entry.target)
			.collect();
		let mut destinations: Vec<_> = mounts
			.iter()
			.filter(|m| m.matches(&device, object.block.device_number))
			.map(|m| m.target.clone())
			.collect();
		let daemon_paths: Vec<_> = filesystem
			.mount_points
			.iter()
			.map(|p| PathBuf::from(std::ffi::OsStr::from_bytes(p.as_bytes())))
			.collect();
		// Operations are unsafe if the daemon sees mounts absent from our namespace.
		let namespace_mismatch = daemon_paths.iter().any(|p| !destinations.contains(p))
			|| destinations.iter().any(|p| !daemon_paths.contains(p));
		let stacked_mounts = destinations
			.iter()
			.any(|p| mounts.iter().filter(|m| &m.target == p).count() > 1);
		destinations.extend(daemon_paths);
		destinations.sort();
		destinations.dedup();

		let name = if !object.block.id_label.trim().is_empty() {
			object.block.id_label.clone()
		} else if !drive.model.trim().is_empty() {
			format!(
				"{} ({})",
				drive.model.trim(),
				device.file_name().unwrap_or_default().to_string_lossy()
			)
		} else {
			device
				.file_name()
				.unwrap_or_default()
				.to_string_lossy()
				.into_owned()
		};

		snapshot.volumes.push(Volume {
			identity: Identity {
				object: path.clone(),
				device,
				number: object.block.device_number,
				uuid: object.block.id_uuid.clone(),
				drive: object.block.drive.clone(),
				serial: drive.serial.clone(),
				detected: drive.time_detected,
				media_detected: drive.time_media_detected,
				size: object.block.size,
				fs_type: object.block.id_type.clone(),
			},
			name,
			drive,
			visible: object.should_show(),
			ambiguous: namespace_mismatch || stacked_mounts,
			mounts: destinations
				.into_iter()
				.map(|path| Destination {
					mount_id: mounts.iter().find(|m| m.target == path).map(|m| m.id),
					root: mounts
						.iter()
						.find(|m| m.target == path)
						.map(|m| m.root.clone()),
					configured: mounts.iter().find(|m| m.target == path).map_or_else(
						|| configured_paths.contains(&&path),
						|m| m.system_configured(mounts, &configured_paths),
					),
					path,
					usage: None,
				})
				.collect(),
		});
	}

	// UUID equality is not proof of membership (clones exist). Share mount knowledge
	// for display only; retain every block identity and prohibit management.
	let original = snapshot.volumes.clone();
	for volume in &mut snapshot.volumes {
		let object = &objects[&volume.identity.object];
		if object.block.id_type != "btrfs" || volume.identity.uuid.is_empty() {
			continue;
		}
		let peers: Vec<_> = original
			.iter()
			.filter(|v| v.identity.uuid == volume.identity.uuid)
			.collect();
		if peers.len() < 2 {
			continue;
		}

		volume.ambiguous = true;
		for peer in peers {
			for mount in &peer.mounts {
				if !volume.mounts.iter().any(|m| m.path == mount.path) {
					volume.mounts.push(mount.clone());
				}
			}
		}
		volume.mounts.sort_by(|a, b| a.path.cmp(&b.path));
	}
	snapshot
		.volumes
		.sort_by(|a, b| a.identity.object.cmp(&b.identity.object));

	snapshot
}
