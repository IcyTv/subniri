use std::{
	cmp::Ordering,
	path::{Path, PathBuf},
};

use clap::Parser;
use iced::{
	Alignment, Element, Length, Subscription, Task, Theme, application, font, theme,
	widget::{Grid, column, container, grid, row, stack, svg, text},
};
use ignore::WalkBuilder;
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{neo_button, neo_card},
};
use undo::Record;
use xdg_mime::SharedMimeInfo;

use crate::entry::DirEntry;

mod disks;
mod entry;

#[derive(Debug, Clone, Parser)]
struct Args {
	#[clap(default_value_os_t = default_path(), value_parser = clap::value_parser!(PathBuf))]
	path: PathBuf,
}

fn default_path() -> PathBuf {
	dirs::home_dir()
		.or_else(|| std::env::current_dir().ok())
		.unwrap_or_else(|| PathBuf::from("/"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	log::init!("file_manager", "fileberg")?;

	let args = Args::parse();

	let app = application(
		move || FileManager::new(args.clone()),
		FileManager::update,
		FileManager::view,
	)
	.style(FileManager::style)
	.subscription(FileManager::subscription);

	Ok(app.run()?)
}

#[derive(Debug, Clone)]
enum Message {
	Clicked(DirEntry),
	GoTo(String),
	GoUp,
	GoBack,
	GoForward,
	ReloadMimes,
}

struct FileManager {
	path: PathBuf,
	history: Record<Navigate>,
	entries: Vec<DirEntry>,
	mimes: SharedMimeInfo,
}

impl FileManager {
	fn new(args: Args) -> Self {
		log::info!("Starting file manager with path: {}", args.path.display());

		let mut out = Self {
			path: args.path,
			history: Record::new(),
			entries: Vec::new(),
			mimes: SharedMimeInfo::new(),
		};
		out.update_dir_entries();
		out
	}

	fn subscription(&self) -> Subscription<Message> {
		let mime_refresh = iced::time::every(iced::time::minutes(10)).map(|_| Message::ReloadMimes);

		Subscription::batch([mime_refresh])
	}

	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Clicked(entry) => {
				if entry.is_dir {
					Task::done(Message::GoTo(entry.name))
				} else {
					iced::widget::operation::focus(entry.id)
				}
			}
			Message::GoTo(name) => {
				self.goto(self.path.join(name));
				Task::none()
			}
			Message::GoUp => {
				if let Some(parent) = self.path.parent() {
					self.goto(parent.to_path_buf());
				}
				Task::none()
			}
			Message::GoBack => {
				self.undo();
				Task::none()
			}
			Message::GoForward => {
				self.redo();
				Task::none()
			}
			Message::ReloadMimes => {
				self.mimes.reload();
				Task::none()
			}
		}
	}

	fn goto(&mut self, path: PathBuf) {
		if path.is_dir() {
			let from = self.path.clone();
			self.history
				.edit(&mut self.path, Navigate { from, to: path });
			self.update_dir_entries();
		}
	}

	fn undo(&mut self) {
		if self.history.can_undo() {
			log::debug!("Undoing navigation, current path: {}", self.path.display());
			self.history.undo(&mut self.path);
			log::trace!("New path after undo: {}", self.path.display());
			self.update_dir_entries();
		}
	}

	fn redo(&mut self) {
		if self.history.can_redo() {
			self.history.redo(&mut self.path);
			self.update_dir_entries();
		}
	}

	fn view(&self) -> Element<'_, Message> {
		let grid = self.file_grid_view();
		let top_bar = self.top_bar_view();

		container(column![top_bar, grid].spacing(16))
			.padding(18)
			.into()
	}

	fn sidebar_view(&self) -> Element<'_, Message> {
		neo_card("").into()
	}

	fn top_bar_view(&self) -> Element<'_, Message> {
		stack![
			row![
				neo_button(svg(phosphor_icon!("arrow-left")).width(24))
					.width(Length::Shrink)
					.on_press(Message::GoBack)
					.enabled(self.history.can_undo()),
				neo_button(svg(phosphor_icon!("arrow-right")).width(24))
					.width(Length::Shrink)
					.on_press(Message::GoForward)
					.enabled(self.history.can_redo()),
				neo_button(svg(phosphor_icon!("arrow-up")).width(24))
					.width(Length::Shrink)
					.on_press(Message::GoUp),
			],
			container(neo_button(
				row![
					svg(phosphor_icon!("folder")).width(32),
					text(format!("{}", self.path.display()))
						.weight(font::Weight::Bold)
						.size(18),
				]
				.align_y(Alignment::Center)
				.spacing(10)
				.width(Length::Fill)
			))
			.center_x(Length::Fill)
			.width(Length::Fill)
		]
		.width(Length::Fill)
		.into()
	}

	fn file_grid_view(&self) -> Grid<'_, Message> {
		let mut grid = grid![].fluid(128).spacing(8);
		for ent in &self.entries {
			grid = grid.push(self.file_entry_view(ent));
		}
		grid
	}

	fn file_entry_view<'a>(&'a self, entry: &'a DirEntry) -> Element<'a, Message> {
		neo_button(
			column![
				svg(entry.icon.clone()).width(64),
				text(&entry.name)
					.weight(font::Weight::Bold)
					.size(14)
					.wrapping(text::Wrapping::Glyph),
			]
			.align_x(Alignment::Center),
		)
		.id(entry.id.clone())
		.radius(8.0)
		.on_press(Message::Clicked(entry.clone()))
		.width(Length::Fill)
		.into()
	}

	fn style(&self, _theme: &Theme) -> theme::Style {
		theme::Style {
			background_color: COLORS.decorative.pink90,
			text_color: COLORS.black,
		}
	}

	fn update_dir_entries(&mut self) {
		self.entries.clear();

		let walk = WalkBuilder::new(&self.path)
			.hidden(false)
			.git_ignore(false)
			.git_exclude(false)
			.git_global(false)
			.max_depth(Some(1))
			.skip_stdout(true)
			.sort_by_file_path(sort_by_file_path)
			.build();

		for result in walk {
			if let Ok(entry) = result {
				if entry.path() == self.path {
					continue;
				}

				self.entries.push(DirEntry::from_entry(&self.mimes, entry));
			}
		}
	}
}

fn sort_by_file_path(a: &Path, b: &Path) -> Ordering {
	// 1. Sort folders before files
	if a.is_dir() && !b.is_dir() {
		return Ordering::Less;
	} else if !a.is_dir() && b.is_dir() {
		return Ordering::Greater;
	}

	// 2. Then sort alphabetically
	a.file_name()
		.and_then(|a_name| b.file_name().map(|b_name| a_name.cmp(b_name)))
		.unwrap_or(Ordering::Equal)
}

#[derive(Clone, Debug)]
pub struct Navigate {
	from: PathBuf,
	to: PathBuf,
}

impl undo::Edit for Navigate {
	type Target = PathBuf;
	type Output = ();

	fn edit(&mut self, current_path: &mut PathBuf) {
		*current_path = self.to.clone();
	}

	fn undo(&mut self, current_path: &mut PathBuf) {
		*current_path = self.from.clone();
	}

	fn redo(&mut self, current_path: &mut PathBuf) {
		*current_path = self.to.clone();
	}
}
