use nix::unistd::AccessFlags;

use super::*;

pub(super) const DEFINITION: Definition = Definition {
	id: "delete",
	label: "Permanently Delete",
	group: "ORGANIZE",
	aliases: "delete remove erase",
	multiple: true,
	scope: Scope::Item,
	execute,
	check: Some(unavailable),
};

fn unavailable(_: &FileManager, target: &Target, _: Action) -> Option<&'static str> {
	unavailable_for_delete(target)
}

fn execute(app: &mut FileManager, target: Target, _: Action) -> Task<Message> {
	app.request_delete_confirmation(target)
}

impl FileManager {
	pub(crate) fn perform_confirmed_delete(
		&mut self, id: iced::window::Id, target: Target,
	) -> Task<Message> {
		Task::perform(
			async move {
				tokio::task::spawn_blocking(move || delete_snapshot(&target))
					.await
					.map_err(|e| e.to_string())
					.and_then(|r| r)
			},
			move |result| crate::dialogs::Message::Deleted(id, result).into(),
		)
	}
}

fn delete_snapshot(target: &Target) -> Result<(), String> {
	// Validate the complete captured batch before touching its first member.
	if !target.available() {
		return Err("Item no longer available".into());
	}

	if let Some(reason) = unavailable_for_delete(target) {
		return Err(reason.into());
	}

	for item in std::iter::once(target).chain(target.others.iter()) {
		if !item.available() {
			return Err(format!("Item no longer available: {}", item.path.display()));
		}

		let metadata = std::fs::symlink_metadata(&item.path)
			.map_err(|e| format!("Cannot inspect {}: {e}", item.path.display()))?;

		// Never follow a selected symlink or use trash as a fallback.
		if metadata.is_dir() {
			std::fs::remove_dir_all(&item.path)
		} else {
			std::fs::remove_file(&item.path)
		}
		.map_err(|e| format!("Cannot permanently delete {}: {e}", item.path.display()))?;

		log::info!(path=%item.path.display(), "Permanently deleted confirmed target");
	}

	Ok(())
}

fn unavailable_for_delete(target: &Target) -> Option<&'static str> {
	for path in target.paths() {
		if path.parent().is_none() {
			return Some("Cannot delete a filesystem root");
		}

		if nix::unistd::eaccess(&path, AccessFlags::W_OK).is_err() {
			return Some("Cannot delete a read-only item");
		}
	}

	None
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::{context_menu::Context, test_support::TestDir};

	#[test]
	fn confirmed_directory_deletion_does_not_follow_symlinks() {
		let dir = TestDir::new();
		let external = dir.path().join("keep");
		std::fs::write(&external, "preserve").unwrap();

		let folder = dir.path().join("selected");
		std::fs::create_dir(&folder).unwrap();
		std::fs::write(folder.join("nested"), "delete").unwrap();
		std::os::unix::fs::symlink(&external, folder.join("link")).unwrap();

		delete_snapshot(&Target::capture(&folder, Context::Folder).unwrap()).unwrap();

		assert!(!folder.exists());
		assert_eq!(std::fs::read_to_string(&external).unwrap(), "preserve");
	}

	#[test]
	fn confirmed_symlink_deletion_removes_only_the_link() {
		let dir = TestDir::new();
		let external = dir.path().join("keep");
		std::fs::write(&external, "preserve").unwrap();

		let link = dir.path().join("selected-link");
		std::os::unix::fs::symlink(&external, &link).unwrap();

		delete_snapshot(&Target::capture(&link, Context::File).unwrap()).unwrap();

		assert!(std::fs::symlink_metadata(&link).is_err());
		assert_eq!(std::fs::read_to_string(&external).unwrap(), "preserve");
	}

	#[test]
	fn stale_batch_member_prevents_any_deletion() {
		let dir = TestDir::new();
		let first = dir.path().join("first");
		let second = dir.path().join("second");

		std::fs::write(&first, "keep").unwrap();
		std::fs::write(&second, "old").unwrap();

		let mut target = Target::capture(&first, Context::File).unwrap();
		target
			.others
			.push(Target::capture(&second, Context::File).unwrap());

		std::fs::rename(&second, dir.path().join("old-second")).unwrap();
		std::fs::write(&second, "new").unwrap();

		assert!(delete_snapshot(&target).is_err());
		assert_eq!(std::fs::read_to_string(first).unwrap(), "keep");
		assert_eq!(std::fs::read_to_string(second).unwrap(), "new");
	}
}
