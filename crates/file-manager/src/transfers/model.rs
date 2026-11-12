use serde::{Deserialize, Serialize};
use std::{
	ffi::OsString,
	os::unix::ffi::{OsStrExt, OsStringExt},
	path::{Path, PathBuf},
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Durability {
	#[default]
	Auto,
	Durable,
	Buffered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Operation {
	Copy,
	Move,
	Automatic,
}

#[derive(Debug, Clone)]
pub(crate) struct Request {
	pub sources: Vec<PathBuf>,
	pub destination: PathBuf,
	pub operation: Operation,
	pub durability: Durability,
	pub acknowledge_volatile: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Choice {
	Merge,
	Replace,
	ReplaceCentral,
	Skip,
	KeepBoth,
	Cancel,
	ApproveRecovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ConflictReason {
	#[default]
	Entry,
	RecoveryBudget {
		bytes: u64,
	},
	CentralRecovery,
	VolatileMove,
}

#[derive(Debug, Clone)]
pub(crate) struct Conflict {
	pub source: PathBuf,
	pub destination: PathBuf,
	pub directories: bool,
	pub reason: ConflictReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
	Preparing,
	Copying,
	Verifying,
	Flushing,
	Committing,
	RollingBack,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
	Progress {
		phase: Phase,
		bytes: u64,
		total: Option<u64>,
	},
	Conflict(Conflict),
	Warning(String),
}

impl std::fmt::Display for Durability {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		formatter.write_str(match self {
			Self::Auto => "Auto",
			Self::Durable => "Durable",
			Self::Buffered => "Buffered",
		})
	}
}

/// Unix paths are arbitrary byte strings, including in persistent job records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct StoredPath(pub Vec<u8>);

impl StoredPath {
	pub fn new(path: &Path) -> Self {
		Self(path.as_os_str().as_bytes().to_owned())
	}

	pub fn path(&self) -> PathBuf {
		OsString::from_vec(self.0.clone()).into()
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Kind {
	File,
	Directory,
	Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Entry {
	pub relative: StoredPath,
	pub device: u64,
	pub inode: u64,
	pub kind: Kind,
	pub mode: u32,
	pub length: u64,
	pub modified: (i64, i64),
	pub changed: (i64, i64),
	pub digest: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Snapshot(pub Vec<Entry>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Step {
	Prepared,
	Staged,
	BackingUp,
	Publishing,
	Published,
	RemovingSource,
	Committed,
	Undoing,
	Undone,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Change {
	pub source: StoredPath,
	pub destination: StoredPath,
	pub operation: Operation,
	pub before: Snapshot,
	pub workspace: Entry,
	pub stage: StoredPath,
	pub staged: Option<Snapshot>,
	pub backup: StoredPath,
	pub replaced: Option<Snapshot>,
	pub published: Option<Snapshot>,
	pub source_removed: bool,
	#[serde(default)]
	pub source_quarantine: Option<StoredPath>,
	#[serde(default)]
	pub quarantine_workspace: Option<Entry>,
	pub renamed: bool,
	pub empty_directory: bool,
	pub step: Step,
	pub undo_stage: StoredPath,
	pub undo_snapshot: Option<Snapshot>,
	#[serde(default)]
	pub undo_workspace: Option<Entry>,
	#[serde(default)]
	pub undo_published: Option<Snapshot>,
	#[serde(default)]
	pub replacement_restored: bool,
	#[serde(default)]
	pub central: Option<super::central::Backup>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Status {
	Running,
	Complete,
	Expiring,
	RollingBack,
	RolledBack,
	NeedsRecovery,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Journal {
	pub version: u32,
	pub id: String,
	pub created: u64,
	pub status: Status,
	pub durability: Durability,
	/// Explicitly approved replacement data outside the ordinary 5 GiB budget.
	/// This allowance belongs only to this job and still expires after seven days.
	#[serde(default)]
	pub approved_recovery_bytes: u64,
	pub(super) changes: Vec<Change>,
	pub problems: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Outcome {
	pub id: String,
	pub status: Status,
	pub problems: Vec<String>,
}
