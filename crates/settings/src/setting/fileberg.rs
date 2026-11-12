use config::{ConfigFile, FilebergDurability};
use iced::{
	Alignment, Border, Color, Element, Length, Task, font,
	widget::{Svg, column, container, row, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::Colors,
	widgets::{neo_button, neo_card},
};

const POLICIES: [(FilebergDurability, &str, &str); 3] = [
	(
		FilebergDurability::Auto,
		"Auto (recommended)",
		"Use buffered completion only for proven fixed local storage. Flush removable, remote, and unknown destinations before reporting completion.",
	),
	(
		FilebergDurability::Durable,
		"Durable",
		"Flush every destination before reporting completion. This can take longer, especially on removable or remote storage.",
	),
	(
		FilebergDurability::Buffered,
		"Buffered",
		"Report completion after writing to the operating system's cache. Data may still be pending; eject removable drives safely before disconnecting.",
	),
];

#[derive(Clone, Copy, Debug)]
pub enum Message {
	SelectDurability(FilebergDurability),
	UpdateConfig(FilebergDurability),
}

pub struct Fileberg;

/// Apply only Snowconf's edit to the latest config, preserving Fileberg's favorites.
pub(super) fn apply_durability(config: &mut ConfigFile, policy: FilebergDurability) {
	config.fileberg.transfer_durability = policy;
}

fn reduce(config: &mut ConfigFile, message: Message) -> Option<FilebergDurability> {
	match message {
		Message::SelectDurability(policy) => {
			apply_durability(config, policy);
			Some(policy)
		}
		Message::UpdateConfig(_) => None,
	}
}

impl Fileberg {
	pub fn accent_color(colors: Colors) -> Color {
		colors.decorative.blue
	}

	pub fn icon<'a>() -> Svg<'a> {
		svg(phosphor_icon!("folder"))
	}

	pub fn update(&mut self, config: &mut ConfigFile, message: Message) -> Task<Message> {
		reduce(config, message).map_or_else(Task::none, |policy| {
			Task::done(Message::UpdateConfig(policy))
		})
	}

	pub fn view<'a>(&'a self, config: &'a ConfigFile, colors: Colors) -> Element<'a, Message> {
		let mut options = column![].spacing(12);
		for (policy, label, explanation) in POLICIES {
			let selected = config.fileberg.transfer_durability == policy;
			options = options.push(
				neo_button(
					column![
						text(if selected {
							format!("{label} (selected)")
						} else {
							label.to_owned()
						})
						.size(18)
						.weight(font::Weight::Bold)
						.color(colors.text),
						text(explanation)
							.size(13)
							.color(colors.text.scale_alpha(0.7))
							.wrapping(text::Wrapping::Word),
					]
					.spacing(4),
				)
				.with_colors(colors)
				.focusable(true)
				.width(Length::Fill)
				.padding(crate::sizing::CARD_PADDING)
				.background(if selected {
					colors.decorative.blue90
				} else {
					colors.surface
				})
				.on_press(Message::SelectDurability(policy)),
			);
		}

		column![
			neo_card(
				row![
					container(Self::icon())
						.width(crate::sizing::HEADER_ICON)
						.height(crate::sizing::HEADER_ICON)
						.padding(crate::sizing::HEADER_ICON_PADDING)
						.style(move |_| container::Style {
							background: Some(colors.surface.into()),
							border: Border {
								color: colors.border,
								width: 2.0,
								radius: 3.into(),
							},
							..Default::default()
						}),
					column![
						text("FILEBERG")
							.width(Length::Fill)
							.color(colors.on_accent)
							.size(crate::sizing::HEADER_TITLE)
							.weight(font::Weight::Bold)
							.stretch(font::Stretch::ExtraExpanded),
						text("Choose when file transfers count as complete")
							.color(colors.on_accent.scale_alpha(0.76))
							.size(14)
							.weight(font::Weight::Bold),
					]
					.spacing(4),
				]
				.align_y(Alignment::Center)
				.spacing(16),
			)
			.with_colors(colors)
			.width(Length::Fill)
			.height(crate::sizing::HEADER_HEIGHT)
			.background(Self::accent_color(colors))
			.padding(crate::sizing::HEADER_PADDING),
			neo_card(
				column![
					text("Transfer completion")
						.color(colors.text)
						.size(18)
						.weight(font::Weight::Bold),
					text("Global default for new copy and move operations. Advanced transfer actions can choose a policy for one operation. Transfers already running keep their policy.")
						.color(colors.text.scale_alpha(0.7))
						.size(13)
						.wrapping(text::Wrapping::Word),
					options,
				]
				.spacing(12),
			)
			.with_colors(colors)
			.width(Length::Fill)
			.padding(16),
		]
		.spacing(16)
		.into()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn reducer_changes_only_durability() {
		let mut config = ConfigFile::default();
		config.fileberg.favorites = vec!["copy".into(), "paste".into()];
		config.nightlight.enabled = true;
		let before = config.clone();

		let save = reduce(
			&mut config,
			Message::SelectDurability(FilebergDurability::Durable),
		);

		assert_eq!(save, Some(FilebergDurability::Durable));
		assert_eq!(
			config.fileberg.transfer_durability,
			FilebergDurability::Durable
		);
		config.fileberg.transfer_durability = before.fileberg.transfer_durability;
		assert_eq!(config, before);
	}

	#[test]
	fn save_snapshot_preserves_concurrently_changed_favorites_and_unrelated_fields() {
		let mut stale = ConfigFile::default();
		stale.fileberg.transfer_durability = FilebergDurability::Durable;
		let policy = reduce(
			&mut stale,
			Message::SelectDurability(FilebergDurability::Auto),
		)
		.unwrap();

		let mut fresh = ConfigFile::default();
		fresh.fileberg.favorites = vec!["paste".into(), "copy".into()];
		fresh.nightlight.enabled = true;
		fresh.fileberg.transfer_durability = FilebergDurability::Buffered;
		let mut expected = fresh.clone();
		expected.fileberg.transfer_durability = FilebergDurability::Auto;
		stale.fileberg.transfer_durability = FilebergDurability::Buffered;

		apply_durability(&mut fresh, policy);

		assert_eq!(fresh, expected);
		assert!(reduce(&mut fresh, Message::UpdateConfig(policy)).is_none());
		assert_eq!(fresh, expected);
	}

	#[test]
	fn selector_exposes_auto_and_both_explicit_policies() {
		let config = ConfigFile::default();
		let snapshot: Vec<_> = POLICIES
			.iter()
			.map(|(policy, label, _)| (*label, *policy == config.fileberg.transfer_durability))
			.collect();

		assert_eq!(
			snapshot,
			vec![
				("Auto (recommended)", true),
				("Durable", false),
				("Buffered", false)
			]
		);
		assert!(
			POLICIES
				.iter()
				.all(|(_, _, explanation)| !explanation.is_empty())
		);
	}

	#[test]
	fn selected_policy_uses_shared_nix_diff_and_returns_to_declarative_value() {
		let declarative = ConfigFile::default();
		let mut effective = declarative.clone();

		reduce(
			&mut effective,
			Message::SelectDurability(FilebergDurability::Durable),
		);

		assert_eq!(
			effective.nix_diff(&declarative),
			"services.subniri.settings.fileberg.transfer_durability = \"durable\";\n"
		);

		reduce(
			&mut effective,
			Message::SelectDurability(FilebergDurability::Auto),
		);

		assert!(effective.nix_diff(&declarative).is_empty());
	}
}
