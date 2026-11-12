use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "cut",
	label: "Cut",
	group: "EDIT",
	aliases: "",
	multiple: true,
	scope: Scope::Item,
	execute,
	check: None,
};

fn execute(app: &mut FileManager, target: Target, _: Action) -> Task<Message> {
	app.update_transfers(crate::transfers::Message::Prepare(
		target,
		crate::transfers::Operation::Move,
		None,
	))
}
