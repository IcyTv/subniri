use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "cut",
	label: "Cut",
	group: "EDIT",
	aliases: "",
	multiple: true,
	scope: Scope::Item,
	execute: stub,
	check: None,
};
