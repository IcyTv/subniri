//! Window lifecycle. Closing a prompt never terminates the registered agent.

use std::sync::{
	Arc,
	atomic::{AtomicU8, Ordering},
};

use iced::{Element, Event as IcedEvent, Size, Subscription, Task, Theme, keyboard, window};
use neo_widgets::appearance::{Appearance, Event as AppearanceEvent};

use crate::{
	dialog::{self, Dialog},
	model::{Event, RequestId},
};

#[derive(Debug, Clone)]
enum Message {
	Appearance(AppearanceEvent),
	Backend(Event),
	Dialog(RequestId, dialog::Message),
	Dismiss(window::Id),
	Opened(window::Id),
	IconsUpdated,
}

pub struct App {
	appearance: Appearance,
	dialog: Option<Dialog>,
	window: Option<window::Id>,
	exit_code: Arc<AtomicU8>,
}

impl App {
	pub fn run(events: async_channel::Receiver<Event>, exit_code: Arc<AtomicU8>) -> iced::Result {
		iced::daemon(
			move || {
				(
					Self {
						appearance: Appearance::default(),
						dialog: None,
						window: None,
						exit_code: exit_code.clone(),
					},
					Task::run(events.clone(), Message::Backend),
				)
			},
			Self::update,
			Self::view,
		)
		.title("Authentication — thawth")
		.theme(|app: &Self, _| app.appearance.colors().iced_theme())
		.style(|app: &Self, _: &Theme| iced::theme::Style {
			background_color: app.appearance.colors().body,
			text_color: app.appearance.colors().text,
		})
		.subscription(Self::subscription)
		.run()
	}

	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Appearance(event) => {
				self.appearance.update(event);
				Task::none()
			}
			Message::IconsUpdated => Task::none(),
			Message::Backend(Event::Started(request, commands)) => {
				self.dialog = Dialog::new(request, commands);
				let (id, task) = window::open(prompt_settings());
				self.window = Some(id);

				task.map(Message::Opened)
			}
			Message::Backend(Event::Finished { id }) => {
				if self
					.dialog
					.as_ref()
					.is_some_and(|dialog| dialog.request.id == id)
				{
					self.dialog = None;
					return self.window.take().map_or_else(Task::none, window::close);
				}
				Task::none()
			}
			Message::Backend(Event::Fatal { code, message }) => {
				log::error!("{message}");
				self.exit_code.store(code, Ordering::Relaxed);
				iced::exit()
			}
			Message::Backend(event) => {
				if self
					.dialog
					.as_mut()
					.is_some_and(|dialog| dialog.apply(event))
				{
					return iced::widget::operation::focus(dialog::INPUT_ID);
				}
				Task::none()
			}
			Message::Dialog(id, message) => {
				if let Some(dialog) = &mut self.dialog
					&& dialog.request.id == id
					&& let Some(command) = dialog.update(message)
				{
					let _ = dialog.commands.try_send(command);
				}
				Task::none()
			}
			Message::Dismiss(id) => {
				if self.window == Some(id)
					&& let Some(dialog) = &mut self.dialog
					&& let Some(command) = dialog.update(dialog::Message::Cancel)
				{
					let _ = dialog.commands.try_send(command);
				}
				Task::none()
			}
			Message::Opened(id) => {
				if self.window == Some(id) {
					iced::widget::operation::focus(dialog::INPUT_ID)
				} else {
					Task::none()
				}
			}
		}
	}

	fn subscription(&self) -> Subscription<Message> {
		let events = iced::event::listen_with(|event, _, id| {
			is_dismissal(&event).then_some(Message::Dismiss(id))
		});
		let events = Subscription::batch([
			events,
			self.appearance.subscription().map(Message::Appearance),
		]);
		if self
			.dialog
			.as_ref()
			.is_some_and(|dialog| !dialog.request.presentation.icon_name.trim().is_empty())
		{
			Subscription::batch([
				events,
				neo_widgets::icons::background::updates().map(|()| Message::IconsUpdated),
			])
		} else {
			events
		}
	}

	fn view(&self, id: window::Id) -> Element<'_, Message> {
		if self.window != Some(id) {
			return iced::widget::text("").into();
		}

		match &self.dialog {
			Some(dialog) => {
				let id = dialog.request.id;
				dialog
					.view(self.appearance.colors())
					.map(move |message| Message::Dialog(id, message))
			}
			None => iced::widget::text("").into(),
		}
	}
}

fn is_dismissal(event: &IcedEvent) -> bool {
	matches!(event, IcedEvent::Window(window::Event::CloseRequested))
		|| matches!(
			event,
			IcedEvent::Keyboard(keyboard::Event::KeyPressed {
				key: keyboard::Key::Named(keyboard::key::Named::Escape),
				..
			})
		)
}

fn prompt_settings() -> window::Settings {
	// TODO: Add a config option to choose between window and UAC-style prompt behavior.
	let size = Size::new(520.0, 420.0);

	window::Settings {
		size,
		min_size: Some(size),
		max_size: Some(size),
		resizable: false,
		minimizable: false,
		exit_on_close_request: false,
		platform_specific: window::settings::PlatformSpecific {
			application_id: "thawth".into(),
			..Default::default()
		},
		..Default::default()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn fixed_constraints_exist_before_mapping() {
		let settings = prompt_settings();

		assert_eq!(settings.min_size, Some(settings.size));
		assert_eq!(settings.max_size, Some(settings.size));
		assert!(!settings.resizable);
		assert!(!settings.exit_on_close_request);
	}

	#[test]
	fn focus_loss_does_not_cancel() {
		assert!(!is_dismissal(&IcedEvent::Window(window::Event::Unfocused)));
		assert!(is_dismissal(&IcedEvent::Window(
			window::Event::CloseRequested
		)));
	}
}
