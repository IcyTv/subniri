use super::*;
use crate::ViewMode;
pub(super) const DEFINITION: Definition = Definition {
	id: "view",
	label: "View…",
	group: "VIEW",
	aliases: "",
	multiple: false,
	scope: Scope::Background,
	execute,
	check: None,
};
pub(super) fn choices() -> Vec<(Action, String)> {
	vec![
		(Action::ViewGrid, "Grid".into()),
		(Action::ViewList, "List".into()),
	]
}
pub(super) fn checked(app: &FileManager, action: Action) -> bool {
	match action {
		Action::ViewGrid => app.view_mode == ViewMode::Grid,
		Action::ViewList => app.view_mode == ViewMode::List,
		_ => false,
	}
}
fn execute(app: &mut FileManager, target: Target, action: Action) -> Task<Message> {
	match action {
		Action::ViewGrid => app.update(crate::browser::Message::View(ViewMode::Grid)),
		Action::ViewList => app.update(crate::browser::Message::View(ViewMode::List)),
		_ => stub(app, target, action),
	}
}
