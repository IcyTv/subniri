use std::{
	os::fd::{AsFd, FromRawFd, IntoRawFd, OwnedFd, RawFd},
	process,
	sync::atomic::{AtomicI32, Ordering},
};

use nix::unistd::{ForkResult, fork, pipe, read, setsid, write};

// -1 means "not daemonized" or "already signaled"; never a valid fd.
static READY_FD: AtomicI32 = AtomicI32::new(-1);

/// Mirrors `swaylock --daemonize`: forks into the background only once the
/// lock screen is actually up, so a caller like swayidle's before-sleep/lock
/// hooks blocks until locking has genuinely succeeded or failed, instead of
/// returning the instant the process starts.
///
/// Must run before anything else in `main`: forking after Wayland, iced, or
/// tokio have opened connections or spawned threads would hand the child a
/// half-initialized copy of them.
pub(crate) fn daemonize() {
	let (read_end, write_end) = pipe().unwrap_or_else(|error| {
		eprintln!("frostguard: daemonize: pipe failed: {error}");
		process::exit(1);
	});

	// Safety: between fork and the child's first non-trivial call (setsid),
	// only the already-performed pipe setup and simple drops run.
	match unsafe { fork() } {
		Ok(ForkResult::Parent { .. }) => {
			// The parent's copy of the write end must close now: EOF on
			// read_end (an unexpected child exit) is only observable once
			// every write end, including this one, is gone.
			drop(write_end);
			let mut status = [0u8];
			let locked = matches!(read(read_end.as_fd(), &mut status), Ok(1) if status[0] == 0);
			process::exit(if locked { 0 } else { 1 });
		}
		Ok(ForkResult::Child) => {
			drop(read_end);
			// Detach from the controlling terminal so the shell or service
			// that spawned us exiting doesn't take the lock down with it.
			if let Err(error) = setsid() {
				eprintln!("frostguard: daemonize: setsid failed: {error}");
			}
			READY_FD.store(write_end.into_raw_fd(), Ordering::Release);
		}
		Err(error) => {
			eprintln!("frostguard: daemonize: fork failed: {error}");
			process::exit(1);
		}
	}
}

/// Reports lock success or failure to a `--daemonize` parent still waiting
/// on `daemonize()`. A no-op once called once, or when never daemonized.
pub(crate) fn signal_ready(success: bool) {
	let fd = READY_FD.swap(-1, Ordering::AcqRel);
	if fd < 0 {
		return;
	}
	// Safety: this fd was stored by `daemonize()` as an owned, open pipe
	// write end that nothing else has touched since; it is taken exactly
	// once, because the swap above already reset READY_FD to -1.
	let write_end = unsafe { OwnedFd::from_raw_fd(fd as RawFd) };
	let _ = write(write_end.as_fd(), &[u8::from(!success)]);
}

#[cfg(test)]
mod tests {
	use super::*;

	// A single test, not two: both exercise the shared READY_FD static, and
	// cargo runs tests in parallel threads within one process.
	#[test]
	fn signal_ready_is_a_no_op_until_daemonized_then_fires_once() {
		signal_ready(true);
		signal_ready(false);

		let (read_end, write_end) = pipe().unwrap();
		READY_FD.store(write_end.into_raw_fd(), Ordering::Release);

		signal_ready(true);
		let mut status = [0xFFu8];
		assert_eq!(read(read_end.as_fd(), &mut status).unwrap(), 1);
		assert_eq!(status[0], 0);

		// The fd was consumed and closed; a second call must not double-close it.
		signal_ready(false);
		assert_eq!(read(read_end.as_fd(), &mut status).unwrap(), 0);
	}
}
