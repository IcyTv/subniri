use super::{ConfirmationAction, Message, Phase, cancel_id, confirm_id};
use crate::{FileManager, ui::style};
use iced::{
	Alignment, Element, Length,
	widget::{Space, column, container, mouse_area, row, text},
};

impl FileManager {
	pub(crate) fn confirmation_view(&self, id: iced::window::Id) -> Element<'_, crate::Message> {
		let colors = self.appearance.colors();
		let Some(pending) = self.dialogs.pending.as_ref().filter(|p| p.id == id) else {
			return Space::new().into();
		};
		let (heading, description, warning, confirm_label) = match &pending.action {
			ConfirmationAction::Drive(operation) => (
				"Confirm storage operation",
				operation.description(),
				"",
				operation.action.label(),
			),
			ConfirmationAction::Delete(target) => {
				let count = target.others.len() + 1;
				let description = if count == 1 {
					format!(
						"Permanently delete “{}”?",
						target
							.path
							.file_name()
							.unwrap_or_default()
							.to_string_lossy()
					)
				} else {
					format!("Permanently delete {count} selected items?")
				};
				(
					"Permanently delete?",
					description,
					"This cannot be undone. Folders and their contents will be deleted.",
					"Permanently Delete",
				)
			}
			ConfirmationAction::TrashDelete { items, empty } => {
				let names = items
					.iter()
					.take(5)
					.map(|item| {
						item.original
							.as_ref()
							.unwrap_or(&item.payload.path)
							.file_name()
							.unwrap_or_default()
							.to_string_lossy()
							.into_owned()
					})
					.collect::<Vec<_>>()
					.join("\n");
				let extra = if items.len() > 5 {
					format!("\n…and {} more", items.len() - 5)
				} else {
					String::new()
				};
				(
					if *empty {
						"Empty Trash?"
					} else {
						"Delete permanently?"
					},
					format!(
						"Permanently delete {} captured trash {}?\n\n{names}{extra}",
						items.len(),
						if items.len() == 1 { "item" } else { "items" }
					),
					"This cannot be undone. Folders and their contents will be deleted. Offline locations and newly arrived items are not included.",
					if *empty {
						"Empty Trash"
					} else {
						"Delete Permanently"
					},
				)
			}
		};

		let title = mouse_area(
			container(text(heading).size(16))
				.padding(14)
				.width(Length::Fill),
		)
		.on_press(Message::Drag(id).into());
		let close = style::control(text("×").size(18), false, colors)
			.on_press(Message::Respond(id, false).into());
		let active = pending.phase == Phase::Open;
		let buttons = row![
			Space::new().width(Length::Fill),
			style::control(text("Cancel").size(13), false, colors)
				.id(cancel_id(id))
				.enabled(active)
				.on_press(Message::Respond(id, false).into()),
			style::primary(text(confirm_label).size(13).color(colors.on_accent), colors)
				.id(confirm_id(id))
				.enabled(active)
				.on_press(Message::Respond(id, true).into()),
		]
		.spacing(12)
		.align_y(Alignment::Center);
		container(
			column![
				row![title, close].align_y(Alignment::Center),
				iced::widget::scrollable(
					container(
						column![text(description).size(14), text(warning).size(12)].spacing(10)
					)
					.padding(14)
				)
				.height(Length::Fill),
				container(buttons).padding(14),
			]
			.height(Length::Fill),
		)
		.width(Length::Fill)
		.height(Length::Fill)
		.style(move |_| container::Style {
			text_color: Some(colors.text),
			..style::panel(colors.body)
		})
		.into()
	}

	pub(crate) fn title(&self, id: iced::window::Id) -> Option<String> {
		if self.transfers.owns(id) {
			return Some("Transfer — Fileberg".into());
		}
		Some(
			match self
				.dialogs
				.pending
				.as_ref()
				.filter(|p| p.id == id)
				.map(|p| &p.action)
			{
				Some(ConfirmationAction::Delete(_)) => "Confirm Permanent Deletion — Fileberg",
				Some(ConfirmationAction::Drive(_)) => "Confirm Storage Operation — Fileberg",
				Some(ConfirmationAction::TrashDelete { .. }) => "Confirm Trash Deletion — Fileberg",
				None => "fileberg",
			}
			.into(),
		)
	}
}
