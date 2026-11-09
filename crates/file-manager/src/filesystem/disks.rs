use std::{collections::HashMap, ffi::CString};

use serde::{Deserialize, Deserializer, de};
use zbus::{
	names::{BusName, InterfaceName},
	proxy,
	zvariant::{self, ObjectPath, OwnedObjectPath, OwnedValue},
};

pub(crate) struct Disks {
	connection: zbus::Connection,
}

impl Disks {
	pub async fn new() -> zbus::Result<Self> {
		let connection = zbus::Connection::system().await?;
		Ok(Self { connection })
	}

	pub async fn objects(&self) -> zbus::Result<HashMap<OwnedObjectPath, UDisks2Object>> {
		let proxy = UDisks2Proxy::new(&self.connection).await?;
		proxy.get_managed_objects().await
	}
}

const INTERFACE: InterfaceName<'static> =
	InterfaceName::from_static_str_checked("org.freedesktop.DBus.ObjectManager");
const PATH: ObjectPath<'static> = ObjectPath::from_static_str_checked("/org/freedesktop/UDisks2");
const SERVICE: BusName<'_> = BusName::from_static_str_checked("org.freedesktop.UDisks2");

#[proxy(
	interface = INTERFACE,
	default_path = PATH,
	default_service = SERVICE,
)]
pub trait UDisks2 {
	#[zbus(name = "GetManagedObjects")]
	fn get_managed_objects(&self) -> zbus::Result<HashMap<OwnedObjectPath, UDisks2Object>>;
}

#[derive(Debug, zvariant::Type, Default, Clone)]
#[zvariant(signature = "a{sa{sv}}")]
pub struct UDisks2Object {
	pub drive: DriveProperties,

	pub block: BlockProperties,

	pub filesystem: Option<FilesystemProperties>,
}

impl UDisks2Object {
	pub fn should_show(&self) -> bool {
		!self.block.hint_ignore && self.filesystem.is_some()
	}
}

impl<'de> Deserialize<'de> for UDisks2Object {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let mut interfaces =
			HashMap::<String, HashMap<String, OwnedValue>>::deserialize(deserializer)?;

		Ok(Self {
			drive: interfaces
				.remove("org.freedesktop.UDisks2.Drive")
				.map(decode_drive_properties)
				.transpose()?
				.unwrap_or_default(),
			block: interfaces
				.remove("org.freedesktop.UDisks2.Block")
				.map(decode_block_properties)
				.transpose()?
				.unwrap_or_default(),
			filesystem: interfaces
				.remove("org.freedesktop.UDisks2.Filesystem")
				.map(decode_filesystem_properties)
				.transpose()?,
		})
	}
}

fn take_property<T, E>(properties: &mut HashMap<String, OwnedValue>, name: &str) -> Result<T, E>
where
	T: Default + TryFrom<OwnedValue>,
	T::Error: std::fmt::Display,
	E: de::Error,
{
	properties
		.remove(name)
		.map(T::try_from)
		.transpose()
		.map_err(E::custom)
		.map(Option::unwrap_or_default)
}

fn decode_drive_properties<E>(
	mut properties: HashMap<String, OwnedValue>,
) -> Result<DriveProperties, E>
where
	E: de::Error,
{
	Ok(DriveProperties {
		model: take_property(&mut properties, "Model")?,
		vendor: take_property(&mut properties, "Vendor")?,
		serial: take_property(&mut properties, "Serial")?,
		size: take_property(&mut properties, "Size")?,
		connection_bus: take_property(&mut properties, "ConnectionBus")?,
		ejectable: take_property(&mut properties, "Ejectable")?,
		can_power_off: take_property(&mut properties, "CanPowerOff")?,
		sibling_id: take_property(&mut properties, "SiblingId")?,
		time_detected: take_property(&mut properties, "TimeDetected")?,
		time_media_detected: take_property(&mut properties, "TimeMediaDetected")?,
	})
}

fn decode_block_properties<E>(
	mut properties: HashMap<String, OwnedValue>,
) -> Result<BlockProperties, E>
where
	E: de::Error,
{
	Ok(BlockProperties {
		id_label: take_property(&mut properties, "IdLabel")?,
		id_uuid: take_property(&mut properties, "IdUUID")?,
		id_type: take_property(&mut properties, "IdType")?,
		size: take_property(&mut properties, "Size")?,
		drive: take_property(&mut properties, "Drive")?,
		device: CString::from_vec_with_nul(take_property::<Vec<u8>, _>(&mut properties, "Device")?)
			.map_err(|e| E::custom(format!("Invalid device path: {}", e)))?,
		hint_ignore: take_property(&mut properties, "HintIgnore")?,
		hint_system: take_property(&mut properties, "HintSystem")?,
		device_number: take_property(&mut properties, "DeviceNumber")?,
		id_usage: take_property(&mut properties, "IdUsage")?,
	})
}

fn decode_filesystem_properties<E>(
	mut properties: HashMap<String, OwnedValue>,
) -> Result<FilesystemProperties, E>
where
	E: de::Error,
{
	Ok(FilesystemProperties {
		mount_points: take_property::<Vec<Vec<u8>>, _>(&mut properties, "MountPoints")?
			.into_iter()
			.map(CString::from_vec_with_nul)
			.collect::<Result<Vec<_>, _>>()
			.map_err(|e| E::custom(format!("Invalid mount point: {}", e)))?,
	})
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct DriveProperties {
	#[serde(rename = "Model", default)]
	pub model: String,

	#[serde(rename = "Vendor", default)]
	pub vendor: String,

	#[serde(rename = "Serial", default)]
	pub serial: String,

	#[serde(rename = "Size", default)]
	pub size: u64,

	#[serde(rename = "ConnectionBus", default)]
	pub connection_bus: String,

	#[serde(rename = "Ejectable", default)]
	pub ejectable: bool,

	pub can_power_off: bool,

	pub sibling_id: String,

	pub time_detected: u64,

	pub time_media_detected: u64,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct BlockProperties {
	#[serde(rename = "IdLabel", default)]
	pub id_label: String,

	#[serde(rename = "IdUUID", default)]
	pub id_uuid: String,

	#[serde(rename = "IdType", default)]
	pub id_type: String, // e.g., "ext4", "vfat"

	#[serde(rename = "Size", default)]
	pub size: u64,

	#[serde(rename = "Drive", default)]
	pub drive: OwnedObjectPath,

	// D-Bus returns paths as byte arrays.
	// Convert to string using: String::from_utf8_lossy(&device).to_string()
	#[serde(rename = "Device", default)]
	pub device: CString,

	#[serde(rename = "HintIgnore", default)]
	pub hint_ignore: bool,

	#[serde(rename = "HintSystem", default)]
	pub hint_system: bool,

	pub device_number: u64,

	pub id_usage: String,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct FilesystemProperties {
	// Array of byte arrays representing mount paths
	#[serde(rename = "MountPoints", default)]
	pub mount_points: Vec<CString>,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn test() -> Result<(), Box<dyn std::error::Error>> {
		let conn = zbus::Connection::system().await?;

		let proxy = UDisks2Proxy::new(&conn).await?;

		let objects = proxy.get_managed_objects().await?;

		for (path, object) in &objects {
			if !object.should_show() {
				continue;
			}
			println!("===================");
			println!("  Object path: {}", path);
			println!("  Block device: {}", object.block.device.to_string_lossy());
			println!("  Drive model: {}", object.drive.model);
			println!("    Ejectable: {}", object.drive.ejectable);
			println!("    Vendor: {}", object.drive.vendor);

			println!("  id_type: {}", object.block.id_type);
			println!("  drive: {}", object.block.drive);
			// println!(
			// 	"  Filesystem mount points: {:?}",
			// 	object
			// 		.filesystem
			// 		.mount_points
			// 		.iter()
			// 		.map(|mp| mp.to_string_lossy().to_string())
			// 		.collect::<Vec<_>>()
			// );
		}

		Ok(())
	}
}
