#![allow(dead_code)]
use iced::Color;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decorative {
	pub pink: Color,
	pub pink50: Color,
	pub pink70: Color,
	pub pink90: Color,
	pub purple: Color,
	pub purple50: Color,
	pub purple70: Color,
	pub purple90: Color,
	pub blue: Color,
	pub blue50: Color,
	pub blue70: Color,
	pub blue90: Color,
	pub yellow: Color,
	pub yellow50: Color,
	pub yellow70: Color,
	pub yellow90: Color,
	pub green: Color,
	pub green50: Color,
	pub green70: Color,
	pub green90: Color,
	pub orange: Color,
	pub orange50: Color,
	pub orange70: Color,
	pub orange90: Color,
	pub coral: Color,
	pub coral50: Color,
	pub coral70: Color,
	pub coral90: Color,
	pub mint: Color,
	pub mint50: Color,
	pub mint70: Color,
	pub mint90: Color,
	pub teal: Color,
	pub teal50: Color,
	pub teal70: Color,
	pub teal90: Color,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Feedback {
	pub danger: Color,
	pub danger50: Color,
	pub danger90: Color,
	pub warning: Color,
	pub warning50: Color,
	pub warning90: Color,
	pub success: Color,
	pub success50: Color,
	pub success90: Color,
	pub info: Color,
	pub info50: Color,
	pub info90: Color,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
	pub decorative: Decorative,
	pub feedback: Feedback,

	pub background: Color,
	pub canvas: Color,
	pub disabled_background: Color,
	pub body: Color,
	pub surface: Color,
	pub raised: Color,
	pub selection: Color,
	pub muted: Color,
	pub on_accent: Color,

	pub text: Color,
	pub secondary: Color,
	pub border: Color,
	pub separator: Color,

	pub white: Color,
	pub black: Color,
}

pub const fn rgb(hex: u32) -> Color {
	Color {
		r: ((hex >> 16) & 0xff) as f32 / 255.0,
		g: ((hex >> 8) & 0xff) as f32 / 255.0,
		b: (hex & 0xff) as f32 / 255.0,
		a: 1.0,
	}
}

pub const DECORATIVE: Decorative = Decorative {
	pink: rgb(0xFF8_ACD),
	pink50: rgb(0xFF8_ACD),
	pink70: rgb(0xFFC_6E7),
	pink90: rgb(0xFFE_2F3),

	purple: rgb(0xB37_DFF),
	purple50: rgb(0xB37_DFF),
	purple70: rgb(0xD9B_EFF),
	purple90: rgb(0xF0E_5FF),

	blue: rgb(0x8AF_1FF),
	blue50: rgb(0x8AF_1FF),
	blue70: rgb(0xB3F_6FF),
	blue90: rgb(0xD6F_AFF),

	yellow: rgb(0xFFE_959),
	yellow50: rgb(0xFFE_959),
	yellow70: rgb(0xFFF_29B),
	yellow90: rgb(0xFFF_8C5),

	green: rgb(0x76F_7AE),
	green50: rgb(0x76F_7AE),
	green70: rgb(0xADF_ACE),
	green90: rgb(0xCFF_CE3),

	orange: rgb(0xFFB_366),
	orange50: rgb(0xFFB_366),
	orange70: rgb(0xFFD_1A3),
	orange90: rgb(0xFFF_0DD),

	coral: rgb(0xFF7_F96),
	coral50: rgb(0xFF7_F96),
	coral70: rgb(0xFFB_8C4),
	coral90: rgb(0xFFE_3E8),

	mint: rgb(0x67E_8C2),
	mint50: rgb(0x67E_8C2),
	mint70: rgb(0xA8F_3DD),
	mint90: rgb(0xDDF_BF2),

	teal: rgb(0x5FD_6D2),
	teal50: rgb(0x5FD_6D2),
	teal70: rgb(0x9BE_9E6),
	teal90: rgb(0xD8F_AF8),
};

pub const FEEDBACK: Feedback = Feedback {
	danger: rgb(0xFF5_454),
	danger50: rgb(0xFF5_454),
	danger90: rgb(0xFFD_6D6),

	warning: rgb(0xFF9_F69),
	warning50: rgb(0xFF9_F69),
	warning90: rgb(0xFFE_AD1),

	success: rgb(0x3CD_39D),
	success50: rgb(0x3CD_39D),
	success90: rgb(0xD7F_8EC),

	info: rgb(0x63A_9FF),
	info50: rgb(0x63A_9FF),
	info90: rgb(0xDCE_BFF),
};

pub const LIGHT: Colors = Colors {
	decorative: DECORATIVE,
	feedback: FEEDBACK,

	background: DECORATIVE.pink,
	canvas: DECORATIVE.pink70,
	disabled_background: rgb(0xA3A_EAF),
	body: rgb(0xFFF_BEE),
	surface: rgb(0xFFF_FFF),
	raised: rgb(0xE2E2E2),
	selection: DECORATIVE.blue90,
	muted: rgb(0xA3A_EAF),
	on_accent: rgb(0x000_000),

	text: rgb(0x000_000),
	secondary: rgb(0x000_000),
	border: rgb(0x000_000),
	separator: rgb(0xE2E_2E2),

	white: rgb(0xFFF_FFF),
	black: rgb(0x000_000),
};

/// Compatibility palette for callers not yet using a theme context.
pub const COLORS: Colors = LIGHT;

/// Punchy Dark accents on neutral charcoal surfaces, preserving module colour roles.
pub const DARK: Colors = Colors {
	decorative: Decorative {
		pink: rgb(0xF28DB7),
		pink50: rgb(0xF28DB7),
		pink70: rgb(0x815068),
		pink90: rgb(0x3F2F36),

		purple: rgb(0xBDA1ED),
		purple50: rgb(0xBDA1ED),
		purple70: rgb(0x695980),
		purple90: rgb(0x372F43),

		blue: rgb(0x8DBEFF),
		blue50: rgb(0x8DBEFF),
		blue70: rgb(0x506B8C),
		blue90: rgb(0x2E3743),

		yellow: rgb(0xF2CE66),
		yellow50: rgb(0xF2CE66),
		yellow70: rgb(0x81703E),
		yellow90: rgb(0x403A2B),

		green: rgb(0x9ED17B),
		green50: rgb(0x9ED17B),
		green70: rgb(0x587342),
		green90: rgb(0x313B2D),

		orange: rgb(0xF1A26C),
		orange50: rgb(0xF1A26C),
		orange70: rgb(0x815B40),
		orange90: rgb(0x403329),

		coral: rgb(0xF28383),
		coral50: rgb(0xF28383),
		coral70: rgb(0x814C4C),
		coral90: rgb(0x402E2E),

		mint: rgb(0x69D6CF),
		mint50: rgb(0x69D6CF),
		mint70: rgb(0x407572),
		mint90: rgb(0x2B3B39),

		teal: rgb(0x69D6CF),
		teal50: rgb(0x69D6CF),
		teal70: rgb(0x407572),
		teal90: rgb(0x2B3B39),
	},
	feedback: Feedback {
		danger: rgb(0xF28383),
		danger50: rgb(0xF28383),
		danger90: rgb(0x402E2E),

		warning: rgb(0xF1A26C),
		warning50: rgb(0xF1A26C),
		warning90: rgb(0x403329),

		success: rgb(0x9ED17B),
		success50: rgb(0x9ED17B),
		success90: rgb(0x313B2D),

		info: rgb(0x8DBEFF),
		info50: rgb(0x8DBEFF),
		info90: rgb(0x2E3743),
	},
	background: rgb(0x202020),
	canvas: rgb(0x171717),
	disabled_background: rgb(0x292929),
	body: rgb(0x202020),
	surface: rgb(0x303030),
	raised: rgb(0x3B3B3B),
	selection: rgb(0x3B3B3B),
	muted: rgb(0x938E85),
	on_accent: rgb(0x181818),
	text: rgb(0xE4DED3),
	secondary: rgb(0xB5AEA3),
	border: rgb(0x000000),
	separator: rgb(0x50504C),
	white: rgb(0xFFFBEE),
	black: rgb(0x000000),
};

fn interpolate_color(source: Color, target: Color, t: f32) -> Color {
	Color {
		r: source.r + (target.r - source.r) * t,
		g: source.g + (target.g - source.g) * t,
		b: source.b + (target.b - source.b) * t,
		a: source.a + (target.a - source.a) * t,
	}
}

macro_rules! interpolate_fields {
	($source:ident, $target:ident, $t:ident; $($field:ident),+ $(,)?) => {
		Self {
			$($field: interpolate_color($source.$field, $target.$field, $t)),+
		}
	};
}

impl Decorative {
	fn interpolate(self, target: Self, t: f32) -> Self {
		interpolate_fields!(self, target, t;
			pink, pink50, pink70, pink90,
			purple, purple50, purple70, purple90,
			blue, blue50, blue70, blue90,
			yellow, yellow50, yellow70, yellow90,
			green, green50, green70, green90,
			orange, orange50, orange70, orange90,
			coral, coral50, coral70, coral90,
			mint, mint50, mint70, mint90,
			teal, teal50, teal70, teal90,
		)
	}
}

impl Feedback {
	fn interpolate(self, target: Self, t: f32) -> Self {
		interpolate_fields!(self, target, t;
			danger, danger50, danger90,
			warning, warning50, warning90,
			success, success50, success90,
			info, info50, info90,
		)
	}
}

impl Colors {
	/// Prefer ordinary semantic text when readable, otherwise contrast it with the fill.
	/// This also keeps selected controls readable while their fill animates.
	pub fn text_on(self, background: Color) -> Color {
		fn luminance(color: Color) -> f32 {
			let linear = |value: f32| {
				if value <= 0.04045 {
					value / 12.92
				} else {
					((value + 0.055) / 1.055).powf(2.4)
				}
			};

			0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
		}

		let background = luminance(background);
		let contrast = |foreground| {
			let foreground = luminance(foreground);
			(foreground.max(background) + 0.05) / (foreground.min(background) + 0.05)
		};

		if contrast(self.text) >= 4.5 {
			self.text
		} else if contrast(self.on_accent) >= 4.5 {
			self.on_accent
		} else if contrast(self.white) > contrast(self.text).max(contrast(self.on_accent)) {
			self.white
		} else if contrast(self.on_accent) > contrast(self.text) {
			self.on_accent
		} else {
			self.text
		}
	}

	pub const fn for_theme(theme: config::AppearanceMode) -> Self {
		match theme {
			config::AppearanceMode::Light => LIGHT,
			config::AppearanceMode::Dark => DARK,
		}
	}

	/// Blend every palette role, including alpha, clamping to exact endpoints.
	#[must_use]
	pub fn interpolate(self, target: Self, t: f32) -> Self {
		if t <= 0.0 || t.is_nan() {
			return self;
		}
		if t >= 1.0 {
			return target;
		}

		Self {
			decorative: self.decorative.interpolate(target.decorative, t),
			feedback: self.feedback.interpolate(target.feedback, t),
			background: interpolate_color(self.background, target.background, t),
			canvas: interpolate_color(self.canvas, target.canvas, t),
			disabled_background: interpolate_color(
				self.disabled_background,
				target.disabled_background,
				t,
			),
			body: interpolate_color(self.body, target.body, t),
			surface: interpolate_color(self.surface, target.surface, t),
			raised: interpolate_color(self.raised, target.raised, t),
			selection: interpolate_color(self.selection, target.selection, t),
			muted: interpolate_color(self.muted, target.muted, t),
			on_accent: interpolate_color(self.on_accent, target.on_accent, t),
			text: interpolate_color(self.text, target.text, t),
			secondary: interpolate_color(self.secondary, target.secondary, t),
			border: interpolate_color(self.border, target.border, t),
			separator: interpolate_color(self.separator, target.separator, t),
			white: interpolate_color(self.white, target.white, t),
			black: interpolate_color(self.black, target.black, t),
		}
	}

	#[must_use]
	pub fn iced_theme(self) -> iced::Theme {
		iced::Theme::custom(
			"Subniri Neo",
			iced::theme::palette::Seed {
				background: self.body,
				text: self.text,
				primary: self.decorative.pink,
				success: self.feedback.success,
				warning: self.feedback.warning,
				danger: self.feedback.danger,
			},
		)
	}
}

#[must_use]
pub fn neo_theme() -> iced::Theme {
	COLORS.iced_theme()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn dark_accents_match_the_approved_palette_without_recolouring_light() {
		let accents = DARK.decorative;

		assert_eq!(accents.pink, rgb(0xF28DB7));
		assert_eq!(accents.teal, rgb(0x69D6CF));
		assert_eq!(accents.blue, rgb(0x8DBEFF));
		assert_eq!(accents.yellow, rgb(0xF2CE66));
		assert_eq!(accents.green, rgb(0x9ED17B));
		assert_eq!(accents.orange, rgb(0xF1A26C));
		assert_eq!(accents.coral, rgb(0xF28383));
		assert_eq!(accents.purple, rgb(0xBDA1ED));
		assert_eq!(LIGHT.decorative, DECORATIVE);
		assert_eq!(LIGHT.feedback, FEEDBACK);
	}

	#[test]
	fn raised_component_surfaces_are_neutral_without_changing_text_selection() {
		assert_eq!(LIGHT.raised, rgb(0xE2E2E2));
		assert_eq!(DARK.raised, rgb(0x3B3B3B));
		assert_eq!(LIGHT.selection, DECORATIVE.blue90);

		for colors in [LIGHT, DARK] {
			assert_eq!(colors.raised.r, colors.raised.g);
			assert_eq!(colors.raised.g, colors.raised.b);
			assert_eq!(colors.text_on(colors.raised), colors.text);
		}
	}

	#[test]
	fn semantic_foregrounds_contrast_with_neutral_and_accent_fills() {
		assert_eq!(DARK.text_on(DARK.surface), DARK.text);
		assert_eq!(DARK.text_on(DARK.black), DARK.text);
		assert_eq!(DARK.text_on(DARK.decorative.pink), DARK.on_accent);
		assert_eq!(DARK.text_on(DARK.decorative.pink70), DARK.text);
		assert_eq!(LIGHT.text_on(LIGHT.surface), LIGHT.text);
		assert_eq!(LIGHT.text_on(LIGHT.black), LIGHT.white);
	}

	#[test]
	fn interpolation_has_exact_clamped_endpoints() {
		assert_eq!(LIGHT.interpolate(DARK, 0.0), LIGHT);
		assert_eq!(LIGHT.interpolate(DARK, 1.0), DARK);
		assert_eq!(LIGHT.interpolate(DARK, -1.0), LIGHT);
		assert_eq!(LIGHT.interpolate(DARK, 2.0), DARK);
		assert_eq!(LIGHT.interpolate(DARK, f32::NAN), LIGHT);
	}

	#[test]
	fn interpolation_blends_color_and_alpha_and_preserves_black_borders() {
		let source = Color {
			a: 0.0,
			..rgb(0x000000)
		};
		let target = rgb(0xFFFFFF);

		assert_eq!(
			interpolate_color(source, target, 0.5),
			Color::from_rgba(0.5, 0.5, 0.5, 0.5)
		);
		assert_eq!(LIGHT.interpolate(DARK, 0.5).border, rgb(0x000000));
		assert_eq!(COLORS, LIGHT);
		assert_eq!(Colors::for_theme(config::AppearanceMode::Dark), DARK);
	}
}
