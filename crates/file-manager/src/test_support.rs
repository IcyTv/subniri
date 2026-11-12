#![allow(clippy::unwrap_used)]
use std::{
	path::{Path, PathBuf},
	sync::atomic::{AtomicU64, Ordering},
};

pub struct TestDir(PathBuf);
impl TestDir {
	pub fn new() -> Self {
		let preferred = PathBuf::from("/tmp/opencode");
		let root = if preferred.is_dir() {
			preferred
		} else {
			std::env::temp_dir()
		};
		Self::new_in(&root)
	}

	pub fn new_in(root: &Path) -> Self {
		static NEXT: AtomicU64 = AtomicU64::new(0);
		let path = root.join(format!(
			"fileberg-test-{}-{}",
			std::process::id(),
			NEXT.fetch_add(1, Ordering::Relaxed)
		));
		std::fs::create_dir(&path).unwrap();
		Self(path)
	}
	pub fn path(&self) -> &Path {
		&self.0
	}
}
impl Drop for TestDir {
	fn drop(&mut self) {
		let _ = std::fs::remove_dir_all(&self.0);
	}
}
