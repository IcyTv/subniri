use std::{
	os::unix::fs::MetadataExt,
	path::{Path, PathBuf},
	sync::mpsc,
};

use ignore::WalkState;
use tantivy::{
	DocAddress, Index, IndexReader, IndexWriter, ReloadPolicy, Score, TantivyDocument,
	collector::TopDocs, query::QueryParser, schema::Schema,
};
use tantivy_derive::{Schema as _, tantivy_document};

pub mod nix;

pub struct Database {
	path: PathBuf,
	schema: Schema,
	index: Index,
	index_writer: IndexWriter,
	index_reader: IndexReader,
}

impl Database {
	pub fn memory(path: impl AsRef<Path>) -> eyre::Result<Self> {
		let schema = Item::schema();
		let index = Index::create_in_ram(schema.clone());

		let index_writer: IndexWriter = index.writer(100_000_000)?;
		let index_reader: IndexReader = index
			.reader_builder()
			.reload_policy(ReloadPolicy::OnCommitWithDelay)
			.try_into()?;

		Ok(Self {
			path: path.as_ref().to_path_buf(),
			schema,
			index,
			index_writer,
			index_reader,
		})
	}

	pub fn path(path: impl AsRef<Path>, cache_path: impl AsRef<Path>) -> eyre::Result<Self> {
		let schema = Item::schema();
		let index = match Index::create_in_dir(&cache_path, schema.clone()) {
			Ok(index) => index,
			Err(tantivy::TantivyError::IndexAlreadyExists) => Index::open_in_dir(cache_path)?,
			Err(e) => return Err(e.into()),
		};

		let index_writer = index.writer(100_000_000)?;
		let index_reader: IndexReader = index
			.reader_builder()
			.reload_policy(ReloadPolicy::OnCommitWithDelay)
			.try_into()?;

		Ok(Self {
			path: path.as_ref().to_path_buf(),
			schema,
			index,
			index_writer,
			index_reader,
		})
	}

	pub fn rescan(&mut self) -> eyre::Result<()> {
		let walk = ignore::WalkBuilder::new(&self.path)
			.ignore(true)
			.hidden(false)
			.build_parallel();

		let (sender, receiver) = mpsc::channel();

		std::thread::spawn(move || {
			walk.run(move || {
				let sender = sender.clone();
				Box::new(move |entry| {
					if let Ok(entry) = entry {
						let item = Item::from(entry);
						let _ = sender.send(item);
					}

					WalkState::Continue
				})
			});
		});

		while let Ok(item) = receiver.recv() {
			self.index_writer.add_document(item.into())?;
		}

		self.index_writer.commit()?;
		self.index_reader.reload()?;

		Ok(())
	}

	pub fn search(&self, query: impl AsRef<str>, limit: usize) -> eyre::Result<Vec<StoredItem>> {
		let searcher = self.index_reader.searcher();

		let path = self.schema.get_field("path")?;
		let filename = self.schema.get_field("filename")?;

		let mut query_parser = QueryParser::for_index(&self.index, vec![path, filename]);
		query_parser.set_conjunction_by_default();
		query_parser.set_field_boost(filename, 3.0);

		let distance = match query.as_ref().chars().count() {
			0..=2 => 0,
			3..=5 => 1,
			_ => 2,
		};
		// query_parser.set_field_fuzzy(path, true, distance, true);
		query_parser.set_field_fuzzy(filename, true, distance, true);

		let query = query_parser.parse_query(query.as_ref())?;

		let top_docs: Vec<(Score, DocAddress)> =
			searcher.search(&query, &TopDocs::with_limit(limit).order_by_score())?;

		top_docs
			.into_iter()
			.map(|(_score, addr)| {
				let retreived_doc = searcher.doc::<TantivyDocument>(addr)?;
				let item: StoredItem = retreived_doc.into();
				Ok(item)
			})
			.collect::<Result<Vec<_>, _>>()
	}

	pub fn dump(&self) {}
}

#[tantivy_document]
#[derive(Debug, serde::Serialize, zvariant::Type)]
pub struct Item {
	#[tantivy(text, stored)]
	pub path: String,
	// #[tantivy(string, stored)]
	// pub path_raw: String,
	#[tantivy(string, stored)]
	pub parent: String,
	#[tantivy(text, stored)]
	pub filename: String,
	#[tantivy(string)]
	pub extensions: String,
	#[tantivy(fast, stored)]
	pub mtime: u64,
	#[tantivy(fast, stored)]
	pub size: u64,
	#[tantivy(fast)]
	pub is_dir: bool,
	#[tantivy(fast)]
	pub inode: u64,
}

impl From<ignore::DirEntry> for Item {
	fn from(entry: ignore::DirEntry) -> Self {
		let path = entry.path();
		let path_str = path.to_string_lossy();
		let parent = path
			.parent()
			.map(|p| p.to_string_lossy())
			.unwrap_or_else(|| "".into());
		let filename = path
			.file_name()
			.map(|f| f.to_string_lossy())
			.unwrap_or_else(|| "".into());
		let extensions = path
			.extension()
			.map(|e| e.to_string_lossy())
			.unwrap_or_else(|| "".into());

		let metadata = path.metadata().ok();
		let mtime = metadata
			.as_ref()
			.and_then(|m| u64::try_from(m.mtime()).ok())
			.unwrap_or(0);
		let size = metadata.as_ref().map(|m| m.size()).unwrap_or(0);
		let inode = metadata.as_ref().map(|m| m.ino()).unwrap_or(0);
		let is_dir = metadata.as_ref().is_some_and(|m| m.is_dir());

		Self {
			path: path_str.to_string(),
			// path_raw: path_str.to_string(),
			parent: parent.to_string(),
			filename: filename.to_string(),
			extensions: extensions.to_string(),

			mtime,
			size,
			is_dir,
			inode,
		}
	}
}

#[derive(Debug)]
pub enum Kind {
	File,
	Dir,
	Project,
}

#[cfg(test)]
mod tests {
	use ignore::Walk;
	use rand::RngExt;

	use super::*;

	fn setup_test_dir() -> PathBuf {
		let dir = std::env::temp_dir().join(format!("{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();

		setup_test_folder(&dir, 5);

		let walk = Walk::new(&dir);

		for ent in walk {
			println!("{ent:?}");
		}

		dir
	}

	fn setup_test_folder(folder: &Path, depth: usize) {
		if depth > 0 {
			let subfolders = rand::rng().random_range(1..=depth);

			for _subfolder in 0..=subfolders {
				let fname = random_word::get(random_word::Lang::En);
				let folder = folder.join(fname);
				std::fs::create_dir_all(&folder).unwrap();
				setup_test_folder(&folder, depth - 1);
			}
		}

		let num_files = if depth == 0 {
			rand::rng().random_range(5..=15)
		} else {
			rand::rng().random_range(0..=(depth * 2))
		};

		for _file in 0..=num_files {
			let name = if rand::rng().random_bool(0.2) {
				random_word::get_starts_with('d', random_word::Lang::En).unwrap()
			} else {
				random_word::get(random_word::Lang::En)
			};
			let file = folder.join(name);

			std::fs::write(&file, random_word::get(random_word::Lang::En)).unwrap();
		}
	}

	#[test]
	fn test() {
		let dir = setup_test_dir();

		let mut db = Database::memory(&dir).unwrap();
		db.rescan().unwrap();

		let res = db.search("d", 10).unwrap();
		println!("{res:?}");

		let _ = std::fs::remove_dir_all(dir);
	}
}
