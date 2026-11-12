//! One inline name editor, shared by rename and exclusive file/folder creation.
use super::Message;
use crate::{
	FileManager,
	context_menu::{Context, Target},
	filesystem::entry::{self, DirEntry, FileKind},
};
use iced::{
	Task,
	widget::{Id, operation},
};
use std::{
	path::PathBuf,
	time::{Duration, Instant},
};

pub(crate) const INPUT_ID: &str = "entry-name";

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(crate) struct State {
	pub editor: Option<Editor>,
	pub busy: bool,
	pub pending: Option<Box<crate::Message>>,
	pub toast: Option<Toast>,
}

pub(crate) struct Editor {
	pub entry: DirEntry,
	pub name: String,
	pub position: usize,
	pub error: Option<String>,
	pub operation: Edit,
}

#[derive(Clone)]
pub(crate) enum Edit {
	Create { parent: Target, folder: bool },
	Rename(Target),
}

impl Edit {
	pub fn run(&self, name: &str) -> Result<PathBuf, String> {
		match self {
			Self::Create { parent, folder } => {
				if !parent.available() {
					return Err("The destination folder changed or disappeared.".into());
				}

				if *folder {
					entry::create_folder(&parent.path, name)
				} else {
					entry::create_file(&parent.path, name)
				}
			}
			Self::Rename(target) => entry::rename(target, name),
		}
	}
}

pub(crate) struct Toast {
	pub path: PathBuf,
	pub text: String,
	pub remaining: Duration,
	pub last_tick: Instant,
	pub hovered: bool,
}

pub(crate) fn selection_end(name: &str, folder: bool) -> usize {
	if folder || name.starts_with('.') {
		return name.len();
	}

	name.rfind('.').filter(|i| *i > 0).unwrap_or(name.len())
}

impl FileManager {
	pub(crate) fn start_name_edit(
		&mut self, folder: Option<bool>, target: Option<Target>,
	) -> Task<crate::Message> {
		if self.loading
			|| self.page != super::Page::Files
			|| self.editing.busy
			|| self.editing.editor.is_some()
			|| self.trash_mutation_blocked()
		{
			return Task::none();
		}

		let result = (|| {
			if let Some(folder) = folder {
				let parent = Target::capture(&self.path, Context::Background)?;
				let base = if folder { "New Folder" } else { "New File" };
				let mut name = base.to_owned();
				let mut suffix = 2;
				while self
					.entries
					.iter()
					.any(|e| e.path.file_name() == Some(std::ffi::OsStr::new(&name)))
				{
					name = format!("{base} ({suffix})");
					suffix += 1;
				}

				let entry = DirEntry {
					path: self.path.join(&name),
					name: name.clone(),
					size: 0,
					id: Id::unique(),
					kind: FileKind::Document,
					thumbnail: None,
					is_dir: folder,
					is_hidden: false,
					is_symlink: false,
					unresolved_link: false,
					folder_identity: None,
				};

				let position = self
					.visible_entries()
					.iter()
					.filter(|e| compare(e, &entry, self.ascending).is_lt())
					.count();
				Ok(Editor {
					entry,
					name,
					position,
					error: None,
					operation: Edit::Create { parent, folder },
				})
			} else {
				let target = if let Some(target) = target {
					target
				} else {
					if self.selection.paths.len() != 1 {
						return Err("Select exactly one item to rename.".into());
					}

					let entry = self
						.entries
						.iter()
						.find(|e| self.selection.paths.contains(&e.path))
						.ok_or("Select an item to rename.")?;

					Target::capture(
						&entry.path,
						if entry.is_dir {
							Context::Folder
						} else {
							Context::File
						},
					)?
				};
				if !target.available() {
					return Err("The item changed or disappeared.".into());
				}

				let entry = self
					.entries
					.iter()
					.find(|e| e.path == target.path)
					.ok_or("Item is not in this folder.")?
					.clone();
				if entry.path.file_name().and_then(|n| n.to_str()).is_none() {
					return Err(
						"This filename is not valid UTF-8 and cannot be edited as text.".into(),
					);
				}

				let position = self
					.visible_entries()
					.iter()
					.position(|e| e.path == entry.path)
					.unwrap_or(0);
				Ok(Editor {
					name: entry.name.clone(),
					entry,
					position,
					error: None,
					operation: Edit::Rename(target),
				})
			}
		})();

		let editor = match result {
			Ok(editor) => editor,
			Err(error) => {
				self.error = Some(error);
				return Task::none();
			}
		};
		let end = if matches!(editor.operation, Edit::Create { .. }) {
			editor.name.len()
		} else {
			selection_end(&editor.name, editor.entry.is_dir)
		};

		self.typeahead.clear();
		self.tag_editor = None;
		self.select_only(editor.entry.path.clone());
		self.editing.editor = Some(editor);

		self.reveal_selection()
			.chain(operation::focus(INPUT_ID))
			.chain(operation::select_range(
				INPUT_ID,
				iced::advanced::text::Position { line: 0, index: 0 },
				iced::advanced::text::Position {
					line: 0,
					index: end,
				},
			))
	}

	pub(crate) fn save_name_edit(&mut self) -> Task<crate::Message> {
		if self.editing.busy {
			return Task::none();
		}
		let Some(editor) = &mut self.editing.editor else {
			return Task::none();
		};
		if let Err(error) = entry::validate_name(&editor.name) {
			editor.error = Some(error);
			self.editing.pending = None;
			return operation::focus(INPUT_ID);
		}

		self.editing.busy = true;
		let (edit, name) = (editor.operation.clone(), editor.name.clone());

		Task::perform(
			async move {
				tokio::task::spawn_blocking(move || edit.run(&name))
					.await
					.map_err(|e| e.to_string())
					.and_then(|r| r)
			},
			|result| Message::EditFinished(result).into(),
		)
	}

	/// Defer the user's action, not a pre-mutated history or destination snapshot.
	pub(crate) fn finish_edit_before(&mut self, message: crate::Message) -> Task<crate::Message> {
		self.editing.pending = Some(Box::new(message));
		self.save_name_edit()
	}

	pub(crate) fn name_edit_finished(
		&mut self, result: Result<PathBuf, String>,
	) -> Task<crate::Message> {
		if !self.editing.busy {
			return Task::none();
		}
		self.editing.busy = false;

		match result {
			Err(error) => {
				self.editing.pending = None;
				self.closing = None;
				if let Some(editor) = &mut self.editing.editor {
					editor.error = Some(error);
				}

				operation::focus(INPUT_ID)
			}
			Ok(path) => {
				let editor = self.editing.editor.take();
				if self.closing.is_some() {
					self.editing.pending = None;
					return self.finish_pending_close();
				}

				self.notify_hidden_edit(&path);
				if let Some(editor) = editor {
					if let Some(message) = self.editing.pending.as_deref_mut() {
						if let crate::Message::Browser(
							Message::Clicked(p) | Message::OpenEntry(p) | Message::GoTo(p),
						) = message && *p == editor.entry.path
						{
							*p = path.clone();
						}
					}
				}

				self.pending_selection = Some(path);
				self.load_directory()
			}
		}
	}

	fn blocking_filters(&self, path: &std::path::Path) -> (bool, bool) {
		let name = path.file_name().unwrap_or_default().to_string_lossy();

		(
			!name.to_lowercase().contains(&self.search.to_lowercase()),
			self.active_tag.is_some_and(|tag| {
				!self
					.tags
					.as_ref()
					.is_some_and(|s| s.data.matches(path, tag))
			}),
		)
	}

	fn notify_hidden_edit(&mut self, path: &std::path::Path) {
		let (search, tag) = self.blocking_filters(path);
		if !search && !tag {
			return;
		}

		let mut filters = Vec::new();
		if search {
			filters.push(format!("search ‘{}’", self.search));
		}
		if tag {
			filters.push(
				self.active_tag
					.and_then(|id| self.tags.as_ref()?.data.tags.get(&id))
					.map_or_else(|| "tag filter".into(), |tag| format!("tag ‘{}’", tag.name)),
			);
		}

		self.editing.toast = Some(Toast {
			path: path.into(),
			text: format!(
				"{} hidden because {} active",
				path.file_name().unwrap_or_default().to_string_lossy(),
				filters.join(" and ")
			),
			remaining: Duration::from_secs(8),
			last_tick: Instant::now(),
			hovered: false,
		});
	}

	pub(crate) fn show_edited_item(&mut self) -> Task<crate::Message> {
		let Some(toast) = self.editing.toast.take() else {
			return Task::none();
		};

		let (search, tag) = self.blocking_filters(&toast.path);
		if search {
			self.search.clear();
		}
		if tag {
			self.active_tag = None;
		}

		self.select_only(toast.path);
		self.reveal_selection()
	}
}

pub(crate) fn compare(a: &DirEntry, b: &DirEntry, ascending: bool) -> std::cmp::Ordering {
	b.is_dir.cmp(&a.is_dir).then_with(|| {
		if ascending {
			a.path.file_name().cmp(&b.path.file_name())
		} else {
			b.path.file_name().cmp(&a.path.file_name())
		}
	})
}
