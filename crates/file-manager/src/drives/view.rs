use super::Message;
use crate::filesystem::entry::format_size;
use crate::{
	FileManager,
	filesystem::{
		storage::Usage,
		volumes::{Action, Identity},
	},
	ui::{icons::ui_icon, style},
};
use iced::{
	Alignment, Element, Length, font,
	widget::{column, container, row, scrollable, svg, text},
};

impl FileManager {
	pub(crate) fn drive_cards(&self) -> Element<'_, crate::Message> {
		let mut cards = column![].spacing(10);
		let mut has_root = false;

		if let Some(snapshot) = &self.drives.snapshot {
			for (volume, destination) in snapshot.cards(self.drives.show_system) {
				let path = destination.map(|m| m.path.clone());
				has_root |= path.as_deref() == Some(std::path::Path::new("/"));
				let label = path
					.as_ref()
					.map_or_else(|| "Not mounted".into(), |p| p.display().to_string());
				let message = path.map_or_else(
					|| Message::Request(volume.identity.clone(), Action::Mount).into(),
					|p| crate::browser::Message::GoTo(p).into(),
				);
				cards = cards.push(self.drive_card(
					&volume.name,
					&label,
					destination.and_then(|m| m.usage),
					Some(volume.identity.clone()),
					message,
				));
			}
		}

		if !has_root {
			cards = cards.push(self.drive_card(
				"Local disk",
				"/",
				None,
				None,
				crate::browser::Message::GoTo("/".into()).into(),
			));
		}
		if self.drives.busy.is_some() {
			cards = cards.push(
				text("Storage operation in progress…")
					.size(10)
					.color(style::MUTED),
			);
		}
		if let Some(problem) = &self.drives.problem {
			cards = cards.push(text(problem).size(10).color(style::MUTED));
		}

		card_list(cards.into(), self.host_size.height)
	}

	fn drive_card<'a>(
		&self, name: &str, destination: &str, usage: Option<Usage>, identity: Option<Identity>,
		message: crate::Message,
	) -> Element<'a, crate::Message> {
		let mut content = column![
			row![
				svg(ui_icon!("hard-drive")).width(19).height(19),
				text(name.to_owned()).size(12).weight(font::Weight::Bold)
			]
			.spacing(8)
			.align_y(Alignment::Center)
			.width(Length::Fill),
			text(destination.to_owned())
				.size(10)
				.font(style::MONO)
				.color(style::MUTED),
		]
		.spacing(10);

		if let Some(usage) = usage {
			content = content.push(
				iced::widget::progress_bar(0.0..=100.0, f32::from(usage.used_percent()))
					.girth(7)
					.style(|_| iced::widget::progress_bar::Style {
						background: style::PAPER.into(),
						bar: style::INK.into(),
						border: iced::Border {
							color: style::INK,
							width: 1.0,
							radius: 0.0.into(),
						},
					}),
			);
			content = content.push(
				text(format!(
					"{} free / {}",
					format_size(usage.available),
					format_size(usage.total)
				))
				.size(9)
				.font(style::MONO)
				.color(style::MUTED),
			);
		} else if matches!(
			&message,
			crate::Message::Browser(crate::browser::Message::GoTo(_))
		) {
			content = content.push(
				text("Usage unavailable")
					.size(9)
					.font(style::MONO)
					.color(style::MUTED),
			);
		}

		let mut card = style::control(content, false)
			.id(format!(
				"drive-card-{}-{destination}",
				identity.as_ref().map_or("root", |i| i.object.as_str())
			))
			.enabled(
				!matches!(&message, crate::Message::Drives(Message::Request(..)))
					|| self.drives.busy.is_none(),
			)
			.padding([12, 11])
			.width(Length::Fill)
			.on_press(message);
		if let Some(identity) = identity {
			card = card.on_context_menu_with_mouse_position(move |point| {
				Message::MenuAt(Some(identity.clone()), point).into()
			});
		} else {
			// A fallback root card is not empty sidebar space.
			card = card.on_context_menu(Message::NoAction.into());
		}

		container(card)
			.padding(1.5)
			.width(Length::Fill)
			.style(|_| container::Style {
				border: iced::Border {
					color: style::INK,
					width: 1.5,
					radius: 0.0.into(),
				},
				..style::panel(style::PAPER)
			})
			.into()
	}

	pub(crate) fn drive_menu_view(&self, id: iced::window::Id) -> Element<'_, crate::Message> {
		let Some(menu) = self.drives.menu.as_ref().filter(|m| m.id == id) else {
			return iced::widget::Space::new().into();
		};
		let mut rows = column![].spacing(6);

		if let Some(identity) = &menu.target {
			if let Some((snapshot, volume)) = self.drives.snapshot.as_ref().and_then(|s| {
				s.volume(&identity.object)
					.filter(|v| v.identity == *identity)
					.map(|v| (s, v))
			}) {
				rows = rows.push(
					text(volume.name.clone())
						.size(14)
						.weight(font::Weight::Bold),
				);
				let mut index = 0;
				for action in [
					Action::Mount,
					Action::Unmount,
					Action::Eject,
					Action::PowerOff,
				] {
					if (action == Action::Mount && !volume.mounts.is_empty())
						|| (action == Action::Unmount && volume.mounts.is_empty())
						|| (action == Action::Eject && !volume.drive.ejectable)
						|| (action == Action::PowerOff && !volume.drive.can_power_off)
					{
						continue;
					}
					let reason = if self.drives.busy.is_some() {
						Some("Another storage operation is running".to_owned())
					} else {
						crate::filesystem::volumes::Operation::prepare(snapshot, volume, action)
							.err()
					};
					let mut label = column![text(action.label()).size(13)].spacing(3);
					if let Some(reason) = &reason {
						label = label.push(text(reason.clone()).size(10).color(style::MUTED));
					}
					let mut button = style::control(label, false)
						.width(Length::Fill)
						.enabled(reason.is_none())
						.on_press(Message::MenuAction(id, action).into());
					if reason.is_none() {
						button = button.id(format!("drive-menu-{id:?}-{index}"));
						index += 1;
					}
					rows = rows.push(button);
				}
			} else {
				rows = rows.push(text("Device is no longer available").size(13));
			}
		} else {
			rows = rows.push(
				style::control(
					text(if self.drives.show_system {
						"✓ Show system mounts"
					} else {
						"Show system mounts"
					})
					.size(13),
					self.drives.show_system,
				)
				.id(format!("drive-menu-{id:?}-0"))
				.width(Length::Fill)
				.on_press(Message::ToggleSystem(id).into()),
			);
		}

		menu_frame(rows.into(), self.host_size.height)
	}
}

fn card_list<'a, R: iced::advanced::text::Renderer + 'a>(
	cards: Element<'a, crate::Message, iced::Theme, R>, host_height: f32,
) -> Element<'a, crate::Message, iced::Theme, R> {
	scrollable(cards)
		.height(Length::Shrink.max(host_height * 0.4))
		.into()
}

// Native popup autosizing lays out against a large limit, not its initial size.
// Keep both axes bounded instead of requesting Fill on that measurement pass.
fn menu_frame<'a, R: iced::advanced::text::Renderer + 'a>(
	rows: Element<'a, crate::Message, iced::Theme, R>, host_height: f32,
) -> Element<'a, crate::Message, iced::Theme, R> {
	container(scrollable(rows))
		.padding(10)
		.width(330)
		.height(host_height.clamp(160.0, 280.0))
		.style(|_| container::Style {
			border: iced::Border {
				color: style::INK,
				width: 1.5,
				radius: 0.0.into(),
			},
			..style::panel(style::PAPER)
		})
		.into()
}

#[cfg(test)]
mod tests {
	use super::*;
	use iced::{
		Size,
		advanced::{layout, widget::Tree},
		widget::Space,
	};

	#[test]
	fn disk_list_shrinks_when_short_and_scrolls_within_a_height_cap() {
		for (content_height, expected_height) in [(100, 100.0), (1000, 320.0)] {
			let cards: Element<'_, crate::Message, iced::Theme, ()> =
				card_list(Space::new().height(content_height).into(), 800.0);
			let mut view: Element<'_, crate::Message, iced::Theme, ()> =
				iced::widget::column![Space::new().height(Length::FillPortion(3)), cards]
					.spacing(16)
					.height(Length::Fill)
					.into();
			let mut tree = Tree::new(view.as_widget());
			view.as_widget_mut().diff(&mut tree);

			let node = view.as_widget_mut().layout(
				&mut tree,
				&(),
				&layout::Limits::new(Size::ZERO, Size::new(220.0, 700.0)),
			);

			let list = node.children().last().expect("disk list layout");
			assert_eq!(list.size().height, expected_height);
			assert_eq!(list.bounds().y + list.size().height, 700.0);
		}
	}

	#[test]
	fn storage_menu_does_not_expand_to_native_autosize_limits() {
		let mut view: Element<'_, crate::Message, iced::Theme, ()> =
			menu_frame(Space::new().height(100).into(), 800.0);
		let mut tree = Tree::new(view.as_widget());
		view.as_widget_mut().diff(&mut tree);

		let node = view.as_widget_mut().layout(
			&mut tree,
			&(),
			&layout::Limits::new(Size::ZERO, Size::new(4096.0, 4096.0)),
		);

		assert_eq!(node.size(), Size::new(330.0, 280.0));
	}
}
