use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "rename",
	label: "Rename",
	group: "EDIT",
	aliases: "",
	multiple: true,
	scope: Scope::Item,
	execute: stub,
	check: None,
};
