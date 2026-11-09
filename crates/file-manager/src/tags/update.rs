use super::{Edit, Message, TagChoice, TagEditor, TagId, TagStore};
use crate::FileManager;
use iced::Task;

impl FileManager {
	pub(crate) fn update_tags(&mut self, message: Message) -> Task<crate::Message> {
		match message {
			Message::TagsLoaded(result) | Message::TagsSaved(result) => {
				self.tag_busy = false;
				match result {
					Ok(store) => {
						self.tags = Some(store);
						self.tag_problem = None;
						self.tag_editor = None;
						self.reconcile_selection();
						if self.closing.is_some()
							&& !self.creating_folder
							&& !self.favorites.running()
						{
							return self.finish_pending_close();
						}
					}
					Err(error) => {
						self.closing = None;
						self.tag_problem = Some(error);
					}
				}
			}
			Message::RetryTags => {
				if !self.tag_busy && self.tags.is_none() {
					self.tag_busy = true;
					return Self::load_tags();
				}
			}
			Message::TagFilter(tag) => {
				self.typeahead.clear();
				self.active_tag = tag;
				self.reconcile_selection();
				log::debug!(
					?tag,
					visible = self.visible_entries().len(),
					"Changed tag filter"
				);
			}
			Message::EditTag(id) => {
				self.typeahead.clear();
				self.folder_name = None;
				if self.tag_busy {
					return Task::none();
				}
				if let Some(store) = &self.tags {
					let tag = id.and_then(|id| store.data.tags.get(&id));
					self.tag_editor = Some(TagEditor {
						id,
						name: tag.map_or_else(String::new, |t| t.name.clone()),
						parent: self.tag_choice(tag.and_then(|t| t.parent)),
					});
					return iced::widget::operation::focus("tag-name");
				}
			}
			Message::TagName(name) => {
				if let Some(editor) = &mut self.tag_editor {
					editor.name = name;
				}
			}
			Message::TagParent(parent) => {
				if let Some(editor) = &mut self.tag_editor {
					editor.parent = parent;
				}
			}
			Message::CancelTag => self.tag_editor = None,
			Message::SaveTag => {
				if let Some(editor) = &self.tag_editor {
					let edit = if let Some(id) = editor.id {
						Edit::Update {
							id,
							name: editor.name.clone(),
							parent: editor.parent.id,
						}
					} else {
						Edit::Create {
							name: editor.name.clone(),
							parent: editor.parent.id,
						}
					};
					return self.save_tags(edit);
				}
			}
			Message::AssignTag(choice) => {
				if let (Some(path), Some(id)) = (
					self.selection
						.paths
						.iter()
						.next()
						.filter(|_| self.selection.paths.len() == 1),
					choice.id,
				) {
					return self.save_tags(Edit::Assign {
						path: path.clone(),
						id,
						assigned: true,
					});
				}
			}
			Message::RemoveTag(id) => {
				if let Some(path) = self
					.selection
					.paths
					.iter()
					.next()
					.filter(|_| self.selection.paths.len() == 1)
				{
					return self.save_tags(Edit::Assign {
						path: path.clone(),
						id,
						assigned: false,
					});
				}
			}
		}
		Task::none()
	}

	pub(crate) fn tag_choice(&self, id: Option<TagId>) -> TagChoice {
		TagChoice {
			id,
			label: id
				.and_then(|id| self.tags.as_ref().map(|s| s.data.label(id)))
				.unwrap_or_else(|| "No parent".into()),
		}
	}
	pub(crate) fn load_tags() -> Task<crate::Message> {
		Task::perform(
			async {
				tokio::task::spawn_blocking(TagStore::load_default)
					.await
					.map_err(|e| e.to_string())
					.and_then(|r| r)
			},
			|result| Message::TagsLoaded(result).into(),
		)
	}
	pub(crate) fn save_tags(&mut self, edit: Edit) -> Task<crate::Message> {
		if self.tag_busy {
			return Task::none();
		}
		let Some(store) = self.tags.clone() else {
			return Task::none();
		};
		self.tag_busy = true;
		Task::perform(
			async move {
				tokio::task::spawn_blocking(move || store.edited(edit))
					.await
					.map_err(|e| e.to_string())
					.and_then(|r| r)
			},
			|result| Message::TagsSaved(result).into(),
		)
	}
}
