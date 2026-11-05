//! Optimistic scalar state with serialized writes and latest-value coalescing.
//! Callers perform effects: request/complete return the next value to write.
#[derive(Debug, Clone)]
pub struct LatestWrite<T> {
	confirmed: T,
	active: Option<T>,
	queued: Option<T>,
}

impl<T: Copy + PartialEq> LatestWrite<T> {
	#[must_use]
	pub fn new(value: T) -> Self {
		Self {
			confirmed: value,
			active: None,
			queued: None,
		}
	}
	#[must_use]
	pub fn value(&self) -> T {
		self.queued.or(self.active).unwrap_or(self.confirmed)
	}
	pub fn observe(&mut self, value: T) {
		// An older property notification cannot overwrite a pending user choice.
		if self.active.is_none() {
			self.confirmed = value;
		}
	}
	pub fn request(&mut self, value: T) -> Option<T> {
		if self.active.is_some() {
			self.queued = Some(value);
			None
		} else if value == self.confirmed {
			None
		} else {
			self.active = Some(value);
			Some(value)
		}
	}
	pub fn complete(&mut self, success: bool) -> Option<T> {
		if let Some(value) = self.active.take().filter(|_| success) {
			self.confirmed = value;
		}
		self.queued.take().and_then(|value| self.request(value))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn rapid_requests_coalesce_and_failures_restore_confirmed_state() {
		let mut value = LatestWrite::new(20);
		assert_eq!(value.request(40), Some(40));
		assert_eq!(value.request(60), None);
		assert_eq!(value.request(80), None);
		value.observe(40);
		assert_eq!(value.value(), 80);
		assert_eq!(value.complete(true), Some(80));
		assert_eq!(value.value(), 80);
		assert_eq!(value.complete(false), None);
		assert_eq!(value.value(), 40);
		value.observe(30);
		assert_eq!(value.value(), 30);
	}
	#[test]
	fn returning_to_confirmed_value_still_undoes_inflight_write() {
		let mut value = LatestWrite::new(20);
		assert_eq!(value.request(40), Some(40));
		assert_eq!(value.request(20), None);
		assert_eq!(value.complete(true), Some(20));
		assert_eq!(value.complete(true), None);
		assert_eq!(value.value(), 20);
	}
}
