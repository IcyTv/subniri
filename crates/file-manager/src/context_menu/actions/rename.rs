use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "rename",
	label: "Rename",
	group: "EDIT",
	aliases: "",
	multiple: false,
	scope: Scope::Item,
	execute,
	check: None,
};

fn execute(app: &mut FileManager, target: Target, _: Action) -> Task<Message> {
	app.start_name_edit(None, Some(target))
}
