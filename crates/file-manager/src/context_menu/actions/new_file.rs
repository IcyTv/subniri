use super::*;

pub(super) const DEFINITION: Definition = Definition {
	id: "new-file",
	label: "New File",
	group: "ORGANIZE",
	aliases: "touch create empty file",
	multiple: false,
	scope: Scope::Background,
	execute,
	check: None,
};

fn execute(app: &mut FileManager, _: Target, _: Action) -> Task<Message> {
	app.update(crate::browser::Message::NewFile)
}
