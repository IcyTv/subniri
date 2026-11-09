use super::{Action, Message, favorites};
use crate::{FileManager, browser::view};
use iced::Task;

impl FileManager {
	pub(crate) fn update_context_menu(&mut self, message: Message) -> Task<crate::Message> {
		match message {
			Message::FavoritesLoaded(revision, result) => {
				if revision == self.favorites.revision {
					self.favorites.observe(result);
					log::debug!(order=?self.favorites.order,"Context favorites loaded");
				}
			}
			Message::FavoritesReload => {
				if !self.favorites.running() {
					return favorites::load(self.favorites.revision);
				}
				self.favorites.reload_pending = true;
			}
			Message::FavoritesSaved(saved, result) => {
				log::debug!(order=?saved,success=result.is_ok(),"Context favorites save completed");
				let failed = result.is_err();
				let mut next = self.favorites.complete(saved, result);
				if !self.favorites.running() && self.favorites.reload_pending {
					self.favorites.reload_pending = false;
					next = next.chain(favorites::load(self.favorites.revision));
				}
				if failed {
					self.closing = None;
					self.error = self.favorites.error.clone();
				}
				if self.closing.is_some()
					&& !self.tag_busy
					&& !self.creating_folder
					&& !self.favorites.running()
				{
					return next.chain(self.finish_pending_close());
				}
				return next;
			}
			Message::MenuPin(id, command) => return self.menu_pin(id, command),
			Message::MenuFavoritePress(id, command, position) => {
				self.menu_favorite_press(id, command, position)
			}
			Message::MenuDragOver(id, command, after) => {
				if let Some(m) = &mut self.menu {
					if let Some(page) = m.page_mut(id) {
						page.hovered = Some(Action::Command(command));
					}
					if let Some(drag) = &mut m.drag
						&& drag.page == id
					{
						drag.over = Some((command, after));
					}
				}
			}
			Message::MenuPointer(id, event) => return self.menu_pointer(id, event),
			Message::MenuOpen(path, position) => return self.open_menu(path, position),
			Message::MenuOpenScrolled(path, position) => {
				return view::window_point(position)
					.map(move |position| Message::MenuOpen(Some(path.clone()), position).into());
			}
			Message::MenuLifecycle(event) => return self.menu_lifecycle(event),
			Message::MenuInput(id, query) => return self.menu_input(id, query),
			Message::MenuText(id, text) => {
				if let Some(page) = self.menu.as_ref().and_then(|m| m.page(id)) {
					return self.menu_input(id, format!("{}{text}", page.query));
				}
			}
			Message::MenuKey(id, event, focused) => return self.menu_key(id, event, focused),
			Message::MenuActivate(id, action, bounds) => {
				return self.menu_activate(id, action, bounds);
			}
			Message::MenuDispatch(target, action) => return self.menu_dispatch(target, action),
			Message::MenuHover(id, action, entered) => {
				if let Some(p) = self.menu.as_mut().and_then(|m| m.page_mut(id)) {
					if entered {
						p.hovered = Some(action);
					} else if p.hovered == Some(action) {
						p.hovered = None;
					}
				}
			}
			Message::MenuPoll(id) => {
				if let Some(target) = self
					.menu
					.as_ref()
					.filter(|m| m.pages.first().is_some_and(|p| p.id == id))
					.map(|m| m.target.clone())
				{
					return Task::perform(
						async move {
							tokio::task::spawn_blocking(move || target.available())
								.await
								.unwrap_or(false)
						},
						move |available| Message::MenuStatus(id, available).into(),
					);
				}
			}
			Message::MenuStatus(id, available) => {
				if let Some(m) = self
					.menu
					.as_mut()
					.filter(|m| m.pages.first().is_some_and(|p| p.id == id))
				{
					if m.available && !available {
						log::debug!(?id, "Context target no longer available");
					}
					m.available &= available;
				}
				self.reconcile_menu_highlight();
			}
		}
		Task::none()
	}
}
