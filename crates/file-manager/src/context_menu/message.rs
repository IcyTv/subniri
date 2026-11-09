use super::{Action, Command, Target};
use iced::{Point, Rectangle, advanced::widget::Id, keyboard, mouse, window};
use std::path::PathBuf;
#[derive(Debug, Clone)]
pub enum Message {
	FavoritesLoaded(u64, Result<Vec<String>, String>),
	FavoritesSaved(Vec<Command>, Result<(), String>),
	FavoritesReload,
	MenuPin(window::Id, Command),
	MenuFavoritePress(window::Id, Command, Point),
	MenuDragOver(window::Id, Command, bool),
	MenuPointer(window::Id, mouse::Event),
	MenuOpen(Option<PathBuf>, Point),
	MenuOpenScrolled(PathBuf, Point),
	MenuInput(window::Id, String),
	MenuText(window::Id, String),
	MenuKey(window::Id, keyboard::Event, Option<Id>),
	MenuActivate(window::Id, Action, Option<Rectangle>),
	MenuDispatch(Target, Action),
	MenuHover(window::Id, Action, bool),
	MenuLifecycle(neo_widgets::popup::Event<()>),
	MenuPoll(window::Id),
	MenuStatus(window::Id, bool),
}
impl From<Message> for crate::Message {
	fn from(message: Message) -> Self {
		Self::ContextMenu(message)
	}
}
