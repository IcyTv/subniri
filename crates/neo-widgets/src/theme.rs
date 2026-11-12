//! Per-application palette transitions without mutable global state.

use std::time::{Duration, Instant};

use config::AppearanceMode;

use crate::style::Colors;

const TRANSITION_DURATION: Duration = Duration::from_millis(150);

#[derive(Debug, Clone)]
pub struct ThemeContext {
	source: Colors,
	target: Colors,
	theme: AppearanceMode,
	start: Instant,
	duration: Duration,
}

impl ThemeContext {
	/// Start at the configured palette, with no startup fade.
	pub fn new(theme: AppearanceMode) -> Self {
		let colors = Colors::for_theme(theme);

		Self {
			source: colors,
			target: colors,
			theme,
			start: Instant::now(),
			duration: TRANSITION_DURATION,
		}
	}

	pub fn colors(&self) -> Colors {
		self.colors_at(Instant::now())
	}

	pub fn selected(&self) -> AppearanceMode {
		self.theme
	}

	pub fn colors_at(&self, now: Instant) -> Colors {
		let elapsed = now.saturating_duration_since(self.start);
		let progress = elapsed.as_secs_f32() / self.duration.as_secs_f32();

		self.source.interpolate(self.target, progress)
	}

	/// Retarget from the currently displayed colors, not the previous endpoint.
	pub fn set_theme(&mut self, theme: AppearanceMode, now: Instant) {
		if self.theme == theme {
			return;
		}

		self.source = self.colors_at(now);
		self.target = Colors::for_theme(theme);
		self.theme = theme;
		self.start = now;
	}

	pub fn is_animating(&self, now: Instant) -> bool {
		self.source != self.target && now.saturating_duration_since(self.start) < self.duration
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::style::{DARK, LIGHT};

	#[test]
	fn starts_at_selected_palette_without_animation() {
		for (theme, colors) in [(AppearanceMode::Light, LIGHT), (AppearanceMode::Dark, DARK)] {
			let context = ThemeContext::new(theme);

			assert_eq!(context.colors_at(context.start), colors);
			assert_eq!(context.colors(), colors);
			assert!(!context.is_animating(context.start));
		}
	}

	#[test]
	fn transition_reaches_exact_endpoints_in_150_ms() {
		let mut context = ThemeContext::new(AppearanceMode::Light);
		let start = context.start;

		context.set_theme(AppearanceMode::Dark, start);

		assert_eq!(context.colors_at(start), LIGHT);
		assert!(context.is_animating(start));
		assert_eq!(
			context.colors_at(start + Duration::from_millis(75)),
			LIGHT.interpolate(DARK, 0.5)
		);
		assert_eq!(context.colors_at(start + TRANSITION_DURATION), DARK);
		assert!(!context.is_animating(start + TRANSITION_DURATION));
	}

	#[test]
	fn interrupted_transition_retargets_without_a_jump() {
		let mut context = ThemeContext::new(AppearanceMode::Light);
		let start = context.start;

		context.set_theme(AppearanceMode::Dark, start);

		let interrupted = start + Duration::from_millis(75);
		let displayed = context.colors_at(interrupted);

		context.set_theme(AppearanceMode::Light, interrupted);

		assert_eq!(context.colors_at(interrupted), displayed);
		assert_eq!(
			context.colors_at(interrupted + Duration::from_millis(75)),
			displayed.interpolate(LIGHT, 0.5)
		);
		assert_eq!(context.colors_at(interrupted + TRANSITION_DURATION), LIGHT);
		assert!(!context.is_animating(interrupted + TRANSITION_DURATION));
	}

	#[test]
	fn repeated_target_does_not_restart_transition() {
		let mut context = ThemeContext::new(AppearanceMode::Light);
		let start = context.start;

		context.set_theme(AppearanceMode::Dark, start);
		context.set_theme(AppearanceMode::Dark, start + Duration::from_millis(75));

		assert_eq!(context.colors_at(start + TRANSITION_DURATION), DARK);
		assert!(!context.is_animating(start + TRANSITION_DURATION));
	}
}
