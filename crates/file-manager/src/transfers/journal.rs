use super::{
	fs,
	model::{Journal, Status},
};
use std::{
	fs::{self as stdfs, File, OpenOptions},
	io::{self, Read, Write},
	os::unix::fs::OpenOptionsExt,
	path::{Path, PathBuf},
	time::{SystemTime, UNIX_EPOCH},
};

pub(crate) struct Store {
	root: PathBuf,
	recovery_root: PathBuf,
	_lock: File,
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::test_support::TestDir;
	use std::sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	};

	#[test]
	fn waiting_for_another_instance_is_cancellable() {
		let directory = TestDir::new();
		let root = directory.path().join("state");

		let owner = Store::open(root.clone()).unwrap();
		assert_eq!(
			Store::open(root.clone()).err().unwrap().kind(),
			io::ErrorKind::WouldBlock
		);
		let cancelled = Arc::new(AtomicBool::new(false));
		let signal = cancelled.clone();

		let worker = std::thread::spawn(move || {
			Store::open_wait(root, &signal)
				.err()
				.map(|error| error.kind())
		});
		cancelled.store(true, Ordering::Release);

		assert_eq!(worker.join().unwrap(), Some(io::ErrorKind::Interrupted));
		drop(owner);
	}
}

impl Store {
	/// Serialize mutating jobs without blocking the UI or failing merely because
	/// a concurrent history read briefly holds the cooperative journal lock.
	pub fn open_wait(root: PathBuf, cancelled: &std::sync::atomic::AtomicBool) -> io::Result<Self> {
		let started = std::time::Instant::now();
		loop {
			if cancelled.load(std::sync::atomic::Ordering::Acquire) {
				return Err(io::Error::new(
					io::ErrorKind::Interrupted,
					"Transfer cancelled while waiting for recovery storage",
				));
			}
			match Self::open(root.clone()) {
				Ok(store) => return Ok(store),
				Err(error)
					if error.kind() == io::ErrorKind::WouldBlock
						&& started.elapsed() < std::time::Duration::from_secs(30) =>
				{
					std::thread::sleep(std::time::Duration::from_millis(50))
				}
				Err(error) => return Err(error),
			}
		}
	}

	pub fn open(root: PathBuf) -> io::Result<Self> {
		let recovery = dirs::data_local_dir()
			.ok_or_else(|| fs::invalid("Cannot locate XDG_DATA_HOME for replacement recovery"))?
			.join("fileberg/recovery");
		Self::open_with_recovery(root, recovery)
	}

	pub(super) fn open_with_recovery(root: PathBuf, recovery_root: PathBuf) -> io::Result<Self> {
		if !fs::exists(&root)? {
			stdfs::create_dir_all(
				root.parent()
					.ok_or_else(|| fs::invalid("Missing state parent"))?,
			)?;
			match fs::private_directory(&root) {
				Ok(()) => {}
				Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
				Err(error) => return Err(error),
			}
		}
		fs::validate_private(&root)?;
		let root = stdfs::canonicalize(root)?;
		let lock = OpenOptions::new()
			.read(true)
			.write(true)
			.create(true)
			.truncate(false)
			.mode(0o600)
			.custom_flags(libc::O_NOFOLLOW)
			.open(root.join("lock"))?;
		fs::lock(&lock)?;

		Ok(Self {
			root,
			recovery_root,
			_lock: lock,
		})
	}

	pub(super) fn recovery_root(&self) -> &Path {
		&self.recovery_root
	}

	pub fn default_path() -> io::Result<PathBuf> {
		dirs::state_dir()
			.map(|path| path.join("fileberg/transfers"))
			.ok_or_else(|| fs::invalid("Cannot locate XDG_STATE_HOME"))
	}

	pub(super) fn workspace(
		&self, destination_parent: &Path, id: &str, index: usize,
	) -> io::Result<PathBuf> {
		use std::os::unix::fs::MetadataExt;
		let destination = OpenOptions::new()
			.read(true)
			.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
			.open(destination_parent)?;
		let state = File::open(&self.root)?;
		let destination_mount = crate::filesystem::classification::mount_id(&destination);
		let state_mount = crate::filesystem::classification::mount_id(&state);
		let parent = if destination.metadata()?.dev() == state.metadata()?.dev()
			&& destination_mount.is_some()
			&& destination_mount == state_mount
		{
			&self.root
		} else {
			destination_parent
		};

		Ok(parent.join(format!(".fileberg-transfer-{id}-{index}")))
	}

	pub(super) fn save(&self, journal: &Journal) -> io::Result<()> {
		validate_id(&journal.id)?;
		let destination = self.root.join(format!("{}.json", journal.id));
		let temporary = self.root.join(format!("{}.next", journal.id));
		let mut file = OpenOptions::new()
			.write(true)
			.create(true)
			.truncate(true)
			.mode(0o600)
			.custom_flags(libc::O_NOFOLLOW)
			.open(&temporary)?;
		let bytes = serde_json::to_vec(journal)?;
		file.write_all(&bytes)?;
		file.sync_all()?;
		stdfs::rename(&temporary, &destination)?;
		fs::sync_parent(&destination)
	}

	pub fn load(&self, id: &str) -> io::Result<Journal> {
		validate_id(id)?;
		let file = OpenOptions::new()
			.read(true)
			.custom_flags(libc::O_NOFOLLOW)
			.open(self.root.join(format!("{id}.json")))?;
		let journal: Journal = serde_json::from_reader(file)?;
		if journal.version != 1 || journal.id != id {
			return Err(fs::invalid("Unsupported or mismatched transfer journal"));
		}

		Ok(journal)
	}

	pub fn history(&self) -> io::Result<Vec<Journal>> {
		let mut records = Vec::new();
		for entry in stdfs::read_dir(&self.root)? {
			let path = entry?.path();
			if path
				.extension()
				.is_some_and(|extension| extension == "json")
			{
				let id = path
					.file_stem()
					.and_then(|stem| stem.to_str())
					.ok_or_else(|| fs::invalid("Invalid journal filename"))?;
				records.push(self.load(id)?);
			}
		}
		records.sort_by_key(|record| std::cmp::Reverse(record.created));

		Ok(records)
	}

	pub(super) fn forget(&self, journal: &Journal) -> io::Result<()> {
		if !matches!(
			journal.status,
			Status::Complete | Status::RolledBack | Status::Expiring
		) {
			return Err(fs::invalid("Cannot expire unfinished recovery"));
		}
		validate_id(&journal.id)?;
		let path = self.root.join(format!("{}.json", journal.id));
		stdfs::remove_file(&path)?;
		fs::sync_parent(&path)
	}
}

fn validate_id(id: &str) -> io::Result<()> {
	if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
		return Err(fs::invalid("Invalid transfer identifier"));
	}

	Ok(())
}

pub(super) fn new_id() -> io::Result<String> {
	let mut bytes = [0u8; 16];
	File::open(Path::new("/dev/urandom"))?.read_exact(&mut bytes)?;
	Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn now() -> io::Result<u64> {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|time| time.as_secs())
		.map_err(|error| fs::invalid(error.to_string()))
}
