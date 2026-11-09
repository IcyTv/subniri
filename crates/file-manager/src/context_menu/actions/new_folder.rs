use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "new-folder",
	label: "New Folder",
	group: "ORGANIZE",
	aliases: "mkdir create directory",
	multiple: false,
	scope: Scope::Background,
	execute,
	check: None,
};
fn execute(app: &mut FileManager, _: Target, _: Action) -> Task<Message> {
	app.update(crate::browser::Message::NewFolder)
}
