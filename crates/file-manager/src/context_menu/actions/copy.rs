use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "copy",
	label: "Copy",
	group: "EDIT",
	aliases: "",
	multiple: true,
	scope: Scope::Item,
	execute: stub,
	check: None,
};
