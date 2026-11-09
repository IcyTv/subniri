use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "open-with",
	label: "Open With…",
	group: "OPEN",
	aliases: "application choose",
	multiple: false,
	scope: Scope::Item,
	execute: stub,
	check: None,
};
pub(super) fn choices() -> Vec<(Action, String)> {
	vec![(Action::ChooseApplication, "Choose Application…".into())]
}
