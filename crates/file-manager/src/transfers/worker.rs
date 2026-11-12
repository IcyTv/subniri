use super::{
	durability, fs,
	journal::{self, Store},
	model::{
		Change, Choice, Conflict, Durability, Event, Journal, Kind, Operation, Outcome, Phase,
		Request, Snapshot, Status, Step, StoredPath,
	},
};
use crate::filesystem::classification::{self, StorageClass};
use std::{
	fs::{self as stdfs, File, FileTimes, OpenOptions, Permissions},
	io,
	os::unix::fs::{OpenOptionsExt, PermissionsExt},
	path::{Path, PathBuf},
	sync::atomic::{AtomicBool, Ordering},
	time::{Duration, UNIX_EPOCH},
};

#[cfg(test)]
#[path = "progress_tests.rs"]
mod progress_tests;

#[cfg(test)]
#[path = "performance_tests.rs"]
mod performance_tests;

pub(crate) fn execute(
	store: &Store, request: Request, cancelled: &AtomicBool,
	conflict: impl FnMut(Conflict) -> Choice, events: impl FnMut(Event),
) -> io::Result<Outcome> {
	execute_with_flush(store, request, cancelled, conflict, events, fs::sync_tree)
}

pub(super) fn execute_with_flush(
	store: &Store, mut request: Request, cancelled: &AtomicBool,
	mut conflict: impl FnMut(Conflict) -> Choice, mut events: impl FnMut(Event),
	mut flush: impl FnMut(&Path, &Snapshot) -> io::Result<()>,
) -> io::Result<Outcome> {
	request.destination = stdfs::canonicalize(&request.destination)?;
	let mut sources = Vec::new();
	for source in &request.sources {
		let absolute = std::path::absolute(source)?;
		let parent = stdfs::canonicalize(
			absolute
				.parent()
				.ok_or_else(|| fs::invalid("Cannot transfer a filesystem root"))?,
		)?;
		let path = parent.join(
			absolute
				.file_name()
				.ok_or_else(|| fs::invalid("Missing source name"))?,
		);
		if !sources.contains(&path) {
			sources.push(path);
		}
	}
	if sources.is_empty() {
		return Err(fs::invalid("Select at least one entry"));
	}
	for source in &sources {
		if sources
			.iter()
			.any(|other| other != source && source.starts_with(other))
		{
			return Err(fs::invalid(
				"Selection contains both an ancestor and its child",
			));
		}
	}
	events(Event::Progress {
		phase: Phase::Preparing,
		bytes: 0,
		total: None,
	});
	let source_totals = sources
		.iter()
		.map(|source| transfer_total(std::slice::from_ref(source), cancelled))
		.collect::<io::Result<Vec<_>>>()?;
	let total = source_totals.iter().try_fold(0u64, |total, bytes| {
		total
			.checked_add(*bytes)
			.ok_or_else(|| fs::invalid("Transfer byte total overflow"))
	})?;
	events(Event::Progress {
		phase: Phase::Preparing,
		bytes: 0,
		total: Some(total),
	});
	check_cancelled(cancelled)?;

	let class = classification::classify(&request.destination)?;
	if request.durability == Durability::Durable && class == StorageClass::Volatile {
		return Err(fs::invalid(
			"Volatile storage cannot provide durable completion",
		));
	}
	if request.operation == Operation::Move
		&& class == StorageClass::Volatile
		&& !request.acknowledge_volatile
	{
		if conflict(Conflict {
			source: sources[0].clone(),
			destination: request.destination.clone(),
			directories: false,
			reason: super::model::ConflictReason::VolatileMove,
		}) != Choice::ApproveRecovery
		{
			return Err(io::ErrorKind::Interrupted.into());
		}
		request.acknowledge_volatile = true;
	}
	let mut journal = Journal {
		version: 1,
		id: journal::new_id()?,
		created: journal::now()?,
		status: Status::Running,
		durability: request.durability,
		approved_recovery_bytes: 0,
		changes: Vec::new(),
		problems: Vec::new(),
	};
	store.save(&journal)?;
	let mut worker = Worker {
		store,
		journal: &mut journal,
		cancelled,
		conflict: &mut conflict,
		events: &mut events,
		flush: &mut flush,
		bytes: 0,
		total,
	};
	let forward = sources
		.iter()
		.zip(source_totals)
		.try_for_each(|(source, bytes)| {
			let destination = request.destination.join(
				source
					.file_name()
					.ok_or_else(|| fs::invalid("Missing source name"))?,
			);
			worker.transfer(source, &destination, &request, Some(bytes))
		});
	if let Err(error) = forward {
		worker.journal.problems.push(error.to_string());
		worker.store.save(worker.journal)?;
		rollback(worker.store, worker.journal, worker.events)?;
	} else {
		worker.journal.status = Status::Complete;
		worker.store.save(worker.journal)?;
	}

	Ok(outcome(&journal))
}

struct Worker<'a, C, E, F> {
	store: &'a Store,
	journal: &'a mut Journal,
	cancelled: &'a AtomicBool,
	conflict: &'a mut C,
	events: &'a mut E,
	flush: &'a mut F,
	bytes: u64,
	total: u64,
}

fn check_cancelled(cancelled: &AtomicBool) -> io::Result<()> {
	if cancelled.load(Ordering::Acquire) {
		return Err(io::ErrorKind::Interrupted.into());
	}

	Ok(())
}

fn checked_progress(bytes: u64, additional: u64, total: u64) -> io::Result<u64> {
	bytes
		.checked_add(additional)
		.filter(|bytes| *bytes <= total)
		.ok_or_else(|| fs::invalid("Source data exceeds the prepared transfer total"))
}

fn snapshot_bytes(snapshot: &Snapshot) -> io::Result<u64> {
	snapshot
		.0
		.iter()
		.filter(|entry| entry.kind == Kind::File)
		.try_fold(0u64, |bytes, entry| {
			bytes
				.checked_add(entry.length)
				.ok_or_else(|| fs::invalid("Transfer byte total overflow"))
		})
}

/// Inspect metadata only; the normal transfer still validates and hashes data.
fn transfer_total(sources: &[PathBuf], cancelled: &AtomicBool) -> io::Result<u64> {
	use std::{os::fd::AsRawFd, os::unix::fs::MetadataExt};

	let mut total = 0u64;
	let mut directories: Vec<(File, stdfs::ReadDir)> = Vec::new();
	for source in sources {
		let mut next = Some(source.clone());
		loop {
			check_cancelled(cancelled)?;
			let path = if let Some(path) = next.take() {
				path
			} else if let Some((_, entries)) = directories.last_mut() {
				match entries.next() {
					Some(entry) => entry?.path(),
					None => {
						directories.pop();
						continue;
					}
				}
			} else {
				break;
			};
			let metadata = stdfs::symlink_metadata(&path)?;
			if metadata.is_file() {
				total = total
					.checked_add(metadata.len())
					.ok_or_else(|| fs::invalid("Transfer byte total overflow"))?;
			} else if metadata.is_dir() {
				if directories.len() >= 256 {
					return Err(fs::invalid("Transfer directory nesting exceeds 256 levels"));
				}
				let directory = OpenOptions::new()
					.read(true)
					.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
					.open(&path)?;
				let opened = directory.metadata()?;
				if (metadata.dev(), metadata.ino()) != (opened.dev(), opened.ino()) {
					return Err(fs::invalid("Directory changed while preparing transfer"));
				}
				// Pin each parent descriptor so renames or symlink swaps cannot
				// redirect the metadata walk into a different tree.
				let entries = stdfs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))?;
				directories.push((directory, entries));
			} else if !metadata.is_symlink() {
				return Err(fs::invalid(format!(
					"Unsupported filesystem entry: {}",
					path.display()
				)));
			}
		}
	}
	check_cancelled(cancelled)?;

	Ok(total)
}

impl<C, E, F> Worker<'_, C, E, F>
where
	C: FnMut(Conflict) -> Choice,
	E: FnMut(Event),
	F: FnMut(&Path, &Snapshot) -> io::Result<()>,
{
	fn check_cancel(&self) -> io::Result<()> {
		check_cancelled(self.cancelled)
	}

	fn transfer(
		&mut self, source: &Path, target: &Path, request: &Request, expected_bytes: Option<u64>,
	) -> io::Result<()> {
		self.check_cancel()?;
		if source == target || target.starts_with(source) {
			return Err(fs::invalid(
				"Cannot transfer an entry into itself or its descendants",
			));
		}
		(self.events)(Event::Progress {
			phase: Phase::Preparing,
			bytes: self.bytes,
			total: Some(self.total),
		});
		let mut before = fs::metadata_snapshot_cancellable(source, self.cancelled)?;
		let entry_bytes = snapshot_bytes(&before)?;
		if expected_bytes.is_some_and(|expected| expected != entry_bytes) {
			return Err(fs::invalid(
				"Source length changed after preparing transfer total",
			));
		}
		let completed_bytes = checked_progress(self.bytes, entry_bytes, self.total)?;
		use std::os::unix::fs::MetadataExt;
		let automatic = request.operation == Operation::Automatic;
		let mut operation = if automatic {
			let parent = target
				.parent()
				.ok_or_else(|| fs::invalid("Missing destination parent"))?;
			let device = stdfs::metadata(parent)?.dev();
			if before.0.first().is_some_and(|entry| entry.device == device) {
				Operation::Move
			} else {
				Operation::Copy
			}
		} else {
			request.operation
		};
		let mut destination = target.to_path_buf();
		let mut replaced = None;
		let mut central_requested = false;
		if fs::exists(&destination)? {
			let old = fs::snapshot(&destination)?;
			let directories = before
				.0
				.first()
				.is_some_and(|entry| entry.kind == Kind::Directory)
				&& old
					.0
					.first()
					.is_some_and(|entry| entry.kind == Kind::Directory);
			match (self.conflict)(Conflict {
				source: source.into(),
				destination: destination.clone(),
				directories,
				reason: super::model::ConflictReason::Entry,
			}) {
				Choice::Cancel => return Err(io::ErrorKind::Interrupted.into()),
				Choice::ApproveRecovery => {
					return Err(fs::invalid(
						"Recovery consent is not a replacement decision",
					));
				}
				Choice::Skip => return Ok(()),
				Choice::KeepBoth => destination = unique_name(&destination)?,
				Choice::Merge if directories => {
					fs::unchanged(&destination, &old)?;
					let names = stdfs::read_dir(source)?
						.map(|entry| entry.map(|e| e.file_name()))
						.collect::<io::Result<Vec<_>>>()?;
					for name in names {
						self.transfer(
							&source.join(&name),
							&destination.join(&name),
							request,
							None,
						)?;
					}
					if operation == Operation::Move && stdfs::read_dir(source)?.next().is_none() {
						self.remove_empty_source(source, &destination)?;
					}
					return Ok(());
				}
				Choice::Merge => return Err(fs::invalid("Only directories can be merged")),
				choice @ (Choice::Replace | Choice::ReplaceCentral) => {
					fs::unchanged(&destination, &old)?;
					// An explicit central choice still preserves replacement-only data.
					// Ordinary Replace continues to prefer a same-filesystem rename.
					central_requested = choice == Choice::ReplaceCentral;
					replaced = Some(old);
				}
			}
		}
		let parent = destination
			.parent()
			.ok_or_else(|| fs::invalid("Missing destination parent"))?;
		if let Some(old) = &replaced {
			let bytes = old
				.0
				.iter()
				.filter(|entry| entry.kind == Kind::File)
				.map(|entry| entry.length)
				.sum();
			reserve_recovery(
				self.store,
				self.journal,
				source,
				&destination,
				bytes,
				self.conflict,
			)?;
		}
		let class = classification::classify(parent)?;
		if operation == Operation::Move
			&& class == StorageClass::Volatile
			&& !request.acknowledge_volatile
		{
			return Err(fs::invalid(
				"Moving into volatile storage requires acknowledgement; copy instead",
			));
		}
		let required = request.durability == Durability::Durable
			|| (request.durability == Durability::Auto
				&& class != StorageClass::Fixed
				&& class != StorageClass::Volatile);
		let paced = matches!(
			class,
			StorageClass::Removable | StorageClass::Network | StorageClass::Unknown
		);
		let work = self
			.store
			.workspace(parent, &self.journal.id, self.journal.changes.len())?;
		fs::private_directory(&work)?;
		fs::sync_parent(&work)?;
		let stage = work.join("part");
		let backup = work.join("replaced");
		let index = self.journal.changes.len();
		let workspace = fs::snapshot(&work)?
			.0
			.into_iter()
			.next()
			.ok_or_else(|| fs::invalid("Missing workspace identity"))?;
		self.journal.changes.push(Change {
			source: StoredPath::new(source),
			destination: StoredPath::new(&destination),
			operation,
			before: before.clone(),
			workspace,
			stage: StoredPath::new(&stage),
			staged: None,
			backup: StoredPath::new(&backup),
			replaced,
			published: None,
			source_removed: false,
			renamed: false,
			empty_directory: false,
			step: Step::Prepared,
			undo_stage: StoredPath::new(&work.join("undo")),
			undo_snapshot: None,
			undo_workspace: None,
			source_quarantine: None,
			quarantine_workspace: None,
			undo_published: None,
			replacement_restored: false,
			central: None,
		});
		self.store.save(self.journal)?;
		if operation == Operation::Move {
			self.change(index)?.renamed = true;
			self.store.save(self.journal)?;
			fs::unchanged(source, &before)?;
			match fs::rename_new(source, &stage) {
				Ok(()) => {
					self.change(index)?.source_removed = true;
				}
				Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
					self.change(index)?.renamed = false;
					if automatic {
						operation = Operation::Copy;
						self.change(index)?.operation = Operation::Copy;
					}
				}
				Err(error) => {
					self.change(index)?.renamed = false;
					return Err(error);
				}
			}
			self.store.save(self.journal)?;
		}
		if !self.change(index)?.renamed {
			let mut progress_error = None;
			before = copy_tree_verified(
				source,
				&stage,
				&before,
				paced,
				self.cancelled,
				&mut |bytes| match checked_progress(self.bytes, bytes, self.total) {
					Ok(bytes) => (self.events)(Event::Progress {
						phase: Phase::Copying,
						bytes,
						total: Some(self.total),
					}),
					Err(error) => progress_error = Some(error),
				},
			)?;
			if let Some(error) = progress_error {
				return Err(error);
			}
			fs::unchanged_metadata(source, &before)?;
			self.change(index)?.before = before.clone();
		}
		let renamed = self.change(index)?.renamed;
		let staged = if renamed {
			fs::relocated_snapshot(&stage, &before)?
		} else {
			(self.events)(Event::Progress {
				phase: Phase::Verifying,
				bytes: completed_bytes,
				total: Some(self.total),
			});
			// Compare the streamed digest with both surviving source and staging.
			// Coarse filesystem timestamps alone cannot prove unchanged contents.
			if fs::snapshot_cancellable(source, self.cancelled)? != before {
				return Err(fs::invalid("Source changed while copying"));
			}
			fs::snapshot_cancellable(&stage, self.cancelled)?
		};
		if !renamed && !fs::same_contents(&staged, &before, false) {
			return Err(fs::invalid("Staged data does not match source"));
		}
		self.bytes = completed_bytes;
		self.change(index)?.staged = Some(staged.clone());
		self.change(index)?.step = Step::Staged;
		self.store.save(self.journal)?;
		self.check_cancel()?;
		if required {
			(self.events)(Event::Progress {
				phase: Phase::Flushing,
				bytes: self.bytes,
				total: Some(self.total),
			});
			self.check_cancel()?;
			(self.flush)(&stage, &staged)?;
		}
		self.check_cancel()?;
		if let Some(old) = self.change(index)?.replaced.clone() {
			fs::unchanged(&destination, &old)?;
			// Recovery is durable even when transferred data uses Buffered mode.
			(self.flush)(&destination, &old)?;
			self.change(index)?.step = Step::BackingUp;
			self.store.save(self.journal)?;
			let local = if central_requested {
				None
			} else {
				Some(fs::rename_new(&destination, &backup))
			};
			match local {
				Some(Ok(())) => {
					fs::sync_parent(&destination)?;
					fs::sync_parent(&backup)?;
					if !fs::same_contents(&fs::snapshot(&backup)?, &old, true) {
						return Err(fs::invalid("Replacement changed while preserving it"));
					}
				}
				otherwise => {
					if let Some(Err(error)) = otherwise {
						(self.events)(Event::Warning(format!(
							"Same-filesystem recovery unavailable: {error}. Existing data has not been replaced."
						)));
						let answer = (self.conflict)(Conflict {
							source: source.into(),
							destination: destination.clone(),
							directories: false,
							reason: super::model::ConflictReason::CentralRecovery,
						});
						if answer != Choice::ApproveRecovery {
							return Err(io::ErrorKind::Interrupted.into());
						}
					}
					let root = self.store.recovery_root().to_path_buf();
					let central = super::central::preserve(
						&root,
						&destination,
						&old,
						self.cancelled,
						|state| {
							self.journal
								.changes
								.get_mut(index)
								.ok_or_else(|| fs::invalid("Missing replacement record"))?
								.central = Some(state.clone());
							self.store.save(self.journal)
						},
					)?;
					self.change(index)?.central = Some(central);
					self.store.save(self.journal)?;
					self.check_cancel()?;
					fs::unchanged(&destination, &old)?;
					fs::remove_recorded(&destination, &old)?;
				}
			}
		}
		self.change(index)?.step = Step::Publishing;
		self.store.save(self.journal)?;
		(self.events)(Event::Progress {
			phase: Phase::Committing,
			bytes: self.bytes,
			total: Some(self.total),
		});
		fs::rename_new(&stage, &destination)?;
		if required {
			fs::sync_parent(&destination)?;
			fs::sync_parent(&stage)?;
			if self.change(index)?.renamed {
				fs::sync_parent(source)?;
			}
		}
		self.change(index)?.published = Some(fs::relocated_snapshot(&destination, &staged)?);
		self.change(index)?.step = Step::Published;
		self.store.save(self.journal)?;
		self.check_cancel()?;
		if operation == Operation::Move && !self.change(index)?.renamed {
			fs::unchanged(source, &before)?;
			self.change(index)?.step = Step::RemovingSource;
			self.store.save(self.journal)?;
			fs::remove_recorded(source, &before)?;
			if required {
				fs::sync_parent(source)?;
			}
			self.change(index)?.source_removed = true;
		}
		self.change(index)?.step = Step::Committed;
		self.store.save(self.journal)
	}

	fn change(&mut self, index: usize) -> io::Result<&mut Change> {
		self.journal
			.changes
			.get_mut(index)
			.ok_or_else(|| fs::invalid("Missing transfer change"))
	}

	fn remove_empty_source(&mut self, source: &Path, destination: &Path) -> io::Result<()> {
		self.check_cancel()?;
		let before = fs::snapshot(source)?;
		if before.0.len() != 1 {
			return Err(fs::invalid("Source directory is no longer empty"));
		}
		let dummy = StoredPath::new(source);
		let workspace = before
			.0
			.first()
			.cloned()
			.ok_or_else(|| fs::invalid("Missing directory identity"))?;
		self.journal.changes.push(Change {
			source: dummy.clone(),
			destination: StoredPath::new(destination),
			operation: Operation::Move,
			before,
			workspace,
			stage: dummy.clone(),
			staged: None,
			backup: dummy.clone(),
			replaced: None,
			published: None,
			source_removed: false,
			renamed: false,
			empty_directory: true,
			step: Step::RemovingSource,
			undo_stage: dummy,
			undo_snapshot: None,
			undo_workspace: None,
			source_quarantine: None,
			quarantine_workspace: None,
			undo_published: None,
			replacement_restored: false,
			central: None,
		});
		self.store.save(self.journal)?;
		stdfs::remove_dir(source)?;
		fs::sync_parent(source)?;
		let change = self
			.journal
			.changes
			.last_mut()
			.ok_or_else(|| fs::invalid("Missing directory change"))?;
		change.source_removed = true;
		change.step = Step::Committed;
		self.store.save(self.journal)
	}
}

pub(super) fn reserve_recovery(
	store: &Store, journal: &mut Journal, source: &Path, destination: &Path, bytes: u64,
	conflict: &mut impl FnMut(Conflict) -> Choice,
) -> io::Result<()> {
	match super::retention::reserve(store, bytes) {
		Ok(()) => Ok(()),
		Err(error) if error.kind() == io::ErrorKind::QuotaExceeded => {
			let answer = conflict(Conflict {
				source: source.into(),
				destination: destination.into(),
				directories: false,
				reason: super::model::ConflictReason::RecoveryBudget { bytes },
			});
			if answer != Choice::ApproveRecovery {
				return Err(io::ErrorKind::Interrupted.into());
			}

			journal.approved_recovery_bytes = journal
				.approved_recovery_bytes
				.checked_add(bytes)
				.ok_or_else(|| fs::invalid("Recovery budget overflow"))?;
			store.save(journal)
		}
		Err(error) => Err(error),
	}
}

fn unique_name(path: &Path) -> io::Result<PathBuf> {
	let name = path
		.file_name()
		.ok_or_else(|| fs::invalid("Missing filename"))?;
	for number in 1..=100_000 {
		let mut candidate = name.to_os_string();
		candidate.push(format!(" (copy {number})"));
		let candidate = path.with_file_name(candidate);
		if !fs::exists(&candidate)? {
			return Ok(candidate);
		}
	}

	Err(fs::invalid("Could not reserve a unique copy name"))
}

fn modified_time(value: (i64, i64)) -> io::Result<std::time::SystemTime> {
	let seconds = value.0.unsigned_abs();
	let base = if value.0 >= 0 {
		UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
	} else {
		UNIX_EPOCH.checked_sub(Duration::from_secs(seconds))
	};
	base.and_then(|time| time.checked_add(Duration::from_nanos(value.1 as u64)))
		.ok_or_else(|| fs::invalid("Unsupported file timestamp"))
}

pub(super) fn copy_tree(
	source: &Path, destination: &Path, expected: &Snapshot, paced: bool, cancelled: &AtomicBool,
	progress: &mut impl FnMut(u64),
) -> io::Result<()> {
	copy_tree_verified(source, destination, expected, paced, cancelled, progress).map(|_| ())
}

fn copy_tree_verified(
	source: &Path, destination: &Path, expected: &Snapshot, paced: bool, cancelled: &AtomicBool,
	progress: &mut impl FnMut(u64),
) -> io::Result<Snapshot> {
	use sha2::{Digest, Sha256};
	use std::io::Read;
	use std::os::unix::fs::MetadataExt;

	struct HashingReader<'a> {
		input: &'a mut File,
		digest: Sha256,
	}

	impl Read for HashingReader<'_> {
		fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
			let length = self.input.read(bytes)?;
			self.digest.update(&bytes[..length]);
			Ok(length)
		}
	}

	let mut verified = expected.clone();
	let mut copied = 0u64;
	for entry in &mut verified.0 {
		if cancelled.load(Ordering::Acquire) {
			return Err(io::ErrorKind::Interrupted.into());
		}
		let source = fs::entry_path(source, &entry.relative);
		let target = fs::entry_path(destination, &entry.relative);
		match entry.kind {
			Kind::Directory => stdfs::create_dir(&target)?,
			Kind::Symlink => {
				std::os::unix::fs::symlink(stdfs::read_link(&source)?, &target)?;
				use std::{ffi::CString, os::unix::ffi::OsStrExt};
				let target = CString::new(target.as_os_str().as_bytes())?;
				let times = [
					libc::timespec {
						tv_sec: 0,
						tv_nsec: libc::UTIME_OMIT,
					},
					libc::timespec {
						tv_sec: entry.modified.0,
						tv_nsec: entry.modified.1,
					},
				];
				if unsafe {
					libc::utimensat(
						libc::AT_FDCWD,
						target.as_ptr(),
						times.as_ptr(),
						libc::AT_SYMLINK_NOFOLLOW,
					)
				} != 0
				{
					return Err(io::Error::last_os_error());
				}
			}
			Kind::File => {
				let mut input = OpenOptions::new()
					.read(true)
					.custom_flags(libc::O_NOFOLLOW)
					.open(source)?;
				let opened = input.metadata()?;
				if (
					opened.dev(),
					opened.ino(),
					opened.len(),
					opened.mtime(),
					opened.mtime_nsec(),
					opened.ctime(),
					opened.ctime_nsec(),
				) != (
					entry.device,
					entry.inode,
					entry.length,
					entry.modified.0,
					entry.modified.1,
					entry.changed.0,
					entry.changed.1,
				) {
					return Err(fs::invalid("Source changed before copying"));
				}
				let mut reader = HashingReader {
					input: &mut input,
					digest: Sha256::new(),
				};
				let mut output = OpenOptions::new()
					.write(true)
					.create_new(true)
					.mode(0o600)
					.custom_flags(libc::O_NOFOLLOW)
					.open(target)?;
				let mut file_bytes = 0u64;
				let mut progress_error = None;
				let count =
					durability::copy_file(&mut reader, &mut output, paced, cancelled, |bytes| {
						if progress_error.is_some() {
							return;
						}
						match checked_progress(file_bytes, bytes, entry.length).and_then(|length| {
							file_bytes = length;
							copied
								.checked_add(length)
								.ok_or_else(|| fs::invalid("Transfer byte total overflow"))
						}) {
							Ok(bytes) => progress(bytes),
							Err(error) => progress_error = Some(error),
						}
					})?;
				if let Some(error) = progress_error {
					return Err(error);
				}
				if count != entry.length {
					return Err(fs::invalid("Source length changed while copying"));
				}
				let digest = reader.digest.finalize().to_vec();
				if !entry.digest.is_empty() && entry.digest != digest {
					return Err(fs::invalid("Source contents changed while copying"));
				}
				entry.digest = digest;
				copied = copied
					.checked_add(count)
					.ok_or_else(|| fs::invalid("Transfer byte total overflow"))?;
			}
		}
	}
	for entry in expected
		.0
		.iter()
		.rev()
		.filter(|entry| entry.kind != Kind::Symlink)
	{
		let target = fs::entry_path(destination, &entry.relative);
		stdfs::set_permissions(&target, Permissions::from_mode(entry.mode & 0o777))?;
		File::open(target)?
			.set_times(FileTimes::new().set_modified(modified_time(entry.modified)?))?;
	}

	Ok(verified)
}

pub(crate) fn undo(store: &Store, id: &str, mut events: impl FnMut(Event)) -> io::Result<Outcome> {
	let mut journal = store.load(id)?;
	if journal.status == Status::Expiring {
		return Err(fs::invalid("This undo record is expiring"));
	}
	rollback(store, &mut journal, &mut events)?;
	Ok(outcome(&journal))
}

fn outcome(journal: &Journal) -> Outcome {
	Outcome {
		id: journal.id.clone(),
		status: journal.status,
		problems: journal.problems.clone(),
	}
}

fn rollback(
	store: &Store, journal: &mut Journal, events: &mut impl FnMut(Event),
) -> io::Result<()> {
	journal.status = Status::RollingBack;
	store.save(journal)?;
	events(Event::Progress {
		phase: Phase::RollingBack,
		bytes: 0,
		total: None,
	});
	let mut blocked = Vec::new();
	for index in (0..journal.changes.len()).rev() {
		let mut change = journal
			.changes
			.get(index)
			.cloned()
			.ok_or_else(|| fs::invalid("Missing rollback record"))?;
		if change.step == Step::Undone {
			continue;
		}
		change.step = Step::Undoing;
		*journal
			.changes
			.get_mut(index)
			.ok_or_else(|| fs::invalid("Missing rollback record"))? = change.clone();
		store.save(journal)?;
		let result = reverse_change(&mut change, journal.durability, |change| {
			*journal
				.changes
				.get_mut(index)
				.ok_or_else(|| fs::invalid("Missing rollback record"))? = change.clone();
			store.save(journal)
		});
		if let Err(error) = result {
			blocked.push(format!("{}: {error}", change.destination.path().display()));
		} else {
			change.step = Step::Undone;
		}
		*journal
			.changes
			.get_mut(index)
			.ok_or_else(|| fs::invalid("Missing rollback record"))? = change;
		store.save(journal)?;
	}
	journal.status = if blocked.is_empty() {
		Status::RolledBack
	} else {
		Status::NeedsRecovery
	};
	journal.problems.extend(blocked);
	store.save(journal)
}

fn reverse_change(
	change: &mut Change, policy: Durability, mut save: impl FnMut(&Change) -> io::Result<()>,
) -> io::Result<()> {
	let source = change.source.path();
	let destination = change.destination.path();
	if change.empty_directory {
		if !fs::exists(&source)? {
			stdfs::create_dir(&source)?;
			let mode = change
				.before
				.0
				.first()
				.ok_or_else(|| fs::invalid("Missing source directory metadata"))?
				.mode;
			stdfs::set_permissions(&source, Permissions::from_mode(mode & 0o777))?;
			fs::sync_parent(&source)?;
			change.undo_published = Some(fs::snapshot(&source)?);
			save(change)?;
		} else {
			use std::os::unix::fs::MetadataExt;
			let current = stdfs::symlink_metadata(&source)?;
			let expected = change
				.undo_published
				.as_ref()
				.unwrap_or(&change.before)
				.0
				.first()
				.ok_or_else(|| fs::invalid("Missing directory identity"))?;
			if !current.is_dir()
				|| (current.dev(), current.ino()) != (expected.device, expected.inode)
			{
				return Err(fs::invalid(
					"Original directory path is occupied; preserving it",
				));
			}
		}
		return Ok(());
	}
	let stage = change.stage.path();
	let backup = change.backup.path();
	let work = stage
		.parent()
		.ok_or_else(|| fs::invalid("Missing workspace"))?;
	fs::validate_private(work)?;
	use std::os::unix::fs::MetadataExt;
	let metadata = stdfs::symlink_metadata(work)?;
	if (metadata.dev(), metadata.ino()) != (change.workspace.device, change.workspace.inode) {
		return Err(fs::invalid(
			"Recovery filesystem/workspace identity changed",
		));
	}
	let discardable_copy = change.operation == Operation::Copy && change.published.is_none();
	if !change.renamed && fs::exists(&stage)? && (change.staged.is_none() || discardable_copy) {
		// The journal owns this private workspace, including an interrupted copy.
		// Never adopt data outside that recorded workspace for cleanup.
		let cleanup_pending = change
			.staged
			.as_ref()
			.map(|expected| fs::removal_pending(&stage, expected))
			.transpose()?
			.unwrap_or(false);
		if !cleanup_pending {
			if let Some(expected) = &change.staged {
				fs::unchanged_metadata(&stage, expected)?;
			}
			change.staged = Some(fs::metadata_snapshot(&stage)?);
			save(change)?;
		}
	}
	let restored_replacement = if let Some(central) = &change.central {
		central.restored(&destination)?
	} else if let Some(original) = &change.replaced {
		!fs::exists(&backup)?
			&& fs::exists(&destination)?
			&& fs::same_contents(&fs::snapshot(&destination)?, original, true)
	} else {
		false
	};
	// An unchanged original destination also exists before publication. Do not
	// mistake that state for completed undo while staged move data still needs
	// to return to its source (or an interrupted copy still needs cleanup).
	let pending_stage_cleanup = change
		.staged
		.as_ref()
		.map(|expected| fs::removal_pending(&stage, expected))
		.transpose()?
		.unwrap_or(false);
	if restored_replacement && !fs::exists(&stage)? && !pending_stage_cleanup {
		if change.operation == Operation::Move {
			let expected = change
				.undo_published
				.as_ref()
				.or(change.undo_snapshot.as_ref())
				.unwrap_or(&change.before);
			let restored = fs::snapshot_matching(&source, expected)?;
			let inverse_rename = change.undo_published.is_none()
				&& change.undo_snapshot.is_some()
				&& !fs::has_content_hashes(expected)
				&& fs::relocated_snapshot(&source, expected).is_ok();
			if !fs::same_contents(&restored, expected, true) && !inverse_rename {
				return Err(fs::invalid("Restored move source changed"));
			}
		}
		change.replacement_restored = true;
		restore_central(change, &destination, &mut save)?;
		save(change)?;
		cleanup_undo_workspace(change, &work.join("undo"))?;
		save(change)?;
		return Ok(());
	}
	// A previous inverse may have published the restored source and died while
	// cleaning destination data. Recognize that commit before inspecting leftovers.
	if change.operation == Operation::Move && fs::exists(&source)? {
		let reference = change
			.undo_published
			.as_ref()
			.or(change.undo_snapshot.as_ref())
			.unwrap_or(&change.before);
		let actual = fs::snapshot_matching(&source, reference)?;
		if change.undo_snapshot.as_ref().is_some_and(|expected| {
			fs::same_contents(&actual, expected, true)
				|| (change.undo_published.is_none()
					&& !fs::has_content_hashes(expected)
					&& fs::relocated_snapshot(&source, expected).is_ok())
		}) {
			change.undo_published = Some(actual);
			save(change)?;
		}
	}
	if let Some(expected) = &change.published {
		if fs::removal_pending(&destination, expected)? {
			if change.operation == Operation::Move
				&& change.undo_published.is_none()
				&& !fs::exists(&source)?
			{
				return Err(fs::invalid(
					"Cannot finish inverse cleanup without a restored source",
				));
			}
			fs::remove_recorded(&destination, expected)?;
		}
	}
	if let Some(expected) = &change.staged {
		if fs::removal_pending(&stage, expected)? {
			if change.operation == Operation::Move && !fs::exists(&source)? {
				return Err(fs::invalid(
					"Cannot discard move staging before the source is restored",
				));
			}
			fs::remove_recorded(&stage, expected)?;
		}
	}
	let expected = change.published.as_ref().or(change.staged.as_ref());
	let mut owned = None;
	if let Some(expected) = expected {
		for path in [&destination, &stage] {
			if fs::exists(path)?
				&& (fs::same_contents(&fs::snapshot_matching(path, expected)?, expected, true)
					|| (change.renamed
						&& change.published.is_none()
						&& *path == destination
						&& !fs::has_content_hashes(expected)
						&& fs::relocated_snapshot(path, expected).is_ok()))
			{
				owned = Some(path.clone());
				break;
			}
		}
	} else if change.renamed
		&& fs::exists(&stage)?
		&& (if fs::has_content_hashes(&change.before) {
			fs::same_contents(&fs::snapshot(&stage)?, &change.before, true)
		} else {
			fs::relocated_snapshot(&stage, &change.before).is_ok()
		}) {
		owned = Some(stage.clone());
	}
	if let Some(owned) = owned {
		let reference = change
			.published
			.as_ref()
			.or(change.staged.as_ref())
			.unwrap_or(&change.before);
		let expected = fs::snapshot_matching(&owned, reference)?;
		if change.operation == Operation::Move {
			if !fs::exists(&source)? {
				change.undo_snapshot = Some(expected.clone());
				save(change)?;
				match fs::rename_new(&owned, &source) {
					Ok(()) => {
						fs::sync_parent(&source)?;
						fs::sync_parent(&owned)?;
						change.undo_published = Some(fs::relocated_snapshot(&source, &expected)?);
						save(change)?;
					}
					Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
						if change.undo_workspace.is_none() {
							let undo_dir = source
								.parent()
								.ok_or_else(|| fs::invalid("Missing source parent"))?
								.join(format!(".fileberg-undo-{}", journal::new_id()?));
							fs::private_directory(&undo_dir)?;
							change.undo_workspace = fs::snapshot(&undo_dir)?.0.into_iter().next();
							change.undo_stage = StoredPath::new(&undo_dir.join("part"));
						}
						save(change)?;
						let undo_stage = change.undo_stage.path();
						validate_undo_workspace(change)?;
						if fs::exists(&undo_stage)? {
							let partial = fs::snapshot(&undo_stage)?;
							fs::remove_recorded(&undo_stage, &partial)?;
						}
						copy_tree(
							&owned,
							&undo_stage,
							&expected,
							true,
							&AtomicBool::new(false),
							&mut |_| {},
						)?;
						let staged = fs::snapshot(&undo_stage)?;
						if !fs::same_contents(&staged, &expected, false) {
							return Err(fs::invalid("Undo copy failed verification"));
						}
						fs::sync_tree(&undo_stage, &staged)?;
						change.undo_snapshot = Some(staged);
						save(change)?;
						fs::rename_new(&undo_stage, &source)?;
						fs::sync_parent(&source)?;
						change.undo_published = Some(fs::snapshot(&source)?);
						save(change)?;
						fs::remove_recorded(&owned, &expected)?;
						fs::sync_parent(&owned)?;
					}
					Err(error) => return Err(error),
				}
			} else {
				let current = fs::snapshot_matching(&source, &change.before)?;
				let original_survives =
					!change.renamed && fs::same_contents(&current, &change.before, true);
				let inverse_survives = change
					.undo_snapshot
					.as_ref()
					.is_some_and(|snapshot| fs::same_contents(&current, snapshot, true));
				if !original_survives && !inverse_survives {
					return Err(fs::invalid(
						"Original path is occupied or changed; preserving both locations",
					));
				}
				fs::remove_recorded(&owned, &expected)?;
			}
		} else {
			fs::remove_recorded(&owned, &expected)?;
			if policy != Durability::Buffered {
				fs::sync_parent(&owned)?;
			}
		}
	} else if change.published.is_some() && fs::exists(&destination)? {
		return Err(fs::invalid(
			"Destination changed; rollback cannot remove it",
		));
	} else if change.operation == Operation::Move && !fs::exists(&source)? {
		return Err(fs::invalid(
			"Move data unavailable; preserving recovery record",
		));
	}
	if fs::removal_pending(&source, &change.before)? {
		if !fs::exists(&source)? {
			return Err(fs::invalid("Source rollback is not yet published"));
		}
		fs::remove_recorded(&source, &change.before)?;
	}
	if change.central.is_some() {
		restore_central(change, &destination, &mut save)?;
	} else if fs::exists(&backup)? {
		let original = change
			.replaced
			.as_ref()
			.ok_or_else(|| fs::invalid("Unrecorded recovery data"))?;
		if !fs::same_contents(&fs::snapshot(&backup)?, original, true) {
			return Err(fs::invalid("Replacement backup changed; preserving it"));
		}
		fs::rename_new(&backup, &destination)?;
		fs::sync_parent(&backup)?;
		fs::sync_parent(&destination)?;
		change.replacement_restored = true;
		save(change)?;
	}
	cleanup_undo_workspace(change, &work.join("undo"))?;
	save(change)?;

	Ok(())
}

fn restore_central(
	change: &mut Change, destination: &Path, save: &mut impl FnMut(&Change) -> io::Result<()>,
) -> io::Result<()> {
	let Some(mut central) = change.central.clone() else {
		return Ok(());
	};
	let original = change
		.replaced
		.clone()
		.ok_or_else(|| fs::invalid("Central recovery lacks original entry metadata"))?;
	super::central::restore(&mut central, destination, &original, |state| {
		change.central = Some(state.clone());
		save(change)
	})?;
	change.central = Some(central);
	change.replacement_restored = true;
	save(change)
}

fn cleanup_undo_workspace(change: &mut Change, default_stage: &Path) -> io::Result<()> {
	if change.undo_workspace.is_some() {
		let stage = change.undo_stage.path();
		let parent = stage
			.parent()
			.ok_or_else(|| fs::invalid("Missing undo workspace"))?;
		if fs::exists(parent)? {
			validate_undo_workspace(change)?;
			// Only an empty, identity-checked workspace can be retired here.
			stdfs::remove_dir(parent)?;
			fs::sync_parent(parent)?;
		} else {
			use std::os::unix::fs::MetadataExt;
			let expected = change
				.undo_workspace
				.as_ref()
				.ok_or_else(|| fs::invalid("Missing workspace identity"))?;
			let outer = parent
				.parent()
				.ok_or_else(|| fs::invalid("Missing workspace parent"))?;
			if stdfs::symlink_metadata(outer)?.dev() != expected.device {
				return Err(fs::invalid("Undo storage is offline"));
			}
		}
		change.undo_workspace = None;
		change.undo_stage = StoredPath::new(default_stage);
	}

	Ok(())
}

fn validate_undo_workspace(change: &Change) -> io::Result<()> {
	use std::os::unix::fs::MetadataExt;
	let stage = change.undo_stage.path();
	let parent = stage
		.parent()
		.ok_or_else(|| fs::invalid("Missing undo workspace"))?;
	fs::validate_private(parent)?;
	let current = stdfs::symlink_metadata(parent)?;
	let expected = change
		.undo_workspace
		.as_ref()
		.ok_or_else(|| fs::invalid("Unrecorded undo workspace"))?;
	if (current.dev(), current.ino()) != (expected.device, expected.inode) {
		return Err(fs::invalid("Undo workspace identity changed"));
	}

	Ok(())
}
