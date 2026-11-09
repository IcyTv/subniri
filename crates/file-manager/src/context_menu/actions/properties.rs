use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "properties",
	label: "Properties",
	group: "VIEW",
	aliases: "information details metadata",
	multiple: true,
	scope: Scope::Any,
	execute,
	check: None,
};
fn execute(app: &mut FileManager, target: Target, action: Action) -> Task<Message> {
	if target.context == Context::Background {
		return stub(app, target, action);
	}
	app.restore_menu_selection(&target);
	app.show_info = true;
	Task::none()
}
