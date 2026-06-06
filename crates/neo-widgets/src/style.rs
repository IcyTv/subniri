#![allow(dead_code)]
use iced::Color;

#[derive(Debug, Clone, Copy)]
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

#[derive(Debug, Clone, Copy)]
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

#[derive(Debug, Clone, Copy)]
pub struct Colors {
	pub decorative: Decorative,
	pub feedback: Feedback,

	pub background: Color,
	pub disabled_background: Color,
	pub body: Color,

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
	pink: rgb(0xFF8ACD),
	pink50: rgb(0xFF8ACD),
	pink70: rgb(0xFFC6E7),
	pink90: rgb(0xFFE2F3),

	purple: rgb(0xB37DFF),
	purple50: rgb(0xB37DFF),
	purple70: rgb(0xD9BEFF),
	purple90: rgb(0xF0E5FF),

	blue: rgb(0x8AF1FF),
	blue50: rgb(0x8AF1FF),
	blue70: rgb(0xB3F6FF),
	blue90: rgb(0xD6FAFF),

	yellow: rgb(0xFFE959),
	yellow50: rgb(0xFFE959),
	yellow70: rgb(0xFFF29B),
	yellow90: rgb(0xFFF8C5),

	green: rgb(0x76F7AE),
	green50: rgb(0x76F7AE),
	green70: rgb(0xADFACE),
	green90: rgb(0xCFFCE3),

	orange: rgb(0xFFB366),
	orange50: rgb(0xFFB366),
	orange70: rgb(0xFFD1A3),
	orange90: rgb(0xFFF0DD),

	coral: rgb(0xFF7F96),
	coral50: rgb(0xFF7F96),
	coral70: rgb(0xFFB8C4),
	coral90: rgb(0xFFE3E8),

	mint: rgb(0x67E8C2),
	mint50: rgb(0x67E8C2),
	mint70: rgb(0xA8F3DD),
	mint90: rgb(0xDDFBF2),

	teal: rgb(0x5FD6D2),
	teal50: rgb(0x5FD6D2),
	teal70: rgb(0x9BE9E6),
	teal90: rgb(0xD8FAF8),
};

pub const FEEDBACK: Feedback = Feedback {
	danger: rgb(0xFF5454),
	danger50: rgb(0xFF5454),
	danger90: rgb(0xFFD6D6),

	warning: rgb(0xFF9F69),
	warning50: rgb(0xFF9F69),
	warning90: rgb(0xFFEAD1),

	success: rgb(0x3CD39D),
	success50: rgb(0x3CD39D),
	success90: rgb(0xD7F8EC),

	info: rgb(0x63A9FF),
	info50: rgb(0x63A9FF),
	info90: rgb(0xDCEBFF),
};

pub const COLORS: Colors = Colors {
	decorative: DECORATIVE,
	feedback: FEEDBACK,

	background: DECORATIVE.pink,
	disabled_background: rgb(0xA3AEAF),
	body: rgb(0xFFFBEE),

	text: rgb(0x000000),
	secondary: rgb(0x000000),
	border: rgb(0x000000),
	separator: rgb(0xE2E2E2),

	white: rgb(0xFFFFFF),
	black: rgb(0x000000),
};

#[must_use]
pub fn neo_theme() -> iced::Theme {
	iced::Theme::custom(
		"Subniri Neo",
		iced::theme::palette::Seed {
			background: COLORS.body,
			text: COLORS.text,
			primary: COLORS.background,
			success: COLORS.feedback.success,
			warning: COLORS.feedback.warning,
			danger: COLORS.feedback.danger,
		},
	)
}
