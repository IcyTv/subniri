use config_macros::{Config, ConfigSerialize};
use garde::Validate;

/// Notification presentation preferences. Disabling presentation does not stop daemon storage.
#[derive(Debug, Clone, PartialEq, Config, ConfigSerialize, Validate)]
pub struct Notifications {
	/// Whether to show the notification bell and toast popups.
	#[config(default = true)]
	#[garde(skip)]
	pub enabled: bool,

	/// How to arrange notifications in the notification center.
	#[config(default)]
	#[garde(skip)]
	pub center_layout: NotificationCenterLayout,

	/// Which outputs should display toast popups.
	#[config(default)]
	#[garde(skip)]
	pub toast_placement: NotificationToastPlacement,

	/// Maximum number of toast popups visible at once.
	#[config(default = 3)]
	#[garde(range(min = 1, max = 10))]
	pub max_visible: u32,

	/// Toast timeout in milliseconds when the sender requests the default timeout.
	#[config(default = 5000)]
	#[garde(range(min = 1, max = 600_000))]
	pub default_timeout_ms: u32,

	/// Whether to play notification sounds. This is the master switch for both sound-file hints
	/// and themed sounds.
	#[config(default = true)]
	#[garde(skip)]
	pub sound: bool,

	/// XDG sound theme used to look up themed notification sounds. `null` (the default) auto-detects
	/// a theme: the user's GTK sound theme, then `freedesktop`, then any installed theme. A name
	/// explicitly overrides detection. Use `sound` to disable playback.
	#[garde(custom(validate_sound_theme))]
	pub sound_theme: Option<String>,
}

impl Default for Notifications {
	fn default() -> Self {
		Self {
			enabled: true,
			center_layout: NotificationCenterLayout::default(),
			toast_placement: NotificationToastPlacement::default(),
			max_visible: 3,
			default_timeout_ms: 5000,
			sound: true,
			sound_theme: None,
		}
	}
}

/// Maximum length in bytes of a sound theme or sound name.
pub const MAX_SOUND_COMPONENT_LEN: usize = 255;

/// Whether `name` is a safe single path component for sound theme and sound name lookup.
///
/// This is the authoritative validator shared by configuration and the sound player.
#[must_use]
pub fn is_valid_sound_component(name: &str) -> bool {
	!name.is_empty()
		&& name.len() <= MAX_SOUND_COMPONENT_LEN
		&& name != "."
		&& name != ".."
		&& !name.contains("..")
		&& !name.contains(['/', '\\'])
		&& !name.chars().any(char::is_control)
}

#[allow(clippy::ref_option)]
fn validate_sound_theme(value: &Option<String>, _ctx: &()) -> garde::Result {
	let Some(name) = value else {
		return Ok(());
	};
	if is_valid_sound_component(name) {
		return Ok(());
	}

	let message = if name.is_empty() {
		"sound theme name must not be empty"
	} else if name.contains(['/', '\\']) {
		"sound theme name must not contain path separators"
	} else if name.chars().any(char::is_control) {
		"sound theme name must not contain control characters"
	} else {
		"invalid sound theme name"
	};

	Err(garde::Error::new(message))
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Config, ConfigSerialize)]
pub enum NotificationCenterLayout {
	#[default]
	NewestFirst,
	GroupedByApp,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Config, ConfigSerialize)]
pub enum NotificationToastPlacement {
	#[default]
	FocusedOutput,
	BarOutput,
	AllOutputs,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn validates_sound_components() {
		assert!(is_valid_sound_component("freedesktop"));
		assert!(is_valid_sound_component("ocean-sound"));

		for name in ["", ".", "..", "a..b", "a/b", "a\\b", "a\nb", "a\u{85}b"] {
			assert!(!is_valid_sound_component(name), "{name:?}");
		}
		assert!(!is_valid_sound_component(
			&"a".repeat(MAX_SOUND_COMPONENT_LEN + 1)
		));
	}
}
