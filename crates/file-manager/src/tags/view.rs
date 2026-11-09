use crate::tags::Message as TagMessage;
use crate::{
	FileManager, Message,
	filesystem::entry::DirEntry,
	tags::TagChoice,
	ui::icons::ui_icon,
	ui::style::{self, control, primary},
};
use iced::{
	Alignment, Element, Length,
	widget::{Space, column, container, pick_list, row, svg, text, text_input},
};

impl FileManager {
	pub(crate) fn tag_sidebar(&self, compact: bool) -> Element<'_, Message> {
		let mut tree = column![].spacing(5);
		let add = control(svg(ui_icon!("plus")).width(15).height(15), false)
			.background(style::SIDEBAR)
			.on_press(TagMessage::EditTag(None).into())
			.enabled(self.tags.is_some() && !self.tag_busy);
		if !compact {
			tree = tree.push(
				row![
					text("TAGS").size(10).font(style::MONO).color(style::MUTED),
					Space::new().width(Length::Fill),
					add
				]
				.align_y(Alignment::Center),
			);
		} else {
			tree = tree.push(add);
		}
		if let Some(store) = &self.tags {
			let mut ordered = store.data.ordered();
			ordered.sort_by_key(|(id, _)| {
				let mut key = vec![*id];
				let mut parent = store.data.tags.get(id).and_then(|t| t.parent);
				while let Some(id) = parent {
					key.push(id);
					parent = store.data.tags.get(&id).and_then(|t| t.parent);
				}
				key.reverse();
				key
			});
			for (id, depth) in ordered {
				let Some(tag) = store.data.tags.get(&id) else {
					continue;
				};
				let symbol = if tag.name == "Starred" {
					ui_icon!("star")
				} else {
					ui_icon!("tag")
				};
				let color = if tag.name == "Starred" {
					style::INK
				} else if store.data.label(id).starts_with("Work") {
					neo_widgets::style::rgb(0x886_91b)
				} else {
					neo_widgets::style::rgb(0x557_4a5)
				};
				let mut label = row![
					svg(symbol)
						.width(16)
						.height(16)
						.style(move |_, _| svg::Style { color: Some(color) })
				]
				.spacing(8)
				.align_y(Alignment::Center);
				if !compact {
					label = label.width(Length::Fill).push(
						text(&tag.name)
							.size(12)
							.wrapping(text::Wrapping::None)
							.ellipsis(iced::advanced::text::Ellipsis::End),
					);
				}
				let filter = control(label, self.active_tag == Some(id))
					.background(if self.active_tag == Some(id) {
						style::YELLOW
					} else {
						style::SIDEBAR
					})
					.padding([7, 10])
					.shadow_width(0.0)
					.on_press(
						TagMessage::TagFilter(if self.active_tag == Some(id) {
							None
						} else {
							Some(id)
						})
						.into(),
					)
					.width(Length::Fill);
				if compact {
					tree = tree.push(crate::browser::view::hint::place_hint(
						filter,
						store.data.label(id),
						true,
					));
				} else {
					let item = row![
						filter,
						control(
							svg(ui_icon!("pencil-simple"))
								.width(12)
								.height(12)
								.style(|_, _| svg::Style {
									color: Some(style::MUTED)
								}),
							false
						)
						.background(style::SIDEBAR)
						.padding(4)
						.on_press(TagMessage::EditTag(Some(id)).into())
						.enabled(!self.tag_busy)
					]
					.spacing(2)
					.align_y(Alignment::Center);
					if depth == 0 {
						tree = tree.push(item);
					} else {
						tree = tree.push(
							row![
								iced::widget::Space::new().width((depth.min(5) * 20) as f32),
								container(iced::widget::Space::new().width(1).height(32))
									.style(|_| style::panel(neo_widgets::style::rgb(0xc6c_8bb))),
								item
							]
							.spacing(8),
						);
					}
				}
			}
		}
		if self.tag_busy && !compact {
			tree = tree.push(
				text(if self.tags.is_some() {
					"Saving tags…"
				} else {
					"Loading tags…"
				})
				.size(11)
				.color(style::MUTED),
			);
		}
		if let Some(problem) = &self.tag_problem {
			if !compact {
				tree = tree.push(text(problem).size(11));
			}
			if self.tags.is_none() {
				tree = tree.push(
					control(text("Retry").size(11), false)
						.on_press(TagMessage::RetryTags.into())
						.enabled(!self.tag_busy),
				);
			}
		}
		tree.into()
	}

	pub(crate) fn tag_form(&self) -> Option<Element<'_, Message>> {
		let editor = self.tag_editor.as_ref()?;
		let store = self.tags.as_ref()?;
		let mut choices = vec![self.tag_choice(None)];
		choices.extend(
			store
				.data
				.ordered()
				.into_iter()
				.filter(|(id, _)| {
					editor
						.id
						.is_none_or(|edited| !store.data.descends_from(*id, edited))
				})
				.map(|(id, _)| self.tag_choice(Some(id))),
		);
		let form = column![
			text(if editor.id.is_some() {
				"EDIT TAG"
			} else {
				"NEW TAG"
			})
			.size(10)
			.color(style::MUTED),
			text_input("Tag name", &editor.name)
				.style(style::input)
				.id("tag-name")
				.on_input(|name| TagMessage::TagName(name).into())
				.on_submit(TagMessage::SaveTag.into())
				.padding(9)
				.size(13),
			row![
				text("Parent").size(12),
				pick_list(Some(editor.parent.clone()), choices, ToString::to_string)
					.style(style::pick_list)
					.on_select(|parent| TagMessage::TagParent(parent).into())
					.text_size(12)
					.width(Length::Fill)
			]
			.spacing(10)
			.align_y(Alignment::Center),
			row![
				primary(
					text(if self.tag_busy {
						"Saving…"
					} else {
						"Save tag"
					})
					.size(12)
				)
				.on_press(TagMessage::SaveTag.into())
				.enabled(!self.tag_busy),
				control(text("Cancel").size(12), false).on_press(TagMessage::CancelTag.into())
			]
			.spacing(8),
		]
		.spacing(9);
		Some(
			container(form)
				.padding(14)
				.width(Length::Fill)
				.style(|_| style::panel(style::SIDEBAR))
				.into(),
		)
	}

	pub(crate) fn selected_tags(&self, entry: &DirEntry) -> Element<'_, Message> {
		let mut content = column![
			row![
				svg(ui_icon!("tag")).width(15).height(15),
				text("TAGS").font(style::MONO).size(10).color(style::MUTED)
			]
			.spacing(7)
		]
		.spacing(9);
		if let Some(store) = &self.tags {
			let assigned = store.data.assigned(&entry.path);
			let mut chips = row![].spacing(7);
			for &id in &assigned {
				chips = chips.push(
					control(
						row![
							svg(ui_icon!("tag")).width(13),
							text(store.data.label(id)).size(11),
							svg(ui_icon!("x")).width(12)
						]
						.spacing(6)
						.align_y(Alignment::Center),
						true,
					)
					.shadow_width(0.0)
					.background(neo_widgets::style::rgb(0xe2e_8f5))
					.padding([4, 7])
					.on_press(TagMessage::RemoveTag(id).into())
					.enabled(!self.tag_busy),
				);
			}
			if assigned.is_empty() {
				chips = chips.push(text("No tags").size(12).color(style::MUTED));
			}
			let choices: Vec<TagChoice> = store
				.data
				.ordered()
				.into_iter()
				.filter(|(id, _)| !assigned.contains(id))
				.map(|(id, _)| self.tag_choice(Some(id)))
				.collect();
			if !self.tag_busy && !choices.is_empty() {
				chips = chips.push(
					pick_list(None::<TagChoice>, choices, ToString::to_string)
						.style(style::pick_list)
						.on_select(|choice| TagMessage::AssignTag(choice).into())
						.placeholder("+ Add tag")
						.text_size(12),
				);
			}
			content = content.push(chips.wrap());
		} else {
			content = content.push(text("Tag storage is unavailable.").size(12));
		}
		container(content)
			.padding([12, 14])
			.width(Length::Fill)
			.style(|_| container::Style {
				border: iced::Border {
					color: style::INK,
					width: 1.5,
					radius: 0.0.into(),
				},
				..style::panel(style::SIDEBAR)
			})
			.into()
	}

	pub(crate) fn tag_filter_label(&self) -> Element<'_, Message> {
		if let Some(id) = self.active_tag {
			let name = self
				.tags
				.as_ref()
				.map_or_else(String::new, |s| s.data.label(id));
			row![
				text("IN THIS FOLDER").size(10).color(style::MUTED),
				primary(row![text(name).size(11), svg(ui_icon!("x")).width(12)].spacing(8))
					.shadow_width(0.0)
					.on_press(TagMessage::TagFilter(None).into())
			]
			.spacing(9)
			.align_y(Alignment::Center)
			.into()
		} else {
			text("FILES").size(10).color(style::MUTED).into()
		}
	}
}
