//! Directory loading, navigation, and the displayed entry order.
use super::{Message as BrowserMessage, navigation::Visit, view};
use crate::{
	FileManager, Message,
	filesystem::{
		entry::{self, DirEntry},
		storage,
	},
};
use iced::Task;
use std::path::PathBuf;

impl FileManager {
	pub(crate) fn goto(&mut self, path: PathBuf) -> Task<Message> {
		self.typeahead.clear();
		let selected = self.selection_snapshot();
		self.history
			.go_to(path, selected)
			.map_or_else(Task::none, |visit| self.navigation_changed(visit))
	}
	pub(crate) fn navigation_changed(&mut self, visit: Visit) -> Task<Message> {
		let close = self.menu_lifecycle(neo_widgets::popup::Event::CloseAll);
		self.typeahead.clear();
		self.path = visit.path;
		self.pending_selection = visit.selected;
		self.selected = None;
		self.selection.clear();
		self.search.clear();
		self.folder_name = None;
		self.active_tag = None;
		self.entries.clear();
		self.storage = None;
		close.chain(self.load_directory())
	}
	pub(crate) fn selection_snapshot(&self) -> Option<PathBuf> {
		self.pending_selection
			.clone()
			.or_else(|| self.selected.clone())
	}
	pub(crate) fn reveal_selection(&self) -> Task<Message> {
		self.entries
			.iter()
			.find(|e| Some(&e.path) == self.selected.as_ref())
			.map_or_else(Task::none, |e| view::reveal_selection(e.id.clone()))
	}
	pub(crate) fn load_directory(&mut self) -> Task<Message> {
		self.request_id += 1;
		self.loading = true;
		self.error = None;
		let (id, path) = (self.request_id, self.path.clone());
		let storage_path = path.clone();
		let listing = Task::perform(
			async move {
				tokio::task::spawn_blocking(move || entry::read_directory(&path))
					.await
					.map_err(|e| e.to_string())
					.and_then(|r| r)
			},
			move |result| BrowserMessage::Loaded(id, result).into(),
		);
		let usage = Task::perform(
			async move {
				tokio::task::spawn_blocking(move || storage::Usage::read(&storage_path))
					.await
					.ok()
					.flatten()
			},
			move |usage| BrowserMessage::StorageLoaded(id, usage).into(),
		);
		Task::batch([listing, usage])
	}
	pub(crate) fn open_selected(&mut self) -> Task<Message> {
		self.typeahead.clear();
		if self.selection.paths.len() != 1 {
			return Task::none();
		}
		let Some(entry) = self
			.entries
			.iter()
			.find(|e| self.selection.paths.contains(&e.path))
		else {
			return Task::none();
		};
		let path = entry.path.clone();
		if entry.is_dir {
			self.goto(path)
		} else {
			Self::open_file(path)
		}
	}
	pub(crate) fn visible_entries(&self) -> Vec<&DirEntry> {
		let query = self.search.to_lowercase();
		let mut entries: Vec<_> = self
			.entries
			.iter()
			.filter(|entry| {
				entry.name.to_lowercase().contains(&query)
					&& self.active_tag.is_none_or(|tag| {
						self.tags
							.as_ref()
							.is_some_and(|s| s.data.matches(&entry.path, tag))
					})
			})
			.collect();
		if !self.ascending {
			// Reverse names within each group, never move folders below files.
			entries.sort_by(|a, b| {
				b.is_dir
					.cmp(&a.is_dir)
					.then_with(|| b.path.file_name().cmp(&a.path.file_name()))
			});
		}
		entries
	}
	pub(crate) fn typeahead_match(&self) -> Option<&DirEntry> {
		if !self.typeahead.is_active() {
			return None;
		}
		let prefix = self.typeahead.text().to_lowercase();
		self.visible_entries()
			.into_iter()
			.find(|e| e.name.to_lowercase().starts_with(&prefix))
	}
	pub(crate) fn reconcile_selection(&mut self) {
		let order = self
			.visible_entries()
			.iter()
			.map(|e| e.path.clone())
			.collect::<Vec<_>>();
		self.selection.retain(&order);
		if !order.iter().any(|p| Some(p) == self.selected.as_ref()) {
			self.selected = None;
		}
	}
}
