use std::{
	ffi::OsString,
	os::unix::{
		ffi::{OsStrExt, OsStringExt},
		fs::MetadataExt,
	},
	path::{Path, PathBuf},
	sync::mpsc,
};

use eyre::OptionExt;
use ignore::{WalkParallel, WalkState};
use tantivy::{
	Index, IndexWriter, doc,
	schema::{FAST, Field, STORED, STRING, Schema, TEXT},
};

pub struct Database {
	schema: Schema,
	fields: DbFields,
	index: Index,
	index_writer: IndexWriter,
}

struct DbFields {
	path: Field,
	path_raw: Field,
	parent: Field,
	filename: Field,
	extensions: Field,
	mtime: Field,
	size: Field,
	is_dir: Field,
	inode: Field,
}

impl Database {
	pub fn memory() -> eyre::Result<Self> {
		let (schema, fields) = Self::schema();
		let index = Index::create_in_ram(schema.clone());

		let index_writer: IndexWriter = index.writer(100_000_000)?;

		Ok(Self {
			schema,
			fields,
			index,
			index_writer,
		})
	}

	fn schema() -> (Schema, DbFields) {
		let mut schema_builder = Schema::builder();
		let path = schema_builder.add_text_field("path", TEXT | STORED);
		let path_raw = schema_builder.add_text_field("path_raw", STRING | STORED);
		let parent = schema_builder.add_text_field("parent", STRING);
		let filename = schema_builder.add_text_field("filename", TEXT);
		let extensions = schema_builder.add_text_field("extensions", STRING);
		let mtime = schema_builder.add_i64_field("mtime", FAST | STORED);
		let size = schema_builder.add_u64_field("size", FAST | STORED);
		let is_dir = schema_builder.add_bool_field("is_dir", FAST);
		let inode = schema_builder.add_u64_field("inode", FAST);

		let fields = DbFields {
			path,
			path_raw,
			parent,
			filename,
			extensions,
			mtime,
			size,
			is_dir,
			inode,
		};

		(schema_builder.build(), fields)
	}

	pub fn rescan(&mut self) -> eyre::Result<()> {
		let walk = ignore::WalkBuilder::new("/").build_parallel();

		let (sender, receiver) = mpsc::channel();

		std::thread::spawn(move || {
			walk.run(move || {
				let sender = sender.clone();
				Box::new(move |entry| {
					if let Ok(entry) = entry {
						let _ = sender.send(entry);
					}

					WalkState::Continue
				})
			});
		});

		while let Ok(entry) = receiver.recv() {
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
			let mtime = metadata.as_ref().map(|m| m.mtime()).unwrap_or(0);
			let size = metadata.as_ref().map(|m| m.size()).unwrap_or(0);
			let inode = metadata.as_ref().map(|m| m.ino()).unwrap_or(0);
			let is_dir = metadata.as_ref().is_some_and(|m| m.is_dir());

			self.index_writer.add_document(doc!(
				self.fields.path => *path_str,
				self.fields.path_raw => *path_str,
				self.fields.parent => *parent,
				self.fields.filename => *filename,
				self.fields.extensions => *extensions,
				self.fields.mtime => mtime,
				self.fields.size => size,
				self.fields.is_dir => is_dir,
				self.fields.inode => inode,
			))?;
		}

		self.index_writer.commit()?;

		Ok(())
	}

	pub fn dump(&self) {}
}

// #[derive(Debug)]
// pub struct Item {
// 	id: i64,
// 	kind: Kind,
// 	title: Option<String>,
// 	path: PathBuf,
// 	size: i64,
// 	subtitle: Option<String>,
// 	tags: Option<String>,
// 	accessed: Option<jiff::Timestamp>,
// 	modified: Option<jiff::Timestamp>,
// 	last_seen_at: jiff::Timestamp,
// 	last_used_at: Option<jiff::Timestamp>,
// 	use_count: i64,
// }

#[derive(Debug)]
pub enum Kind {
	File,
	Dir,
	Project,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test() {
		let mut db = Database::memory().unwrap();
		db.rescan().unwrap();
	}
}
