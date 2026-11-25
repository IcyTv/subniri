use std::path::PathBuf;

use daemon_common::{ChargeLimit, ChargeLimitState};

const DEFAULT_BATTERY_DEVICE: &str = "BAT0";
const POWER_SUPPLY_ROOT: &str = "/sys/class/power_supply";

#[derive(Clone)]
pub struct ChargeLimitService {
	end_threshold_path: PathBuf,
	start_threshold_path: PathBuf,
	available: bool,
}

impl ChargeLimitService {
	pub fn new(enabled: bool, battery_device: Option<&str>) -> Self {
		let device = battery_device.unwrap_or(DEFAULT_BATTERY_DEVICE);
		let root = PathBuf::from(POWER_SUPPLY_ROOT).join(device);
		let end_threshold_path = root.join("charge_control_end_threshold");
		let start_threshold_path = root.join("charge_control_start_threshold");

		Self {
			available: enabled && end_threshold_path.exists(),
			end_threshold_path,
			start_threshold_path,
		}
	}

}

impl ChargeLimit for ChargeLimitService {
	async fn state(&self) -> ChargeLimitState {
		if !self.available {
			return ChargeLimitState::default();
		}

		match read_threshold(&self.end_threshold_path).await {
			Ok(end_threshold) => ChargeLimitState {
				available: true,
				end_threshold,
			},
			Err(error) => {
				log::warn!("Failed to read charge limit: {error}");
				ChargeLimitState::default()
			}
		}
	}

	async fn set_end_threshold(&self, value: u32) -> zbus::fdo::Result<()> {
		if !self.available {
			return Err(zbus::fdo::Error::Failed(
				"Charge limit is not supported on this hardware".into(),
			));
		}
		if value > 100 {
			return Err(zbus::fdo::Error::InvalidArgs(
				"Charge limit must be between 0 and 100".into(),
			));
		}

		write_threshold(&self.end_threshold_path, value)
			.await
			.map_err(permission_error)?;
		// Best-effort hysteresis; the end threshold is the control users actually see.
		let _ = write_threshold(&self.start_threshold_path, value.saturating_sub(5)).await;

		Ok(())
	}
}

fn permission_error(error: std::io::Error) -> zbus::fdo::Error {
	zbus::fdo::Error::Failed(format!(
		"Failed to set charge limit: {error}. Add the user to the subniri-power group \
		 (see NixOS config) and re-login."
	))
}

async fn read_threshold(path: &std::path::Path) -> std::io::Result<u32> {
	let contents = tokio::fs::read_to_string(path).await?;
	contents
		.trim()
		.parse()
		.map_err(|_| std::io::Error::other(format!("invalid charge threshold in {path:?}")))
}

async fn write_threshold(path: &std::path::Path, value: u32) -> std::io::Result<()> {
	tokio::fs::write(path, value.to_string()).await
}

#[cfg(test)]
mod tests {
	use super::*;

	struct TempDir(PathBuf);

	impl TempDir {
		fn create() -> Self {
			let path = std::env::temp_dir().join(format!(
				"subniri-charge-limit-test-{}",
				uuid::Uuid::new_v4()
			));
			std::fs::create_dir_all(&path).unwrap();
			Self(path)
		}
	}

	impl Drop for TempDir {
		fn drop(&mut self) {
			let _ = std::fs::remove_dir_all(&self.0);
		}
	}

	fn service_for(dir: &std::path::Path) -> ChargeLimitService {
		ChargeLimitService {
			available: dir.join("charge_control_end_threshold").exists(),
			end_threshold_path: dir.join("charge_control_end_threshold"),
			start_threshold_path: dir.join("charge_control_start_threshold"),
		}
	}

	#[tokio::test]
	async fn reports_unavailable_without_sysfs_files() {
		let dir = TempDir::create();
		let service = service_for(&dir.0);
		assert_eq!(service.state().await, ChargeLimitState::default());
		assert!(matches!(
			service.set_end_threshold(80).await,
			Err(zbus::fdo::Error::Failed(_))
		));
	}

	#[tokio::test]
	async fn reads_and_writes_thresholds_and_rejects_out_of_range() {
		let dir = TempDir::create();
		std::fs::write(dir.0.join("charge_control_end_threshold"), "100\n").unwrap();
		std::fs::write(dir.0.join("charge_control_start_threshold"), "0\n").unwrap();
		let service = service_for(&dir.0);

		assert_eq!(
			service.state().await,
			ChargeLimitState {
				available: true,
				end_threshold: 100,
			}
		);

		service.set_end_threshold(80).await.unwrap();
		assert_eq!(
			service.state().await,
			ChargeLimitState {
				available: true,
				end_threshold: 80,
			}
		);
		assert_eq!(
			read_threshold(&dir.0.join("charge_control_start_threshold"))
				.await
				.unwrap(),
			75
		);

		assert!(matches!(
			service.set_end_threshold(101).await,
			Err(zbus::fdo::Error::InvalidArgs(_))
		));
	}
}
