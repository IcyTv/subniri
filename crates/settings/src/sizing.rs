//! Fixed Settings design metrics, before the application's 75% scale is applied.

pub const SCALE: f32 = 0.75;
pub const HEADER_HEIGHT: f32 = 64.0 / SCALE;
pub const HEADER_TITLE: f32 = 22.0 / SCALE;
pub const HEADER_ICON: f32 = 32.0 / SCALE;
pub const HEADER_PADDING: u16 = 8;
pub const HEADER_ICON_PADDING: u16 = 6;
pub const CARD_HEIGHT: f32 = 72.0 / SCALE;
pub const CARD_PADDING: u16 = 12;

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn reviewed_metrics_are_scaled_once() {
		let rendered = |value: f32| value * SCALE;

		assert_eq!(rendered(260.0), 195.0);
		assert_eq!(rendered(HEADER_HEIGHT), 64.0);
		assert_eq!(rendered(HEADER_TITLE), 22.0);
		assert_eq!(rendered(HEADER_ICON), 32.0);
		assert_eq!(rendered(CARD_HEIGHT), 72.0);
	}
}
