//! File gestures retain their source snapshot while spring navigation changes views.
pub(crate) mod preview;
pub(crate) mod widget;

use crate::{
	FileManager, Message as AppMessage,
	browser::Message as BrowserMessage,
	context_menu::{Context, Target},
	filesystem::entry::DirEntry,
	transfers::{self, Operation},
};
use iced::{Point, Rectangle, Task, advanced::widget::Id, keyboard};
use std::{
	path::PathBuf,
	time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub(crate) enum Area {
	Entry { path: PathBuf, directory: bool },
	Background(PathBuf),
	Sidebar(PathBuf),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Hit {
	pub entry: Option<PathBuf>,
	pub target: Option<PathBuf>,
	pub navigation: bool,
	pub scroll: Option<Scroll>,
}

#[derive(Debug, Clone)]
pub(crate) struct Scroll {
	pub id: Id,
	pub bounds: Rectangle,
	pub offset: f32,
	pub maximum: f32,
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
	Pressed(PathBuf, Point),
	Motion(Point, Hit),
	Released(Point, Hit),
	Cancel,
	Tick(Instant),
	Layout(u64, Hit),
	Prepared(u64, Result<Target, String>),
}

impl From<Message> for AppMessage {
	fn from(message: Message) -> Self {
		Self::FileDrag(message)
	}
}

#[derive(Default)]
pub(crate) struct State {
	pub active: bool,
	pub external: bool,
	pub external_copy: bool,
	pub origin: Option<Origin>,
	pub target: Option<PathBuf>,
	pub valid: bool,
	pub sources: Vec<PathBuf>,
	pub generation: u64,
	guard: Option<Target>,
	pending: Option<(PathBuf, Operation)>,
	position: Point,
	preview_entry: Option<DirEntry>,
	hit: Hit,
	hover: Option<(PathBuf, Instant)>,
	last_click: Option<(PathBuf, Instant)>,
}

pub(crate) struct Origin {
	path: PathBuf,
	point: Point,
	modifiers: keyboard::Modifiers,
	applied: bool,
}

impl State {
	pub(crate) fn preview_entry(&self) -> Option<&DirEntry> {
		self.preview_entry.as_ref()
	}

	pub(crate) fn position(&self) -> Point {
		self.position
	}

	pub(crate) fn highlights(&self, path: &std::path::Path) -> bool {
		self.active && self.valid && self.target.as_deref() == Some(path)
	}

	fn accepts(&self, target: &std::path::Path) -> bool {
		if self.external {
			return self.external_copy;
		}
		!self.sources.is_empty()
			&& self.sources.iter().all(|source| {
				!target.starts_with(source)
					&& source
						.file_name()
						.is_some_and(|name| target.join(name) != *source)
			})
	}

	fn observe(&mut self, point: Point, hit: Hit) {
		self.position = point;
		self.target = hit.target.clone();
		self.valid = self
			.target
			.as_deref()
			.is_some_and(|path| self.accepts(path));
		if self.valid && hit.navigation {
			if let Some(target) = &self.target {
				if self.hover.as_ref().is_none_or(|(path, _)| path != target) {
					self.hover = Some((target.clone(), Instant::now()));
				}
			}
		} else {
			self.hover = None;
		}
		self.hit = hit;
	}

	pub(crate) fn outside(&self, size: iced::Size) -> bool {
		self.position.x < 0.0
			|| self.position.y < 0.0
			|| self.position.x >= size.width
			|| self.position.y >= size.height
	}

	pub(crate) fn near_edge(&self, size: iced::Size) -> bool {
		self.position.x < 64.0
			|| self.position.y < 64.0
			|| self.position.x >= size.width - 64.0
			|| self.position.y >= size.height - 64.0
	}

	pub(crate) fn source_guard(&self) -> Option<Target> {
		self.guard.clone()
	}

	pub(crate) fn begin_external(&mut self, copy: bool) {
		self.preview_entry = None;
		self.generation += 1;
		self.origin = None;
		self.guard = None;
		self.pending = None;
		self.sources.clear();
		self.external = true;
		self.external_copy = copy;
		self.active = true;
	}

	pub(crate) fn external_hit(&mut self, point: Point, hit: Hit) {
		self.observe(point, hit);
	}

	pub(crate) fn end_external(&mut self) {
		self.preview_entry = None;
		self.active = false;
		self.external = false;
		self.external_copy = false;
		self.origin = None;
		self.target = None;
		self.valid = false;
		self.hover = None;
		self.guard = None;
		self.pending = None;
	}
}

impl FileManager {
	pub(crate) fn update_file_drag(&mut self, message: Message) -> Task<AppMessage> {
		match message {
			Message::Pressed(path, point) => {
				if self.transfers.busy
					|| !self.transfers.loaded
					|| self.dialogs.blocked()
					|| self.transfers.blocked()
				{
					return Task::none();
				}
				self.typeahead.clear();
				let applied = !self.selection.paths.contains(&path)
					|| self.modifiers.control()
					|| self.modifiers.shift();
				if applied {
					self.apply_selection(path.clone(), self.modifiers, true);
				}
				self.drag.generation += 1;
				self.drag.origin = Some(Origin {
					path,
					point,
					modifiers: self.modifiers,
					applied,
				});
				self.drag.sources = self.selection.paths.iter().cloned().collect();
				self.drag.guard = None;
				self.drag.pending = None;
				self.drag.preview_entry = None;
			}
			Message::Motion(point, hit) => {
				let start = self
					.drag
					.origin
					.as_ref()
					.is_some_and(|origin| origin.point.distance(point) >= 6.0);
				self.drag.observe(point, hit);
				if start && !self.drag.active {
					self.drag.preview_entry = self.drag.origin.as_ref().and_then(|origin| {
						self.entries
							.iter()
							.find(|entry| entry.path == origin.path)
							.cloned()
					});
					self.drag.active = true;
					self.drag.last_click = None;
					let paths = self.drag.sources.clone();
					let directories: Vec<_> = self
						.entries
						.iter()
						.filter(|entry| entry.is_dir)
						.map(|entry| entry.path.clone())
						.collect();
					let generation = self.drag.generation;
					return Task::perform(
						async move {
							tokio::task::spawn_blocking(move || {
								let mut targets = paths
									.into_iter()
									.map(|path| {
										let context = if directories.contains(&path) {
											Context::Folder
										} else {
											Context::File
										};
										Target::capture(&path, context)
									})
									.collect::<Result<Vec<_>, _>>()?
									.into_iter();
								let mut target = targets
									.next()
									.ok_or_else(|| "No drag sources".to_string())?;
								target.others = targets.collect();
								Ok(target)
							})
							.await
							.map_err(|error| error.to_string())
							.and_then(|result| result)
						},
						move |result| Message::Prepared(generation, result).into(),
					);
				}
				if self.drag.active {
					return self.try_start_native_drag();
				}
			}
			Message::Released(point, hit) => {
				self.drag.preview_entry = None;
				self.drag.observe(point, hit);
				let origin = self.drag.origin.take();
				if self.drag.active {
					self.drag.active = false;
					self.drag.hover = None;
					if self.drag.valid {
						let operation = if self.modifiers.control() {
							Operation::Copy
						} else if self.modifiers.shift() {
							Operation::Move
						} else {
							Operation::Automatic
						};
						self.drag.pending = self.drag.target.clone().map(|path| (path, operation));
						let cleanup = self.abandon_native_preparation();
						return Task::batch([cleanup, self.finish_file_drop()]);
					}
					self.drag.guard = None;
					return self.abandon_native_preparation();
				} else if let Some(origin) = origin {
					if !origin.applied {
						self.apply_selection(origin.path.clone(), origin.modifiers, true);
					}
					let double = !origin.modifiers.control()
						&& !origin.modifiers.shift()
						&& self.drag.last_click.as_ref().is_some_and(|(path, time)| {
							*path == origin.path && time.elapsed() < Duration::from_millis(400)
						});
					self.drag.last_click = Some((origin.path.clone(), Instant::now()));
					if double {
						self.drag.last_click = None;
						return self.update_browser(BrowserMessage::OpenEntry(origin.path));
					}
					if let Some(entry) = self.entries.iter().find(|entry| entry.path == origin.path)
					{
						return iced::widget::operation::focus(entry.id.clone());
					}
				}
			}
			Message::Prepared(generation, result)
				if generation == self.drag.generation && !self.drag.external =>
			{
				match result {
					Ok(target) => {
						self.drag.guard = Some(target.clone());
						if self.drag.pending.is_some() {
							return self.finish_file_drop();
						}
						if self.drag.active {
							return self.prepare_native_drag(generation, target);
						}
					}
					Err(error) => {
						self.error = Some(error);
						return self.update_file_drag(Message::Cancel);
					}
				}
			}
			Message::Tick(now) if self.drag.active => {
				let mut tasks = Vec::new();
				if let Some((path, since)) = &self.drag.hover {
					if now.saturating_duration_since(*since) >= Duration::from_millis(650)
						&& *path != self.path
					{
						let path = path.clone();
						self.drag.hover = None;
						tasks.push(self.goto(path));
					}
				}
				if let Some(scroll) = &self.drag.hit.scroll {
					let delta = edge_scroll(self.drag.position.y, scroll.bounds);
					if delta != 0.0 {
						let operation = iced::advanced::widget::operation::scrollable::scroll_to(
							scroll.id.clone(),
							iced::advanced::widget::operation::scrollable::AbsoluteOffset {
								x: None,
								y: Some((scroll.offset + delta).clamp(0.0, scroll.maximum)),
							},
						);
						tasks.push(iced::advanced::widget::operate(operation));
					}
				}
				let generation = self.drag.generation;
				tasks.push(
					widget::hit_test(self.drag.position)
						.map(move |hit| Message::Layout(generation, hit).into()),
				);
				return Task::batch(tasks);
			}
			Message::Layout(generation, hit)
				if self.drag.active && generation == self.drag.generation =>
			{
				self.drag.observe(self.drag.position, hit)
			}
			Message::Cancel => {
				self.drag.preview_entry = None;
				self.drag.generation += 1;
				self.drag.active = false;
				self.drag.external = false;
				self.drag.origin = None;
				self.drag.guard = None;
				self.drag.pending = None;
				self.drag.target = None;
				self.drag.hover = None;
				self.drag.last_click = None;
				return self.abandon_native_preparation();
			}
			_ => {}
		}

		Task::none()
	}

	fn finish_file_drop(&mut self) -> Task<AppMessage> {
		let Some((destination, operation)) = self.drag.pending.clone() else {
			return Task::none();
		};
		let Some(guard) = self.drag.guard.take() else {
			return Task::none();
		};
		self.drag.pending = None;
		let request = transfers::Request {
			sources: guard.paths(),
			destination,
			operation,
			durability: self.transfers.policy,
			acknowledge_volatile: false,
		};
		self.update_transfers(transfers::Message::StartGuarded(request, guard))
	}
}

fn edge_scroll(y: f32, viewport: Rectangle) -> f32 {
	let edge = 28.0;
	if y < viewport.y + edge {
		-((viewport.y + edge - y) / edge).clamp(0.0, 1.0) * 24.0
	} else if y > viewport.y + viewport.height - edge {
		((y - viewport.y - viewport.height + edge) / edge).clamp(0.0, 1.0) * 24.0
	} else {
		0.0
	}
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod geometry_tests {
	use super::*;
	use crate::{Args, filesystem::entry::FileKind};

	#[test]
	fn preview_snapshots_pressed_entry_survives_hover_navigation_and_clears_on_end() {
		for end in 0..4 {
			// None of the returned tasks are polled: no filesystem or compositor.
			let (mut app, _) = FileManager::new(Args {
				path: "/source".into(),
			});
			let source = PathBuf::from("/source/file");
			let destination = PathBuf::from("/destination");
			app.transfers.loaded = true;
			app.entries = vec![DirEntry {
				name: "file".into(),
				path: source.clone(),
				size: 17,
				id: Id::unique(),
				kind: FileKind::Text,
				thumbnail: None,
				is_dir: false,
				is_hidden: false,
			}];

			let _ = app.update_file_drag(Message::Pressed(source.clone(), Point::ORIGIN));
			assert!(app.drag.preview_entry().is_none());
			let point = Point::new(6.0, 10.0);
			let _ = app.update_file_drag(Message::Motion(
				point,
				Hit {
					target: Some(destination.clone()),
					navigation: true,
					..Hit::default()
				},
			));
			assert_eq!(
				app.drag.preview_entry().map(|entry| &entry.path),
				Some(&source)
			);
			assert_eq!(app.drag.position(), point);

			let _ = app.update_file_drag(Message::Tick(Instant::now() + Duration::from_secs(1)));
			assert_eq!(app.path, destination);
			assert!(app.entries.is_empty());
			assert_eq!(
				app.drag.preview_entry().map(|entry| &entry.path),
				Some(&source)
			);
			assert_eq!(app.drag.preview_entry().map(|entry| entry.size), Some(17));

			match end {
				0 => {
					let _ = app.update_file_drag(Message::Cancel);
				}
				1 => {
					let _ = app.update_file_drag(Message::Released(Point::ORIGIN, Hit::default()));
				}
				2 => app.drag.end_external(),
				_ => app.drag.begin_external(true),
			}
			assert!(app.drag.preview_entry().is_none());
		}
	}

	#[test]
	fn targets_reject_self_descendants_and_noop_background_drops() {
		let state = State {
			sources: vec![PathBuf::from("/home/user/folder")],
			..State::default()
		};
		assert!(!state.accepts(std::path::Path::new("/home/user/folder")));
		assert!(!state.accepts(std::path::Path::new("/home/user/folder/child")));
		assert!(!state.accepts(std::path::Path::new("/home/user")));
		assert!(state.accepts(std::path::Path::new("/home/user/other")));
	}

	#[test]
	fn edge_scrolling_is_bounded_and_has_a_neutral_center() {
		let bounds = Rectangle {
			x: 0.0,
			y: 100.0,
			width: 300.0,
			height: 400.0,
		};
		assert_eq!(edge_scroll(300.0, bounds), 0.0);
		assert_eq!(edge_scroll(100.0, bounds), -24.0);
		assert_eq!(edge_scroll(500.0, bounds), 24.0);
	}
}
