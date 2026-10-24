use config::ConfigFile;
use indexer::{
	Database,
	nix::{self},
};
use indexer_common::{
	FILE_INDEXER_PATH, INDEXER_BUS_NAME, Indexer, IndexerServer, IndexerStatus, NIX_INDEXER_PATH,
	NixIndexer, NixIndexerServer,
};

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
		status: IndexerStatus::Idle,
		database,
	};

	let nix_service = NixShellProvider {
		database: nix::NixDatabase::path(&nix_cache_path)?,
	};

	log::info!("Launching indexer");

	let _connection = zbus::connection::Builder::session()?
		.name(INDEXER_BUS_NAME)?
		.serve_at(FILE_INDEXER_PATH, IndexerServer(service))?
		.serve_at(NIX_INDEXER_PATH, NixIndexerServer(nix_service))?
		.build()
		.await?;

	tokio::signal::ctrl_c().await?;

	Ok(())
}

struct IndexerDbus {
	database: Database,

	enabled: bool,
	status: IndexerStatus,
}

impl Indexer for IndexerDbus {
	fn enbabled(&self) -> bool {
		self.enabled
	}

	fn status(&self) -> IndexerStatus {
		self.status
	}

	async fn search(
		&self, query: &str, limit: u32,
	) -> zbus::fdo::Result<Vec<indexer_common::Item>> {
		log::debug!("Searching {query}");
		let start = jiff::Timestamp::now();
		let res = self
			.database
			.search(&query, limit as usize)
			.map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
		let elapsed = jiff::Timestamp::now().since(start).unwrap();
		log::debug!("Searching took {elapsed:#}",);
		Ok(res
			.into_iter()
			.map(|item| indexer_common::Item {
				path: item.path,
				parent: item.parent,
				filename: item.filename,
				mtime: item.mtime,
				size: item.size,
			})
			.collect())
	}

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

struct NixShellProvider {
	database: nix::NixDatabase,
}

impl NixIndexer for NixShellProvider {
	fn enabled(&self) -> bool {
		true
	}

	fn status(&self) -> IndexerStatus {
		IndexerStatus::Idle
	}

	async fn search(
		&self, query: &str, limit: u32,
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
