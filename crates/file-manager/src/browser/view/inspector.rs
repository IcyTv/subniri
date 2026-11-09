//! Selection changes content, not file-viewport geometry. Only an explicit
//! details toggle changes the reserved inspector height.
use crate::browser::Message as BrowserMessage;
use crate::{
	FileManager, Message,
	ui::{icons::ui_icon, style},
};
use iced::{
	Alignment, Element, Length,
	advanced::Renderer,
	widget::{Space, column, container, row, scrollable, svg, text},
};

const STRIP_HEIGHT: f32 = 40.0;
const DETAILS_HEIGHT: f32 = 104.0;

impl FileManager {
	pub(super) fn selection_inspector(&self, gutter: u16) -> Element<'_, Message> {
		let selected = self.entries.iter().find(|entry| {
			self.selection.paths.len() == 1 && self.selection.paths.contains(&entry.path)
		});
		let count = self.selection.paths.len();
		let details: Element<'_, Message> = if self.show_info {
			let content = selected.map_or_else(
				|| {
					container(
						text(if count > 1 {
							"Multiple items selected. Per-item details and tags require a single selection."
						} else {
							"Select an item to view its details."
						})
						.size(12)
						.color(style::MUTED),
					)
					.padding(14)
					.width(Length::Fill)
					.style(|_| style::panel(style::SIDEBAR))
					.into()
				},
				|entry| self.selected_tags(entry),
			);
			scrollable(content)
				.id("selection-details")
				.height(Length::Fill)
				.into()
		} else {
			Space::new().into()
		};
		let action_color = if count == 1 {
			style::PAPER
		} else {
			style::MUTED
		};
		let icon = |handle| {
			svg(handle)
				.width(16)
				.height(16)
				.style(move |_, _| svg::Style {
					color: Some(action_color),
				})
		};
		let strip = container(
			row![
				svg(ui_icon!("check-circle"))
					.width(16)
					.height(16)
					.style(move |_, _| svg::Style {
						color: Some(if count > 0 { style::BLUE } else { style::MUTED }),
					}),
				text(selected.map_or_else(
					|| if count > 1 {
						format!("{count} items selected")
					} else {
						"Select an item".to_owned()
					},
					|entry| format!("{} selected", entry.name)
				))
				.size(11)
				.width(Length::Fill)
				.wrapping(text::Wrapping::None)
				.ellipsis(iced::advanced::text::Ellipsis::End),
				style::dark_control(
					row![
						icon(ui_icon!("arrow-square-out")),
						text("Open").size(11).color(action_color)
					]
					.spacing(6)
				)
				.id("selection-open")
				.on_press(BrowserMessage::OpenSelected.into())
				.enabled(count == 1)
				.disabled_background(style::INK),
				style::dark_control(
					row![
						icon(ui_icon!("info")),
						text(if self.show_info { "Hide info" } else { "Info" }).size(11)
					]
					.spacing(6)
				)
				.id("selection-info")
				.on_press(BrowserMessage::ToggleInfo.into()),
				style::dark_control(icon(ui_icon!("x")))
					.id("selection-clear")
					.on_press(BrowserMessage::ClearSelection.into())
					.enabled(count > 0)
					.disabled_background(style::INK),
			]
			.spacing(8)
			.align_y(Alignment::Center),
		)
		.padding([4, 13])
		.width(Length::Fill)
		.height(Length::Fill)
		.center_y(Length::Fill)
		.style(|_| container::Style {
			text_color: Some(style::PAPER),
			..style::panel(style::INK)
		});
		container(inspector_layout(strip.into(), details, self.show_info))
			.padding(iced::Padding {
				left: f32::from(gutter),
				right: f32::from(gutter),
				bottom: 12.0,
				top: 0.0,
			})
			.into()
	}
}

fn inspector_layout<'a, M: 'a, R: Renderer + 'a>(
	strip: Element<'a, M, iced::Theme, R>, details: Element<'a, M, iced::Theme, R>, expanded: bool,
) -> Element<'a, M, iced::Theme, R> {
	column![
		container(details)
			.width(Length::Fill)
			.height(if expanded { DETAILS_HEIGHT } else { 0.0 }),
		container(strip).width(Length::Fill).height(STRIP_HEIGHT),
	]
	.spacing(if expanded { 8 } else { 0 })
	.into()
}

#[cfg(test)]
mod tests {
	use super::*;
	use iced::{
		Size,
		advanced::{layout, widget::Tree},
	};

	#[test]
	fn selection_and_detail_content_cannot_resize_the_file_viewport() {
		for expanded in [false, true] {
			let mut heights = Vec::new();
			for detail_height in [0, 30, 300] {
				let mut view: Element<'_, (), iced::Theme, ()> = iced::widget::column![
					scrollable(Space::new().height(2000)).height(Length::Fill),
					inspector_layout(
						Space::new().height(30).into(),
						Space::new().height(detail_height).into(),
						expanded
					),
				]
				.height(500)
				.into();
				let mut tree = Tree::new(view.as_widget());
				view.as_widget_mut().diff(&mut tree);
				let node = view.as_widget_mut().layout(
					&mut tree,
					&(),
					&layout::Limits::new(Size::ZERO, Size::new(500.0, 500.0)),
				);
				heights.push(node.children().first().map(|node| node.size().height));
			}
			assert_eq!(heights, vec![Some(if expanded { 348.0 } else { 460.0 }); 3]);
		}
	}
}
