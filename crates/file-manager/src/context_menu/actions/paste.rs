use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "paste",
	label: "Paste",
	group: "EDIT",
	aliases: "",
	multiple: false,
	scope: Scope::Background,
	execute: stub,
	check: None,
};
