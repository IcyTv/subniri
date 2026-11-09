//! Centralized app-local tags. The UI never knows the on-disk representation.
//! Paths are Unix byte strings, not lossy display names. External renames are not tracked.
use serde::{Deserialize, Serialize};
mod message;
mod update;
mod view;
pub use message::Message;
use std::{
	collections::{BTreeMap, BTreeSet},
	fs::{self, File, OpenOptions},
	io::Write,
	os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
	path::{Path, PathBuf},
	sync::{
		Arc,
		atomic::{AtomicU64, Ordering},
	},
};

pub type TagId = u64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
	pub name: String,
	pub parent: Option<TagId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Assignment {
	path: Vec<u8>,
	tags: BTreeSet<TagId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagData {
	version: u32,
	next_id: TagId,
	pub tags: BTreeMap<TagId, Tag>,
	assignments: Vec<Assignment>,
}

impl Default for TagData {
	fn default() -> Self {
		Self {
			version: 1,
			next_id: 2,
			tags: BTreeMap::from([(
				1,
				Tag {
					name: "Starred".into(),
					parent: None,
				},
			)]),
			assignments: Vec::new(),
		}
	}
}

#[derive(Debug, Clone)]
pub enum Edit {
	Create {
		name: String,
		parent: Option<TagId>,
	},
	Update {
		id: TagId,
		name: String,
		parent: Option<TagId>,
	},
	Assign {
		path: PathBuf,
		id: TagId,
		assigned: bool,
	},
}

impl TagData {
	pub fn assigned(&self, path: &Path) -> Vec<TagId> {
		self.assignments
			.iter()
			.find(|a| a.path == path.as_os_str().as_bytes())
			.map_or_else(Vec::new, |a| a.tags.iter().copied().collect())
	}

	pub fn matches(&self, path: &Path, filter: TagId) -> bool {
		self.assigned(path)
			.iter()
			.any(|id| self.descends_from(*id, filter))
	}

	pub fn descends_from(&self, mut id: TagId, ancestor: TagId) -> bool {
		// Bound traversal even if handed invalid data before validation.
		for _ in 0..=self.tags.len() {
			if id == ancestor {
				return true;
			}
			match self.tags.get(&id).and_then(|t| t.parent) {
				Some(parent) => id = parent,
				None => return false,
			}
		}
		false
	}

	pub fn label(&self, mut id: TagId) -> String {
		let mut names = Vec::new();
		for _ in 0..self.tags.len() {
			let Some(tag) = self.tags.get(&id) else {
				break;
			};
			names.push(tag.name.as_str());
			if let Some(parent) = tag.parent {
				id = parent;
			} else {
				break;
			}
		}
		names.reverse();
		names.join(" / ")
	}

	pub fn ordered(&self) -> Vec<(TagId, usize)> {
		fn append(
			data: &TagData, parent: Option<TagId>, depth: usize, result: &mut Vec<(TagId, usize)>,
		) {
			let mut children: Vec<_> = data
				.tags
				.iter()
				.filter(|(_, t)| t.parent == parent)
				.collect();
			children.sort_by(|(_, a), (_, b)| a.name.cmp(&b.name));
			for (&id, _) in children {
				result.push((id, depth));
				append(data, Some(id), depth + 1, result);
			}
		}
		let mut result = Vec::new();
		append(self, None, 0, &mut result);
		result
	}

	fn apply(&mut self, edit: Edit) -> Result<(), String> {
		match edit {
			Edit::Create { name, parent } => {
				let id = self.next_id;
				self.check_tag(id, &name, parent)?;
				self.next_id = id.checked_add(1).ok_or("Tag ID limit reached")?;
				self.tags.insert(
					id,
					Tag {
						name: name.trim().into(),
						parent,
					},
				);
			}
			Edit::Update { id, name, parent } => {
				if !self.tags.contains_key(&id) {
					return Err("Tag no longer exists".into());
				}
				self.check_tag(id, &name, parent)?;
				self.tags.insert(
					id,
					Tag {
						name: name.trim().into(),
						parent,
					},
				);
			}
			Edit::Assign { path, id, assigned } => {
				if !path.is_absolute() {
					return Err("Tag assignments need absolute paths".into());
				}
				if !self.tags.contains_key(&id) {
					return Err("Tag no longer exists".into());
				}
				let bytes = path.as_os_str().as_bytes();
				if let Some(assignment) = self.assignments.iter_mut().find(|a| a.path == bytes) {
					if assigned {
						assignment.tags.insert(id);
					} else {
						assignment.tags.remove(&id);
					}
				} else if assigned {
					self.assignments.push(Assignment {
						path: bytes.to_vec(),
						tags: BTreeSet::from([id]),
					});
				}
				self.assignments.retain(|a| !a.tags.is_empty());
			}
		}
		Ok(())
	}

	fn check_tag(&self, id: TagId, name: &str, parent: Option<TagId>) -> Result<(), String> {
		let name = name.trim();
		if name.is_empty() || name.contains('/') || name.chars().any(char::is_control) {
			return Err(
				"Tag names must be nonempty and cannot contain / or control characters".into(),
			);
		}
		if let Some(parent) = parent {
			if !self.tags.contains_key(&parent) {
				return Err("Parent tag does not exist".into());
			}
			if self.descends_from(parent, id) {
				return Err("A tag cannot be nested inside itself or its descendants".into());
			}
		}
		if self.tags.iter().any(|(&other, t)| {
			other != id && t.parent == parent && t.name.to_lowercase() == name.to_lowercase()
		}) {
			return Err("A tag with that name already exists under this parent".into());
		}
		Ok(())
	}

	fn validate(&self) -> Result<(), String> {
		if self.version != 1 {
			return Err(format!("Unsupported tag storage version {}", self.version));
		}
		if self.next_id == 0 || self.tags.keys().any(|&id| id == 0 || id >= self.next_id) {
			return Err("Invalid tag IDs".into());
		}
		for (&id, tag) in &self.tags {
			self.check_tag(id, &tag.name, tag.parent)?;
		}
		let mut paths = BTreeSet::new();
		for assignment in &self.assignments {
			if assignment.path.first() != Some(&b'/')
				|| assignment.path.contains(&0)
				|| !paths.insert(&assignment.path)
				|| assignment.tags.iter().any(|id| !self.tags.contains_key(id))
			{
				return Err("Invalid tag assignments".into());
			}
		}
		Ok(())
	}
}

#[derive(Debug, Clone)]
pub struct TagStore {
	path: PathBuf,
	pub data: TagData,
	// Retain the lock across asynchronous edits and cloned snapshots.
	_lock: Arc<File>,
}

impl TagStore {
	pub fn load_default() -> Result<Self, String> {
		let root = dirs::data_local_dir().ok_or("Cannot find the user data directory")?;
		Self::open(root.join("fileberg/tags.json"))
	}

	pub fn open(path: PathBuf) -> Result<Self, String> {
		let parent = path.parent().ok_or("Tag storage has no parent directory")?;
		fs::create_dir_all(parent).map_err(|e| format!("Cannot create tag storage: {e}"))?;
		let lock = OpenOptions::new()
			.create(true)
			.truncate(false)
			.read(true)
			.write(true)
			.mode(0o600)
			.open(path.with_extension("lock"))
			.map_err(|e| format!("Cannot open tag lock: {e}"))?;
		lock.try_lock()
			.map_err(|e| format!("Tag storage is in use by another Fileberg window: {e}"))?;
		let data: TagData = match fs::read(&path) {
			Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
				format!(
					"Cannot read {}: {e}. Original file was not changed.",
					path.display()
				)
			})?,
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => TagData::default(),
			Err(e) => return Err(format!("Cannot read {}: {e}", path.display())),
		};
		data.validate()?;
		Ok(Self {
			path,
			data,
			_lock: Arc::new(lock),
		})
	}

	/// Commit on a clone: validation or write failure leaves the displayed snapshot intact.
	pub fn edited(mut self, edit: Edit) -> Result<Self, String> {
		self.data.apply(edit)?;
		self.save()?;
		Ok(self)
	}

	fn save(&self) -> Result<(), String> {
		static NEXT: AtomicU64 = AtomicU64::new(0);
		let temporary = self.path.with_extension(format!(
			"{}.{}.tmp",
			std::process::id(),
			NEXT.fetch_add(1, Ordering::Relaxed)
		));
		let bytes = serde_json::to_vec_pretty(&self.data).map_err(|e| e.to_string())?;
		let mut file = OpenOptions::new()
			.write(true)
			.create_new(true)
			.mode(0o600)
			.open(&temporary)
			.map_err(|e| format!("Cannot create tag save file: {e}"))?;
		let result = (|| {
			file.write_all(&bytes)?;
			file.write_all(b"\n")?;
			file.sync_all()?;
			fs::rename(&temporary, &self.path)
		})();
		if result.is_err() {
			let _ = fs::remove_file(&temporary);
		}
		result.map_err(|e| {
			format!(
				"Cannot save tags to {}: {e}. Changes were not applied.",
				self.path.display()
			)
		})
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagChoice {
	pub id: Option<TagId>,
	pub label: String,
}
impl std::fmt::Display for TagChoice {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		self.label.fmt(f)
	}
}

#[derive(Debug, Clone)]
pub struct TagEditor {
	pub id: Option<TagId>,
	pub name: String,
	pub parent: TagChoice,
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::test_support::TestDir;
	#[test]
	fn hierarchy_assignments_and_cycle_validation() {
		let mut data = TagData::default();
		data.apply(Edit::Create {
			name: "Work".into(),
			parent: None,
		})
		.unwrap();
		data.apply(Edit::Create {
			name: "Design".into(),
			parent: Some(2),
		})
		.unwrap();
		data.apply(Edit::Assign {
			path: "/a".into(),
			id: 3,
			assigned: true,
		})
		.unwrap();
		assert!(data.matches(Path::new("/a"), 2));
		assert!(!data.matches(Path::new("/a"), 1));
		assert_eq!(data.label(3), "Work / Design");
		assert_eq!(data.assigned(Path::new("/a")), vec![3]);
		data.apply(Edit::Assign {
			path: "/parent-only".into(),
			id: 2,
			assigned: true,
		})
		.unwrap();
		assert!(!data.matches(Path::new("/parent-only"), 3));
		assert!(
			data.apply(Edit::Update {
				id: 2,
				name: "Work".into(),
				parent: Some(3)
			})
			.is_err()
		);
		assert!(
			data.apply(Edit::Create {
				name: "design".into(),
				parent: Some(2)
			})
			.is_err()
		);
		assert!(
			data.apply(Edit::Create {
				name: "".into(),
				parent: None
			})
			.is_err()
		);
		data.apply(Edit::Update {
			id: 3,
			name: "Visual".into(),
			parent: None,
		})
		.unwrap();
		assert!(!data.matches(Path::new("/a"), 2));
		assert!(data.matches(Path::new("/a"), 3));
	}
	#[test]
	fn persisted_roundtrip_preserves_ids_and_non_utf8_paths() {
		use std::{ffi::OsString, os::unix::ffi::OsStringExt};
		let dir = TestDir::new();
		let location = dir.path().join("tags.json");
		let store = TagStore::open(location.clone()).unwrap();
		let path = dir.path().join(OsString::from_vec(vec![b'a', 255]));
		let store = store
			.edited(Edit::Assign {
				path: path.clone(),
				id: 1,
				assigned: true,
			})
			.unwrap();
		drop(store);
		let store = TagStore::open(location).unwrap();
		assert_eq!(store.data.assigned(&path), vec![1]);
		let store = store
			.edited(Edit::Update {
				id: 1,
				name: "Favorite".into(),
				parent: None,
			})
			.unwrap();
		assert_eq!(store.data.assigned(&path), vec![1]);
		let store = store
			.edited(Edit::Assign {
				path: path.clone(),
				id: 1,
				assigned: false,
			})
			.unwrap();
		assert!(store.data.assigned(&path).is_empty());
	}
	#[test]
	fn malformed_or_future_storage_is_never_overwritten() {
		let dir = TestDir::new();
		let path = dir.path().join("tags.json");
		for bytes in [
			b"broken".as_slice(),
			br#"{"version":99,"next_id":1,"tags":{},"assignments":[]}"#,
		] {
			fs::write(&path, bytes).unwrap();
			assert!(TagStore::open(path.clone()).is_err());
			assert_eq!(fs::read(&path).unwrap(), bytes);
		}
	}
	#[test]
	fn single_writer_and_failed_save_preserve_original_snapshot() {
		let dir = TestDir::new();
		let path = dir.path().join("tags.json");
		let store = TagStore::open(path.clone()).unwrap();
		assert!(TagStore::open(path.clone()).is_err());
		fs::create_dir(&path).unwrap(); // Force atomic rename to fail.
		assert!(
			store
				.clone()
				.edited(Edit::Create {
					name: "Work".into(),
					parent: None
				})
				.is_err()
		);
		assert_eq!(store.data.tags.len(), 1);
	}

	#[test]
	fn invalid_hierarchies_and_assignments_are_rejected_on_load() {
		let mut data = TagData::default();
		data.tags.get_mut(&1).unwrap().parent = Some(1);
		assert!(data.validate().is_err());
		data.tags.get_mut(&1).unwrap().parent = Some(42);
		assert!(data.validate().is_err());
		data.tags.get_mut(&1).unwrap().parent = None;
		data.assignments.push(Assignment {
			path: b"/file".to_vec(),
			tags: BTreeSet::from([42]),
		});
		assert!(data.validate().is_err());
	}
}
