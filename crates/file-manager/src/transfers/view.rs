use super::{
	Choice, Operation, Phase, Status,
	app::{DialogContent, Message},
};
use crate::{FileManager, ui::style};
use iced::{
	Alignment, Element, Length,
	widget::{Space, column, container, mouse_area, progress_bar, row, scrollable},
};

const FONT: iced::Font = iced::Font::new("DM Sans");

impl FileManager {
	pub(crate) fn transfer_controls(&self) -> Element<'_, crate::Message> {
		let colors = self.appearance.colors();
		let state = &self.transfers;
		let foreground = colors.text_on(colors.black);
		if state.busy {
			return row![
				text(phase_label(state.phase.unwrap_or(Phase::Preparing)))
					.size(11)
					.color(foreground)
					.width(Length::Fill),
				style::dark_control(text("Cancel").size(11).color(foreground), colors)
					.on_press(Message::Cancel.into())
					.enabled(state.phase != Some(Phase::RollingBack)),
			]
			.spacing(8)
			.align_y(Alignment::Center)
			.into();
		}
		let label = if let Some(pending) = &state.pending {
			let action = match pending.operation {
				Operation::Copy => "Copying",
				Operation::Move => "Moving",
				Operation::Automatic => "Transferring",
			};
			let count = pending.target.others.len() + 1;
			format!(
				"{action} {count} {} to {}",
				if count == 1 { "item" } else { "items" },
				self.path.display()
			)
		} else {
			return row![
				text("Receiving dropped files…")
					.size(11)
					.color(foreground)
					.width(Length::Fill),
				style::dark_control(text("Cancel receipt").size(11).color(foreground), colors)
					.id("transfer-cancel-receipt")
					.on_press(super::native::Message::CancelPending.into()),
			]
			.spacing(8)
			.align_y(Alignment::Center)
			.into();
		};
		row![
			text(label)
				.size(11)
				.color(foreground)
				.width(Length::Fill)
				.wrapping(iced::widget::text::Wrapping::None)
				.ellipsis(iced::advanced::text::Ellipsis::End),
			style::dark_control(text("Paste").size(11).color(foreground), colors)
				.id("transfer-paste")
				.on_press(Message::Paste(self.path.clone()).into())
				.enabled(state.can_paste()),
			style::dark_control(text("Cancel").size(11).color(foreground), colors)
				.id("transfer-cancel-prepared")
				.on_press(Message::CancelPrepared.into()),
		]
		.spacing(8)
		.align_y(Alignment::Center)
		.into()
	}

	pub(crate) fn sidebar_transfers(&self) -> Element<'_, crate::Message> {
		let colors = self.appearance.colors();
		let state = &self.transfers;
		if !state.busy || !state.progress_visible {
			return Space::new().into();
		}
		let phase = state.phase.unwrap_or(Phase::Preparing);
		let total = state.total_bytes;
		let bytes = crate::filesystem::entry::format_size(state.bytes);
		let amount = total.map_or_else(
			|| format!("{bytes} / unknown"),
			|total| format!("{bytes} / {}", crate::filesystem::entry::format_size(total)),
		);
		let eta = if phase == Phase::Copying {
			state.eta().map_or_else(
				|| "Estimating…".to_owned(),
				|eta| {
					let seconds = eta.as_secs();
					format!("ETA {}m {}s", seconds / 60, seconds % 60)
				},
			)
		} else if matches!(phase, Phase::Preparing) {
			"Estimating…".to_owned()
		} else {
			"ETA unavailable".to_owned()
		};
		// The bar represents processed file bytes, not durability or commit completion.
		let fraction = total.map_or(0.0, |total| {
			if total == 0 {
				0.0
			} else {
				(state.bytes as f64 / total as f64).min(1.0) as f32
			}
		});
		let card = column![
			text(phase_label(phase)).size(12),
			progress_bar(0.0..=1.0, fraction).girth(6).style(move |_| {
				iced::widget::progress_bar::Style {
					background: colors.body.into(),
					bar: colors.decorative.blue.into(),
					border: iced::Border::default(),
				}
			}),
			text("Bytes completed").size(10).color(colors.secondary),
			text(amount).size(10),
			text(eta).size(10).color(colors.secondary),
			style::control(text("Cancel").size(11), false, colors)
				.id("transfer-cancel-running")
				.on_press(Message::Cancel.into())
				.enabled(phase != Phase::RollingBack),
		]
		.spacing(6)
		.width(Length::Fill);
		scrollable(
			container(card)
				.padding(8)
				.width(Length::Fill)
				.style(move |_| style::panel(colors.body)),
		)
		.id("sidebar-transfers")
		.height(150)
		.width(Length::Fill)
		.into()
	}

	pub(crate) fn transfer_history_view(&self) -> Element<'_, crate::Message> {
		let colors = self.appearance.colors();
		let state = &self.transfers;
		let mut history = column![].spacing(12).width(Length::Fill);
		if state.history.is_empty() {
			history = history.push(
				text("No transfer history.")
					.size(12)
					.color(colors.secondary),
			);
		}
		for journal in &state.history {
			let action = match journal.status {
				Status::Complete => Some("Undo"),
				Status::Running | Status::RollingBack | Status::NeedsRecovery => {
					Some("Retry rollback")
				}
				Status::Expiring | Status::RolledBack => None,
			};
			let mut heading = row![
				text(status_label(journal.status)).size(12),
				text(journal.id.as_str())
					.size(11)
					.color(colors.secondary)
					.width(Length::Fill),
			]
			.spacing(8)
			.align_y(Alignment::Center);
			if let Some(action) = action {
				heading = heading.push(
					style::control(text(action).size(12), false, colors)
						.on_press(Message::Undo(journal.id.clone()).into())
						.enabled(!state.busy && state.loaded && !state.blocked()),
				);
			}

			let mut entry = column![heading].spacing(4).width(Length::Fill);
			if let Some(change) = journal.changes.first() {
				entry = entry.push(
					text(format!(
						"{} → {}",
						change.source.path().display(),
						change.destination.path().display()
					))
					.size(11)
					.width(Length::Fill)
					.wrapping(iced::widget::text::Wrapping::WordOrGlyph),
				);
			}
			history = history.push(
				container(entry)
					.padding(8)
					.width(Length::Fill)
					.style(move |_| style::panel(colors.surface)),
			);
		}
		container(
			column![
				row![
					text("History").size(27).width(Length::Fill),
					style::control(text("Files").size(12), false, colors)
						.id("transfer-history-files")
						.on_press(Message::ShowHistory(false).into()),
					style::control(text("Refresh").size(12), false, colors)
						.id("transfer-history-refresh")
						.on_press(Message::RefreshHistory.into()),
				]
				.spacing(12)
				.align_y(Alignment::Center),
				scrollable(history)
					.id("transfer-history")
					.height(Length::Fill),
			]
			.spacing(20)
			.height(Length::Fill),
		)
		.padding(20)
		.width(Length::Fill)
		.height(Length::Fill)
		.into()
	}

	pub(crate) fn transfer_dialog_view(&self, id: iced::window::Id) -> Element<'_, crate::Message> {
		let colors = self.appearance.colors();
		let Some(dialog) = self
			.transfers
			.dialog
			.as_ref()
			.filter(|dialog| dialog.id == id)
		else {
			return Space::new().into();
		};
		let active = dialog.open;
		let heading = match &dialog.content {
			DialogContent::Conflict(conflict)
				if matches!(
					conflict.reason,
					super::model::ConflictReason::RecoveryBudget { .. }
				) =>
			{
				"Recovery storage approval"
			}
			DialogContent::Conflict(conflict)
				if conflict.reason == super::model::ConflictReason::CentralRecovery =>
			{
				"Use central recovery storage?"
			}
			DialogContent::Conflict(conflict)
				if conflict.reason == super::model::ConflictReason::VolatileMove =>
			{
				"Move into volatile storage?"
			}
			DialogContent::Conflict(_) => "Destination already exists",
		};
		let title = mouse_area(
			container(text(heading).size(16))
				.padding(14)
				.width(Length::Fill),
		)
		.on_press(Message::Drag(id).into());
		let close = style::control(text("×").size(18), false, colors)
			.enabled(active)
			.on_press(Message::DismissDialog.into());

		let (body, buttons): (Element<'_, crate::Message>, Element<'_, crate::Message>) =
			match &dialog.content {
				DialogContent::Conflict(conflict) => {
					let apply = style::control(
						text(if self.transfers.apply_remaining {
							"Apply to remaining conflicts of this kind"
						} else {
							"Apply to this conflict only"
						})
						.size(12),
						self.transfers.apply_remaining,
						colors,
					)
					.id("transfer-remaining")
					.enabled(active)
					.on_press(Message::ApplyRemaining(!self.transfers.apply_remaining).into());
					let mut body = column![
						text("Source").size(12),
						text(conflict.source.display().to_string())
							.size(13)
							.width(Length::Fill)
							.wrapping(iced::widget::text::Wrapping::WordOrGlyph),
						text("Destination").size(12),
						text(conflict.destination.display().to_string())
							.size(13)
							.width(Length::Fill)
							.wrapping(iced::widget::text::Wrapping::WordOrGlyph),
					]
					.spacing(10)
					.width(Length::Fill);
					let consent = if let super::model::ConflictReason::RecoveryBudget { bytes } =
						conflict.reason
					{
						body = body.push(text(format!("This replacement needs {} of recovery data outside the normal 5 GiB limit. Approve extra storage for this job? Its undo record still expires after seven days. Cancelling leaves the existing data untouched.", crate::filesystem::entry::format_size(bytes))).size(13));
						true
					} else if conflict.reason == super::model::ConflictReason::CentralRecovery {
						body = body.push(text("A safe same-filesystem backup could not be made. Use central recovery storage under your XDG data directory instead? Fileberg will check capacity and durably copy and verify the existing destination before replacing it. This can take extra time and space.").size(13));
						true
					} else if conflict.reason == super::model::ConflictReason::VolatileMove {
						body = body.push(text("This destination is volatile storage. Moving removes your originals, and the destination data may disappear after restart or power loss. Cancel and use Copy to keep your originals, or explicitly accept this risk for this job.").size(13));
						true
					} else {
						body = body.push(apply);
						false
					};
					let mut buttons = row![Space::new().width(Length::Fill)]
						.spacing(8)
						.align_y(Alignment::Center);
					for (label, choice) in [
						("Merge", Choice::Merge),
						("Replace", Choice::Replace),
						("Central backup", Choice::ReplaceCentral),
						("Skip", Choice::Skip),
						("Keep both", Choice::KeepBoth),
						("Cancel", Choice::Cancel),
						(
							if conflict.reason == super::model::ConflictReason::VolatileMove {
								"Move anyway"
							} else if conflict.reason
								== super::model::ConflictReason::CentralRecovery
							{
								"Use central storage"
							} else {
								"Approve extra storage"
							},
							Choice::ApproveRecovery,
						),
					] {
						if (consent && !matches!(choice, Choice::Cancel | Choice::ApproveRecovery))
							|| (!consent && choice == Choice::ApproveRecovery)
						{
							continue;
						}
						if choice == Choice::Merge && !conflict.directories {
							continue;
						}
						buttons = buttons.push(
							style::control(text(label).size(12), false, colors)
								.id(choice_id(choice))
								.enabled(active)
								.on_press(Message::Choose(choice).into()),
						);
					}
					(body.into(), buttons.into())
				}
			};

		container(
			column![
				row![title, close].align_y(Alignment::Center),
				scrollable(container(body).padding(14).width(Length::Fill)).height(Length::Fill),
				container(buttons).padding(14).width(Length::Fill),
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
}

fn text<'a>(content: impl iced::widget::text::IntoFragment<'a>) -> iced::widget::Text<'a> {
	iced::widget::text(content).font(FONT)
}

fn phase_label(phase: Phase) -> &'static str {
	match phase {
		Phase::Preparing => "Preparing",
		Phase::Copying => "Copying",
		Phase::Verifying => "Verifying",
		Phase::Flushing => "Flushing",
		Phase::Committing => "Committing",
		Phase::RollingBack => "Rolling back",
	}
}

fn status_label(status: Status) -> &'static str {
	match status {
		Status::Running => "Running",
		Status::Complete => "Complete",
		Status::Expiring => "Expiring",
		Status::RollingBack => "Rolling back",
		Status::RolledBack => "Changes rolled back",
		Status::NeedsRecovery => "Needs recovery",
	}
}

pub(super) fn choice_id(choice: Choice) -> &'static str {
	match choice {
		Choice::Merge => "transfer-merge",
		Choice::Replace => "transfer-replace",
		Choice::ReplaceCentral => "transfer-central",
		Choice::Skip => "transfer-skip",
		Choice::KeepBoth => "transfer-both",
		Choice::Cancel => "transfer-cancel",
		Choice::ApproveRecovery => "transfer-recovery-approve",
	}
}
