use crate::{browser::Message as BrowserMessage, context_menu::Message as ContextMenuMessage};
use iced::{
	Alignment, Element, Length, font,
	widget::{Space, column, container, row, scrollable, svg, text, text_input},
};
use neo_widgets::phosphor_icon;

use crate::{
	FileManager, Message, ViewMode,
	browser::navigation,
	filesystem::entry::{DirEntry, format_size},
	ui::icons::{file_art, ui_icon},
	ui::style::{self, control, primary},
};

mod grid;
pub(crate) mod hint;
mod inspector;
pub(crate) mod selection;
mod sidebar;
use crate::ui::input as typeahead;
pub(crate) use selection::reveal_selection;
pub(crate) use selection::window_point;

impl FileManager {
	pub fn view(&self, id: iced::window::Id) -> Element<'_, Message> {
		if self.dialogs.owns(id) {
			return self.confirmation_view(id);
		}
		if id != self.host {
			return if self.drives.menu.as_ref().is_some_and(|m| m.id == id) {
				self.drive_menu_view(id)
			} else if self.menu.as_ref().is_some_and(|m| m.page(id).is_some()) {
				self.menu_view(id)
			} else {
				Space::new().into()
			};
		}
		// The layout responds to the tiled window, not the overall output size.
		let content = typeahead::gate(
			container(iced::widget::responsive(|size| self.layout(size.width)))
				.width(Length::Fill)
				.height(Length::Fill)
				.style(|_| style::panel(style::PAPER))
				.into(),
			self.typeahead.is_active(),
			!self.loading && self.menu.is_none() && self.drives.menu.is_none(),
			self.visible_entries()
				.iter()
				.map(|e| (e.id.clone(), e.path.clone()))
				.collect(),
		);
		let content = typeahead::host_gate(content, self.popups.topmost());
		// Keep the overlay at a stable tree position across modal transitions.
		let dim = container(Space::new().width(Length::Fill).height(Length::Fill)).style(|_| {
			container::Style {
				background: Some(
					iced::Color::from_rgba(
						0.0,
						0.0,
						0.0,
						if self.dialogs.blocked() { 0.28 } else { 0.0 },
					)
					.into(),
				),
				..Default::default()
			}
		});
		typeahead::modal_gate(
			iced::widget::stack![content, dim].into(),
			self.dialogs.blocked(),
		)
	}

	fn layout(&self, width: f32) -> Element<'_, Message> {
		let compact = width <= 800.0;
		let gutter = if width <= 1100.0 { 20 } else { 29 };
		let identity = container(
			row![
				svg(phosphor_icon!("squares-four", "fill")).width(23),
				text("fileberg")
					.size(21)
					.weight(font::Weight::Bold)
					.color(style::INK)
			]
			.spacing(10)
			.align_y(Alignment::Center),
		)
		.padding([9, 22])
		.height(46)
		.width(Length::Fill)
		.style(|_| style::panel(style::YELLOW));
		let mut content = column![
			container(self.toolbar(compact)).padding([12, 20]),
			style::rule(),
			container(self.content_heading(compact)).padding([24, gutter])
		]
		.width(Length::Fill)
		.height(Length::Fill);
		let mut error_notice = column![];
		if let Some(error) = &self.error {
			error_notice = error_notice.push(
				container(
					row![
						text(error).size(12).width(Length::Fill),
						control(text("Dismiss").size(11), false)
							.on_press(BrowserMessage::DismissError.into())
					]
					.spacing(8),
				)
				.padding([10, 20])
				.style(|_| style::panel(neo_widgets::style::rgb(0xefb_6a9))),
			);
		}
		content = content.push(error_notice);
		if let Some(name) = &self.folder_name {
			content = content.push(
				container(
					row![
						text_input("Folder name", name)
							.id("folder-name")
							.on_input(|name| BrowserMessage::FolderName(name).into())
							.on_submit(BrowserMessage::CreateFolder.into())
							.padding(10)
							.size(13)
							.style(style::input),
						primary(
							text(if self.creating_folder {
								"Creating…"
							} else {
								"Create"
							})
							.size(12)
						)
						.on_press(BrowserMessage::CreateFolder.into())
						.enabled(!self.creating_folder),
						control(text("Cancel").size(12), false)
							.on_press(BrowserMessage::CancelFolder.into())
					]
					.spacing(10)
					.align_y(Alignment::Center),
				)
				.padding([8, gutter]),
			);
		}
		if let Some(form) = self.tag_form() {
			content = content.push(container(form).padding([8, gutter]));
		}
		content = content.push(
			container(
				row![
					self.tag_filter_label(),
					Space::new().width(Length::Fill),
					control(
						row![
							text("Name").size(11),
							svg(if self.ascending {
								ui_icon!("arrow-down")
							} else {
								ui_icon!("arrow-up")
							})
							.width(13)
						]
						.spacing(7),
						false
					)
					.on_press(BrowserMessage::Sort.into())
				]
				.align_y(Alignment::Center),
			)
			.padding([0, gutter]),
		);
		let visible = self.visible_entries();
		let files: Element<'_, Message> = if self.loading && self.entries.is_empty() {
			container(text("Loading…").size(13).color(style::MUTED))
				.center(Length::Fill)
				.into()
		} else if visible.is_empty() {
			container(
				column![
					svg(ui_icon!("folder-open")).width(42),
					text(if self.search.is_empty() && self.active_tag.is_none() {
						"This folder is empty."
					} else {
						"No files match the current filters."
					})
					.size(13)
					.color(style::MUTED)
				]
				.spacing(14)
				.align_x(Alignment::Center),
			)
			.center(Length::Fill)
			.into()
		} else {
			let entries: Element<'_, Message> = match self.view_mode {
				ViewMode::Grid => {
					grid::fixed_grid(visible.iter().map(|entry| self.file_tile(entry)))
				}
				ViewMode::List => {
					let mut files = column![].spacing(3);
					for entry in &visible {
						files = files.push(self.file_row(entry));
					}
					files.into()
				}
			};
			scrollable(entries)
				.id(selection::FILES_ID)
				.height(Length::Fill)
				.into()
		};
		// Keep the scrollable at the same tree position when the preview opens or
		// closes; dismissal must not reset the viewport around the selected file.
		let mut files = iced::widget::stack![
			style::context_area(
				iced::widget::mouse_area(files).on_press(BrowserMessage::ClearSelection.into())
			)
			.width(Length::Fill)
			.height(Length::Fill)
			.on_context_menu_with_mouse_position(
				|point| ContextMenuMessage::MenuOpen(None, point).into()
			)
		];
		if self.typeahead.is_active() {
			let preview = container(
				row![
					text("Jump to").size(10).color(style::MUTED),
					text(self.typeahead.text())
						.font(style::MONO)
						.size(12)
						.width(Length::Fill)
						.wrapping(text::Wrapping::None)
						.ellipsis(iced::advanced::text::Ellipsis::End),
					text(if self.typeahead_match().is_some() {
						"Esc"
					} else {
						"No match · Esc"
					})
					.size(10)
					.color(style::MUTED),
				]
				.spacing(10)
				.align_y(Alignment::Center),
			)
			.width(260)
			.padding([8, 12])
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
				..style::panel(style::YELLOW)
			});
			files = files.push(
				container(iced::widget::opaque(preview))
					.padding(12)
					.width(Length::Fill)
					.height(Length::Fill)
					.align_x(Alignment::End)
					.align_y(Alignment::End),
			);
		}
		content = content.push(container(files).padding([12, gutter]).height(Length::Fill));
		content = content.push(self.selection_inspector(gutter));
		let folders = visible.iter().filter(|entry| entry.is_dir).count();
		let mut status_content = row![
			text(format!(
				"{} items · {folders} folders, {} files",
				visible.len(),
				visible.len() - folders
			))
			.size(11)
			.color(style::MUTED),
			Space::new().width(Length::Fill)
		]
		.align_y(Alignment::Center);
		if let Some(usage) = self.storage {
			status_content = status_content.push(
				row![
					svg(ui_icon!("hard-drive")).width(13).height(13),
					text(format!("{} free", format_size(usage.available)))
						.font(style::MONO)
						.size(10)
						.color(style::MUTED)
				]
				.spacing(7)
				.align_y(Alignment::Center),
			);
		}
		let status = container(status_content)
			.padding([8, 16])
			.width(Length::Fill)
			.style(|_| style::panel(style::SIDEBAR));
		column![
			identity,
			style::divider(false),
			row![
				self.sidebar(
					compact,
					if compact {
						61
					} else if width <= 1100.0 {
						185
					} else {
						220
					}
				),
				style::divider(true),
				content
			]
			.height(Length::Fill),
			style::divider(false),
			status
		]
		.height(Length::Fill)
		.into()
	}

	fn toolbar(&self, compact: bool) -> Element<'_, Message> {
		let home = dirs::home_dir();
		let disk = self
			.drives
			.snapshot
			.as_ref()
			.and_then(|s| s.breadcrumb_root(&self.path));
		let (anchor, parts): (Element<'_, Message>, _) = if let Some((volume, mount)) = disk {
			let anchor = control(
				row![
					svg(ui_icon!("hard-drive")).width(17).height(17),
					text(volume.name.clone())
						.size(12)
						.weight(font::Weight::Bold),
				]
				.spacing(6)
				.align_y(Alignment::Center),
				false,
			)
			.padding(4)
			.on_press(BrowserMessage::GoTo(mount.path.clone()).into());

			(
				anchor.into(),
				navigation::mounted_breadcrumbs(&self.path, &mount.path, home.as_deref()),
			)
		} else {
			let anchor = control(svg(ui_icon!("house")).width(17).height(17), false)
				.padding(4)
				.on_press(BrowserMessage::GoTo(crate::default_path()).into());

			(
				anchor.into(),
				navigation::breadcrumbs(&self.path, home.as_deref()),
			)
		};
		let mut crumbs = row![anchor].spacing(11).align_y(Alignment::Center);

		for (label, path) in parts {
			crumbs = crumbs.push(svg(ui_icon!("caret-right")).width(12).height(12));
			crumbs = crumbs.push(
				control(
					text(label).size(12).weight(if path == self.path {
						font::Weight::Bold
					} else {
						font::Weight::Normal
					}),
					false,
				)
				.padding(4)
				.on_press(BrowserMessage::GoTo(path).into()),
			);
		}
		let navigation = row![
			control(svg(ui_icon!("arrow-left")).width(18).height(18), false)
				.on_press(BrowserMessage::GoBack.into())
				.enabled(self.history.can_undo()),
			control(svg(ui_icon!("arrow-right")).width(18).height(18), false)
				.on_press(BrowserMessage::GoForward.into())
				.enabled(self.history.can_redo()),
			control(svg(ui_icon!("arrow-up")).width(18).height(18), false)
				.on_press(BrowserMessage::GoUp.into())
				.enabled(self.path.parent().is_some()),
			scrollable(crumbs)
				.direction(iced::widget::scrollable::Direction::Horizontal(
					Default::default()
				))
				.width(Length::Fill),
			control(svg(ui_icon!("arrow-clockwise")).width(17).height(17), false)
				.on_press(BrowserMessage::Refresh.into()),
		]
		.spacing(3)
		.align_y(Alignment::Center);
		let search = text_input("Find something…", &self.search)
			.style(style::search_input)
			.id("search")
			.on_input(|query| BrowserMessage::Search(query).into())
			.on_submit(BrowserMessage::OpenSelected.into())
			.padding(0)
			.size(12);
		let mut search_content = row![
			svg(ui_icon!("magnifying-glass")).width(17).height(17),
			search
		]
		.spacing(9)
		.align_y(Alignment::Center);
		if !compact {
			search_content =
				search_content.push(text("Ctrl K").size(9).font(style::MONO).color(style::MUTED));
		}
		let search = container(search_content)
			.padding([9, 10])
			.style(|_| container::Style {
				border: iced::Border {
					color: style::INK,
					width: 1.5,
					radius: 0.0.into(),
				},
				..style::panel(style::PAPER)
			});
		if compact {
			column![navigation, search.width(Length::Fill)]
				.spacing(8)
				.into()
		} else {
			row![navigation, search.width(215)]
				.spacing(15)
				.align_y(Alignment::Center)
				.into()
		}
	}

	fn content_heading(&self, compact: bool) -> Element<'_, Message> {
		let heading = if dirs::home_dir().as_ref() == Some(&self.path) {
			std::borrow::Cow::Borrowed("Home")
		} else {
			self.path
				.file_name()
				.map_or_else(|| "/".into(), |name| name.to_string_lossy())
		};
		let title = row![
			text(heading)
				.size(if compact { 27 } else { 34 })
				.weight(font::Weight::Bold)
				.wrapping(text::Wrapping::None)
				.ellipsis(iced::advanced::text::Ellipsis::End),
			container(
				text(self.visible_entries().len())
					.size(11)
					.font(style::MONO)
			)
			.padding([4, 7])
			.style(|_| container::Style {
				border: iced::Border {
					color: style::INK,
					width: 1.0,
					radius: 0.0.into()
				},
				..style::panel(style::SIDEBAR)
			}),
		]
		.spacing(14)
		.align_y(Alignment::Center);
		let mut new_folder = row![svg(ui_icon!("plus")).width(16).height(16)].spacing(6);
		if !compact {
			new_folder = new_folder.push(text("New folder").size(12).weight(font::Weight::Bold));
		}
		let mut views = row![].spacing(0);
		for (mode, icon) in [
			(ViewMode::Grid, ui_icon!("squares-four")),
			(ViewMode::List, ui_icon!("list-bullets")),
		] {
			let selected = self.view_mode == mode;
			views = views.push(
				control(
					svg(icon)
						.width(18)
						.height(18)
						.style(move |_, _| svg::Style {
							color: Some(if selected { style::PAPER } else { style::INK }),
						}),
					false,
				)
				.width(33)
				.height(33)
				.background(if selected { style::INK } else { style::PAPER })
				.focus_color(if selected { style::INK } else { style::PAPER })
				.on_press(BrowserMessage::View(mode).into()),
			);
		}
		row![
			container(title).width(Length::Fill),
			primary(new_folder)
				.on_press(BrowserMessage::NewFolder.into())
				.enabled(!self.loading),
			container(views)
				.style(|_| container::Style {
					border: iced::Border {
						color: style::INK,
						width: 2.0,
						radius: 0.0.into()
					},
					..style::panel(style::INK)
				})
				.padding(2),
		]
		.spacing(12)
		.align_y(Alignment::Center)
		.into()
	}

	fn sidebar(&self, compact: bool, width: u16) -> Element<'_, Message> {
		let active_place = [
			dirs::home_dir(),
			dirs::document_dir(),
			dirs::download_dir(),
			dirs::picture_dir(),
		]
		.into_iter()
		.flatten()
		.filter(|p| self.path.starts_with(p))
		.max_by_key(|p| p.components().count());
		let mut places = column![].spacing(5);
		if !compact {
			places = places.push(
				container(
					text("PLACES")
						.size(10)
						.font(style::MONO)
						.color(style::MUTED),
				)
				.padding([0, 12]),
			);
		}
		for (name, icon, path) in [
			("Home", ui_icon!("house"), dirs::home_dir()),
			("Documents", ui_icon!("files"), dirs::document_dir()),
			(
				"Downloads",
				ui_icon!("download-simple"),
				dirs::download_dir(),
			),
			("Pictures", ui_icon!("image"), dirs::picture_dir()),
		] {
			if let Some(path) = path {
				let active = active_place.as_ref() == Some(&path);
				let mut label = row![svg(icon).width(21).height(21)]
					.spacing(11)
					.align_y(Alignment::Center);
				if !compact {
					label = label
						.push(text(name).size(14).weight(font::Weight::Semibold))
						.width(Length::Fill);
				}
				places = places.push(hint::place_hint(
					control(label, active)
						.background(if active { style::BLUE } else { style::SIDEBAR })
						.width(Length::Fill)
						.padding(if compact {
							iced::Padding::from(9)
						} else {
							iced::Padding::from([10, 12])
						})
						.on_press(BrowserMessage::GoTo(path).into()),
					name,
					compact,
				));
			}
		}
		if !compact {
			places = places
				.push(Space::new().height(22))
				.push(self.tag_sidebar(false));
		}
		let mut sidebar = column![scrollable(places).height(Length::FillPortion(3))]
			.spacing(16)
			.height(Length::Fill);
		if !compact {
			sidebar = sidebar.push(self.drive_cards());
		}
		let sidebar = sidebar::background(sidebar.into());
		container(sidebar)
			.padding(if compact {
				iced::Padding::from([20, 6])
			} else {
				iced::Padding {
					top: 23.0,
					left: 14.0,
					right: 14.0,
					bottom: 17.0,
				}
			})
			.width(f32::from(width))
			.height(Length::Fill)
			.style(|_| style::panel(style::SIDEBAR))
			.into()
	}

	fn file_tile<'a>(&self, entry: &'a DirEntry) -> Element<'a, Message> {
		self.entry_button(
			entry,
			column![
				container(file_art(entry, 1.0))
					.height(84)
					.center_x(Length::Fill)
					.align_y(Alignment::Center),
				text(&entry.name)
					.size(12)
					.weight(font::Weight::Bold)
					.color(if entry.is_hidden {
						style::MUTED
					} else {
						style::INK
					})
					.wrapping(text::Wrapping::None)
					.ellipsis(iced::advanced::text::Ellipsis::End),
				text(if entry.is_dir {
					"Folder".into()
				} else {
					format_size(entry.size)
				})
				.size(9)
				.font(style::MONO)
				.color(style::MUTED)
			]
			.spacing(5)
			.align_x(Alignment::Center),
			Length::Fixed(grid::TILE_HEIGHT),
		)
	}

	fn file_row<'a>(&self, entry: &'a DirEntry) -> Element<'a, Message> {
		self.entry_button(
			entry,
			row![
				container(file_art(entry, 0.4))
					.width(30)
					.height(30)
					.center(Length::Shrink),
				text(&entry.name)
					.size(13)
					.color(if entry.is_hidden {
						style::MUTED
					} else {
						style::INK
					})
					.width(Length::Fill)
					.wrapping(text::Wrapping::None)
					.ellipsis(iced::advanced::text::Ellipsis::End),
				text(if entry.is_dir {
					"Folder".into()
				} else {
					format_size(entry.size)
				})
				.size(11)
				.color(style::MUTED)
			]
			.spacing(16)
			.align_y(Alignment::Center),
			Length::Shrink,
		)
	}

	fn entry_button<'a>(
		&self, entry: &'a DirEntry, content: impl Into<Element<'a, Message>>, height: Length,
	) -> Element<'a, Message> {
		// Mouse selection and keyboard activation are deliberately separate. Enter on
		// a focused file opens it; a single mouse click only selects it.
		let selected = self.selection.paths.contains(&entry.path);
		let tile = container(content)
			.padding(8)
			.width(Length::Fill)
			.height(height);
		let content: Element<'a, Message> = if matches!(height, Length::Fixed(_)) {
			let mut badges = row![];
			if selected {
				badges = badges.push(
					container(svg(ui_icon!("check")).width(11).height(11).style(|_, _| {
						svg::Style {
							color: Some(style::PAPER),
						}
					}))
					.width(15)
					.height(15)
					.center(Length::Shrink)
					.style(|_| style::panel(style::INK)),
				);
			}
			badges = badges.push(Space::new().width(Length::Fill));
			if self
				.tags
				.as_ref()
				.is_some_and(|store| !store.data.assigned(&entry.path).is_empty())
			{
				badges = badges.push(svg(ui_icon!("tag")).width(13).height(13).style(|_, _| {
					svg::Style {
						color: Some(style::MUTED),
					}
				}));
			}
			iced::widget::stack![tile, container(badges).padding(8).width(Length::Fill)].into()
		} else {
			tile.into()
		};
		let pointer = iced::widget::mouse_area(content)
			.on_press(BrowserMessage::Clicked(entry.path.clone()).into())
			.on_double_click(BrowserMessage::OpenEntry(entry.path.clone()).into())
			.interaction(iced::mouse::Interaction::Pointer);
		control(pointer, selected)
			.padding(0)
			.id(entry.id.clone())
			.on_context_menu_with_mouse_position({
				let path = entry.path.clone();
				move |point| ContextMenuMessage::MenuOpenScrolled(path.clone(), point).into()
			})
			.on_press(BrowserMessage::OpenEntry(entry.path.clone()).into())
			.width(Length::Fill)
			.height(height)
			.into()
	}
}
