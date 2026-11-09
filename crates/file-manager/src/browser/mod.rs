//! Directory browsing, selection, navigation, and inline folder creation.
mod directory;
pub(crate) mod navigation;
pub(crate) mod selection;
pub(crate) mod typeahead;
mod update;
pub(crate) mod view;

use crate::{
	ViewMode,
	filesystem::{entry::DirEntry, storage},
};
use iced::keyboard;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Message {
	Clicked(PathBuf),
	SelectionMove(PathBuf, keyboard::Modifiers),
	SelectionKey(keyboard::Key, keyboard::Modifiers, Option<PathBuf>),
	Modifiers(keyboard::Modifiers),
	OpenEntry(PathBuf),
	OpenSelected,
	GoTo(PathBuf),
	GoUp,
	GoBack,
	GoForward,
	Refresh,
	Loaded(u64, Result<Vec<DirEntry>, String>),
	StorageLoaded(u64, Option<storage::Usage>),
	Search(String),
	View(ViewMode),
	Sort,
	NewFolder,
	CancelFolder,
	FolderName(String),
	CreateFolder,
	FolderCreated(PathBuf, Result<PathBuf, String>),
	OperationFinished(Result<(), String>),
	ClearSelection,
	DismissError,
	Keyboard(keyboard::Event),
	TypeAhead(typeahead::Edit),
	ToggleInfo,
}
impl From<Message> for crate::Message {
	fn from(message: Message) -> Self {
		Self::Browser(message)
	}
}
