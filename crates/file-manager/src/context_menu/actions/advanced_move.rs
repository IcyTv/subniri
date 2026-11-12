use super::*;

pub(super) const DEFINITION: Definition = Definition {
	id: "advanced-move",
	label: "Advanced Move",
	group: "EDIT",
	aliases: "move to destination durability",
	multiple: true,
	scope: Scope::Item,
	execute,
	check: None,
};

pub(super) const AUTO: Definition = Definition {
	id: "advanced-move-auto",
	label: "Advanced Move: Auto",
	..DEFINITION
};

pub(super) const DURABLE: Definition = Definition {
	id: "advanced-move-durable",
	label: "Advanced Move: Durable",
	..DEFINITION
};

pub(super) const BUFFERED: Definition = Definition {
	id: "advanced-move-buffered",
	label: "Advanced Move: Buffered",
	..DEFINITION
};

pub(super) fn choices() -> Vec<(Action, String)> {
	[
		(Command::AdvancedMoveAuto, "Auto"),
		(Command::AdvancedMoveDurable, "Durable"),
		(Command::AdvancedMoveBuffered, "Buffered"),
	]
	.into_iter()
	.map(|(command, label)| (Action::Command(command), label.into()))
	.collect()
}

fn execute(app: &mut FileManager, target: Target, action: Action) -> Task<Message> {
	use crate::transfers::{Durability, Operation};

	let durability = match action {
		Action::Command(Command::AdvancedMoveAuto) => Durability::Auto,
		Action::Command(Command::AdvancedMoveDurable) => Durability::Durable,
		Action::Command(Command::AdvancedMoveBuffered) => Durability::Buffered,
		_ => return Task::none(),
	};

	app.update_transfers(crate::transfers::Message::Prepare(
		target,
		Operation::Move,
		Some(durability),
	))
}
