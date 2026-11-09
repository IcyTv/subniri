use crate::context_menu::Message as ContextMenuMessage;
use crate::{FileManager, Message, context_menu::Action, ui::style};
use iced::{
	Alignment, Element, Length,
	widget::{column, container, mouse_area, row, scrollable, text, text_input},
	window,
};

impl FileManager {
	pub(crate) fn menu_view(&self, id: window::Id) -> Element<'_, Message> {
		let Some(menu) = &self.menu else {
			return iced::widget::Space::new().into();
		};
		let Some(page) = menu.page(id) else {
			return iced::widget::Space::new().into();
		};
		let rows = self.menu_rows(id);
		let mut items = column![].spacing(2);
		let mut previous = "";
		for item in rows {
			let pinned =
				matches!(item.action,Action::Command(c) if self.favorites.order.contains(&c));
			let favorite = pinned && page.query.is_empty() && page.parent_action.is_none();
			let group = if favorite {
				"FAVORITES"
			} else {
				item.action.parent().group()
			};
			if page.query.is_empty() && page.parent_action.is_none() && group != previous {
				items = items.push(
					container(text(group).font(style::MONO).size(9).color(style::MUTED))
						.padding([6, 8]),
				);
				previous = group;
			}
			let selected =
				page.highlighted == Some(item.action) || page.hovered == Some(item.action);
			let submenu = matches!(item.action,Action::Command(c) if c.submenu());
			let mut content = column![
				row![
					text(if item.checked { "✓" } else { " " })
						.size(12)
						.width(17),
					text(item.label).size(13).width(Length::Fill),
					text(if submenu { "›" } else { "" }).size(16)
				]
				.spacing(5)
				.align_y(Alignment::Center)
			];
			if let Some(reason) = item.disabled {
				content = content.push(text(reason).size(10).color(style::MUTED));
			}
			let action = item.action;
			let mut button = style::menu_control(content, selected)
				.enabled(item.disabled.is_none())
				.id(page.row_id(action))
				.width(Length::Fill)
				.padding([7, 8]);
			if favorite {
				button = button.on_press_started_with_mouse_position(move |point| {
					ContextMenuMessage::MenuFavoritePress(id, action.parent(), point).into()
				});
			}
			if item.disabled.is_none() && !favorite {
				button = if submenu {
					button.on_press_started_with_bounds(move |bounds| {
						ContextMenuMessage::MenuActivate(id, action, Some(bounds)).into()
					})
				} else {
					button.on_press(ContextMenuMessage::MenuActivate(id, action, None).into())
				};
			}
			let pin: Element<'_, Message> = if let Action::Command(command) = action {
				if selected && self.favorites.loaded && item.disabled.is_none() {
					style::menu_control(
						text(if pinned { "★" } else { "☆" }).size(16).center(),
						selected,
					)
					.padding([7, 8])
					.width(34)
					.id(page.pin_id(action))
					.on_press(ContextMenuMessage::MenuPin(id, command).into())
					.into()
				} else {
					iced::widget::Space::new().width(34).into()
				}
			} else {
				iced::widget::Space::new().width(34).into()
			};
			let separator = container(iced::widget::Space::new().width(1).height(18))
				.style(move |_| style::panel(if selected { style::INK } else { style::PAPER }));
			let line = container(
				row![button, separator, pin]
					.spacing(0)
					.align_y(Alignment::Center),
			)
			.padding(1)
			.style(move |_| container::Style {
				border: iced::Border {
					color: if selected {
						style::INK
					} else {
						iced::Color::TRANSPARENT
					},
					width: 1.0,
					radius: 0.0.into(),
				},
				..style::panel(if selected { style::BLUE } else { style::PAPER })
			});
			let area =
				mouse_area(line).on_exit(ContextMenuMessage::MenuHover(id, action, false).into());
			let area = if favorite {
				area.on_move(move |p| {
					ContextMenuMessage::MenuDragOver(id, action.parent(), p.y > 18.0).into()
				})
			} else {
				area.on_enter(ContextMenuMessage::MenuHover(id, action, true).into())
			};
			let line = container(area).id(format!("{}-bounds", page.row_id(action)));
			if favorite {
				let insertion = menu
					.drag
					.as_ref()
					.filter(|d| d.page == id && d.dragging)
					.and_then(|d| d.over)
					.filter(|(c, _)| *c == action.parent());
				let marker = |show| {
					container(iced::widget::Space::new().height(2))
						.width(Length::Fill)
						.style(move |_| style::panel(if show { style::INK } else { style::PAPER }))
				};
				items = items.push(column![
					marker(insertion.is_some_and(|(_, after)| !after)),
					line,
					marker(insertion.is_some_and(|(_, after)| after))
				]);
			} else {
				items = items.push(line);
			}
		}
		if !page.query.is_empty() && self.menu_rows(id).is_empty() {
			items = items.push(
				container(text("No matching actions").size(12).color(style::MUTED)).padding(8),
			);
		}
		let mut body = column![
			text_input("Search actions…", &page.query)
				.id(page.search_id())
				.size(13)
				.padding([13, 12])
				.on_input(move |query| ContextMenuMessage::MenuInput(id, query).into())
				.style(style::menu_search)
		]
		.spacing(0);
		// Keep the scrollable at a stable child index while save/error status changes.
		let mut status = column![];
		if !menu.available {
			status = status.push(
				container(text("Item no longer available").size(12).color(style::INK))
					.padding([8, 10]),
			);
		}
		if let Some(error) = &self.favorites.error {
			status =
				status.push(container(text(error).size(10).color(style::INK)).padding([8, 10]));
		}
		if self.favorites.running() {
			status = status.push(
				container(text("Saving favorites…").size(10).color(style::MUTED)).padding([8, 10]),
			);
		}
		body = body.push(status);
		body = body.push(
			container(
				scrollable(container(items.width(Length::Fill)).padding([0, 8]))
					.id(page.scroll_id())
					.spacing(8)
					.height(Length::Fill),
			)
			.padding(iced::Padding {
				top: 6.0,
				bottom: 6.0,
				right: 4.0,
				left: 0.0,
			})
			.height(Length::Fill),
		);
		let content = container(body)
			.width(320)
			.height(self.host_size.height.min(480.0).max(160.0))
			.padding(1.5)
			.style(|_| container::Style {
				border: iced::Border {
					color: style::INK,
					width: 1.5,
					radius: 0.0.into(),
				},
				shadow: iced::Shadow {
					color: style::INK,
					offset: iced::Vector::new(3.0, 3.0),
					blur_radius: 0.0,
				},
				..style::panel(style::PAPER)
			});
		crate::ui::input::menu_gate(
			container(content)
				.padding([0, 0])
				.width(324)
				.height(self.host_size.height.min(484.0).max(164.0))
				.into(),
			id,
		)
	}
}
