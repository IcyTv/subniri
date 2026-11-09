use super::*;
pub(super) const DEFINITION: Definition = Definition {
	id: "open",
	label: "Open",
	group: "OPEN",
	aliases: "",
	multiple: false,
	scope: Scope::Item,
	execute,
	check: None,
};
fn execute(app: &mut FileManager, target: Target, _: Action) -> Task<Message> {
	app.restore_menu_selection(&target);
	if target.context == Context::Folder {
		app.goto(target.path)
	} else {
		FileManager::open_file(target.path)
	}
}

impl FileManager {
	pub(crate) fn open_file(path: std::path::PathBuf) -> Task<Message> {
		Task::perform(
			async move {
				let status = tokio::process::Command::new("xdg-open")
					.arg(&path)
					.status()
					.await
					.map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
				if status.success() {
					Ok(())
				} else {
					Err(format!("Could not open {} ({status})", path.display()))
				}
			},
			|result| crate::browser::Message::OperationFinished(result).into(),
		)
	}
}
