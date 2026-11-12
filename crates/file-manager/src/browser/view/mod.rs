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
		let colors = self.appearance.colors();

		if self.dialogs.owns(id) {
			return self.confirmation_view(id);
		}
		if self.transfers.owns(id) {
			return self.transfer_dialog_view(id);
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
				.style(move |_| container::Style {
					text_color: Some(colors.text),
					..style::panel(colors.body)
				})
				.into(),
			self.typeahead.is_active(),
			!self.loading
				&& !self.transfers.show_history
				&& self.menu.is_none()
				&& self.drives.menu.is_none(),
			self.visible_entries()
				.iter()
				.map(|e| (e.id.clone(), e.path.clone()))
				.collect(),
		);
		let content = crate::browser::drag::widget::gate(
			content,
			!self.transfers.busy
				&& self.transfers.loaded
				&& self.menu.is_none()
				&& self.drives.menu.is_none()
				&& self.native_drag.active_source.is_none()
				&& !self.drag.external,
			self.drag.origin.is_some() || self.drag.active,
		);
		let content = typeahead::host_gate(content, self.popups.topmost());
		let preview = if self.drag.active && !self.drag.external {
			self.drag.preview_entry().map(|entry| {
				let action = if self.modifiers.control() {
					"Copy"
				} else if self.modifiers.shift() {
					"Move"
				} else {
					"Move or copy"
				};
				container(
					row![
						file_art(entry, 0.6, colors),
						column![
							text(format!("{action} · {} items", self.drag.sources.len())).size(12),
							text(if self.drag.valid {
								"Release to transfer"
							} else {
								"Choose a destination folder"
							})
							.size(10)
							.color(colors.secondary),
						]
						.spacing(4),
					]
					.spacing(10)
					.align_y(Alignment::Center),
				)
				.padding(8)
				.style(move |_| container::Style {
					border: iced::Border {
						color: colors.border,
						width: 1.5,
						radius: 0.0.into(),
					},
					..style::panel(colors.surface)
				})
				.into()
			})
		} else {
			None
		};
		let content = if let Some(preview) = preview {
			crate::browser::drag::preview::with_preview(content, preview, self.drag.position())
		} else {
			content
		};

		// Keep the overlay at a stable tree position across modal transitions.
		let dim =
			container(Space::new().width(Length::Fill).height(Length::Fill)).style(move |_| {
				container::Style {
					background: Some(
						colors
							.black
							.scale_alpha(if self.dialogs.blocked() || self.transfers.blocked() {
								0.28
							} else {
								0.0
							})
							.into(),
					),
					..Default::default()
				}
			});
		typeahead::modal_gate(
			iced::widget::stack![content, dim].into(),
			self.dialogs.blocked() || self.transfers.blocked(),
		)
	}

	fn layout(&self, width: f32) -> Element<'_, Message> {
		let colors = self.appearance.colors();
		let compact = width <= 800.0;
		let gutter = if width <= 1100.0 { 20 } else { 29 };

		let identity = container(
			row![
				svg(phosphor_icon!("squares-four", "fill"))
					.width(23)
					.style(move |_, _| svg::Style {
						color: Some(colors.on_accent)
					}),
				text("fileberg")
					.size(21)
					.weight(font::Weight::Bold)
					.color(colors.on_accent)
			]
			.spacing(10)
			.align_y(Alignment::Center),
		)
		.padding([9, 22])
		.height(46)
		.width(Length::Fill)
		.style(move |_| style::panel(colors.decorative.yellow));
		let mut content = column![
			container(self.toolbar(compact)).padding([12, 20]),
			style::rule(colors),
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
						control(text("Dismiss").size(11), false, colors)
							.on_press(BrowserMessage::DismissError.into())
					]
					.spacing(8),
				)
				.padding([10, 20])
				.style(move |_| container::Style {
					text_color: Some(colors.on_accent),
					..style::panel(colors.decorative.pink)
				}),
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
							.style(move |theme, status| style::input(theme, status, colors)),
						primary(
							text(if self.creating_folder {
								"Creating…"
							} else {
								"Create"
							})
							.size(12)
							.color(colors.text_on(colors.decorative.yellow)),
							colors,
						)
						.on_press(BrowserMessage::CreateFolder.into())
						.enabled(!self.creating_folder),
						control(text("Cancel").size(12), false, colors)
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
							.style(move |_, _| svg::Style {
								color: Some(colors.text),
							})
						]
						.spacing(7),
						false,
						colors,
					)
					.on_press(BrowserMessage::Sort.into())
				]
				.align_y(Alignment::Center),
			)
			.padding([0, gutter]),
		);
		let visible = self.visible_entries();
		let files: Element<'_, Message> = if self.loading && self.entries.is_empty() {
			container(text("Loading…").size(13).color(colors.secondary))
				.center(Length::Fill)
				.into()
		} else if visible.is_empty() {
			container(
				column![
					svg(ui_icon!("folder-open"))
						.width(42)
						.style(move |_, _| svg::Style {
							color: Some(colors.secondary),
						}),
					text(if self.search.is_empty() && self.active_tag.is_none() {
						"This folder is empty."
					} else {
						"No files match the current filters."
					})
					.size(13)
					.color(colors.secondary)
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
		let drop_highlight = self.drag.highlights(&self.path);
		let files = container(files)
			.width(Length::Fill)
			.height(Length::Fill)
			.style(move |_| container::Style {
				border: iced::Border {
					color: colors.decorative.blue,
					width: if drop_highlight { 2.0 } else { 0.0 },
					radius: 0.0.into(),
				},
				..Default::default()
			});
		let files = crate::browser::drag::widget::zone(
			files.into(),
			crate::browser::drag::Area::Background(self.path.clone()),
		);
		// Keep the scrollable at the same tree position when the preview opens or
		// closes; dismissal must not reset the viewport around the selected file.
		let mut files = iced::widget::stack![
			style::context_area(
				iced::widget::mouse_area(files).on_press(BrowserMessage::ClearSelection.into()),
				colors,
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
					text("Jump to").size(10).color(colors.on_accent),
					text(self.typeahead.text())
						.font(style::MONO)
						.size(12)
						.color(colors.on_accent)
						.width(Length::Fill)
						.wrapping(text::Wrapping::None)
						.ellipsis(iced::advanced::text::Ellipsis::End),
					text(if self.typeahead_match().is_some() {
						"Esc"
					} else {
						"No match · Esc"
					})
					.size(10)
					.color(colors.on_accent),
				]
				.spacing(10)
				.align_y(Alignment::Center),
			)
			.width(260)
			.padding([8, 12])
			.style(move |_| container::Style {
				border: iced::Border {
					color: colors.border,
					width: 1.5,
					radius: 0.0.into(),
				},
				shadow: iced::Shadow {
					color: colors.border,
					offset: iced::Vector::new(3.0, 3.0),
					blur_radius: 0.0,
				},
				..style::panel(colors.decorative.yellow)
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
		if self.drag.active && self.drag.external {
			let action = if self.drag.external || self.modifiers.control() {
				"Copy"
			} else if self.modifiers.shift() {
				"Move"
			} else {
				"Move on same filesystem; otherwise copy"
			};
			let count = if self.drag.external {
				"Files from another application".to_string()
			} else {
				format!("{} items", self.drag.sources.len())
			};
			let feedback = container(
				text(format!(
					"{action} · {count} · {}",
					if self.drag.valid {
						"Release to transfer"
					} else {
						"Choose a destination folder"
					}
				))
				.size(12),
			)
			.padding(8)
			.style(move |_| style::panel(colors.surface));
			files = files.push(
				container(feedback)
					.padding(8)
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
			text(if self.transfers.show_history {
				format!("{} transfers", self.transfers.history.len())
			} else {
				format!(
					"{} items · {folders} folders, {} files",
					visible.len(),
					visible.len() - folders
				)
			})
			.size(11)
			.color(colors.secondary),
			Space::new().width(Length::Fill)
		]
		.align_y(Alignment::Center);
		if let Some(usage) = self.storage {
			status_content = status_content.push(
				row![
					svg(ui_icon!("hard-drive"))
						.width(13)
						.height(13)
						.style(move |_, _| svg::Style {
							color: Some(colors.secondary),
						}),
					text(format!("{} free", format_size(usage.available)))
						.font(style::MONO)
						.size(10)
						.color(colors.secondary)
				]
				.spacing(7)
				.align_y(Alignment::Center),
			);
		}
		let status = container(status_content)
			.padding([8, 16])
			.width(Length::Fill)
			.style(move |_| style::panel(colors.surface));
		let content: Element<'_, Message> = if self.transfers.show_history {
			column![
				self.transfer_history_view(),
				self.selection_inspector(gutter)
			]
			.height(Length::Fill)
			.width(Length::Fill)
			.into()
		} else {
			content.into()
		};
		column![
			identity,
			style::divider(false, colors),
			row![
				self.sidebar(
					compact && !(self.transfers.busy && self.transfers.progress_visible),
					if compact && self.transfers.busy && self.transfers.progress_visible {
						185
					} else if compact {
						61
					} else if width <= 1100.0 {
						185
					} else {
						220
					}
				),
				style::divider(true, colors),
				content
			]
			.height(Length::Fill),
			style::divider(false, colors),
			status
		]
		.height(Length::Fill)
		.into()
	}

	fn toolbar(&self, compact: bool) -> Element<'_, Message> {
		let colors = self.appearance.colors();

		let home = dirs::home_dir();
		let disk = self
			.drives
			.snapshot
			.as_ref()
			.and_then(|s| s.breadcrumb_root(&self.path));
		let (anchor, parts): (Element<'_, Message>, _) = if let Some((volume, mount)) = disk {
			let anchor = control(
				row![
					svg(ui_icon!("hard-drive"))
						.width(17)
						.height(17)
						.style(move |_, _| svg::Style {
							color: Some(colors.text),
						}),
					text(volume.name.clone())
						.size(12)
						.weight(font::Weight::Bold),
				]
				.spacing(6)
				.align_y(Alignment::Center),
				false,
				colors,
			)
			.padding(4)
			.on_press(BrowserMessage::GoTo(mount.path.clone()).into());

			(
				anchor.into(),
				navigation::mounted_breadcrumbs(&self.path, &mount.path, home.as_deref()),
			)
		} else {
			let anchor = control(
				svg(ui_icon!("house"))
					.width(17)
					.height(17)
					.style(move |_, _| svg::Style {
						color: Some(colors.text),
					}),
				false,
				colors,
			)
			.padding(4)
			.on_press(BrowserMessage::GoTo(crate::default_path()).into());

			(
				anchor.into(),
				navigation::breadcrumbs(&self.path, home.as_deref()),
			)
		};
		let mut crumbs = row![anchor].spacing(11).align_y(Alignment::Center);

		for (label, path) in parts {
			crumbs = crumbs.push(svg(ui_icon!("caret-right")).width(12).height(12).style(
				move |_, _| svg::Style {
					color: Some(colors.secondary),
				},
			));
			crumbs = crumbs.push(
				control(
					text(label).size(12).weight(if path == self.path {
						font::Weight::Bold
					} else {
						font::Weight::Normal
					}),
					false,
					colors,
				)
				.padding(4)
				.on_press(BrowserMessage::GoTo(path).into()),
			);
		}
		let navigation = row![
			control(
				svg(ui_icon!("arrow-left"))
					.width(18)
					.height(18)
					.style(move |_, _| svg::Style {
						color: Some(colors.text),
					}),
				false,
				colors
			)
			.on_press(BrowserMessage::GoBack.into())
			.enabled(self.history.can_undo()),
			control(
				svg(ui_icon!("arrow-right"))
					.width(18)
					.height(18)
					.style(move |_, _| svg::Style {
						color: Some(colors.text),
					}),
				false,
				colors
			)
			.on_press(BrowserMessage::GoForward.into())
			.enabled(self.history.can_redo()),
			control(
				svg(ui_icon!("arrow-up"))
					.width(18)
					.height(18)
					.style(move |_, _| svg::Style {
						color: Some(colors.text),
					}),
				false,
				colors
			)
			.on_press(BrowserMessage::GoUp.into())
			.enabled(self.path.parent().is_some()),
			scrollable(crumbs)
				.direction(iced::widget::scrollable::Direction::Horizontal(
					Default::default()
				))
				.width(Length::Fill),
			control(
				svg(ui_icon!("arrow-clockwise"))
					.width(17)
					.height(17)
					.style(move |_, _| svg::Style {
						color: Some(colors.text),
					}),
				false,
				colors
			)
			.on_press(BrowserMessage::Refresh.into()),
		]
		.spacing(3)
		.align_y(Alignment::Center);
		let search = text_input("Find something…", &self.search)
			.style(move |theme, status| style::search_input(theme, status, colors))
			.id("search")
			.on_input(|query| BrowserMessage::Search(query).into())
			.on_submit(BrowserMessage::OpenSelected.into())
			.padding(0)
			.size(12);
		let mut search_content = row![
			svg(ui_icon!("magnifying-glass"))
				.width(17)
				.height(17)
				.style(move |_, _| svg::Style {
					color: Some(colors.text),
				}),
			search
		]
		.spacing(9)
		.align_y(Alignment::Center);
		if !compact {
			search_content = search_content.push(
				text("Ctrl K")
					.size(9)
					.font(style::MONO)
					.color(colors.secondary),
			);
		}
		let search = container(search_content)
			.padding([9, 10])
			.style(move |_| container::Style {
				border: iced::Border {
					color: colors.border,
					width: 1.5,
					radius: 0.0.into(),
				},
				..style::panel(colors.body)
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
		let colors = self.appearance.colors();

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
			.style(move |_| container::Style {
				border: iced::Border {
					color: colors.border,
					width: 1.0,
					radius: 0.0.into()
				},
				..style::panel(colors.surface)
			}),
		]
		.spacing(14)
		.align_y(Alignment::Center);
		let new_folder_color = colors.text_on(colors.decorative.yellow);
		let mut new_folder = row![
			svg(ui_icon!("plus"))
				.width(16)
				.height(16)
				.style(move |_, _| svg::Style {
					color: Some(new_folder_color),
				})
		]
		.spacing(6);
		if !compact {
			new_folder = new_folder.push(
				text("New folder")
					.size(12)
					.weight(font::Weight::Bold)
					.color(new_folder_color),
			);
		}
		let mut views = row![].spacing(0);
		for (mode, icon) in [
			(ViewMode::Grid, ui_icon!("squares-four")),
			(ViewMode::List, ui_icon!("list-bullets")),
		] {
			let selected = self.view_mode == mode;
			let background = if selected { colors.text } else { colors.body };
			views = views.push(
				control(
					svg(icon)
						.width(18)
						.height(18)
						.style(move |_, _| svg::Style {
							color: Some(colors.text_on(background)),
						}),
					false,
					colors,
				)
				.width(33)
				.height(33)
				.background(background)
				.focus_color(background)
				.on_press(BrowserMessage::View(mode).into()),
			);
		}
		row![
			container(title).width(Length::Fill),
			primary(new_folder, colors)
				.on_press(BrowserMessage::NewFolder.into())
				.enabled(!self.loading),
			container(views)
				.style(move |_| container::Style {
					border: iced::Border {
						color: colors.border,
						width: 2.0,
						radius: 0.0.into()
					},
					..style::panel(colors.text)
				})
				.padding(2),
		]
		.spacing(12)
		.align_y(Alignment::Center)
		.into()
	}

	fn sidebar(&self, compact: bool, width: u16) -> Element<'_, Message> {
		let colors = self.appearance.colors();

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
						.color(colors.secondary),
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
				let active = !self.transfers.show_history && active_place.as_ref() == Some(&path);
				let background = if self.drag.highlights(&path) {
					colors.decorative.green
				} else if active {
					colors.decorative.blue
				} else {
					colors.surface
				};
				let foreground = colors.text_on(background);
				let mut label =
					row![
						svg(icon)
							.width(21)
							.height(21)
							.style(move |_, _| svg::Style {
								color: Some(foreground),
							})
					]
					.spacing(11)
					.align_y(Alignment::Center);
				if !compact {
					label = label
						.push(
							text(name)
								.size(14)
								.weight(font::Weight::Semibold)
								.color(foreground),
						)
						.width(Length::Fill);
				}
				places = places.push(crate::browser::drag::widget::zone(
					hint::place_hint(
						control(label, active, colors)
							.background(background)
							.width(Length::Fill)
							.padding(if compact {
								iced::Padding::from(9)
							} else {
								iced::Padding::from([10, 12])
							})
							.on_press(BrowserMessage::GoTo(path.clone()).into()),
						name,
						compact,
						colors,
					),
					crate::browser::drag::Area::Sidebar(path),
				));
			}
		}
		let background = if self.transfers.show_history {
			colors.decorative.blue
		} else {
			colors.surface
		};
		let foreground = colors.text_on(background);
		let mut history_label = row![
			svg(ui_icon!("clock-counter-clockwise"))
				.width(21)
				.height(21)
				.style(move |_, _| svg::Style {
					color: Some(foreground)
				}),
		]
		.spacing(11)
		.align_y(Alignment::Center);
		if !compact {
			history_label = history_label
				.push(
					text("History")
						.size(14)
						.weight(font::Weight::Semibold)
						.color(foreground),
				)
				.width(Length::Fill);
		}
		places = places.push(hint::place_hint(
			control(history_label, self.transfers.show_history, colors)
				.id("transfer-history-tab")
				.background(background)
				.width(Length::Fill)
				.padding(if compact {
					iced::Padding::from(9)
				} else {
					iced::Padding::from([10, 12])
				})
				.on_press(crate::transfers::Message::ShowHistory(true).into()),
			"History",
			compact,
			colors,
		));
		if !compact {
			places = places
				.push(Space::new().height(22))
				.push(self.tag_sidebar(false));
		}
		let mut sidebar = column![
			scrollable(places)
				.id("fileberg-sidebar")
				.height(Length::FillPortion(3))
		]
		.spacing(16)
		.height(Length::Fill);
		if self.transfers.busy && self.transfers.progress_visible {
			sidebar = sidebar.push(self.sidebar_transfers());
		}
		if !compact {
			sidebar = sidebar.push(self.drive_cards());
		}
		let sidebar = sidebar::background(sidebar.into(), colors);
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
			.style(move |_| style::panel(colors.surface))
			.into()
	}

	fn file_tile<'a>(&self, entry: &'a DirEntry) -> Element<'a, Message> {
		let colors = self.entry_colors(entry);

		self.entry_button(
			entry,
			column![
				container(file_art(entry, 1.0, colors))
					.height(84)
					.center_x(Length::Fill)
					.align_y(Alignment::Center),
				text(&entry.name)
					.size(12)
					.weight(font::Weight::Bold)
					.color(if entry.is_hidden {
						colors.secondary
					} else {
						colors.text
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
				.color(colors.secondary)
			]
			.spacing(5)
			.align_x(Alignment::Center),
			Length::Fixed(grid::TILE_HEIGHT),
		)
	}

	fn file_row<'a>(&self, entry: &'a DirEntry) -> Element<'a, Message> {
		let colors = self.entry_colors(entry);

		self.entry_button(
			entry,
			row![
				container(file_art(entry, 0.4, colors))
					.width(30)
					.height(30)
					.center(Length::Shrink),
				text(&entry.name)
					.size(13)
					.color(if entry.is_hidden {
						colors.secondary
					} else {
						colors.text
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
				.color(colors.secondary)
			]
			.spacing(16)
			.align_y(Alignment::Center),
			Length::Shrink,
		)
	}

	fn entry_button<'a>(
		&self, entry: &'a DirEntry, content: impl Into<Element<'a, Message>>, height: Length,
	) -> Element<'a, Message> {
		let colors = self.appearance.colors();

		// Mouse selection and keyboard activation are deliberately separate. Enter on
		// a focused file opens it; a single mouse click only selects it.
		let selected = self.selection.paths.contains(&entry.path);
		let prepared = self.transfers.prepared_operation(&entry.path);
		let tile = container(content)
			.padding(8)
			.width(Length::Fill)
			.height(height);
		let content: Element<'a, Message> = if matches!(height, Length::Fixed(_)) {
			let mut badges = row![];
			if selected {
				let check = svg(ui_icon!("check"))
					.width(11)
					.height(11)
					.style(move |_, _| svg::Style {
						color: Some(colors.on_accent),
					});

				badges = badges.push(
					container(check)
						.width(15)
						.height(15)
						.center(Length::Shrink)
						.style(move |_| style::panel(colors.text)),
				);
			}
			badges = badges.push(Space::new().width(Length::Fill));
			if self
				.tags
				.as_ref()
				.is_some_and(|store| !store.data.assigned(&entry.path).is_empty())
			{
				badges = badges.push(svg(ui_icon!("tag")).width(13).height(13).style(
					move |_, _| svg::Style {
						color: Some(colors.secondary),
					},
				));
			}
			iced::widget::stack![tile, container(badges).padding(8).width(Length::Fill)].into()
		} else {
			tile.into()
		};
		let content = if prepared == Some(crate::transfers::Operation::Move) {
			iced::widget::stack![
				content,
				container(Space::new())
					.width(Length::Fill)
					.height(Length::Fill)
					.style(move |_| style::panel(colors.body.scale_alpha(0.45))),
			]
			.into()
		} else {
			content
		};
		let pointer = iced::widget::mouse_area(content)
			.on_press(BrowserMessage::Clicked(entry.path.clone()).into())
			.on_double_click(BrowserMessage::OpenEntry(entry.path.clone()).into())
			.interaction(iced::mouse::Interaction::Pointer);
		let target = self.drag.highlights(&entry.path);
		crate::browser::drag::widget::zone(
			control(
				pointer,
				selected || target || prepared == Some(crate::transfers::Operation::Copy),
				colors,
			)
			.padding(0)
			.id(entry.id.clone())
			.on_context_menu_with_mouse_position({
				let path = entry.path.clone();
				move |point| ContextMenuMessage::MenuOpenScrolled(path.clone(), point).into()
			})
			.on_press(BrowserMessage::OpenEntry(entry.path.clone()).into())
			.width(Length::Fill)
			.height(height)
			.into(),
			crate::browser::drag::Area::Entry {
				path: entry.path.clone(),
				directory: entry.is_dir,
			},
		)
	}

	fn entry_colors(&self, entry: &DirEntry) -> neo_widgets::style::Colors {
		let mut colors = self.appearance.colors();
		if self.transfers.prepared_operation(&entry.path) == Some(crate::transfers::Operation::Move)
		{
			colors.text = colors.secondary;
			colors.decorative.blue = colors.secondary;
			colors.decorative.green = colors.secondary;
			colors.decorative.yellow = colors.secondary;
			colors.decorative.pink = colors.secondary;
			colors.decorative.blue90 = colors.body;
			colors.decorative.green90 = colors.body;
			colors.decorative.coral90 = colors.body;
			colors.decorative.purple90 = colors.body;
		}
		colors
	}
}
