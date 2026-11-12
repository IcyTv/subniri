use super::*;

pub(super) const DEFINITION: Definition = Definition {
	id: "advanced-copy",
	label: "Advanced Copy",
	group: "EDIT",
	aliases: "copy to destination durability",
	multiple: true,
	scope: Scope::Item,
	execute,
	check: None,
};

pub(super) const AUTO: Definition = Definition {
	id: "advanced-copy-auto",
	label: "Advanced Copy: Auto",
	..DEFINITION
};

pub(super) const DURABLE: Definition = Definition {
	id: "advanced-copy-durable",
	label: "Advanced Copy: Durable",
	..DEFINITION
};

pub(super) const BUFFERED: Definition = Definition {
	id: "advanced-copy-buffered",
	label: "Advanced Copy: Buffered",
	..DEFINITION
};

pub(super) fn choices() -> Vec<(Action, String)> {
	[
		(Command::AdvancedCopyAuto, "Auto"),
		(Command::AdvancedCopyDurable, "Durable"),
		(Command::AdvancedCopyBuffered, "Buffered"),
	]
	.into_iter()
	.map(|(command, label)| (Action::Command(command), label.into()))
	.collect()
}

fn execute(app: &mut FileManager, target: Target, action: Action) -> Task<Message> {
	use crate::transfers::{Durability, Operation};

	let durability = match action {
		Action::Command(Command::AdvancedCopyAuto) => Durability::Auto,
		Action::Command(Command::AdvancedCopyDurable) => Durability::Durable,
		Action::Command(Command::AdvancedCopyBuffered) => Durability::Buffered,
		_ => return Task::none(),
	};

	app.update_transfers(crate::transfers::Message::Prepare(
		target,
		Operation::Copy,
		Some(durability),
	))
}
