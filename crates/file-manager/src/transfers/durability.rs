//! Blocking worker-side copying and writeback pacing, not final durability.
//!
//! Range writeback is synchronous: there is no copy/writeback overlap. Callers
//! must still sync the finished file and affected directories before success.
//! This streaming implementation does not preserve sparse holes.

use std::{
	fs::{File, OpenOptions},
	io::{self, Read, Seek, Write},
	os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
	path::Path,
	sync::atomic::{AtomicBool, Ordering},
	time::{Duration, Instant},
};

const BUFFER_SIZE: usize = 64 * 1024;
const INITIAL_WINDOW: u64 = 1024 * 1024;
const MIN_WINDOW: u64 = BUFFER_SIZE as u64;
const MAX_WINDOW: u64 = 16 * 1024 * 1024;
const TARGET_TIME: Duration = Duration::from_millis(200);

/// Copy from the current cursor positions, reporting successfully written byte
/// deltas. Cancellation leaves both paths and any partially written data intact.
/// Pacing bounds submitted dirty bytes, not the latency of a blocking syscall.
pub(crate) fn copy_file(
	input: &mut impl Read, output: &mut File, paced: bool, cancelled: &AtomicBool,
	progress: impl FnMut(u64),
) -> io::Result<u64> {
	let mut fallback = false;
	copy_with_flush(
		input,
		output,
		paced,
		cancelled,
		progress,
		|file, offset, length| flush_range(file, offset, length, &mut fallback, sync_file_range),
	)
}

fn copy_with_flush(
	input: &mut impl Read, output: &mut File, paced: bool, cancelled: &AtomicBool,
	mut progress: impl FnMut(u64), mut flush: impl FnMut(&File, u64, u64) -> io::Result<()>,
) -> io::Result<u64> {
	check_cancelled(cancelled)?;
	let mut offset = if paced { output.stream_position()? } else { 0 };
	let mut buffer = vec![0; BUFFER_SIZE];
	let mut copied = 0;
	let mut pending = 0;
	let mut window = INITIAL_WINDOW;
	let mut started = Instant::now();

	loop {
		check_cancelled(cancelled)?;
		let capacity = if paced {
			(window - pending).min(BUFFER_SIZE as u64) as usize
		} else {
			BUFFER_SIZE
		};
		let length = match input.read(&mut buffer[..capacity]) {
			Ok(0) => break,
			Ok(length) => length,
			Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
			Err(error) => return Err(error),
		};

		let mut written = 0;
		while written < length {
			check_cancelled(cancelled)?;
			match output.write(&buffer[written..length]) {
				Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
				Ok(bytes) => {
					written += bytes;
					copied += bytes as u64;
					pending += bytes as u64;
					progress(bytes as u64);
				}
				Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
				Err(error) => return Err(error),
			}
		}

		check_cancelled(cancelled)?;
		if paced && (pending == window || started.elapsed() >= TARGET_TIME) {
			flush(output, offset, pending)?;
			offset = offset.checked_add(pending).ok_or_else(offset_overflow)?;
			window = next_window(window, pending, started.elapsed());
			pending = 0;
			started = Instant::now();
		}
	}

	check_cancelled(cancelled)?;
	if paced && pending > 0 {
		flush(output, offset, pending)?;
	}
	check_cancelled(cancelled)?;

	Ok(copied)
}

fn check_cancelled(cancelled: &AtomicBool) -> io::Result<()> {
	if cancelled.load(Ordering::Relaxed) {
		Err(io::Error::new(io::ErrorKind::Interrupted, "copy cancelled"))
	} else {
		Ok(())
	}
}

fn next_window(current: u64, bytes: u64, elapsed: Duration) -> u64 {
	let target = (u128::from(bytes) * TARGET_TIME.as_nanos()) / elapsed.as_nanos().max(1);
	target.clamp(
		u128::from(MIN_WINDOW),
		u128::from((current * 2).min(MAX_WINDOW)),
	) as u64
}

fn offset_overflow() -> io::Error {
	io::Error::new(
		io::ErrorKind::InvalidInput,
		"writeback range exceeds file offsets",
	)
}

fn sync_file_range(file: &File, offset: u64, length: u64) -> io::Result<()> {
	let end = offset.checked_add(length).ok_or_else(offset_overflow)?;
	if end > libc::off64_t::MAX as u64 {
		return Err(offset_overflow());
	}

	loop {
		// SAFETY: the file owns a live descriptor; checked offsets fit off64_t.
		let result = unsafe {
			libc::sync_file_range(
				file.as_raw_fd(),
				offset as libc::off64_t,
				length as libc::off64_t,
				libc::SYNC_FILE_RANGE_WRITE
					| libc::SYNC_FILE_RANGE_WAIT_BEFORE
					| libc::SYNC_FILE_RANGE_WAIT_AFTER,
			)
		};
		if result == 0 {
			return Ok(());
		}

		let error = io::Error::last_os_error();
		if error.kind() != io::ErrorKind::Interrupted {
			return Err(error);
		}
	}
}

fn flush_range(
	file: &File, offset: u64, length: u64, fallback: &mut bool,
	mut range: impl FnMut(&File, u64, u64) -> io::Result<()>,
) -> io::Result<()> {
	if !*fallback {
		match range(file, offset, length) {
			Ok(()) => return Ok(()),
			// Valid regular-file ranges can be rejected by unsupported filesystems
			// with EINVAL. Overflow is rejected before entering the syscall.
			Err(error)
				if matches!(
					error.raw_os_error(),
					Some(libc::ENOSYS | libc::EOPNOTSUPP | libc::EINVAL)
				) =>
			{
				*fallback = true;
			}
			Err(error) => return Err(error),
		}
	}

	file.sync_data()
}

/// Sync a directory without following a symlink in the final path component.
pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
	OpenOptions::new()
		.read(true)
		.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
		.open(path)?
		.sync_all()
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]

	use super::*;
	use crate::test_support::TestDir;

	#[test]
	fn copies_stream_and_reports_byte_deltas() {
		let directory = TestDir::new();
		let source = directory.path().join("source");
		let target = directory.path().join("target");
		let bytes: Vec<_> = (0..INITIAL_WINDOW as usize + 137)
			.map(|index| (index % 251) as u8)
			.collect();
		std::fs::write(&source, &bytes).unwrap();

		for paced in [false, true] {
			let mut input = File::open(&source).unwrap();
			let mut output = File::create(&target).unwrap();
			let mut reported = 0;
			let copied = copy_file(
				&mut input,
				&mut output,
				paced,
				&AtomicBool::new(false),
				|delta| reported += delta,
			)
			.unwrap();

			assert_eq!(copied, bytes.len() as u64);
			assert_eq!(reported, copied);
			assert_eq!(std::fs::read(&target).unwrap(), bytes);
		}
	}

	#[test]
	fn cancellation_preserves_paths_and_partial_output() {
		let directory = TestDir::new();
		let source = directory.path().join("source");
		let target = directory.path().join("target");
		std::fs::write(&source, vec![7; BUFFER_SIZE * 3]).unwrap();
		let mut input = File::open(&source).unwrap();
		let mut output = File::create(&target).unwrap();
		let cancelled = AtomicBool::new(false);

		let result = copy_file(&mut input, &mut output, true, &cancelled, |_| {
			cancelled.store(true, Ordering::Relaxed)
		});

		assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
		assert!(source.exists());
		assert_eq!(std::fs::read(&target).unwrap(), vec![7; BUFFER_SIZE]);
	}

	#[test]
	fn pre_cancelled_copy_writes_nothing() {
		let directory = TestDir::new();
		let source = directory.path().join("source");
		let target = directory.path().join("target");
		std::fs::write(&source, b"untouched").unwrap();
		let mut input = File::open(&source).unwrap();
		let mut output = File::create(&target).unwrap();

		let result = copy_file(
			&mut input,
			&mut output,
			true,
			&AtomicBool::new(true),
			|_| panic!("no progress"),
		);

		assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
		assert_eq!(output.metadata().unwrap().len(), 0);
		assert!(source.exists());
		assert!(target.exists());
	}

	#[test]
	fn window_adaptation_is_rate_based_and_bounded() {
		assert_eq!(
			next_window(INITIAL_WINDOW, INITIAL_WINDOW, TARGET_TIME),
			INITIAL_WINDOW
		);
		assert_eq!(
			next_window(INITIAL_WINDOW, INITIAL_WINDOW, Duration::ZERO),
			INITIAL_WINDOW * 2
		);
		assert_eq!(
			next_window(MAX_WINDOW, MAX_WINDOW, Duration::from_millis(1)),
			MAX_WINDOW
		);
		assert_eq!(
			next_window(INITIAL_WINDOW, INITIAL_WINDOW, Duration::from_secs(100)),
			MIN_WINDOW
		);
		assert_eq!(
			next_window(INITIAL_WINDOW, INITIAL_WINDOW, Duration::from_millis(400)),
			INITIAL_WINDOW / 2
		);
	}

	#[test]
	fn range_errors_propagate_and_unsupported_ranges_fall_back() {
		let directory = TestDir::new();
		let file = File::create(directory.path().join("target")).unwrap();
		let mut fallback = false;

		let error = flush_range(&file, 0, 1, &mut fallback, |_, _, _| {
			Err(io::Error::from_raw_os_error(libc::EIO))
		})
		.unwrap_err();
		assert_eq!(error.raw_os_error(), Some(libc::EIO));
		assert!(!fallback);

		flush_range(&file, 0, 1, &mut fallback, |_, _, _| {
			Err(io::Error::from_raw_os_error(libc::ENOSYS))
		})
		.unwrap();
		assert!(fallback);
		flush_range(&file, 0, 1, &mut fallback, |_, _, _| {
			panic!("fallback must stay enabled")
		})
		.unwrap();
	}

	#[test]
	fn paced_ranges_are_contiguous_bounded_and_include_final_tail() {
		let directory = TestDir::new();
		let source = directory.path().join("source");
		let target = directory.path().join("target");
		let length = INITIAL_WINDOW * 3 + 137;
		std::fs::write(&source, vec![3; length as usize]).unwrap();
		let mut input = File::open(&source).unwrap();
		let mut output = File::create(&target).unwrap();
		output.write_all(b"prefix").unwrap();
		let mut next_offset = 6;
		let mut flushed = 0;

		let copied = copy_with_flush(
			&mut input,
			&mut output,
			true,
			&AtomicBool::new(false),
			|_| {},
			|_, offset, bytes| {
				assert_eq!(offset, next_offset);
				assert!((1..=MAX_WINDOW).contains(&bytes));
				next_offset += bytes;
				flushed += bytes;
				Ok(())
			},
		)
		.unwrap();

		assert_eq!(copied, length);
		assert_eq!(flushed, length);
		assert_eq!(next_offset, length + 6);
	}

	#[test]
	fn copy_propagates_writeback_failure_without_removing_paths() {
		let directory = TestDir::new();
		let source = directory.path().join("source");
		let target = directory.path().join("target");
		std::fs::write(&source, b"tail").unwrap();
		let mut input = File::open(&source).unwrap();
		let mut output = File::create(&target).unwrap();

		let result = copy_with_flush(
			&mut input,
			&mut output,
			true,
			&AtomicBool::new(false),
			|_| {},
			|_, _, _| Err(io::Error::from_raw_os_error(libc::EIO)),
		);

		assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::EIO));
		assert!(source.exists());
		assert_eq!(std::fs::read(&target).unwrap(), b"tail");
	}

	#[test]
	fn directory_sync_rejects_symlinks_and_regular_files() {
		let directory = TestDir::new();
		let link = directory.path().join("link");
		let file = directory.path().join("file");
		std::os::unix::fs::symlink(directory.path(), &link).unwrap();
		std::fs::write(&file, b"file").unwrap();

		sync_directory(directory.path()).unwrap();
		assert!(sync_directory(&link).is_err());
		assert!(sync_directory(&file).is_err());
	}
}
