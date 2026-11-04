//! Card decisions independent of animation, tasks and D-Bus.
use daemon_common::HomeassistantCapability;

#[derive(Debug, Clone, Default)]
pub(super) struct Interaction {
	capabilities: Vec<HomeassistantCapability>,
	available: bool,
	reported: bool,
	enabled: bool,
	pending: bool,
}

pub(super) enum Event {
	Snapshot {
		state: String,
		available: bool,
		capabilities: Vec<HomeassistantCapability>,
	},
	Disconnected,
	Activate,
	Finished {
		success: bool,
	},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Command {
	Call,
	SetEnabled(bool),
}

#[derive(Default)]
pub(super) struct Change {
	pub animate: Option<bool>,
	pub command: Option<Command>,
}

impl Interaction {
	pub fn callable(&self) -> bool {
		self.capabilities
			.contains(&HomeassistantCapability::Callable)
	}
	pub fn actionable(&self) -> bool {
		self.available
			&& (self.callable()
				|| self
					.capabilities
					.contains(&HomeassistantCapability::Togglable))
	}

	pub fn update(&mut self, event: Event) -> Change {
		let mut change = Change::default();
		match event {
			Event::Snapshot {
				state,
				available,
				capabilities,
			} => {
				self.available = available;
				self.capabilities = capabilities;
				self.reported = state == "on";
				if !self.pending {
					change.animate = Some(self.reported);
				}
			}
			Event::Disconnected => self.available = false,
			Event::Activate if self.actionable() && !self.pending => {
				let target = self.callable() || !self.enabled;
				change.animate = Some(target);
				self.pending = true;
				change.command = Some(if self.callable() {
					Command::Call
				} else {
					Command::SetEnabled(target)
				});
			}
			Event::Finished { success } => {
				self.pending = false;
				if !success || self.callable() {
					change.animate = Some(self.reported);
				}
			}
			Event::Activate => {}
		}
		if let Some(enabled) = change.animate {
			self.enabled = enabled;
		}
		change
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn snapshot(capabilities: Vec<HomeassistantCapability>) -> Event {
		Event::Snapshot {
			state: "off".into(),
			available: true,
			capabilities,
		}
	}

	#[test]
	fn capabilities_pending_and_reconciliation_have_one_owner() {
		let mut interaction = Interaction::default();
		assert!(interaction.update(Event::Activate).command.is_none());
		interaction.update(snapshot(vec![]));
		assert!(interaction.update(Event::Activate).command.is_none());
		interaction.update(snapshot(vec![HomeassistantCapability::Togglable]));
		let change = interaction.update(Event::Activate);
		assert_eq!(change.command, Some(Command::SetEnabled(true)));
		assert_eq!(change.animate, Some(true));
		assert!(interaction.update(Event::Activate).command.is_none());
		assert!(
			interaction
				.update(snapshot(vec![HomeassistantCapability::Togglable]))
				.animate
				.is_none()
		);
		assert_eq!(
			interaction
				.update(Event::Finished { success: false })
				.animate,
			Some(false)
		);
		interaction.update(snapshot(vec![
			HomeassistantCapability::Callable,
			HomeassistantCapability::Togglable,
		]));
		assert_eq!(
			interaction.update(Event::Activate).command,
			Some(Command::Call)
		);
		assert_eq!(
			interaction
				.update(Event::Finished { success: true })
				.animate,
			Some(false)
		);
		interaction.update(Event::Disconnected);
		assert!(!interaction.actionable());
		assert!(interaction.update(Event::Activate).command.is_none());
	}
}
