use config::ConfigFile;
use indexer::{Database, StoredItem};
use indexer_common::{INDEXER_BUS_NAME, INDEXER_INTERFACE, INDEXER_PATH};
use serde::{Deserialize, Serialize};
use zvariant::{OwnedValue, Type, Value};

#[tokio::main]
async fn main() -> eyre::Result<()> {
	let _ = pretty_env_logger::try_init();

	let (_doc, config) = ConfigFile::load()?;

	let cache_path = dirs::cache_dir()
		.unwrap_or_else(|| {
			log::warn!("No cache directory found, using temp directory to store index.");
			log::warn!("Set XDG_CACHE_HOME to specify a cache directory.");
			std::env::temp_dir()
		})
		.join("subniri/index");

	std::fs::create_dir_all(&cache_path)?;

	let database = Database::path("/", cache_path)?;

	let service = IndexerDbus {
		enabled: config.indexing.enabled,
		status: Status::Idle,
		database,
	};

	log::info!("Launching indexer");

	let _connection = zbus::connection::Builder::session()?
		.name(INDEXER_BUS_NAME)?
		.serve_at(INDEXER_PATH, service)?
		.build()
		.await?;

	tokio::signal::ctrl_c().await?;

	Ok(())
}

struct IndexerDbus {
	database: Database,

	enabled: bool,
	status: Status,
}

#[zbus::interface(name = INDEXER_INTERFACE)]
impl IndexerDbus {
	#[zbus(property, name = "Enbabled")]
	fn enabled(&self) -> bool {
		self.enabled
	}

	#[zbus(property, name = "Status")]
	fn status(&self) -> Status {
		self.status
	}

	#[zbus(name = "Search")]
	async fn search(&self, query: String, limit: u32) -> zbus::fdo::Result<Vec<StoredItem>> {
		log::debug!("Searching {query}");
		let start = jiff::Timestamp::now();
		let res = self
			.database
			.search(&query, limit as usize)
			.map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
		let elapsed = jiff::Timestamp::now().since(start).unwrap();
		log::debug!("Searching took {elapsed:#}",);
		Ok(res)
	}

	#[zbus(name = "Rescan")]
	fn rescan(&mut self) -> zbus::fdo::Result<()> {
		log::debug!("Scanning root directory");
		let start = jiff::Timestamp::now();
		self.database
			.rescan()
			.map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
		let elapsed = jiff::Timestamp::now().since(start).unwrap();
		log::debug!("Scanning done in {elapsed:#}");

		Ok(())
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Type, Value, OwnedValue)]
#[repr(u8)]
enum Status {
	Idle = 0,
	Indexing,
	Disabled,
}
