use std::{hash::Hash, time::Instant};

use config::{ConfigFile, KdlDocument};
use iced::{
	Animation, Border, Color, Element, Length, Task, font,
	widget::{Svg, column, row, svg, text, text_input},
};
use neo_widgets::{phosphor_icon, style::Colors, widgets::neo_card};

mod homeassistant;
mod nightlight;
mod spotify;

#[derive(Clone, Debug)]
pub enum Message {
	Nightlight(nightlight::Message),
	Spotify(spotify::Message),
	Homeassistant(homeassistant::Message),
}

pub struct Tab {
	pub selected: Animation<bool>,
	contents: TabContents,
}

pub enum TabContents {
	Appearance,
	Nightlight(nightlight::Nightlight),
	Homeassistant(homeassistant::Homeassistant),
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

	pub fn appearance() -> Self {
		Self {
			selected: Self::default_animation(),
			contents: TabContents::Appearance,
		}
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
			contents: TabContents::Homeassistant(homeassistant::Homeassistant::new()),
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

	pub fn accent(&self, colors: Colors) -> Color {
		self.contents.accent(colors)
	}

	pub fn color(&self, at: Instant, colors: Colors) -> Color {
		self.selected
			.interpolate(colors.surface, self.accent(colors), at)
	}

	pub fn icon_bg_color(&self, at: Instant, colors: Colors) -> Color {
		self.selected
			.interpolate(self.accent(colors), colors.surface, at)
	}

	pub fn shadow_width(&self, at: Instant) -> f32 {
		self.selected.interpolate(4.0, 7.0, at)
	}

	pub fn is_animating(&self, at: Instant) -> bool {
		self.selected.is_animating(at)
	}

	pub fn view<'a>(&'a self, config: &'a ConfigFile, colors: Colors) -> Element<'a, Message> {
		self.contents.view(config, colors)
	}

	pub fn update(
		&mut self, config: &mut ConfigFile, doc: &mut KdlDocument, message: Message,
	) -> Task<Message> {
		match message {
			message @ (Message::Nightlight(nightlight::Message::UpdateConfig)
			| Message::Spotify(spotify::Message::UpdateConfig)
			| Message::Homeassistant(homeassistant::Message::UpdateConfig)) => {
				// Commit only this integration's edit under the shared writer lock.
				// Other processes may have changed appearance or menu configuration.
				let result = ConfigFile::update(|fresh| match message {
					Message::Nightlight(_) => fresh.nightlight = config.nightlight.clone(),
					Message::Spotify(_) => fresh.spotify = config.spotify.clone(),
					Message::Homeassistant(_) => fresh.homeassistant = config.homeassistant.clone(),
				});

				match result {
					Ok((fresh_doc, fresh)) => {
						*doc = fresh_doc;
						*config = fresh;
					}
					Err(error) => log::error!("Failed to save config: {error}"),
				}

				Task::none()
			}
			message
			@ (Message::Nightlight(_) | Message::Spotify(_) | Message::Homeassistant(_)) => {
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
			Self::Appearance => svg(phosphor_icon!("palette")),
			Self::Nightlight(_) => nightlight::Nightlight::icon(),
			Self::Spotify(_) => spotify::Spotify::icon(),
			Self::Homeassistant(_) => homeassistant::Homeassistant::icon(),
			Self::MoreSoon => svg(phosphor_icon!("question-mark")),
		}
	}

	fn name(&self) -> &'static str {
		match self {
			Self::Appearance => "Appearance",
			Self::Nightlight(_) => "Nightlight",
			Self::Homeassistant(_) => "Homeassistant",
			Self::Spotify(_) => "Spotify",
			Self::MoreSoon => "More Soon",
		}
	}

	fn accent(&self, colors: Colors) -> Color {
		match self {
			Self::Appearance => colors.decorative.purple,
			Self::Nightlight(_) => nightlight::Nightlight::accent_color(colors),
			Self::Homeassistant(_) => homeassistant::Homeassistant::accent_color(colors),
			Self::Spotify(_) => spotify::Spotify::accent_color(colors),
			Self::MoreSoon => colors.decorative.yellow,
		}
	}

	fn init(&self, config: &ConfigFile) -> Task<Message> {
		match self {
			Self::Spotify(_) => spotify::Spotify::init(config).map(Message::Spotify),
			Self::Homeassistant(_) => {
				homeassistant::Homeassistant::init(config).map(Message::Homeassistant)
			}
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
			(Self::Homeassistant(homeassistant), Message::Homeassistant(message)) => homeassistant
				.update(config, message)
				.map(Message::Homeassistant),
			_ => Task::none(),
		}
	}

	fn view<'a>(&'a self, config: &'a ConfigFile, colors: Colors) -> Element<'a, Message> {
		match self {
			Self::Nightlight(nightlight) => {
				nightlight.view(config, colors).map(Message::Nightlight)
			}
			Self::Spotify(spotify) => spotify.view(config, colors).map(Message::Spotify),
			Self::Homeassistant(homeassistant) => homeassistant
				.view(config, colors)
				.map(Message::Homeassistant),
			Self::Appearance | Self::MoreSoon => "".into(),
		}
	}
}

#[derive(Debug)]
struct InputCardOptions<
	'a,
	Message,
	D: text::IntoFragment<'a>,
	V: text::IntoFragment<'a>,
	FI: Fn(String) -> Message + 'a,
> {
	colors: Colors,
	background: Color,
	border: Color,
	display: D,
	value: V,
	placeholder: &'a str,
	on_input: FI,
}

impl<
	'a,
	Message,
	D: text::IntoFragment<'a>,
	V: text::IntoFragment<'a>,
	FI: Fn(String) -> Message + 'a,
> InputCardOptions<'a, Message, D, V, FI>
{
	fn new(
		value: V, on_input: FI, colors: Colors,
	) -> InputCardOptions<'a, Message, &'a str, V, FI> {
		InputCardOptions {
			colors,
			background: colors.surface,
			border: colors.border,
			display: "Input",
			value,
			placeholder: "Enter value",
			on_input,
		}
	}

	fn background(mut self, color: Color) -> Self {
		self.background = color;
		self
	}

	fn border(mut self, color: Color) -> Self {
		self.border = color;
		self
	}

	fn display(mut self, display: D) -> Self {
		self.display = display;
		self
	}

	fn placeholder(mut self, placeholder: &'a str) -> Self {
		self.placeholder = placeholder;
		self
	}
}

fn input_card<'a, Message: Clone + 'a>(
	options: InputCardOptions<
		'a,
		Message,
		impl text::IntoFragment<'a>,
		impl text::IntoFragment<'a>,
		impl Fn(String) -> Message + 'a,
	>,
) -> Element<'a, Message> {
	neo_card(
		column![
			row![
				text(options.display)
					.width(Length::Fill)
					.color(options.colors.text)
					.size(18)
					.weight(font::Weight::Bold)
			]
			.spacing(8),
			text_input(options.placeholder, options.value)
				.on_input(options.on_input)
				.style(move |_, _status| {
					text_input::Style {
						border: Border {
							width: 2.0,
							color: options.border,
							radius: 3.into(),
						},
						background: options.colors.surface.into(),
						placeholder: options.colors.secondary.scale_alpha(0.76),
						value: options.colors.text,
						selection: options.colors.selection,
					}
				})
				.padding(6)
		]
		.spacing(12),
	)
	.with_colors(options.colors)
	.width(Length::Fill)
	.height(crate::sizing::CARD_HEIGHT)
	.padding(crate::sizing::CARD_PADDING)
	.background(options.background)
	.into()
}
