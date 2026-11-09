use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "sort",
	label: "Sort By…",
	group: "VIEW",
	aliases: "",
	multiple: false,
	scope: Scope::Background,
	execute,
	check: None,
};
pub(super) fn choices() -> Vec<(Action, String)> {
	vec![
		(Action::SortAscending, "Name Ascending".into()),
		(Action::SortDescending, "Name Descending".into()),
	]
}
pub(super) fn checked(app: &FileManager, action: Action) -> bool {
	match action {
		Action::SortAscending => app.ascending,
		Action::SortDescending => !app.ascending,
		_ => false,
	}
}
fn execute(app: &mut FileManager, target: Target, action: Action) -> Task<Message> {
	match action {
		Action::SortAscending => app.ascending = true,
		Action::SortDescending => app.ascending = false,
		_ => return stub(app, target, action),
	}
	Task::none()
}
