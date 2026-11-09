use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "duplicate",
	label: "Duplicate",
	group: "EDIT",
	aliases: "clone",
	multiple: true,
	scope: Scope::Item,
	execute: stub,
	check: None,
};
