use super::*;
use crate::tags;
pub(super) const DEFINITION: Definition = Definition {
	id: "tags",
	label: "Tags…",
	group: "ORGANIZE",
	aliases: "",
	multiple: false,
	scope: Scope::Item,
	execute,
	check: Some(unavailable),
};
fn unavailable(app: &FileManager, _: &Target, action: Action) -> Option<&'static str> {
	if app.tags.is_none() {
		Some("Tags are not available")
	} else if app.tag_busy {
		Some("Saving tags…")
	} else if matches!(action,Action::Tag(id) if app.tags.as_ref().is_none_or(|s| !s.data.tags.contains_key(&id)))
	{
		Some("Tag no longer available")
	} else {
		None
	}
}
pub(super) fn choices(app: &FileManager) -> Vec<(Action, String)> {
	app.tags
		.as_ref()
		.map(|s| {
			s.data
				.tags
				.keys()
				.map(|id| (Action::Tag(*id), s.data.label(*id)))
				.collect()
		})
		.unwrap_or_default()
}
pub(super) fn checked(app: &FileManager, target: &Target, action: Action) -> bool {
	matches!(action, Action::Tag(id) if app.tags.as_ref().is_some_and(|s| s.data.assigned(&target.path).contains(&id)))
}
fn execute(app: &mut FileManager, target: Target, action: Action) -> Task<Message> {
	if let Action::Tag(id) = action {
		let assigned = app
			.tags
			.as_ref()
			.is_some_and(|s| s.data.assigned(&target.path).contains(&id));
		app.save_tags(tags::Edit::Assign {
			path: target.path,
			id,
			assigned: !assigned,
		})
	} else {
		stub(app, target, action)
	}
}
