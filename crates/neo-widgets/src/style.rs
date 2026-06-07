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

pub const COLORS: Colors = Colors {
	decorative: DECORATIVE,
	feedback: FEEDBACK,

	background: DECORATIVE.pink,
	disabled_background: rgb(0xA3A_EAF),
	body: rgb(0xFFF_BEE),

	text: rgb(0x000_000),
	secondary: rgb(0x000_000),
	border: rgb(0x000_000),
	separator: rgb(0xE2E_2E2),

	white: rgb(0xFFF_FFF),
	black: rgb(0x000_000),
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
