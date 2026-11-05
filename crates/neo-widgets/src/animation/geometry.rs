//! Interruptible geometry motion and compositor-aware popup surface sizing.
use iced::{Rectangle, Size};
use std::time::{Duration, Instant};

pub const LAYOUT_DURATION: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub struct RectangleMotion {
	from: Rectangle,
	to: Rectangle,
	started: Instant,
}

impl RectangleMotion {
	pub fn new(bounds: Rectangle, now: Instant) -> Self {
		Self {
			from: bounds,
			to: bounds,
			started: now,
		}
	}
	pub fn target(&self) -> Rectangle {
		self.to
	}
	pub fn value(&self, now: Instant) -> Rectangle {
		let t = (now.saturating_duration_since(self.started).as_secs_f32()
			/ LAYOUT_DURATION.as_secs_f32())
		.min(1.0);
		let t = 1.0 - (1.0 - t).powi(3);
		let lerp = |a: f32, b: f32| a + (b - a) * t;
		Rectangle {
			x: lerp(self.from.x, self.to.x),
			y: lerp(self.from.y, self.to.y),
			width: lerp(self.from.width, self.to.width),
			height: lerp(self.from.height, self.to.height),
		}
	}
	pub fn retarget(&mut self, target: Rectangle, now: Instant) {
		if self.to != target {
			self.from = self.value(now);
			self.to = target;
			self.started = now;
		}
	}
	pub fn active(&self, now: Instant) -> bool {
		self.from != self.to && now.saturating_duration_since(self.started) < LAYOUT_DURATION
	}
	pub fn freeze(&mut self, now: Instant) {
		let bounds = self.value(now);
		self.from = bounds;
		self.to = bounds;
		self.started = now;
	}
}

/// The surface envelope grows immediately, but visual motion waits for a configure.
/// On shrink only `presented` may release the envelope, after a UI snapshot
/// built from the final model update has actually been displayed.
/// A fresh instance per popup prevents acknowledgements from an old popup being reused.
#[derive(Debug, Clone)]
pub struct PopupResize {
	motion: RectangleMotion,
	pub envelope: Size,
	actual: Size,
	waiting: bool,
	settled_at: Option<Instant>,
	reserved: Size,
}

impl PopupResize {
	pub fn new(size: Size, now: Instant) -> Self {
		Self {
			motion: RectangleMotion::new(Rectangle::with_size(size), now),
			envelope: size,
			actual: Size::ZERO,
			waiting: false,
			settled_at: None,
			reserved: size,
		}
	}
	pub fn target(&self) -> Size {
		self.motion.target().size()
	}
	pub fn visual(&self, now: Instant) -> Size {
		self.motion.value(now).size()
	}
	pub fn request(&mut self, size: Size, now: Instant) {
		self.request_reserved(size, size, now);
	}
	/// Reserve transparent capacity independently of the visible content target.
	/// The caller supplies capacity for any content offset as part of `reserved`.
	pub fn request_reserved(&mut self, size: Size, reserved: Size, now: Instant) {
		self.request_bounds(Rectangle::with_size(size), reserved, now);
	}
	/// Animate both visible origin and size only after their outer capacity is configured.
	pub fn request_bounds(&mut self, bounds: Rectangle, reserved: Size, now: Instant) {
		let reserved = Size::new(
			(bounds.x + bounds.width).max(reserved.width),
			(bounds.y + bounds.height).max(reserved.height),
		);
		if self.motion.target() == bounds && self.reserved == reserved {
			return;
		}
		let current = self.frame_bounds(now);
		self.reserved = reserved;
		self.motion.from = current;
		self.motion.to = bounds;
		self.motion.started = now;
		self.envelope = Size::new(
			self.envelope.width.max(reserved.width),
			self.envelope.height.max(reserved.height),
		);
		self.waiting = !fits(self.envelope, self.actual);
		self.settled_at = None;
	}
	pub fn configured(&mut self, actual: Size, now: Instant) {
		self.actual = actual;
		if self.waiting && fits(self.envelope, actual) {
			self.motion.started = now;
			self.waiting = false;
		}
	}
	pub fn waiting(&self) -> bool {
		self.waiting
	}
	pub fn needs_frame(&self) -> bool {
		!self.waiting
			&& (self.motion.from != self.motion.to
				|| (self.settled_at.is_none() && self.envelope != self.reserved))
	}
	pub fn tick(&mut self, now: Instant) {
		if self.waiting {
			return;
		}
		if !self.motion.active(now) {
			self.motion.from = self.motion.to;
			// The event's frame timestamp predates this model update. Only a view
			// built after the update can contain its final geometry.
			if self.settled_at.is_none() {
				self.settled_at = Some(Instant::now());
			}
		}
	}

	/// Confirm a displayed UI snapshot. Never call for redraw, buffer submission,
	/// discarded presentation feedback, or a different popup window.
	pub fn presented(&mut self, content_updated_at: Instant) {
		if !self.waiting
			&& self
				.settled_at
				.is_some_and(|settled| content_updated_at >= settled)
		{
			self.envelope = self.reserved;
			self.settled_at = None;
		}
	}

	pub fn is_settled(&self) -> bool {
		!self.waiting && self.motion.from == self.motion.to && self.envelope == self.reserved
	}

	pub fn awaiting_presentation(&self) -> bool {
		self.settled_at.is_some() && self.envelope != self.reserved
	}
	pub fn frame_visual(&self, now: Instant) -> Size {
		self.frame_bounds(now).size()
	}
	pub fn frame_bounds(&self, now: Instant) -> Rectangle {
		if self.waiting {
			self.motion.from
		} else {
			self.motion.value(now)
		}
	}
}

fn fits(required: Size, actual: Size) -> bool {
	// Backend sizes are rounded at fractional scaling.
	actual.width + 1.0 >= required.width && actual.height + 1.0 >= required.height
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn reserved_capacity_is_independent_and_released_only_after_presentation() {
		let now = Instant::now();
		let size = Size::new(200.0, 100.0);
		let reserve = Size::new(456.0, 356.0);
		let mut popup = PopupResize::new(size, now);
		popup.configured(size, now);
		popup.request_reserved(size, reserve, now);
		assert!(popup.waiting());
		popup.configured(reserve, now);
		popup.request_reserved(Size::new(250.0, 150.0), reserve, now);
		assert!(!popup.waiting());
		popup.tick(now + LAYOUT_DURATION);
		popup.presented(Instant::now());
		assert!(popup.is_settled());
		assert_eq!(popup.envelope, reserve);
		popup.request(Size::new(250.0, 150.0), now + LAYOUT_DURATION);
		assert!(
			popup.needs_frame(),
			"capacity-only shrink still needs a final model snapshot"
		);
		popup.tick(now + LAYOUT_DURATION * 2);
		assert_eq!(popup.envelope, reserve);
		popup.presented(Instant::now());
		assert_eq!(popup.envelope, popup.target());
	}
	#[test]
	fn offset_waits_for_capacity_and_returns_without_jumping() {
		let now = Instant::now();
		let size = Size::new(200.0, 100.0);
		let mut popup = PopupResize::new(size, now);
		popup.configured(size, now);
		let offset = Rectangle {
			x: 128.0,
			y: 128.0,
			..Rectangle::with_size(size)
		};
		popup.request_bounds(offset, Size::new(456.0, 356.0), now);
		assert_eq!(
			popup.frame_bounds(now + LAYOUT_DURATION),
			Rectangle::with_size(size)
		);
		popup.configured(popup.envelope, now);
		assert_eq!(popup.frame_bounds(now), Rectangle::with_size(size));
		let middle = now + Duration::from_millis(80);
		let before = popup.frame_bounds(middle);
		popup.request(size, middle);
		assert_eq!(popup.frame_bounds(middle), before);
		popup.tick(middle + LAYOUT_DURATION);
		assert_eq!(
			popup.frame_bounds(middle + LAYOUT_DURATION),
			Rectangle::with_size(size)
		);
		assert_ne!(popup.envelope, size);
		popup.presented(Instant::now());
		assert_eq!(popup.envelope, size);
	}
	#[test]
	fn growth_waits_and_shrink_releases_after_final_frame() {
		let now = Instant::now();
		let small = Size::new(200.0, 100.0);
		let large = Size::new(400.0, 300.0);
		let mut popup = PopupResize::new(small, now);
		popup.configured(small, now);
		popup.request(large, now);
		assert_eq!(popup.envelope, large);
		assert_eq!(popup.frame_visual(now + LAYOUT_DURATION), small);
		popup.configured(small, now);
		assert!(popup.waiting());
		popup.configured(large, now + LAYOUT_DURATION);
		assert_eq!(popup.frame_visual(now + LAYOUT_DURATION), small);
		popup.tick(now + LAYOUT_DURATION * 2);
		assert_eq!(popup.frame_visual(now + LAYOUT_DURATION * 2), large);
		popup.request(small, now + LAYOUT_DURATION * 2);
		popup.tick(now + LAYOUT_DURATION * 3);
		assert_eq!(popup.envelope, large);
		popup.tick(now + LAYOUT_DURATION * 3 + Duration::from_millis(16));
		assert_eq!(
			popup.envelope, large,
			"redraws are not presentation acknowledgements"
		);
		popup.presented(now);
		assert_eq!(
			popup.envelope, large,
			"old UI submissions cannot release shrink"
		);
		popup.presented(Instant::now());
		assert_eq!(popup.envelope, small);
	}

	#[test]
	fn interrupted_shrink_rejects_feedback_before_new_final_model() {
		let now = Instant::now();
		let mut popup = PopupResize::new(Size::new(400.0, 300.0), now);
		popup.configured(popup.envelope, now);
		popup.request(Size::new(200.0, 100.0), now);
		popup.tick(now + LAYOUT_DURATION);
		let old_snapshot = Instant::now();
		popup.request(Size::new(300.0, 200.0), now + LAYOUT_DURATION);
		popup.presented(old_snapshot);
		assert_eq!(popup.envelope, Size::new(400.0, 300.0));
		popup.tick(now + LAYOUT_DURATION * 2);
		popup.presented(old_snapshot);
		assert_eq!(popup.envelope, Size::new(400.0, 300.0));
		popup.presented(Instant::now());
		assert!(popup.is_settled());
	}
	#[test]
	fn retarget_is_continuous_and_pending_growth_can_reverse() {
		let now = Instant::now();
		let mut motion = RectangleMotion::new(Rectangle::with_size(Size::new(10.0, 10.0)), now);
		motion.retarget(Rectangle::with_size(Size::new(100.0, 100.0)), now);
		let middle = now + Duration::from_millis(80);
		let before = motion.value(middle);
		motion.retarget(Rectangle::with_size(Size::new(20.0, 20.0)), middle);
		assert_eq!(motion.value(middle), before);
		let mut popup = PopupResize::new(Size::new(100.0, 100.0), now);
		popup.configured(Size::new(100.0, 100.0), now);
		popup.request(Size::new(400.0, 300.0), now);
		popup.request(Size::new(120.0, 120.0), middle);
		assert!(popup.waiting());
		popup.configured(Size::new(400.0, 300.0), middle);
		assert_eq!(popup.target(), Size::new(120.0, 120.0));
	}
}
