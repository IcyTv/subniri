use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "paste",
	label: "Paste",
	group: "EDIT",
	aliases: "",
	multiple: false,
	scope: Scope::Any,
	execute,
	check: Some(check),
};

fn check(app: &FileManager, target: &Target, _: Action) -> Option<&'static str> {
	if target.context == Context::File {
		return Some("Select a folder or right-click empty space");
	}
	if !app.transfers.can_paste() {
		return Some("Clipboard has no files to paste");
	}

	None
}

fn execute(app: &mut FileManager, target: Target, _: Action) -> Task<Message> {
	app.update_transfers(message(target))
}

fn message(target: Target) -> crate::transfers::Message {
	crate::transfers::Message::PasteTarget(target)
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::test_support::TestDir;

	#[test]
	fn paste_dispatches_the_clicked_folder_or_background_path() {
		let dir = TestDir::new();
		let folder = dir.path().join("folder");
		std::fs::create_dir(&folder).unwrap();

		for (path, context) in [
			(dir.path(), Context::Background),
			(folder.as_path(), Context::Folder),
		] {
			let target = Target::capture(path, context).unwrap();
			let crate::transfers::Message::PasteTarget(destination) = message(target) else {
				panic!("Paste must dispatch a transfer destination");
			};

			assert_eq!(destination.path, path);
			assert!(destination.available());
		}
	}
}
