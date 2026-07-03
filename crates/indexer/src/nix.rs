use std::{collections::HashMap, path::Path};

use serde::{Deserialize, Serialize};
use tantivy::{
	DocId, Index, IndexReader, IndexWriter, Score, SegmentReader, TantivyDocument,
	collector::TopDocs, query::QueryParser, schema::Schema,
};
use tantivy_derive::Schema as _;
use tokio::process::Command;

const FIELD_ATTR_NAME: &str = "attr_name";
const FIELD_ATTR_NAME_EXACT: &str = "attr_name_exact";
const FIELD_DESCRIPTION: &str = "description";
const FIELD_PNAME: &str = "pname";
const FIELD_PNAME_EXACT: &str = "pname_exact";
const FIELD_PNAME_LEN: &str = "pname_len";

const DESCRIPTION_FIELD_BOOST: Score = 1.0;
const PNAME_FIELD_BOOST: Score = DESCRIPTION_FIELD_BOOST * 4.0;
const ATTR_NAME_FIELD_BOOST: Score = DESCRIPTION_FIELD_BOOST * 8.0;

const PNAME_EXACT_BONUS: Score = PNAME_FIELD_BOOST * 3.0;
const PNAME_PREFIX_BONUS: Score = PNAME_FIELD_BOOST;
const PNAME_CONTAINS_BONUS: Score = PNAME_FIELD_BOOST * 0.5;
const ATTR_NAME_EXACT_BONUS: Score = ATTR_NAME_FIELD_BOOST + 3.0;
const ATTR_NAME_PREFIX_BONUS: Score = ATTR_NAME_FIELD_BOOST - 1.0;
const ATTR_NAME_CONTAINS_BONUS: Score = ATTR_NAME_FIELD_BOOST + 2.0;
const PNAME_LENGTH_PENALTY: Score = 0.02;

const FUZZY_DISTANCE_SHORT_QUERY: u8 = 0;
const FUZZY_DISTANCE_MEDIUM_QUERY: u8 = 1;
const FUZZY_DISTANCE_LONG_QUERY: u8 = 2;
const SHORT_QUERY_MAX_CHARS: usize = 2;
const MEDIUM_QUERY_MAX_CHARS: usize = 5;

pub struct NixDatabase {
	schema: Schema,
	index: Index,
	index_writer: IndexWriter,
	index_reader: IndexReader,
}

impl NixDatabase {
	pub fn path(cache_path: impl AsRef<Path>) -> eyre::Result<Self> {
		let schema = Package::schema();
		let index = match Index::create_in_dir(&cache_path, schema.clone()) {
			Ok(index) => index,
			Err(tantivy::TantivyError::IndexAlreadyExists) => Index::open_in_dir(cache_path)?,
			Err(e) => return Err(e.into()),
		};

		let index_writer = index.writer(100_000_000)?;
		let index_reader: IndexReader = index
			.reader_builder()
			.reload_policy(tantivy::ReloadPolicy::OnCommitWithDelay)
			.try_into()?;

		Ok(Self {
			schema,
			index,
			index_writer,
			index_reader,
		})
	}

	pub async fn rescan(&mut self) -> eyre::Result<()> {
		let output = Command::new("nix")
			.args(["flake", "metadata", "nixpkgs", "--json"])
			.output()
			.await?;

		if !output.status.success() {
			eyre::bail!(
				"Failed to resolve flake metadata: {}",
				String::from_utf8_lossy(&output.stderr)
			);
		}

		let stdout = String::from_utf8_lossy(&output.stdout);
		let metadata = serde_json::from_str::<FlakeMetadata>(&stdout)?;

		let output = Command::new("nix")
			.args(["search", "--json", &metadata.resolved_url, "^"])
			.output()
			.await?;

		if !output.status.success() {
			eyre::bail!(
				"Failed to search flake metadata: {}",
				String::from_utf8_lossy(&output.stderr)
			);
		}

		let stdout = String::from_utf8_lossy(&output.stdout);
		let packages: HashMap<String, Package> = serde_json::from_str(&stdout)?;

		for (attr_path, package) in packages.into_iter() {
			let package = package.with_calculated(attr_path);
			self.index_writer.add_document(package.into())?;
		}

		self.index_writer.commit()?;
		self.index_reader.reload()?;

		Ok(())
	}

	pub fn search(&self, query: impl AsRef<str>, limit: usize) -> eyre::Result<Vec<StoredPackage>> {
		let query_text = query.as_ref();
		let searcher = self.index_reader.searcher();

		let attr_name = self.schema.get_field(FIELD_ATTR_NAME)?;
		let pname = self.schema.get_field(FIELD_PNAME)?;
		let description = self.schema.get_field(FIELD_DESCRIPTION)?;

		let distance = match query_text.chars().count() {
			0..=SHORT_QUERY_MAX_CHARS => FUZZY_DISTANCE_SHORT_QUERY,
			..=MEDIUM_QUERY_MAX_CHARS => FUZZY_DISTANCE_MEDIUM_QUERY,
			_ => FUZZY_DISTANCE_LONG_QUERY,
		};

		let mut query_parser =
			QueryParser::for_index(&self.index, vec![attr_name, pname, description]);
		query_parser.set_conjunction_by_default();
		query_parser.set_field_fuzzy(attr_name, true, distance, true);
		query_parser.set_field_fuzzy(pname, true, distance, true);
		query_parser.set_field_boost(attr_name, ATTR_NAME_FIELD_BOOST);
		query_parser.set_field_boost(pname, PNAME_FIELD_BOOST);
		query_parser.set_field_boost(description, DESCRIPTION_FIELD_BOOST);

		let query = query_parser.parse_query(query_text)?;

		let query_text = query_text.to_lowercase();
		let top_docs = TopDocs::with_limit(limit).tweak_score(move |segment_reader| {
			package_score_for_segment(segment_reader, query_text.clone())
		});

		let top_docs: Vec<(_, tantivy::DocAddress)> = searcher.search(&query, &top_docs)?;

		top_docs
			.into_iter()
			.map(|(_score, addr)| {
				let retreived_doc = searcher.doc::<TantivyDocument>(addr)?;
				let item: StoredPackage = retreived_doc.into();
				Ok(item)
			})
			.collect::<Result<Vec<_>, _>>()
	}
}

fn package_score_for_segment(
	segment_reader: &SegmentReader, query_text: String,
) -> impl Fn(DocId, Score) -> Score + 'static + use<> {
	let attr_name = segment_reader
		.fast_fields()
		.str(FIELD_ATTR_NAME_EXACT)
		.expect("attr_name_exact fast field")
		.expect("attr_name_exact fast field");
	let pname = segment_reader
		.fast_fields()
		.str(FIELD_PNAME_EXACT)
		.expect("pname_exact fast field")
		.expect("pname_exact fast field");
	let pname_len = segment_reader
		.fast_fields()
		.u64(FIELD_PNAME_LEN)
		.expect("pname_len fast field");

	move |doc, score| {
		let attr_name_bonus = attr_name
			.term_ords(doc)
			.next()
			.and_then(|ord| string_from_ord(&attr_name, ord))
			.map_or(0.0, |attr_name| {
				attr_name_score_bonus(&attr_name, &query_text)
			});

		let pname_bonus = pname
			.term_ords(doc)
			.next()
			.and_then(|ord| string_from_ord(&pname, ord))
			.map_or(0.0, |pname| pname_score_bonus(&pname, &query_text));

		let penalty = pname_len.first(doc).unwrap_or(0) as Score * PNAME_LENGTH_PENALTY;
		score + attr_name_bonus + pname_bonus - penalty
	}
}

fn string_from_ord(column: &tantivy::columnar::StrColumn, ord: u64) -> Option<String> {
	let mut output = String::new();
	column
		.ord_to_str(ord, &mut output)
		.ok()
		.and_then(|found| found.then_some(output.to_lowercase()))
}

fn attr_name_score_bonus(attr_name: &str, query_text: &str) -> Score {
	match attr_name {
		name if name == query_text => ATTR_NAME_EXACT_BONUS,
		name if name.starts_with(query_text) => ATTR_NAME_PREFIX_BONUS,
		name if name.contains(query_text) => ATTR_NAME_CONTAINS_BONUS,
		_ => 0.0,
	}
}

fn pname_score_bonus(pname: &str, query_text: &str) -> Score {
	match pname {
		name if name == query_text => PNAME_EXACT_BONUS,
		name if name.starts_with(query_text) => PNAME_PREFIX_BONUS,
		name if name.contains(query_text) => PNAME_CONTAINS_BONUS,
		_ => 0.0,
	}
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlakeMetadata {
	resolved_url: String,
}

#[tantivy_derive::tantivy_document]
#[derive(Debug, Deserialize)]
pub struct Package {
	#[tantivy(text, stored)]
	description: String,
	#[tantivy(text, stored)]
	#[serde(skip)]
	attr_name: String,
	#[tantivy(string, fast)]
	#[serde(skip)]
	attr_name_exact: String,
	#[tantivy(string, stored)]
	#[serde(skip)]
	attr_path: String,
	#[tantivy(text, stored)]
	pname: String,
	#[tantivy(string, fast)]
	#[serde(skip)]
	pname_exact: String,
	#[tantivy(fast)]
	#[serde(skip)]
	pname_len: u64,
	#[tantivy(string, stored)]
	version: String,
}

impl Package {
	#[inline]
	#[must_use]
	fn with_calculated(mut self, attr_path: String) -> Self {
		self.attr_name = attr_path
			.rsplit_once('.')
			.map_or_else(|| attr_path.clone(), |(_, name)| name.to_owned());
		self.attr_name_exact.clone_from(&self.attr_name);
		self.attr_path = attr_path;
		self.pname_exact = self.pname.clone();
		self.pname_len = self.pname.chars().count() as u64;
		self
	}
}

impl From<StoredPackage> for indexer_common::Package {
	fn from(value: StoredPackage) -> Self {
		Self {
			description: value.description,
			attr_name: value.attr_name,
			attr_path: value.attr_path,
			pname: value.pname,
			version: value.version,
		}
	}
}
