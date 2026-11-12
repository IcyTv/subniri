//! Copy-only throughput samples; setup, verification, prompts and sync are not ETA.
use std::{
	collections::VecDeque,
	time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct Rate {
	samples: VecDeque<(Instant, u64)>,
}

impl Rate {
	pub fn reset(&mut self) {
		self.samples.clear();
	}

	pub fn observe(&mut self, now: Instant, bytes: u64) {
		if let Some(&(last, previous)) = self.samples.back() {
			if bytes < previous || now < last {
				self.reset();
			} else if now.duration_since(last) < Duration::from_millis(250) {
				return;
			}
		}
		self.samples.push_back((now, bytes));
		while self.samples.len() > 2
			&& self
				.samples
				.get(1)
				.is_some_and(|(at, _)| now.duration_since(*at) > Duration::from_secs(4))
		{
			self.samples.pop_front();
		}
	}

	pub fn eta(&self, bytes: u64, total: u64) -> Option<Duration> {
		let &(first, start) = self.samples.front()?;
		let &(last, end) = self.samples.back()?;
		let elapsed = last.checked_duration_since(first)?;
		if elapsed < Duration::from_secs(2)
			|| self.samples.len() < 5
			|| end <= start
			|| bytes >= total
		{
			return None;
		}
		Duration::try_from_secs_f64(
			elapsed.as_secs_f64() * (total - bytes) as f64 / (end - start) as f64,
		)
		.ok()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn warmup_hides_startup_and_smooths_bursty_samples() {
		let now = Instant::now();
		let mut rate = Rate::default();
		for tick in 0..8 {
			rate.observe(now + Duration::from_millis(tick * 250), tick * 25);
			assert_eq!(rate.eta(tick * 25, 4000), None);
		}
		rate.observe(now + Duration::from_secs(2), 200);
		assert_eq!(rate.eta(200, 4000), Some(Duration::from_secs(38)));

		// A frame's burst of queued notices must not create a new rate window.
		for bytes in 201..210 {
			rate.observe(now + Duration::from_secs(2), bytes);
		}
		assert!(rate.eta(209, 4000).is_some_and(|eta| eta.as_secs() == 37));
		rate.reset();
		rate.observe(now + Duration::from_secs(60), 209);
		assert_eq!(rate.eta(209, 4000), None);
	}

	#[test]
	fn rolling_window_forgets_slow_start_and_rejects_reset_or_finished_progress() {
		let now = Instant::now();
		let mut rate = Rate::default();
		for tick in 0..=32 {
			let bytes = if tick < 8 { tick } else { 8 + (tick - 8) * 25 };
			rate.observe(now + Duration::from_millis(tick * 250), bytes);
		}
		assert_eq!(rate.eta(608, 1008), Some(Duration::from_secs(4)));
		assert_eq!(rate.eta(1008, 1008), None);
		rate.observe(now + Duration::from_secs(9), 0);
		assert_eq!(rate.eta(0, 1008), None);
	}
}
