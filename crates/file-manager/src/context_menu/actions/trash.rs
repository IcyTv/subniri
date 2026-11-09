//! Move to Trash. Path-specific availability rules belong here.
use super::*;

pub(super) const DEFINITION: Definition = Definition {
	id: "trash",
	label: "Move to Trash",
	group: "ORGANIZE",
	aliases: "delete remove recycle",
	multiple: true,
	scope: Scope::Item,
	execute,
	check: Some(unavailable),
};

fn unavailable(_: &FileManager, target: &Target, _: Action) -> Option<&'static str> {
	for path in target.paths() {
		if crate::trash::is_in_trash(path).unwrap_or(true) {
			return Some("Cannot re-trash a trashed item");
		}
	}

	None
}

fn execute(_: &mut FileManager, target: Target, _: Action) -> Task<Message> {
	for path in target.paths() {
		match crate::trash::trash(&path) {
			Ok(_) => log::info!("Trashed {}", path.display()),
			Err(e) => log::error!("Failed to trash {}: {}", path.display(), e),
		}
	}
	Task::done(crate::browser::Message::Refresh.into())
}
