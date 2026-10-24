use zbus::{
	names::{BusName, InterfaceName},
	zvariant::ObjectPath,
};

pub const INDEXER_INTERFACE: InterfaceName =
	InterfaceName::from_static_str_checked("de.icytv.subniri.Indexer");
pub const INDEXER_BUS_NAME: BusName = BusName::from_static_str_checked("de.icytv.subniri.Indexer");
pub const FILE_INDEXER_PATH: ObjectPath =
	ObjectPath::from_static_str_checked("/de/icytv/subniri/Indexer");
pub const NIX_INDEXER_PATH: ObjectPath =
	ObjectPath::from_static_str_checked("/de/icytv/subniri/NixIndexer");

#[zbus::interface(
	name = "de.icytv.subniri.Indexer",
	proxy(
		default_service = "de.icytv.subniri.Indexer",
		default_path = "/de/icytv/subniri/Indexer",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait Indexer {
	#[zbus(property)]
	fn enbabled(&self) -> bool;

	#[zbus(property)]
	fn status(&self) -> IndexerStatus;

	async fn search(&self, query: &str, limit: u32) -> zbus::fdo::Result<Vec<Item>>;
	fn rescan(&mut self) -> zbus::fdo::Result<()>;
}

#[derive(Debug, serde::Deserialize, serde::Serialize, zvariant::Type)]
pub struct Item {
	pub path: String,
	pub parent: String,
	pub filename: String,
	pub mtime: u64,
	pub size: u64,
}

#[zbus::interface(
	name = "de.icytv.subniri.Indexer",
	proxy(
		default_service = "de.icytv.subniri.Indexer",
		default_path = "/de/icytv/subniri/NixIndexer",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait NixIndexer {
	#[zbus(property)]
	fn enabled(&self) -> bool;

	#[zbus(property)]
	fn status(&self) -> IndexerStatus;

	async fn search(&self, query: &str, limit: u32) -> zbus::fdo::Result<Vec<Package>>;
	async fn rescan(&mut self) -> zbus::fdo::Result<()>;
}

#[derive(
	Clone,
	Copy,
	Debug,
	PartialEq,
	serde::Serialize,
	serde::Deserialize,
	zvariant::Type,
	zvariant::Value,
	zvariant::OwnedValue,
)]
#[repr(u8)]
pub enum IndexerStatus {
	Idle = 0,
	Indexing,
	Disabled,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, zvariant::Type)]
pub struct Package {
	pub description: String,
	pub attr_name: String,
	pub attr_path: String,
	pub pname: String,
	pub version: String,
}
