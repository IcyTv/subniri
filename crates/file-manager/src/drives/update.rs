use super::{Menu, Message};
use crate::{
	FileManager,
	filesystem::volumes::{self, Operation},
};
use iced::{
	Task,
	keyboard::{Key, key::Named},
	window,
};
use iced_exwlshell::{actions::IcedNewPopupSettings, reexport::PixelSize};
use neo_widgets::popup::Event;

impl FileManager {
	pub(crate) fn update_drives(&mut self, message: Message) -> Task<crate::Message> {
		match message {
			Message::NoAction => {}
			Message::Refresh => {
				if self.drives.refreshing.is_some()
					|| self.drives.busy.is_some()
					|| self.closing.is_some()
				{
					return Task::none();
				}

				self.drives.generation += 1;
				let generation = self.drives.generation;
				self.drives.refreshing = Some(generation);

				return Task::perform(volumes::discover(), move |result| {
					Message::Loaded(generation, result).into()
				});
			}
			Message::Loaded(generation, result) => {
				if self.drives.refreshing != Some(generation) {
					return Task::none();
				}

				self.drives.refreshing = None;
				match result {
					Ok(snapshot) => {
						log::debug!(
							volumes = snapshot.volumes.len(),
							cards = snapshot.cards(self.drives.show_system).len(),
							"Storage discovery completed"
						);
						self.drives.snapshot = Some(snapshot);
						self.drives.problem = None;
					}
					Err(error) => {
						self.drives.snapshot = None;
						self.drives.problem = Some(format!("Storage discovery failed: {error}"));
					}
				}
			}
			Message::MenuAt(target, position) => {
				return crate::browser::view::window_point(position)
					.map(move |point| Message::MenuOpen(target.clone(), point).into());
			}
			Message::MenuOpen(target, position) => {
				let id = window::Id::unique();
				self.typeahead.clear();
				let close = self.menu_lifecycle(Event::Open {
					id,
					parent: Some(self.host),
					owner: (),
				});
				self.drives.menu = Some(Menu { id, target });
				log::debug!(
					?id,
					sidebar = self
						.drives
						.menu
						.as_ref()
						.is_some_and(|m| m.target.is_none()),
					"Storage context menu opened"
				);
				let position = iced::Point::new(
					position.x.clamp(0.0, (self.host_size.width - 1.0).max(0.0)),
					position
						.y
						.clamp(0.0, (self.host_size.height - 1.0).max(0.0)),
				);

				return close.chain(Task::done(crate::Message::NewPopUp {
					id,
					settings: IcedNewPopupSettings::at_position(
						self.host,
						PixelSize::px(330, 280),
						(position.x as i32, position.y as i32),
					),
				}));
			}
			Message::ToggleSystem(id) => {
				if self
					.drives
					.menu
					.as_ref()
					.is_some_and(|m| m.id == id && m.target.is_none())
				{
					self.drives.show_system = !self.drives.show_system;
					log::debug!(
						visible = self.drives.show_system,
						"System mount visibility changed"
					);
					return self.menu_lifecycle(Event::CloseAll);
				}
			}
			Message::MenuAction(id, action) => {
				if let Some(target) = self
					.drives
					.menu
					.as_ref()
					.filter(|m| m.id == id)
					.and_then(|m| m.target.clone())
				{
					return self
						.menu_lifecycle(Event::CloseAll)
						.chain(Task::done(Message::Request(target, action).into()));
				}
			}
			Message::Request(identity, action) => {
				if self.drives.busy.is_some()
					|| self.dialogs.blocked()
					|| self.dialogs.deleting.is_some()
					|| self.closing.is_some()
				{
					return Task::none();
				}

				let result = self
					.drives
					.snapshot
					.as_ref()
					.ok_or_else(|| "Storage discovery is unavailable".to_owned())
					.and_then(|snapshot| {
						let volume = snapshot
							.volume(&identity.object)
							.filter(|v| v.identity == identity)
							.ok_or("Device is no longer available")?;
						Operation::prepare(snapshot, volume, action)
					});

				match result {
					Ok(operation) => return self.request_drive_confirmation(operation),
					Err(error) => self.error = Some(error),
				}
			}
			Message::Finished(generation, result) => {
				if self.drives.busy != Some(generation) {
					return Task::none();
				}

				self.drives.busy = None;
				let navigation = match result {
					Ok(Some(path)) if self.closing.is_none() => {
						self.update_browser(crate::browser::Message::GoTo(path))
					}
					Err(error) => {
						self.error = Some(error);
						Task::none()
					}
					_ => Task::none(),
				};
				if self.closing.is_some() {
					return self.finish_pending_close();
				}

				return navigation.chain(self.update_drives(Message::Refresh));
			}
		}

		Task::none()
	}

	pub(crate) fn perform_drive_operation(&mut self, operation: Operation) -> Task<crate::Message> {
		if self.drives.busy.is_some() || self.closing.is_some() {
			return Task::none();
		}

		self.drives.generation += 1;
		let generation = self.drives.generation;
		self.drives.refreshing = None;
		self.drives.busy = Some(generation);

		Task::perform(operation.execute(), move |result| {
			Message::Finished(generation, result).into()
		})
	}

	pub(crate) fn drive_menu_event(
		&mut self, id: window::Id, event: window::Event,
	) -> Task<crate::Message> {
		match event {
			window::Event::Opened { .. } => {
				iced::widget::operation::focus(format!("drive-menu-{id:?}-0"))
			}
			window::Event::Closed => self.menu_lifecycle(Event::Closed(id)),
			window::Event::CloseRequested => self.menu_lifecycle(Event::CloseAll),
			_ => Task::none(),
		}
	}

	pub(crate) fn drive_menu_key(&mut self, event: iced::keyboard::Event) -> Task<crate::Message> {
		let iced::keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
			return Task::none();
		};

		match key.as_ref() {
			Key::Named(Named::Escape) => self.menu_lifecycle(Event::CloseAll),
			Key::Named(Named::Tab) if modifiers.shift() => {
				iced::widget::operation::focus_previous()
			}
			Key::Named(Named::Tab | Named::ArrowDown) => iced::widget::operation::focus_next(),
			Key::Named(Named::ArrowUp) => iced::widget::operation::focus_previous(),
			_ => Task::none(),
		}
	}
}
