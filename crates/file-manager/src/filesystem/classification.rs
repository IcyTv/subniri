//! Destination storage identity. Call from a background worker: procfs and sysfs may block.
//! Policy (including conservative Auto behavior) belongs to the caller.

use std::{
	collections::HashSet,
	fs::{self, File, OpenOptions},
	io,
	os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
	path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageClass {
	Fixed,
	Removable,
	Network,
	Unknown,
	Volatile,
}

pub(crate) fn classify(directory: &Path) -> io::Result<StorageClass> {
	// O_PATH pins the actual destination, including symlink resolution and bind mounts.
	let directory = OpenOptions::new()
		.read(true)
		.custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC)
		.open(directory)?;
	let Some(id) = mount_id(&directory) else {
		return Ok(StorageClass::Unknown);
	};
	let Ok(mountinfo) = fs::read("/proc/self/mountinfo") else {
		return Ok(StorageClass::Unknown);
	};
	let Some(mount) = find_mount(&mountinfo, id) else {
		return Ok(StorageClass::Unknown);
	};

	Ok(classify_mount(&mount, Path::new("/sys")))
}

pub(crate) fn mount_id(file: &File) -> Option<u64> {
	// SAFETY: statx is a plain C output structure; all-zero initialization is valid.
	let mut stat: libc::statx = unsafe { std::mem::zeroed() };
	// STATX_MNT_ID is the ordinary mountinfo ID, not STATX_MNT_ID_UNIQUE.
	// SAFETY: the FD stays open and both the empty C string and output pointer are valid.
	let result = unsafe {
		libc::statx(
			file.as_raw_fd(),
			c"".as_ptr(),
			libc::AT_EMPTY_PATH | libc::AT_STATX_DONT_SYNC,
			libc::STATX_MNT_ID,
			&mut stat,
		)
	};
	if result == 0 && stat.stx_mask & libc::STATX_MNT_ID != 0 {
		return Some(stat.stx_mnt_id);
	}

	let info = fs::read(format!("/proc/self/fdinfo/{}", file.as_raw_fd())).ok()?;
	parse_fdinfo(&info)
}

fn parse_fdinfo(info: &[u8]) -> Option<u64> {
	info.split(|byte| *byte == b'\n').find_map(|line| {
		let value = line.strip_prefix(b"mnt_id:")?;
		decimal(value.trim_ascii())
	})
}

struct Mount<'a> {
	major: u32,
	minor: u32,
	fs_type: &'a [u8],
}

fn decimal(bytes: &[u8]) -> Option<u64> {
	if bytes.is_empty() {
		return None;
	}

	bytes.iter().try_fold(0_u64, |number, byte| {
		if !byte.is_ascii_digit() {
			return None;
		}
		number.checked_mul(10)?.checked_add(u64::from(byte - b'0'))
	})
}

fn find_mount(mountinfo: &[u8], id: u64) -> Option<Mount<'_>> {
	let mut found = None;
	for line in mountinfo.split(|byte| *byte == b'\n') {
		let fields: Vec<_> = line.split(|byte| *byte == b' ').collect();
		if fields.first().and_then(|field| decimal(field)) != Some(id) {
			continue;
		}
		// Duplicate IDs, malformed records, and incomplete snapshots are not evidence.
		if found.is_some() || fields.len() < 10 {
			return None;
		}
		let separator = fields.iter().position(|field| *field == b"-")?;
		if separator < 6 || fields.len() != separator + 4 {
			return None;
		}
		let colon = fields[2].iter().position(|byte| *byte == b':')?;
		let (major, minor) = (&fields[2][..colon], &fields[2][colon + 1..]);
		found = Some(Mount {
			major: decimal(major)?.try_into().ok()?,
			minor: decimal(minor)?.try_into().ok()?,
			fs_type: fields[separator + 1],
		});
	}

	found
}

fn classify_mount(mount: &Mount<'_>, sys: &Path) -> StorageClass {
	match mount.fs_type {
		b"nfs" | b"nfs4" | b"cifs" | b"smb3" | b"smbfs" | b"ceph" | b"afs" | b"9p" | b"coda"
		| b"ncpfs" | b"fuse.sshfs" | b"fuse.ceph" | b"fuse.glusterfs" | b"glusterfs"
		| b"fuse.s3fs" => StorageClass::Network,
		b"tmpfs" | b"ramfs" => StorageClass::Volatile,
		// Btrfs's mount device number does not enumerate its complete backing set.
		b"ext2" | b"ext3" | b"ext4" | b"xfs" | b"vfat" | b"exfat" | b"ntfs" | b"ntfs3"
		| b"f2fs" => {
			let device = sys
				.join("dev/block")
				.join(format!("{}:{}", mount.major, mount.minor));
			classify_block(&device, sys, &mut HashSet::new()).unwrap_or(StorageClass::Unknown)
		}
		_ => StorageClass::Unknown,
	}
}

fn classify_block(
	device: &Path, sys: &Path, visiting: &mut HashSet<PathBuf>,
) -> io::Result<StorageClass> {
	let sys = sys.canonicalize()?;
	let mut device = device.canonicalize()?;
	if !device.starts_with(sys.join("devices")) || visiting.len() >= 64 {
		return Ok(StorageClass::Unknown);
	}
	if read_optional(&device.join("partition"))?.is_some() {
		let Some(parent) = device.parent() else {
			return Ok(StorageClass::Unknown);
		};
		device = parent.to_owned();
	}
	if !visiting.insert(device.clone()) {
		return Ok(StorageClass::Unknown);
	}

	let result = classify_block_inner(&device, &sys, visiting);
	visiting.remove(&device);
	result
}

fn classify_block_inner(
	device: &Path, sys: &Path, visiting: &mut HashSet<PathBuf>,
) -> io::Result<StorageClass> {
	let ancestry = inspect_ancestry(device, sys)?;
	if ancestry.removable {
		return Ok(StorageClass::Removable);
	}

	let mut slaves = Vec::new();
	for entry in fs::read_dir(device.join("slaves"))? {
		slaves.push(entry?.path());
	}
	if !slaves.is_empty() {
		let mut fixed = true;
		let mut removable = false;
		for slave in slaves {
			match classify_block(&slave, sys, visiting).unwrap_or(StorageClass::Unknown) {
				StorageClass::Removable => removable = true,
				StorageClass::Fixed => {}
				_ => fixed = false,
			}
		}
		if removable {
			return Ok(StorageClass::Removable);
		}

		// Only supported device-mapper topologies can inherit Fixed from all slaves.
		let uuid = read_optional(&device.join("dm/uuid"))?.unwrap_or_default();
		let supported = uuid.starts_with(b"CRYPT-") || uuid.starts_with(b"LVM-");
		return Ok(if fixed && supported && !ancestry.uncertain {
			StorageClass::Fixed
		} else {
			StorageClass::Unknown
		});
	}

	let removable = read_optional(&device.join("removable"))?;
	let physical = device.join("device").canonicalize()?;
	if !physical.starts_with(sys.join("devices")) {
		return Ok(StorageClass::Unknown);
	}
	let physical = inspect_ancestry(&physical, sys)?;
	if physical.removable {
		return Ok(StorageClass::Removable);
	}

	Ok(
		if removable.as_deref().map(<[u8]>::trim_ascii) == Some(b"0")
			&& !ancestry.uncertain
			&& !physical.uncertain
			&& physical.pci
			&& physical.storage
		{
			StorageClass::Fixed
		} else {
			StorageClass::Unknown
		},
	)
}

#[derive(Default)]
struct Ancestry {
	removable: bool,
	uncertain: bool,
	pci: bool,
	storage: bool,
}

fn inspect_ancestry(path: &Path, sys: &Path) -> io::Result<Ancestry> {
	let mut result = Ancestry::default();
	for ancestor in path.ancestors().take_while(|path| path.starts_with(sys)) {
		if let Some(value) = read_optional(&ancestor.join("removable"))? {
			match value.trim_ascii() {
				b"1" | b"removable" => result.removable = true,
				b"0" | b"fixed" => {}
				_ => result.uncertain = true,
			}
		}
		if let Some(value) = read_optional(&ancestor.join("type"))?
			&& value.trim_ascii() == b"SD"
		{
			result.removable = true;
		}

		let subsystem = ancestor.join("subsystem");
		match fs::symlink_metadata(&subsystem) {
			Ok(_) => {
				let subsystem = subsystem.canonicalize()?;
				match subsystem.file_name().and_then(|name| name.to_str()) {
					Some("usb" | "thunderbolt" | "firewire") => result.removable = true,
					Some("pci") => result.pci = true,
					Some("scsi" | "nvme" | "ata" | "mmc") => result.storage = true,
					_ => {}
				}
			}
			Err(error) if error.kind() == io::ErrorKind::NotFound => {
				// Every device ancestor must resolve, especially USB bridges above SCSI.
				// These are sysfs containers rather than individual devices.
				let devices = sys.join("devices");
				let container = ancestor == sys
					|| ancestor == devices
					|| ancestor == devices.join("virtual")
					|| ancestor.file_name().is_some_and(|name| name == "block")
					|| (ancestor.parent() == Some(devices.as_path())
						&& ancestor.join("pci_bus").is_dir());
				if !container {
					result.uncertain = true;
				}
			}
			Err(error) => return Err(error),
		}
	}

	Ok(result)
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
	match fs::read(path) {
		Ok(value) => Ok(Some(value)),
		Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
		Err(error) => Err(error),
	}
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]

	use std::os::unix::fs::symlink;

	use super::*;
	use crate::test_support::TestDir;

	fn write(path: &Path, value: &[u8]) {
		fs::create_dir_all(path.parent().unwrap()).unwrap();
		fs::write(path, value).unwrap();
	}

	fn subsystem(sys: &Path, device: &Path, name: &str) {
		let target = sys.join("bus").join(name);
		fs::create_dir_all(&target).unwrap();
		fs::create_dir_all(device).unwrap();
		symlink(target, device.join("subsystem")).unwrap();
	}

	fn disk(sys: &Path, name: &str, number: &str, external: bool) -> PathBuf {
		let controller = sys.join("devices/pci0000:00/0000:00:01.0");
		fs::create_dir_all(sys.join("devices/pci0000:00/pci_bus")).unwrap();
		if !controller.join("subsystem").exists() {
			subsystem(sys, &controller, "pci");
		}
		let host = if external {
			let bridge = controller.join(format!("usb-{name}"));
			subsystem(sys, &bridge, "usb");
			bridge.join(name)
		} else {
			controller.join(name)
		};
		subsystem(sys, &host, "scsi");
		let device = host.join("block").join(name);
		fs::create_dir_all(device.join("slaves")).unwrap();
		subsystem(sys, &device, "block");
		write(&device.join("removable"), b"0\n");
		symlink(&host, device.join("device")).unwrap();
		fs::create_dir_all(sys.join("dev/block")).unwrap();
		symlink(&device, sys.join("dev/block").join(number)).unwrap();
		device
	}

	fn class(sys: &Path, number: &str, fs_type: &[u8]) -> StorageClass {
		let (major, minor) = number.split_once(':').unwrap();
		classify_mount(
			&Mount {
				major: major.parse().unwrap(),
				minor: minor.parse().unwrap(),
				fs_type,
			},
			sys,
		)
	}

	#[test]
	fn mount_identity_uses_ids_and_accepts_non_utf8_paths() {
		let info = b"41 1 8:1 / /same rw - ext4 /dev/a rw\n42 1 0:12 /a\\040b /same/\xff rw shared:1 - nfs server:/x rw\n";

		let mount = find_mount(info, 42).unwrap();
		assert_eq!(mount.fs_type, b"nfs");
		assert_eq!((mount.major, mount.minor), (0, 12));
		assert_eq!(find_mount(info, 41).unwrap().fs_type, b"ext4");
		assert!(find_mount(info, 43).is_none());
		assert!(find_mount(b"42 1 8:1 / / rw - ext4", 42).is_none());
		assert!(find_mount(&[info.as_slice(), info.as_slice()].concat(), 42).is_none());
		assert_eq!(parse_fdinfo(b"pos:\t0\nmnt_id:\t42\n"), Some(42));
		assert_eq!(parse_fdinfo(b"mnt_id:\t18446744073709551616\n"), None);
	}

	#[test]
	fn network_volatile_and_unresolved_filesystems() {
		let dir = TestDir::new();
		for fs in [b"nfs".as_slice(), b"cifs", b"ceph", b"fuse.sshfs"] {
			assert_eq!(class(dir.path(), "0:1", fs), StorageClass::Network);
		}
		for fs in [b"tmpfs".as_slice(), b"ramfs"] {
			assert_eq!(class(dir.path(), "0:1", fs), StorageClass::Volatile);
		}
		for fs in [b"overlay".as_slice(), b"fuse", b"fuse.unknown", b"btrfs"] {
			assert_eq!(class(dir.path(), "0:1", fs), StorageClass::Unknown);
		}
	}

	#[test]
	fn physical_partition_usb_ssd_and_sd() {
		let dir = TestDir::new();
		let sys = dir.path();
		let internal = disk(sys, "sda", "8:0", false);
		let partition = internal.join("sda1");
		write(&partition.join("partition"), b"1\n");
		symlink(&partition, sys.join("dev/block/8:1")).unwrap();
		let usb = disk(sys, "sdb", "8:16", true);
		let sd = disk(sys, "mmcblk0", "179:0", false);
		write(&sd.join("device/type"), b"SD\n");

		assert_eq!(class(sys, "8:0", b"ext4"), StorageClass::Fixed);
		assert_eq!(class(sys, "8:1", b"xfs"), StorageClass::Fixed);
		assert_eq!(fs::read(usb.join("removable")).unwrap(), b"0\n");
		assert_eq!(class(sys, "8:16", b"ext4"), StorageClass::Removable);
		assert_eq!(class(sys, "179:0", b"ext4"), StorageClass::Removable);
	}

	#[test]
	fn removable_ancestors_and_incomplete_ancestry() {
		let dir = TestDir::new();
		let sys = dir.path();
		let device = disk(sys, "sda", "8:0", false);
		write(&device.join("device/removable"), b"1\n");

		assert_eq!(class(sys, "8:0", b"ext4"), StorageClass::Removable);

		write(&device.join("device/removable"), b"unknown\n");
		assert_eq!(class(sys, "8:0", b"ext4"), StorageClass::Unknown);

		fs::remove_file(device.join("device")).unwrap();
		symlink(sys.join("devices/disappeared"), device.join("device")).unwrap();
		assert_eq!(class(sys, "8:0", b"ext4"), StorageClass::Unknown);
		assert_eq!(class(sys, "8:99", b"ext4"), StorageClass::Unknown);
	}

	#[test]
	fn usb_ssd_without_resolved_external_ancestry_is_not_fixed() {
		let dir = TestDir::new();
		let sys = dir.path();
		let device = disk(sys, "sdb", "8:16", true);
		let scsi = device.join("device").canonicalize().unwrap();
		let bridge = scsi.parent().unwrap();

		fs::remove_file(bridge.join("subsystem")).unwrap();
		assert_eq!(class(sys, "8:16", b"ext4"), StorageClass::Unknown);

		symlink(sys.join("bus/missing"), bridge.join("subsystem")).unwrap();
		assert_eq!(class(sys, "8:16", b"ext4"), StorageClass::Unknown);
	}

	#[test]
	fn unsupported_virtual_device_and_cycles_are_unknown() {
		let dir = TestDir::new();
		let sys = dir.path();
		let device = sys.join("devices/virtual/block/loop0");
		fs::create_dir_all(device.join("slaves")).unwrap();
		fs::create_dir_all(sys.join("dev/block")).unwrap();
		subsystem(sys, &device, "block");
		write(&device.join("removable"), b"0\n");
		symlink(&device, sys.join("dev/block/7:0")).unwrap();

		assert_eq!(class(sys, "7:0", b"ext4"), StorageClass::Unknown);

		write(&device.join("dm/uuid"), b"CRYPT-test\n");
		symlink(&device, device.join("slaves/self")).unwrap();
		assert_eq!(class(sys, "7:0", b"ext4"), StorageClass::Unknown);
	}

	#[test]
	fn device_mapper_requires_all_backings_and_supported_topology() {
		let dir = TestDir::new();
		let sys = dir.path();
		let internal = disk(sys, "sda", "8:0", false);
		let external = disk(sys, "sdb", "8:16", true);
		let dm = sys.join("devices/virtual/block/dm-0");
		fs::create_dir_all(dm.join("slaves")).unwrap();
		subsystem(sys, &dm, "block");
		write(&dm.join("dm/uuid"), b"CRYPT-LUKS2-test\n");
		symlink(&internal, dm.join("slaves/sda")).unwrap();
		symlink(&dm, sys.join("dev/block/253:0")).unwrap();

		assert_eq!(class(sys, "253:0", b"ext4"), StorageClass::Fixed);

		write(&dm.join("dm/uuid"), b"unsupported\n");
		assert_eq!(class(sys, "253:0", b"ext4"), StorageClass::Unknown);

		write(&dm.join("dm/uuid"), b"CRYPT-LUKS2-test\n");
		symlink(sys.join("devices/missing"), dm.join("slaves/missing")).unwrap();
		assert_eq!(class(sys, "253:0", b"ext4"), StorageClass::Unknown);

		symlink(&external, dm.join("slaves/sdb")).unwrap();
		assert_eq!(class(sys, "253:0", b"ext4"), StorageClass::Removable);
	}
}
