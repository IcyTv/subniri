//! Selection-driven, read-only previews. Workers never mutate selected files.
mod convert;
mod load;
mod split;
#[cfg(test)]
mod tests;
mod view;

use std::{
	path::PathBuf,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
};

use iced::Task;

use crate::{FileManager, browser::Page};

pub(crate) use split::divider;

pub(crate) const MIN_WINDOW_WIDTH: f32 = 1000.0;
pub(crate) const MIN_PANE_WIDTH: f32 = 260.0;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
	path: PathBuf,
	// Refreshing the same path must invalidate its previous contents.
	revision: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
	Loaded(u64, Result<Box<load::Loaded>, String>),
	Reveal(u64),
	Dismiss,
	Page(u32),
	Resize(f32),
	IgnoreLink(String),
}

impl From<Message> for crate::Message {
	fn from(message: Message) -> Self {
		Self::Preview(message)
	}
}

pub(crate) struct State {
	target: Option<Target>,
	dismissed: Option<PathBuf>,
	generation: u64,
	cancel: Arc<AtomicBool>,
	pub(crate) width: f32,
	page: u32,
	pages: u32,
	content: Option<Box<load::Loaded>>,
	error: Option<String>,
	loading: bool,
	revealed: bool,
}

impl Default for State {
	fn default() -> Self {
		Self {
			target: None,
			dismissed: None,
			generation: 0,
			cancel: Arc::new(AtomicBool::new(false)),
			width: 360.0,
			page: 1,
			pages: 1,
			content: None,
			error: None,
			loading: false,
			revealed: false,
		}
	}
}

impl Drop for State {
	fn drop(&mut self) {
		self.cancel.store(true, Ordering::Relaxed);
	}
}

impl State {
	pub(crate) fn visible(&self, width: f32) -> bool {
		self.target.is_some()
			&& self.dismissed.is_none()
			&& self.revealed
			&& width >= MIN_WINDOW_WIDTH
	}

	pub(crate) fn pane_width(&self, width: f32) -> f32 {
		self.width
			.clamp(MIN_PANE_WIDTH, (width * 0.5).max(MIN_PANE_WIDTH))
	}

	fn invalidate(&mut self) {
		self.cancel.store(true, Ordering::Relaxed);
		self.cancel = Arc::new(AtomicBool::new(false));
		self.generation = self.generation.wrapping_add(1);
	}

	fn start(&mut self) -> Task<crate::Message> {
		let Some(target) = self.target.clone() else {
			return Task::none();
		};
		if self.dismissed.as_ref() == Some(&target.path) {
			return Task::none();
		}

		self.invalidate();
		self.loading = true;
		self.error = None;
		let cancel = self.cancel.clone();
		let generation = self.generation;
		let page = self.page;

		let load = Task::perform(
			async move {
				// Arrow-key browsing must not launch a converter for every intermediate file.
				tokio::time::sleep(std::time::Duration::from_millis(120)).await;
				static WORKERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
				let _permit = WORKERS.acquire().await.map_err(|e| e.to_string())?;
				if cancel.load(Ordering::Relaxed) {
					return Err("Preview cancelled".into());
				}

				tokio::task::spawn_blocking(move || load::read(&target.path, page, &cancel))
					.await
					.map_err(|e| format!("Preview worker failed: {e}"))?
					.map(Box::new)
			},
			move |result| Message::Loaded(generation, result).into(),
		);
		let reveal = if self.revealed {
			Task::none()
		} else {
			reveal_after(generation, std::time::Duration::from_millis(400))
		};

		Task::batch([load, reveal])
	}
}

fn reveal_after(generation: u64, delay: std::time::Duration) -> Task<crate::Message> {
	Task::perform(async move { tokio::time::sleep(delay).await }, move |_| {
		Message::Reveal(generation).into()
	})
}

impl FileManager {
	pub(crate) fn reconcile_preview(&mut self) -> Task<crate::Message> {
		if self.preview.dismissed.as_ref().is_some_and(|path| {
			self.page != Page::Files
				|| self.selection.paths.len() != 1
				|| !self.selection.paths.contains(path)
		}) {
			self.preview.dismissed = None;
		}

		let target = (self.page == Page::Files
			&& self.closing.is_none()
			&& self.selection.paths.len() == 1
			&& self.host_size.width >= MIN_WINDOW_WIDTH)
			.then(|| {
				self.entries
					.iter()
					.find(|entry| self.selection.paths.contains(&entry.path))
			})
			.flatten()
			.map(|entry| Target {
				path: entry.path.clone(),
				revision: self.request_id,
			});
		if target == self.preview.target {
			return Task::none();
		}

		self.preview.invalidate();
		self.preview.target = target;
		if self.preview.target.is_none() {
			self.preview.revealed = false;
		}

		self.preview.content = None;
		self.preview.error = None;
		self.preview.page = 1;
		self.preview.pages = 1;
		self.preview.loading = false;

		self.preview.start()
	}

	pub(crate) fn update_preview(&mut self, message: Message) -> Task<crate::Message> {
		match message {
			Message::Dismiss => {
				self.preview.dismissed = self
					.preview
					.target
					.as_ref()
					.map(|target| target.path.clone());
				self.preview.invalidate();
				self.preview.revealed = false;
				self.preview.loading = false;
				self.preview.content = None;
				self.preview.error = None;
			}
			Message::Reveal(generation)
				if generation == self.preview.generation
					&& self.preview.target.is_some()
					&& self.preview.dismissed.is_none() =>
			{
				if let Some(delay) = self.drag.preview_reflow_delay() {
					return reveal_after(generation, delay);
				}

				self.preview.revealed = true;
			}
			Message::Loaded(generation, result) if generation == self.preview.generation => {
				self.preview.loading = false;
				match result {
					Ok(content) => {
						self.preview.pages = content.pages;
						self.preview.content = Some(content);
					}
					Err(error) => self.preview.error = Some(error),
				}

				return iced::advanced::widget::operate(
					iced::advanced::widget::operation::scrollable::scroll_to(
						iced::widget::Id::from("file-preview"),
						iced::advanced::widget::operation::scrollable::AbsoluteOffset {
							x: Some(0.0),
							y: Some(0.0),
						},
					),
				);
			}
			Message::Page(page)
				if page >= 1 && page <= self.preview.pages && page != self.preview.page =>
			{
				self.preview.page = page;
				self.preview.content = None;

				return self.preview.start();
			}
			Message::Resize(width) if width.is_finite() => {
				self.preview.width = width.clamp(
					MIN_PANE_WIDTH,
					(self.host_size.width * 0.5).max(MIN_PANE_WIDTH),
				);
			}
			// Preview links are deliberately inert: no file launch or network access.
			Message::IgnoreLink(url) => {
				let _ = url;
			}
			_ => {}
		}

		Task::none()
	}
}
