use std::{hash::Hash, time::Instant};

use config::{ConfigFile, KdlDocument};
use iced::{
	Animation, Color, Element, Task,
	widget::{Svg, svg},
};
use neo_widgets::{phosphor_icon, style::COLORS};

mod nightlight;

#[derive(Clone, Debug)]
pub enum Message {
	Nightlight(nightlight::Message),
}

#[derive(Clone)]
pub struct Tab {
	pub kind: SettingKind,
	pub selected: Animation<bool>,
	nightlight: nightlight::State,
}

impl Hash for Tab {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		self.kind.hash(state);
	}
}

impl Tab {
	fn default_animation() -> Animation<bool> {
		Animation::new(false).quick()
	}

	pub fn nightlight() -> Self {
		Self {
			kind: SettingKind::Nightlight,
			selected: Self::default_animation(),
			nightlight: nightlight::State::default(),
		}
	}

	pub fn homeassistant() -> Self {
		Self {
			kind: SettingKind::Homeassistant,
			selected: Self::default_animation(),
			nightlight: nightlight::State::default(),
		}
	}

	pub fn spotify() -> Self {
		Self {
			kind: SettingKind::Spotify,
			selected: Self::default_animation(),
			nightlight: nightlight::State::default(),
		}
	}

	pub fn more_soon() -> Self {
		Self {
			kind: SettingKind::MoreSoon,
			selected: Self::default_animation(),
			nightlight: nightlight::State::default(),
		}
	}

	pub fn icon<'a>(&'a self) -> Svg<'a> {
		self.kind.icon()
	}

	pub fn name(&self) -> &'static str {
		self.kind.name()
	}

	pub fn accent(&self) -> Color {
		self.kind.accent()
	}

	pub fn color(&self, at: Instant) -> Color {
		self.selected.interpolate(COLORS.white, self.accent(), at)
	}

	pub fn icon_bg_color(&self, at: Instant) -> Color {
		self.selected.interpolate(self.accent(), COLORS.white, at)
	}

	pub fn shadow_width(&self, at: Instant) -> f32 {
		self.selected.interpolate(4.0, 7.0, at)
	}

	pub fn is_animating(&self, at: Instant) -> bool {
		self.selected.is_animating(at)
	}

	pub fn view<'a>(&'a self, config: &'a ConfigFile) -> Element<'a, Message> {
		self.kind.view(config, &self.nightlight)
	}

	pub fn update(
		&mut self, config: &mut ConfigFile, doc: &mut KdlDocument, message: Message,
	) -> Task<Message> {
		self.kind.update(config, doc, &mut self.nightlight, message)
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingKind {
	Nightlight,
	Homeassistant,
	Spotify,
	MoreSoon,
}

impl SettingKind {
	pub fn icon<'a>(&'a self) -> Svg<'a> {
		match self {
			Self::Nightlight => nightlight::icon(),
			Self::Homeassistant => svg(phosphor_icon!("question-mark")),
			Self::Spotify => svg(phosphor_icon!("spotify-logo")),
			Self::MoreSoon => svg(phosphor_icon!("question-mark")),
		}
	}

	pub fn name(&self) -> &'static str {
		match self {
			Self::Nightlight => "Nightlight",
			Self::Homeassistant => "Homeassistant",
			Self::Spotify => "Spotify",
			Self::MoreSoon => "More Soon",
		}
	}

	pub fn accent(&self) -> Color {
		match self {
			Self::Nightlight => nightlight::accent_color(),
			Self::Homeassistant => COLORS.decorative.blue,
			Self::Spotify => COLORS.decorative.green,
			Self::MoreSoon => COLORS.decorative.yellow,
		}
	}

	pub fn update(
		&self, config: &mut ConfigFile, doc: &mut KdlDocument,
		nightlight_state: &mut nightlight::State, message: Message,
	) -> Task<Message> {
		match (self, message) {
			(_, Message::Nightlight(nightlight::Message::UpdateConfig)) => {
				if let Err(e) = config.write(doc) {
					log::error!("Failed to save config: {e}");
				}

				Task::none()
			}
			(Self::Nightlight, Message::Nightlight(message)) => {
				nightlight::update(config, nightlight_state, message).map(Message::Nightlight)
			}
			_ => Task::none(),
		}
	}

	pub fn view<'a>(
		&'a self, config: &'a ConfigFile, nightlight_state: &'a nightlight::State,
	) -> Element<'a, Message> {
		match self {
			Self::Nightlight => nightlight::view(config, nightlight_state).map(Message::Nightlight),
			Self::Homeassistant | Self::Spotify | Self::MoreSoon => "".into(),
		}
	}
}
