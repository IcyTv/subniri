use super::{Confirmation, ConfirmationAction, Message, Phase, cancel_id, confirm_id};
use crate::{
	FileManager,
	context_menu::{Action, Command, Target},
};
use iced::{
	Task,
	advanced::widget::{
		Id, Operation,
		operation::{Focusable, Outcome},
	},
	keyboard::{Key, key::Named},
	window,
};
use iced_exwlshell::{actions::IcedXdgWindowSettings, reexport::PixelSize};

#[derive(Default)]
struct FocusSnapshot(Option<Id>);
impl Operation<Option<Id>> for FocusSnapshot {
	fn traverse(&mut self, visit: &mut dyn FnMut(&mut dyn Operation<Option<Id>>)) {
		visit(self);
	}

	fn focusable(&mut self, id: Option<&Id>, _: iced::Rectangle, state: &mut dyn Focusable) {
		if state.is_focused() {
			self.0 = id.cloned();
		}
	}

	fn finish(&self) -> Outcome<Option<Id>> {
		Outcome::Some(self.0.clone())
	}
}

impl FileManager {
	pub(crate) fn request_delete_confirmation(&mut self, target: Target) -> Task<crate::Message> {
		self.request_confirmation(ConfirmationAction::Delete(target))
	}

	pub(crate) fn request_drive_confirmation(
		&mut self, operation: crate::filesystem::volumes::Operation,
	) -> Task<crate::Message> {
		self.request_confirmation(ConfirmationAction::Drive(operation))
	}

	fn request_confirmation(&mut self, action: ConfirmationAction) -> Task<crate::Message> {
		if self.dialogs.blocked()
			|| self.dialogs.deleting.is_some()
			|| self.drives.busy.is_some()
			|| self.closing.is_some()
		{
			return Task::none();
		}
		let id = window::Id::unique();
		self.dialogs.pending = Some(Confirmation {
			id,
			action,
			phase: Phase::Opening,
			prepared: false,
			previous_focus: None,
			confirm_focused: false,
		});
		self.typeahead.clear();
		self.modifiers = iced::keyboard::Modifiers::empty();
		self.selection.end_range();
		let close = self.menu_lifecycle(neo_widgets::popup::Event::CloseAll);
		log::debug!(?id, "Confirmation requested");
		close.chain(
			iced_runtime::task::widget(FocusSnapshot::default())
				.map(move |focus| Message::Prepared(id, focus).into()),
		)
	}

	pub(crate) fn update_dialog(&mut self, message: Message) -> Task<crate::Message> {
		if let Message::Deleted(id, result) = message {
			if self.dialogs.deleting != Some(id) {
				return Task::none();
			}
			self.dialogs.deleting = None;
			if let Err(error) = result {
				self.error = Some(error);
			}
			if self.closing.is_some() {
				return self.finish_pending_close();
			}
			return self.load_directory();
		}
		let Some(pending) = self.dialogs.pending.as_mut() else {
			return Task::none();
		};
		match message {
			Message::Prepared(id, focus)
				if pending.id == id && pending.phase == Phase::Opening && !pending.prepared =>
			{
				pending.prepared = true;
				pending.previous_focus = focus;
				Task::done(crate::Message::NewBaseWindow {
					id,
					settings: IcedXdgWindowSettings {
						parent: Some(self.host),
						size: Some(match pending.action {
							ConfirmationAction::Delete(_) => PixelSize::px(480, 230),
							ConfirmationAction::Drive(_) => PixelSize::px(520, 340),
						}),
						client_side_decorations: true,
					},
				})
			}
			Message::Respond(id, confirm) if pending.id == id && pending.phase == Phase::Open => {
				pending.phase = Phase::Closing { confirm };
				log::debug!(?id, confirm, "Delete confirmation response consumed");
				window::close(id)
			}
			Message::Drag(id) if pending.id == id && pending.phase == Phase::Open => {
				log::debug!(?id, "Dialog title-bar drag requested");
				window::drag(id)
			}
			Message::Key(id, iced::keyboard::Event::KeyPressed { key, modifiers, .. })
				if pending.id == id && pending.phase == Phase::Open =>
			{
				match key.as_ref() {
					Key::Named(Named::Escape) => self.update_dialog(Message::Respond(id, false)),
					Key::Named(Named::Tab)
						if !modifiers.alt() && !modifiers.control() && !modifiers.logo() =>
					{
						pending.confirm_focused = !pending.confirm_focused;
						iced::widget::operation::focus(if pending.confirm_focused {
							confirm_id(id)
						} else {
							cancel_id(id)
						})
					}
					_ => Task::none(),
				}
			}
			_ => Task::none(),
		}
	}

	pub(crate) fn dialog_window_event(
		&mut self, id: window::Id, event: window::Event,
	) -> Task<crate::Message> {
		let Some(pending) = self.dialogs.pending.as_mut().filter(|p| p.id == id) else {
			return Task::none();
		};
		match event {
			window::Event::Opened { .. } if pending.phase == Phase::Opening && pending.prepared => {
				pending.phase = Phase::Open;
				log::debug!(?id, "Delete confirmation opened");
				iced::widget::operation::focus(cancel_id(id))
			}
			window::Event::CloseRequested => self.update_dialog(Message::Respond(id, false)),
			window::Event::Closed => {
				let Some(pending) = self.dialogs.pending.take() else {
					return Task::none();
				};
				self.modifiers = iced::keyboard::Modifiers::empty();
				log::debug!(?id, "Delete confirmation closed");
				let focus = pending.previous_focus.or_else(|| {
					self.entries
						.iter()
						.find(|e| Some(&e.path) == self.selected.as_ref())
						.map(|e| e.id.clone())
				});
				let restore = focus.map_or_else(Task::none, iced::widget::operation::focus);
				if pending.phase == Phase::Opening {
					self.error = Some("Could not open confirmation dialog".into());
					return restore;
				}
				if pending.phase == (Phase::Closing { confirm: true }) && self.closing.is_none() {
					let target = match pending.action {
						ConfirmationAction::Drive(operation) => {
							return restore.chain(self.perform_drive_operation(operation));
						}
						ConfirmationAction::Delete(target) => target,
					};
					let action = Action::Command(Command::Delete);
					if let Some(reason) = action.unavailable(self, &target, target.available()) {
						self.error = Some(reason.into());
						return restore;
					}
					self.dialogs.deleting = Some(id);
					return restore.chain(self.perform_confirmed_delete(id, target));
				}
				restore
			}
			_ => Task::none(),
		}
	}

	pub(crate) fn cancel_dialog_for_shutdown(&mut self) -> Task<crate::Message> {
		self.dialogs
			.pending
			.take()
			.map_or_else(Task::none, |pending| window::close(pending.id))
	}
}
