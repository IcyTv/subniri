//! Search, ordering, and row data. Availability belongs to the owning action.
use super::{Action, Command};
use crate::FileManager;
use iced::window;
use utilities::matching::{self, SkimMatcherV2, TypoOptions};

#[derive(Clone)]
pub struct Row {
	pub action: Action,
	pub label: String,
	pub disabled: Option<&'static str>,
	pub checked: bool,
}

impl FileManager {
	pub fn menu_rows(&self, id: window::Id) -> Vec<Row> {
		let Some(menu) = &self.menu else {
			return Vec::new();
		};
		let Some(page) = menu.page(id) else {
			return Vec::new();
		};
		let searching = !page.query.is_empty();
		let mut rows = Vec::new();
		for command in Command::ALL {
			let root_visible =
				command.menu_parent().is_none() || self.favorites.order.contains(&command);
			if (searching && command.menu_parent().is_none())
				|| (!searching && page.parent_action.is_none() && root_visible)
			{
				rows.push(self.menu_row(Action::Command(command), command.label().into()));
			}
			if searching || page.parent_action == Some(command) {
				for (action, label) in command.choices(self) {
					rows.push(self.menu_row(
						action,
						if searching {
							format!("{} › {label}", command.label().trim_end_matches('…'))
						} else {
							label
						},
					));
				}
			}
		}
		if !searching {
			rows.retain(|r| r.disabled.is_none());
			if page.parent_action.is_none() {
				rows.sort_by_key(|r| {
					self.favorites
						.order
						.iter()
						.position(|c| r.action == Action::Command(*c))
						.unwrap_or(usize::MAX)
				});
			}
			return rows;
		}
		let matcher = SkimMatcherV2::default().ignore_case();
		let typo = TypoOptions::default();
		let mut scored: Vec<_> = rows
			.into_iter()
			.filter_map(|row| {
				let mut candidates = vec![row.label.as_str()];
				if let Action::Command(c) = row.action {
					candidates.extend(c.aliases().split_whitespace());
				}
				let best = candidates
					.into_iter()
					.filter_map(|label| {
						matching::analyze(label, &page.query, &matcher, Some(&typo))
					})
					.max_by_key(|m| (m.kind.priority(), m.score))?;
				Some((row, best.kind.priority(), best.score))
			})
			.collect();
		scored.sort_by(|a, b| {
			b.1.cmp(&a.1)
				.then_with(|| b.2.cmp(&a.2))
				.then_with(|| a.0.label.cmp(&b.0.label))
				.then_with(|| a.0.action.key().cmp(&b.0.action.key()))
		});
		scored.into_iter().map(|r| r.0).collect()
	}

	fn menu_row(&self, action: Action, label: String) -> Row {
		let Some(menu) = self.menu.as_ref() else {
			return Row {
				action,
				label,
				disabled: Some("Context menu closed"),
				checked: false,
			};
		};
		Row {
			action,
			label,
			disabled: action.unavailable(self, &menu.target, menu.available),
			checked: action.checked(self, &menu.target),
		}
	}
}
