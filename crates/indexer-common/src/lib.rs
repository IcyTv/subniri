use zbus::{
	names::{BusName, InterfaceName},
	zvariant::ObjectPath,
};

pub const INDEXER_INTERFACE: InterfaceName =
	InterfaceName::from_static_str_checked("de.icytv.subniri.Indexer");
pub const INDEXER_BUS_NAME: BusName = BusName::from_static_str_checked("de.icytv.subniri.Indexer");
pub const INDEXER_PATH: ObjectPath =
	ObjectPath::from_static_str_checked("/de/icytv/subniri/Indexer");

pub const INDEXER_SEARCH: &str = "Search";

#[zbus::proxy(
	interface = INDEXER_INTERFACE,
	default_service = INDEXER_BUS_NAME,
	default_path = INDEXER_PATH,
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
