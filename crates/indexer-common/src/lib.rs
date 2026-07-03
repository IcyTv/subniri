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

pub const INDEXER_SEARCH: &str = "Search";

#[zbus::proxy(
	interface = INDEXER_INTERFACE,
	default_service = INDEXER_BUS_NAME,
	default_path = FILE_INDEXER_PATH,
)]
pub trait Indexer {
	#[zbus(name = "Search")]
	async fn search(&self, query: &str, limit: u32) -> zbus::Result<Vec<Item>>;
}

#[derive(Debug, serde::Deserialize, zvariant::Type)]
pub struct Item {
	pub path: String,
	pub parent: String,
	pub filename: String,
	pub mtime: u64,
	pub size: u64,
}

#[zbus::proxy(
	interface = INDEXER_INTERFACE,
	default_service = INDEXER_BUS_NAME,
	default_path = NIX_INDEXER_PATH
)]
pub trait NixIndexer {
	#[zbus(name = "Search")]
	async fn search(&self, query: &str, limit: u32) -> zbus::Result<Vec<Package>>;
}

#[derive(Debug, serde::Deserialize, serde::Serialize, zvariant::Type)]
pub struct Package {
	pub description: String,
	pub attr_name: String,
	pub attr_path: String,
	pub pname: String,
	pub version: String,
}
