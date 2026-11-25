use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
	Acquiring,
	Locked,
	Authenticating(u64, Origin),
	Authorized,
	Failed,
}

/// Distinguishes a user-submitted attempt from a background fingerprint/Howdy
/// probe (see [`LockState::begin_probe`]). Only submitted attempts rate-limit
/// on failure or report rejection text; a probe that finds no finger/face is
/// expected, not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
	Probe,
	Submit,
}

pub(crate) struct LockState {
	phase: Phase,
	next_attempt: u64,
	started: Instant,
	retry_after: Option<Instant>,
	invalidated: bool,
}

impl LockState {
	pub(crate) fn new(now: Instant) -> Self {
		Self {
			phase: Phase::Acquiring,
			next_attempt: 0,
			started: now,
			retry_after: None,
			invalidated: false,
		}
	}

	pub(crate) fn locked(&mut self) {
		if self.phase == Phase::Acquiring {
			self.phase = Phase::Locked;
		}
	}

	pub(crate) fn startup_timed_out(&self, now: Instant) -> bool {
		self.phase == Phase::Acquiring
			&& now.duration_since(self.started) >= Duration::from_secs(10)
	}

	pub(crate) fn refresh(&mut self, now: Instant) {
		if self
			.retry_after
			.is_some_and(|retry_after| now >= retry_after)
		{
			self.retry_after = None;
		}
	}

	pub(crate) fn can_edit(&self) -> bool {
		self.phase == Phase::Locked
	}

	pub(crate) fn cooling_down(&self) -> bool {
		self.retry_after.is_some()
	}

	pub(crate) fn can_submit(&self) -> bool {
		self.can_edit() && !self.cooling_down()
	}

	pub(crate) fn begin(&mut self, now: Instant) -> Option<u64> {
		self.refresh(now);
		if !self.can_submit() {
			return None;
		}

		self.start(Origin::Submit)
	}

	/// Starts a background fingerprint/Howdy probe. Unlike [`Self::begin`],
	/// this ignores the password-retry cooldown: a probe is a separate,
	/// silent channel from a submitted guess and shouldn't be rate-limited
	/// alongside it. It still refuses to start alongside any other
	/// in-flight attempt (probe or submit), since only one PAM conversation
	/// may run at a time.
	pub(crate) fn begin_probe(&mut self, now: Instant) -> Option<u64> {
		self.refresh(now);
		if !self.can_edit() {
			return None;
		}

		self.start(Origin::Probe)
	}

	fn start(&mut self, origin: Origin) -> Option<u64> {
		self.next_attempt = self.next_attempt.checked_add(1)?;
		self.invalidated = false;
		self.phase = Phase::Authenticating(self.next_attempt, origin);
		Some(self.next_attempt)
	}

	pub(crate) fn complete(&mut self, attempt: u64, accepted: bool, now: Instant) -> bool {
		let Phase::Authenticating(current, origin) = self.phase else {
			return false;
		};
		if current != attempt {
			return false;
		}

		if accepted && !self.invalidated {
			self.phase = Phase::Authorized;
			true
		} else {
			self.phase = Phase::Locked;
			// A probe finding no finger/face is expected, not a guess to
			// rate-limit; only a submitted attempt pays the cooldown.
			if origin == Origin::Submit {
				self.retry_after = Some(now + Duration::from_secs(2));
			}
			false
		}
	}

	pub(crate) fn cancel_attempt(&mut self) {
		if matches!(self.phase, Phase::Authenticating(..)) {
			// Keep the worker slot occupied until PAM returns. Hotplug must not
			// allow overlapping conversations or revive an old successful attempt.
			self.invalidated = true;
		}
	}

	pub(crate) fn fail(&mut self) {
		self.phase = Phase::Failed;
	}

	pub(crate) fn authenticating(&self) -> bool {
		matches!(self.phase, Phase::Authenticating(..))
	}

	/// The attempt id currently in flight, if any. Lets a caller tag
	/// transient progress text (PAM conversation messages) to the attempt
	/// that produced it, and discard it once stale.
	pub(crate) fn current_attempt(&self) -> Option<u64> {
		match self.phase {
			Phase::Authenticating(attempt, _) => Some(attempt),
			_ => None,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn classified_results_preserve_matching_attempt_unlock_invariant() {
		use crate::auth::{AuthResult, Stage};

		for result in [
			AuthResult::Rejected,
			AuthResult::WorkerFailure,
			AuthResult::Failure {
				stage: Stage::Account,
				status: 13,
			},
			AuthResult::Accepted,
		] {
			let now = Instant::now();
			let mut state = LockState::new(now);
			state.locked();
			let attempt = state.begin(now).unwrap();

			assert!(!state.complete(attempt + 1, result.accepted(), now));
			assert_eq!(
				state.complete(attempt, result.accepted(), now),
				result.accepted()
			);
			assert!(!state.complete(attempt, result.accepted(), now));
		}
	}

	#[test]
	fn confirmed_lock_without_surfaces_does_not_time_out() {
		let now = Instant::now();
		let mut state = LockState::new(now);

		state.locked();

		assert!(!state.startup_timed_out(now + Duration::from_secs(30)));
	}

	#[test]
	fn unconfirmed_lock_times_out_after_startup_deadline() {
		let now = Instant::now();
		let state = LockState::new(now);

		assert!(!state.startup_timed_out(now + Duration::from_secs(9)));
		assert!(state.startup_timed_out(now + Duration::from_secs(10)));
	}

	#[test]
	fn only_matching_success_after_lock_confirmation_authorizes_unlock() -> Result<(), &'static str>
	{
		let now = Instant::now();
		let mut state = LockState::new(now);

		assert_eq!(state.begin(now), None);
		assert!(!state.complete(1, true, now));

		state.locked();
		let attempt = state
			.begin(now)
			.ok_or("confirmed lock must allow an attempt")?;

		assert_eq!(state.begin(now), None);
		assert!(!state.complete(attempt + 1, true, now));
		assert!(state.complete(attempt, true, now));
		assert!(!state.complete(attempt, true, now));
		assert_eq!(state.begin(now), None);
		Ok(())
	}

	#[test]
	fn rejection_and_worker_errors_stay_locked_and_rate_limit_retry() -> Result<(), &'static str> {
		let now = Instant::now();
		let mut state = LockState::new(now);
		state.locked();
		let attempt = state
			.begin(now)
			.ok_or("confirmed lock must allow an attempt")?;

		assert!(!state.complete(attempt, false, now));
		assert_eq!(state.begin(now), None);
		assert!(state.begin(now + Duration::from_secs(2)).is_some());
		Ok(())
	}

	#[test]
	fn cooldown_allows_editing_but_blocks_submission_until_ready() -> Result<(), &'static str> {
		let now = Instant::now();
		let mut state = LockState::new(now);
		state.locked();
		let attempt = state
			.begin(now)
			.ok_or("confirmed lock must allow an attempt")?;

		assert!(!state.complete(attempt, false, now));
		assert!(state.can_edit());
		assert!(state.cooling_down());
		assert!(!state.can_submit());
		assert_eq!(state.begin(now + Duration::from_secs(1)), None);

		state.refresh(now + Duration::from_secs(2));

		assert!(state.can_edit());
		assert!(!state.cooling_down());
		assert!(state.can_submit());
		Ok(())
	}

	#[test]
	fn active_authentication_blocks_editing_and_submission() -> Result<(), &'static str> {
		let now = Instant::now();
		let mut state = LockState::new(now);
		state.locked();
		let attempt = state
			.begin(now)
			.ok_or("confirmed lock must allow an attempt")?;

		assert!(state.authenticating());
		assert!(!state.can_edit());
		assert!(!state.can_submit());
		assert_eq!(state.begin(now + Duration::from_secs(3)), None);
		assert!(!state.complete(attempt, false, now + Duration::from_secs(3)));
		Ok(())
	}

	#[test]
	fn output_loss_or_lifecycle_failure_invalidates_in_flight_success() -> Result<(), &'static str>
	{
		let now = Instant::now();
		let mut state = LockState::new(now);
		state.locked();
		let attempt = state
			.begin(now)
			.ok_or("confirmed lock must allow an attempt")?;

		state.cancel_attempt();
		assert_eq!(state.begin(now), None);
		assert!(!state.complete(attempt, true, now));

		let later = now + Duration::from_secs(2);
		let next = state
			.begin(later)
			.ok_or("retry must be ready after cooldown")?;

		assert_ne!(attempt, next);
		assert!(!state.complete(attempt, true, later));

		state.fail();
		assert!(!state.complete(next, true, later));
		state.locked();
		assert_eq!(state.begin(later), None);
		Ok(())
	}

	#[test]
	fn probe_failure_never_sets_the_submit_cooldown() -> Result<(), &'static str> {
		let now = Instant::now();
		let mut state = LockState::new(now);
		state.locked();
		let attempt = state
			.begin_probe(now)
			.ok_or("confirmed lock must allow a probe")?;

		assert!(!state.complete(attempt, false, now));
		assert!(!state.cooling_down());
		assert!(state.can_submit());
		Ok(())
	}

	#[test]
	fn probe_success_authorizes_unlock_same_as_a_submitted_attempt() -> Result<(), &'static str> {
		let now = Instant::now();
		let mut state = LockState::new(now);
		state.locked();
		let attempt = state
			.begin_probe(now)
			.ok_or("confirmed lock must allow a probe")?;

		assert!(state.complete(attempt, true, now));
		Ok(())
	}

	#[test]
	fn probe_ignores_an_active_submit_cooldown_but_not_a_live_attempt() -> Result<(), &'static str>
	{
		let now = Instant::now();
		let mut state = LockState::new(now);
		state.locked();
		let attempt = state
			.begin(now)
			.ok_or("confirmed lock must allow an attempt")?;

		// A probe can't start alongside an attempt already in flight.
		assert_eq!(state.begin_probe(now), None);
		assert!(!state.complete(attempt, false, now));

		// But once that submit's cooldown kicks in, a probe is unaffected.
		assert!(state.cooling_down());
		assert!(!state.can_submit());
		assert!(state.begin_probe(now).is_some());
		Ok(())
	}

	#[test]
	fn probe_refuses_to_start_before_the_lock_is_confirmed() {
		let now = Instant::now();
		let mut state = LockState::new(now);

		assert_eq!(state.begin_probe(now), None);
	}
}
