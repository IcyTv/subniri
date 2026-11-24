//! Toast lifetime state. Output selection and layer-shell effects belong to the bar.
use daemon_common::Notification;
use std::{
	collections::VecDeque,
	time::{Duration, Instant},
};

#[derive(Debug)]
pub struct Toast {
	pub id: u32,
	pub revision: u64,
	remaining: Option<Duration>,
	last_tick: Instant,
	awaiting_presentation: Option<Instant>,
	pub hovered: bool,
	pub transient: bool,
}

#[derive(Debug, Default)]
pub struct Queue {
	pub visible: VecDeque<Toast>,
	pending: VecDeque<Toast>,
}

impl Queue {
	pub fn clear(&mut self) {
		self.visible.clear();
		self.pending.clear();
	}

	pub fn upsert(&mut self, n: &Notification, replacement: bool, maximum: u32, default_ms: u32) {
		let critical = n.hints.get("urgency").and_then(|v| u8::try_from(v).ok()) == Some(2);
		let remaining = if critical || n.expire_timeout == 0 {
			None
		} else {
			Some(Duration::from_millis(if n.expire_timeout < 0 {
				u64::from(default_ms)
			} else {
				n.expire_timeout as u64
			}))
		};
		if replacement {
			if let Some(toast) = self
				.visible
				.iter_mut()
				.chain(self.pending.iter_mut())
				.find(|t| t.id == n.id)
			{
				toast.revision = n.revision;
				toast.remaining = remaining;
				toast.transient = n
					.hints
					.get("transient")
					.and_then(|v| bool::try_from(v).ok())
					.unwrap_or(false);
				toast.awaiting_presentation = Some(Instant::now());
				toast.last_tick = Instant::now();
			}
			return;
		}
		let toast = Toast {
			id: n.id,
			revision: n.revision,
			remaining,
			last_tick: Instant::now(),
			awaiting_presentation: Some(Instant::now()),
			hovered: false,
			transient: n
				.hints
				.get("transient")
				.and_then(|v| bool::try_from(v).ok())
				.unwrap_or(false),
		};
		let maximum = maximum.clamp(1, 10) as usize;
		if critical {
			if self.visible.len() >= maximum {
				self.visible.pop_front();
			}
			self.visible.push_back(toast);
		} else if self.visible.len() < maximum {
			self.visible.push_back(toast);
		} else {
			self.pending.push_back(toast);
		}
	}

	pub fn remove(&mut self, id: u32, maximum: u32) {
		self.visible.retain(|t| t.id != id);
		self.pending.retain(|t| t.id != id);
		self.promote(maximum);
	}

	pub fn rebalance(&mut self, maximum: u32) {
		while self.visible.len() > maximum.clamp(1, 10) as usize {
			if let Some(toast) = self.visible.pop_back() {
				self.pending.push_front(toast);
			}
		}
		self.promote(maximum);
	}

	fn promote(&mut self, maximum: u32) {
		while self.visible.len() < maximum.clamp(1, 10) as usize {
			let Some(mut toast) = self.pending.pop_front() else {
				break;
			};
			toast.last_tick = Instant::now();
			toast.awaiting_presentation = Some(Instant::now());
			self.visible.push_back(toast);
		}
	}

	pub fn awaiting_presentation(&self) -> bool {
		self.visible
			.iter()
			.any(|toast| toast.awaiting_presentation.is_some())
	}

	pub fn contains(&self, id: u32) -> bool {
		self.visible
			.iter()
			.chain(self.pending.iter())
			.any(|toast| toast.id == id)
	}

	pub fn surface_changed(&mut self) {
		for toast in &mut self.visible {
			toast.awaiting_presentation = Some(Instant::now());
		}
	}

	pub fn presented(&mut self, updated_at: Instant) {
		for toast in &mut self.visible {
			if toast
				.awaiting_presentation
				.is_some_and(|since| updated_at >= since)
			{
				toast.awaiting_presentation = None;
				toast.last_tick = Instant::now();
			}
		}
	}

	pub fn hover(&mut self, id: u32, hovered: bool) {
		if let Some(toast) = self.visible.iter_mut().find(|t| t.id == id) {
			toast.last_tick = Instant::now();
			toast.hovered = hovered;
		}
	}

	pub fn tick(&mut self, maximum: u32) -> Vec<(u32, u64)> {
		let now = Instant::now();
		let mut expired = Vec::new();
		self.visible.retain_mut(|toast| {
			if let Some(remaining) = &mut toast.remaining {
				if !toast.hovered && toast.awaiting_presentation.is_none() {
					*remaining = remaining.saturating_sub(now.duration_since(toast.last_tick));
				}
			}
			toast.last_tick = now;
			let keep = toast.remaining != Some(Duration::ZERO);
			if !keep && toast.transient {
				expired.push((toast.id, toast.revision));
			}
			keep
		});
		self.promote(maximum);
		expired
	}
}
