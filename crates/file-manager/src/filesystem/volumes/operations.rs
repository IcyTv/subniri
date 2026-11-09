use std::{
	collections::HashMap,
	path::{Path, PathBuf},
};

use zbus::zvariant::Value;

use super::{Identity, Snapshot, Volume, discover_with_usage};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
	Mount,
	Unmount,
	Eject,
	PowerOff,
}

impl Action {
	pub(crate) fn label(self) -> &'static str {
		match self {
			Self::Mount => "Mount and open",
			Self::Unmount => "Unmount",
			Self::Eject => "Eject media",
			Self::PowerOff => "Safely remove drive",
		}
	}

	fn drive_wide(self) -> bool {
		matches!(self, Self::Eject | Self::PowerOff)
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct State {
	identity: Identity,
	mounts: Vec<(PathBuf, Option<u64>, Option<PathBuf>)>,
	ejectable: bool,
	can_power_off: bool,
	sibling_id: String,
}

impl From<&Volume> for State {
	fn from(volume: &Volume) -> Self {
		Self {
			identity: volume.identity.clone(),
			mounts: volume
				.mounts
				.iter()
				.map(|m| (m.path.clone(), m.mount_id, m.root.clone()))
				.collect(),
			ejectable: volume.drive.ejectable,
			can_power_off: volume.drive.can_power_off,
			sibling_id: volume.drive.sibling_id.clone(),
		}
	}
}

#[derive(Debug, Clone)]
pub(crate) struct Operation {
	pub action: Action,
	pub target: Identity,
	pub name: String,
	states: Vec<State>,
}

impl Operation {
	pub(crate) fn prepare(
		snapshot: &Snapshot, volume: &Volume, action: Action,
	) -> Result<Self, String> {
		if volume.ambiguous {
			return Err(
				"Management is unavailable for ambiguous devices or mount namespaces".into(),
			);
		}

		match action {
			Action::Mount if !volume.mounts.is_empty() => {
				return Err("Filesystem is already mounted".into());
			}
			Action::Unmount if volume.mounts.is_empty() => {
				return Err("Filesystem is not mounted".into());
			}
			Action::Eject if !volume.drive.ejectable => {
				return Err("This drive cannot eject media".into());
			}
			Action::PowerOff if !volume.drive.can_power_off => {
				return Err("This drive cannot be powered off".into());
			}
			_ => {}
		}

		let affected: Vec<_> = snapshot
			.volumes
			.iter()
			.filter(|v| {
				if action.drive_wide() {
					v.identity.drive == volume.identity.drive
				} else {
					v.identity.object == volume.identity.object
				}
			})
			.collect();
		if action != Action::Mount
			&& affected
				.iter()
				.any(|v| v.mounts.iter().any(|m| m.path == Path::new("/")))
		{
			return Err("The root filesystem cannot be unmounted or removed".into());
		}
		if action != Action::Mount && affected.iter().any(|v| v.ambiguous || v.mounts.len() > 1) {
			return Err("Management of multi-mount filesystems is not supported yet".into());
		}
		if action.drive_wide() {
			if volume.identity.drive.as_str() == "/"
				|| snapshot.unsafe_drives.contains(&volume.identity.drive)
			{
				return Err("The drive contains an unsupported active device or filesystem".into());
			}
			if action == Action::PowerOff && snapshot.shared_drives.contains(&volume.identity.drive)
			{
				return Err(
					"Safe removal may affect other drives in the same physical device".into(),
				);
			}
		}

		Ok(Self {
			action,
			target: volume.identity.clone(),
			name: volume.name.clone(),
			states: affected.into_iter().map(State::from).collect(),
		})
	}

	pub(crate) fn description(&self) -> String {
		let mut lines = vec![format!(
			"{} “{}” ({})?",
			self.action.label(),
			self.name,
			self.target.device.display()
		)];

		for state in &self.states {
			if self.action.drive_wide() && state.mounts.is_empty() {
				lines.push(format!("{} (not mounted)", state.identity.device.display()));
			}
			for (path, _, _) in &state.mounts {
				lines.push(format!(
					"{}: {}",
					state.identity.device.display(),
					path.display()
				));
			}
		}

		if self.action == Action::Mount {
			lines.push("The system chooses the mount location and options. Authentication may be required.".into());
		} else {
			lines.push(
				"Close files using these destinations first. Busy filesystems will not be forced."
					.into(),
			);
		}

		lines.join("\n")
	}

	fn revalidate(&self, snapshot: &Snapshot) -> Result<(), String> {
		let volume = snapshot
			.volume(&self.target.object)
			.ok_or("Device is no longer available")?;
		let current = Self::prepare(snapshot, volume, self.action)?;
		if self.target != current.target || self.states != current.states {
			return Err("Device or mounts changed. Review the operation again.".into());
		}

		Ok(())
	}

	fn raced_mount(&self, snapshot: &Snapshot) -> Option<PathBuf> {
		let volume = snapshot.volume(&self.target.object)?;
		(self.action == Action::Mount
			&& volume.identity == self.target
			&& !volume.ambiguous
			&& volume.mounts.len() == 1)
			.then(|| volume.mounts.first().map(|m| m.path.clone()))
			.flatten()
	}

	pub(crate) async fn execute(self) -> Result<Option<PathBuf>, String> {
		let connection = zbus::Connection::system()
			.await
			.map_err(|e| e.to_string())?;

		self.execute_with(&mut System { connection }).await
	}

	async fn execute_with(self, backend: &mut impl Backend) -> Result<Option<PathBuf>, String> {
		let snapshot = backend.snapshot().await?;
		if let Some(path) = self.raced_mount(&snapshot) {
			return Ok(Some(path));
		}
		self.revalidate(&snapshot)?;

		if self.action == Action::Mount {
			return match backend.mount(&self.target).await {
				Ok(path) => Ok(Some(path)),
				Err(MountError::AlreadyMounted) => self
					.raced_mount(&backend.snapshot().await?)
					.map(Some)
					.ok_or_else(|| {
						"Filesystem was mounted elsewhere; refresh and select a destination".into()
					}),
				Err(MountError::Failed(error)) => Err(error),
			};
		}

		let mut unmounts: Vec<_> = self
			.states
			.iter()
			.filter(|s| !s.mounts.is_empty())
			.collect();
		unmounts.sort_by_key(|s| {
			std::cmp::Reverse(
				s.mounts
					.iter()
					.map(|(p, _, _)| p.components().count())
					.max()
					.unwrap_or(0),
			)
		});

		for state in unmounts {
			// Recheck the full affected scope before every destructive step. Earlier
			// successful unmounts are the only changes permitted during removal.
			let current = backend.snapshot().await?;
			self.validate_remaining(&current)?;
			backend.unmount(&state.identity).await?;
		}

		if self.action.drive_wide() {
			let current = backend.snapshot().await?;
			self.validate_remaining(&current)?;
			if current
				.volumes
				.iter()
				.any(|v| v.identity.drive == self.target.drive && !v.mounts.is_empty())
			{
				return Err("Drive is still mounted; removal stopped".into());
			}
			backend.remove(&self.target, self.action).await?;
		}

		Ok(None)
	}

	fn validate_remaining(&self, snapshot: &Snapshot) -> Result<(), String> {
		let volume = snapshot
			.volume(&self.target.object)
			.ok_or("Device is no longer available")?;
		// Preparing a drive-wide operation checks root, ambiguity and new siblings.
		let current = Self::prepare(snapshot, volume, self.action)?;
		if current.states.len() != self.states.len()
			|| current
				.states
				.iter()
				.zip(&self.states)
				.any(|(now, before)| {
					now.identity != before.identity
						|| now.ejectable != before.ejectable
						|| now.can_power_off != before.can_power_off
						|| now.sibling_id != before.sibling_id
						|| (!now.mounts.is_empty() && now.mounts != before.mounts)
				}) {
			return Err("Drive contents changed; removal stopped".into());
		}

		Ok(())
	}
}

enum MountError {
	AlreadyMounted,
	Failed(String),
}

trait Backend: Send {
	fn snapshot(&mut self) -> impl std::future::Future<Output = Result<Snapshot, String>> + Send;

	fn mount(
		&mut self, target: &Identity,
	) -> impl std::future::Future<Output = Result<PathBuf, MountError>> + Send;

	fn unmount(
		&mut self, target: &Identity,
	) -> impl std::future::Future<Output = Result<(), String>> + Send;

	fn remove(
		&mut self, target: &Identity, action: Action,
	) -> impl std::future::Future<Output = Result<(), String>> + Send;
}

struct System {
	connection: zbus::Connection,
}

impl Backend for System {
	async fn snapshot(&mut self) -> Result<Snapshot, String> {
		discover_with_usage(false).await
	}

	async fn mount(&mut self, target: &Identity) -> Result<PathBuf, MountError> {
		let proxy = filesystem(&self.connection, &target.object)
			.await
			.map_err(MountError::Failed)?;
		let result: zbus::Result<String> = proxy.call("Mount", &(options(),)).await;

		match result {
			Ok(path) => Ok(path.into()),
			Err(zbus::Error::MethodError(name, ..))
				if name.as_str() == "org.freedesktop.UDisks2.Error.AlreadyMounted" =>
			{
				Err(MountError::AlreadyMounted)
			}
			Err(error) => Err(MountError::Failed(error.to_string())),
		}
	}

	async fn unmount(&mut self, target: &Identity) -> Result<(), String> {
		let proxy = filesystem(&self.connection, &target.object).await?;

		proxy
			.call("Unmount", &(options(),))
			.await
			.map_err(|e| e.to_string())
	}

	async fn remove(&mut self, target: &Identity, action: Action) -> Result<(), String> {
		let proxy = zbus::Proxy::new(
			&self.connection,
			"org.freedesktop.UDisks2",
			target.drive.as_str(),
			"org.freedesktop.UDisks2.Drive",
		)
		.await
		.map_err(|e| e.to_string())?;

		proxy
			.call(
				if action == Action::Eject {
					"Eject"
				} else {
					"PowerOff"
				},
				&(options(),),
			)
			.await
			.map_err(|e| e.to_string())
	}
}

fn options() -> HashMap<&'static str, Value<'static>> {
	HashMap::new()
}

async fn filesystem<'a>(
	connection: &'a zbus::Connection, path: &'a zbus::zvariant::OwnedObjectPath,
) -> Result<zbus::Proxy<'a>, String> {
	zbus::Proxy::new(
		connection,
		"org.freedesktop.UDisks2",
		path.as_str(),
		"org.freedesktop.UDisks2.Filesystem",
	)
	.await
	.map_err(|e| e.to_string())
}
