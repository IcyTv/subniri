use config::ConfigFile;
use indexer::{
	Database, StoredItem,
	nix::{self},
};
use indexer_common::{FILE_INDEXER_PATH, INDEXER_BUS_NAME, INDEXER_INTERFACE, NIX_INDEXER_PATH};
use serde::{Deserialize, Serialize};
use zvariant::{OwnedValue, Type, Value};

#[tokio::main]
async fn main() -> eyre::Result<()> {
	log::init!("indexer", "icepickd").map_err(|error| eyre::eyre!(error.to_string()))?;

	let (_doc, config) = ConfigFile::load()?;

	let cache_dir = dirs::cache_dir().unwrap_or_else(|| {
		log::warn!("No cache directory found, using temp directory to store index.");
		log::warn!("Set XDG_CACHE_HOME to specify a cache directory.");
		std::env::temp_dir()
	});

	let files_cache_path = cache_dir.join("subniri/index/files");
	let nix_cache_path = cache_dir.join("subniri/index/nix");

	std::fs::create_dir_all(&files_cache_path)?;
	std::fs::create_dir_all(&nix_cache_path)?;

	let database = Database::path("/", files_cache_path)?;

	let service = IndexerDbus {
		enabled: config.indexing.enabled,
		status: Status::Idle,
		database,
	};

	let nix_service = NixShellProvider {
		database: nix::NixDatabase::path(&nix_cache_path)?,
	};

	log::info!("Launching indexer");

	let _connection = zbus::connection::Builder::session()?
		.name(INDEXER_BUS_NAME)?
		.serve_at(FILE_INDEXER_PATH, service)?
		.serve_at(NIX_INDEXER_PATH, nix_service)?
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

struct NixShellProvider {
	database: nix::NixDatabase,
}

#[zbus::interface(name = INDEXER_INTERFACE)]
impl NixShellProvider {
	#[zbus(property, name = "Enabled")]
	fn enabled(&self) -> bool {
		true
	}

	#[zbus(property, name = "Status")]
	fn status(&self) -> Status {
		Status::Idle
	}

	#[zbus(name = "Search")]
	async fn search(
		&self, query: String, limit: u32,
	) -> zbus::fdo::Result<Vec<indexer_common::Package>> {
		log::debug!("Searching {query}");

		let start = jiff::Timestamp::now();
		let res = self
			.database
			.search(&query, limit as usize)
			.map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

		let res = res.into_iter().map(Into::into).collect();

		let elapsed = jiff::Timestamp::now().since(start).unwrap();
		log::debug!("Searching took {elapsed:#}",);

		Ok(res)
	}

	#[zbus(name = "Rescan")]
	async fn rescan(&mut self) -> zbus::fdo::Result<()> {
		log::debug!("Scanning nix packages");

		let start = jiff::Timestamp::now();
		self.database
			.rescan()
			.await
			.map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

		let elapsed = jiff::Timestamp::now().since(start).unwrap();
		log::debug!("Scanning done in {elapsed:#}");

		Ok(())
	}
}
