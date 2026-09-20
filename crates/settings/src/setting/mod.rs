use std::{hash::Hash, time::Instant};

use config::{ConfigFile, KdlDocument};
use iced::{
	Animation, Color, Element, Task,
	widget::{Svg, svg},
};
use neo_widgets::{phosphor_icon, style::COLORS};

mod nightlight;
mod spotify;

#[derive(Clone, Debug)]
pub enum Message {
	Nightlight(nightlight::Message),
	Spotify(spotify::Message),
}

pub struct Tab {
	pub selected: Animation<bool>,
	contents: TabContents,
}

pub enum TabContents {
	Nightlight(nightlight::Nightlight),
	Homeassistant,
	Spotify(spotify::Spotify),
	MoreSoon,
}

impl Hash for Tab {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		std::mem::discriminant(&self.contents).hash(state);
	}
}

impl Tab {
	fn default_animation() -> Animation<bool> {
		Animation::new(false).quick()
	}

	pub fn nightlight() -> Self {
		Self {
			selected: Self::default_animation(),
			contents: TabContents::Nightlight(nightlight::Nightlight::default()),
		}
	}

	pub fn homeassistant() -> Self {
		Self {
			selected: Self::default_animation(),
			contents: TabContents::Homeassistant,
		}
	}

	pub fn spotify() -> Self {
		Self {
			selected: Self::default_animation(),
			contents: TabContents::Spotify(spotify::Spotify::new()),
		}
	}

	pub fn more_soon() -> Self {
		Self {
			selected: Self::default_animation(),
			contents: TabContents::MoreSoon,
		}
	}

	pub fn icon<'a>(&self) -> Svg<'a> {
		self.contents.icon()
	}

	pub fn name(&self) -> &'static str {
		self.contents.name()
	}

	pub fn accent(&self) -> Color {
		self.contents.accent()
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
		self.contents.view(config)
	}

	pub fn update(
		&mut self, config: &mut ConfigFile, doc: &mut KdlDocument, message: Message,
	) -> Task<Message> {
		match message {
			Message::Nightlight(nightlight::Message::UpdateConfig)
			| Message::Spotify(spotify::Message::UpdateConfig) => {
				if let Err(error) = config.write(doc) {
					log::error!("Failed to save config: {error}");
				}

				Task::none()
			}
			message @ (Message::Nightlight(_) | Message::Spotify(_)) => {
				self.contents.update(config, message)
			}
		}
	}

	pub fn init(&self, config: &ConfigFile) -> Task<Message> {
		self.contents.init(config)
	}
}

impl TabContents {
	fn icon<'a>(&self) -> Svg<'a> {
		match self {
			Self::Nightlight(_) => nightlight::Nightlight::icon(),
			Self::Spotify(_) => spotify::Spotify::icon(),
			Self::Homeassistant | Self::MoreSoon => svg(phosphor_icon!("question-mark")),
		}
	}

	fn name(&self) -> &'static str {
		match self {
			Self::Nightlight(_) => "Nightlight",
			Self::Homeassistant => "Homeassistant",
			Self::Spotify(_) => "Spotify",
			Self::MoreSoon => "More Soon",
		}
	}

	fn accent(&self) -> Color {
		match self {
			Self::Nightlight(_) => nightlight::Nightlight::accent_color(),
			Self::Homeassistant => COLORS.decorative.blue,
			Self::Spotify(_) => spotify::Spotify::accent_color(),
			Self::MoreSoon => COLORS.decorative.yellow,
		}
	}

	fn init(&self, config: &ConfigFile) -> Task<Message> {
		match self {
			Self::Spotify(_) => spotify::Spotify::init(config).map(Message::Spotify),
			_ => Task::none(),
		}
	}

	fn update(&mut self, config: &mut ConfigFile, message: Message) -> Task<Message> {
		match (self, message) {
			(Self::Nightlight(nightlight), Message::Nightlight(message)) => {
				nightlight.update(config, message).map(Message::Nightlight)
			}
			(Self::Spotify(spotify), Message::Spotify(message)) => {
				spotify.update(config, message).map(Message::Spotify)
			}
			_ => Task::none(),
		}
	}

	fn view<'a>(&'a self, config: &'a ConfigFile) -> Element<'a, Message> {
		match self {
			Self::Nightlight(nightlight) => nightlight.view(config).map(Message::Nightlight),
			Self::Spotify(spotify) => spotify.view(config).map(Message::Spotify),
			Self::Homeassistant | Self::MoreSoon => "".into(),
		}
	}
}
