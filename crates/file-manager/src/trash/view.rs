use super::Message;
use crate::{
	FileManager,
	ui::{icons::ui_icon, style},
};
use iced::{
	Alignment, Element, Length, font,
	widget::{Space, column, container, row, scrollable, svg, text},
};

const FONT: iced::Font = iced::Font::new("DM Sans");

impl FileManager {
	pub(crate) fn trash_view(&self) -> Element<'_, crate::Message> {
		let colors = self.appearance.colors();
		let state = &self.trash_state;
		let count = state
			.listing
			.as_ref()
			.map_or(0, |listing| listing.contents.entries.len());
		let selected = state.selected.len();
		let blocked = self.trash_mutation_blocked() || state.loading;
		let can_restore = selected > 0
			&& state.listing.as_ref().is_some_and(|listing| {
				listing
					.contents
					.entries
					.iter()
					.filter(|entry| state.selected.contains(&entry.stored_path))
					.all(|entry| {
						entry.original_path.is_some()
							&& entry.info_bytes.is_some()
							&& entry.info_identity.is_some()
							&& entry.identity.is_some()
					})
			});
		let heading = row![
			column![
				label("Trash").size(27).weight(font::Weight::Bold),
				label(format!(
					"{count} {} · Mounted locations",
					if count == 1 { "item" } else { "items" }
				))
				.size(11)
				.color(colors.secondary)
			]
			.spacing(6)
			.width(Length::Fill),
			style::control(label("Files").size(12), false, colors)
				.id("trash-files")
				.on_press(crate::transfers::Message::ShowHistory(false).into()),
			style::control(label("Refresh").size(12), false, colors)
				.id("trash-refresh")
				.enabled(!state.busy && !self.transfers.busy && !state.loading)
				.on_press(Message::Refresh.into()),
			style::control(label("Empty Trash").size(12), false, colors)
				.id("trash-empty")
				.enabled(!blocked && count > 0)
				.on_press(Message::Empty.into()),
		]
		.spacing(10)
		.align_y(Alignment::Center);

		let mut body = column![].spacing(10);
		for (error, dismiss) in [
			(state.error.as_ref(), Message::Dismiss.into()),
			(
				self.error.as_ref(),
				crate::browser::Message::DismissError.into(),
			),
		] {
			if let Some(error) = error {
				body = body.push(
					container(
						row![
							label(error).size(12).width(Length::Fill),
							style::control(label("Dismiss").size(11), false, colors)
								.on_press(dismiss)
						]
						.spacing(10),
					)
					.padding(12)
					.width(Length::Fill)
					.style(move |_| style::panel(colors.decorative.pink)),
				);
			}
		}
		if let Some(listing) = &state.listing {
			for issue in &listing.contents.issues {
				body = body.push(
					label(format!("{}: {}", issue.path.display(), issue.error))
						.size(11)
						.color(colors.secondary),
				);
			}
			for warning in &listing.warnings {
				body = body.push(label(warning).size(11).color(colors.secondary));
			}
			for pending in &listing.pending {
				body = body.push(
					container(
						column![
							row![
								label(format!(
									"Deletion interrupted: {}",
									pending.original.display()
								))
								.size(12)
								.width(Length::Fill),
								style::control(label("Retry deletion").size(11), false, colors)
									.id(format!("trash-retry-{}", pending.id))
									.enabled(!blocked)
									.on_press(Message::Retry(pending.id.clone()).into())
							]
							.spacing(10)
							.align_y(Alignment::Center),
							label(
								"Continues a previously confirmed permanent deletion. Other trash items are not included."
							)
							.size(11)
							.color(colors.secondary),
						]
						.spacing(6),
					)
					.padding(12)
					.width(Length::Fill)
					.style(move |_| style::panel(colors.surface)),
				);
			}
		}
		if state.loading && state.listing.is_none() {
			body = body.push(label("Loading Trash…").size(14));
		} else if count == 0 {
			body = body.push(
				container(
					column![
						svg(ui_icon!("trash"))
							.width(36)
							.height(36)
							.style(move |_, _| svg::Style {
								color: Some(colors.secondary)
							}),
						label("Trash is empty.").size(19),
						label("Drop files on Trash in the sidebar to move them here.")
							.size(12)
							.color(colors.secondary),
					]
					.spacing(12)
					.align_x(Alignment::Start),
				)
				.padding([30, 12]),
			);
		} else if let Some(listing) = &state.listing {
			body = body.push(
				row![
					label("Name").size(11).width(Length::FillPortion(3)),
					label("Original location")
						.size(11)
						.width(Length::FillPortion(5)),
					label("Deleted").size(11).width(150)
				]
				.spacing(14),
			);
			body = body.push(style::rule(colors));
			let mut entries: Vec<_> = listing.contents.entries.iter().collect();
			entries.sort_by(|a, b| {
				b.deletion_date
					.cmp(&a.deletion_date)
					.then_with(|| a.original_path.cmp(&b.original_path))
					.then_with(|| a.stored_path.cmp(&b.stored_path))
			});
			for entry in entries {
				let path = entry.stored_path.clone();
				let checked = state.selected.contains(&path);
				let name = entry
					.original_path
					.as_ref()
					.and_then(|path| path.file_name())
					.or_else(|| entry.stored_path.file_name())
					.unwrap_or_default()
					.to_string_lossy()
					.into_owned();
				let mut detail = column![label(name).size(13).weight(font::Weight::Semibold)]
					.spacing(4)
					.width(Length::FillPortion(3));
				if !entry.issues.is_empty() {
					detail = detail.push(
						label(
							entry
								.issues
								.iter()
								.map(|issue| issue.error.to_string())
								.collect::<Vec<_>>()
								.join("; "),
						)
						.size(10)
						.color(colors.secondary),
					);
				}
				let original = entry.original_path.as_ref().map_or_else(
					|| "Original location unavailable".to_owned(),
					|path| path.display().to_string(),
				);
				let date = entry
					.deletion_date
					.as_ref()
					.map_or_else(|| "Unknown".to_owned(), |date| date.replace('T', " "));
				let item = row![
					svg(if checked {
						ui_icon!("check-square")
					} else {
						ui_icon!("square")
					})
					.width(18)
					.height(18)
					.style(move |_, _| svg::Style {
						color: Some(colors.text)
					}),
					detail,
					label(original)
						.size(11)
						.width(Length::FillPortion(5))
						.wrapping(iced::widget::text::Wrapping::WordOrGlyph),
					label(date).size(11).width(150),
				]
				.spacing(14)
				.width(Length::Fill)
				.align_y(Alignment::Start);
				body = body.push(
					style::control(item, checked, colors)
						.id(format!("trash-item-{}", entry.stored_path.display()))
						.width(Length::Fill)
						.padding(10)
						.enabled(!blocked)
						.on_press(Message::Select(path, !checked).into()),
				);
			}
		}
		let status = state
			.status
			.map(str::to_owned)
			.unwrap_or_else(|| format!("{selected} selected"));
		let controls = row![
			label(status).size(12),
			Space::new().width(Length::Fill),
			style::control(
				label(if selected == count && count > 0 {
					"Deselect all"
				} else {
					"Select all"
				})
				.size(12),
				false,
				colors
			)
			.id("trash-select-all")
			.enabled(!blocked && count > 0)
			.on_press(Message::SelectAll(selected != count).into()),
			style::control(label("Delete Permanently").size(12), false, colors)
				.id("trash-delete")
				.enabled(!blocked && selected > 0)
				.on_press(Message::Delete.into()),
			style::primary(label("Restore").size(12).color(colors.on_accent), colors)
				.id("trash-restore")
				.enabled(!blocked && can_restore)
				.on_press(Message::Restore.into()),
		]
		.spacing(10)
		.align_y(Alignment::Center);
		container(
			column![
				heading,
				style::rule(colors),
				scrollable(body).id("trash-items").height(Length::Fill),
				style::rule(colors),
				controls
			]
			.spacing(18),
		)
		.padding(24)
		.width(Length::Fill)
		.height(Length::Fill)
		.into()
	}
}

fn label<'a>(value: impl iced::widget::text::IntoFragment<'a>) -> iced::widget::Text<'a> {
	text(value).font(FONT)
}
