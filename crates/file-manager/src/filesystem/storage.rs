use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub struct Usage {
	pub total: u64,
	pub available: u64,
}

impl Usage {
	pub fn read(path: &Path) -> Option<Self> {
		let stat = nix::sys::statvfs::statvfs(path).ok()?;
		Some(Self {
			total: stat.blocks().saturating_mul(stat.fragment_size()),
			available: stat.blocks_available().saturating_mul(stat.fragment_size()),
		})
	}
	pub fn used_percent(self) -> u16 {
		if self.total == 0 {
			return 0;
		}
		((u128::from(self.total.saturating_sub(self.available)) * 100) / u128::from(self.total))
			.min(100) as u16
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn capacity_and_progress_are_real_and_bounded() {
		let usage = Usage::read(Path::new("/"));
		assert!(
			usage.is_some_and(|u| u.total > 0 && u.available <= u.total && u.used_percent() <= 100)
		);
		assert_eq!(
			Usage {
				total: 500,
				available: 150
			}
			.used_percent(),
			70
		);
		assert_eq!(
			Usage {
				total: 0,
				available: 0
			}
			.used_percent(),
			0
		);
	}
}
