use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "open-window",
	label: "Open in New Window",
	group: "OPEN",
	aliases: "",
	multiple: false,
	scope: Scope::Folder,
	execute: stub,
	check: None,
};
