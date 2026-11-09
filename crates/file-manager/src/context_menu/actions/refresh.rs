use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "refresh",
	label: "Refresh",
	group: "VIEW",
	aliases: "reload",
	multiple: false,
	scope: Scope::Background,
	execute,
	check: None,
};
fn execute(app: &mut FileManager, _: Target, _: Action) -> Task<Message> {
	app.update(crate::browser::Message::Refresh)
}
