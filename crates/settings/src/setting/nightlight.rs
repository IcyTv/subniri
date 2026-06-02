use config::ConfigFile;
use iced::{
	Alignment, Border, Color, Element, Length, Task,
	widget::{Svg, column, container, grid, row, space, svg, text},
};
use iced_runtime::core::font;
use jiff::ToSpan;
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{neo_button, neo_card, neo_slider, neo_toggle},
};
use num_traits::{AsPrimitive, Num, NumCast};

#[derive(Clone)]
pub enum Message {
	IncreaseDawn,
	DecreaseDawn,
	IncreaseDusk,
	DecreaseDusk,

	UpdateDayTemp(u32),
	UpdateDayBright(f64),
	UpdateNightTemp(u32),
	UpdateNightBright(f64),
	PreviewDayTemp(u32),
	PreviewDayBright(f64),
	PreviewNightTemp(u32),
	PreviewNightBright(f64),

	UpdateConfig,
}

#[derive(Clone, Default)]
pub struct State {
	preview_day_temp: Option<u32>,
	preview_day_bright: Option<f64>,
	preview_night_temp: Option<u32>,
	preview_night_bright: Option<f64>,
}

#[inline(always)]
pub fn accent_color() -> Color {
	COLORS.decorative.orange
}

pub fn icon<'a>() -> Svg<'a> {
	svg(phosphor_icon!("lightbulb-filament"))
}

pub fn update(config: &mut ConfigFile, state: &mut State, message: Message) -> Task<Message> {
	match message {
		Message::IncreaseDawn => {
			if let Some(dawn) = config.nightlight.dawn.as_mut() {
				*dawn += 15.minutes();
				return Task::done(Message::UpdateConfig);
			}
			return Task::none();
		}
		Message::DecreaseDawn => {
			if let Some(dawn) = config.nightlight.dawn.as_mut() {
				*dawn -= 15.minutes();
				return Task::done(Message::UpdateConfig);
			}
			return Task::none();
		}
		Message::IncreaseDusk => {
			if let Some(dusk) = config.nightlight.dusk.as_mut() {
				*dusk += 15.minutes();
				return Task::done(Message::UpdateConfig);
			}
			return Task::none();
		}
		Message::DecreaseDusk => {
			if let Some(dusk) = config.nightlight.dusk.as_mut() {
				*dusk -= 15.minutes();
			}
			return Task::none();
		}
		Message::PreviewDayTemp(temp) => {
			state.preview_day_temp = Some(temp);
			return Task::none();
		}
		Message::PreviewDayBright(bright) => {
			state.preview_day_bright = Some(bright);
			return Task::none();
		}
		Message::PreviewNightTemp(temp) => {
			state.preview_night_temp = Some(temp);
			return Task::none();
		}
		Message::PreviewNightBright(bright) => {
			state.preview_night_bright = Some(bright);
			return Task::none();
		}
		Message::UpdateDayTemp(temp) => {
			state.preview_day_temp = None;
			config.nightlight.day.temperature = temp;
		}
		Message::UpdateDayBright(bright) => {
			state.preview_day_bright = None;
			config.nightlight.day.brightness = bright;
		}
		Message::UpdateNightTemp(temp) => {
			state.preview_night_temp = None;
			config.nightlight.night.temperature = temp;
		}
		Message::UpdateNightBright(bright) => {
			state.preview_night_bright = None;
			config.nightlight.night.brightness = bright;
		}
		_ => return Task::none(),
	}

	Task::done(Message::UpdateConfig)
}

pub fn view<'a>(config: &'a ConfigFile, state: &'a State) -> Element<'a, Message> {
	let dawn = config
		.nightlight
		.dawn
		.unwrap_or(jiff::civil::Time::new(7, 0, 0, 0).unwrap());

	let dusk = config
		.nightlight
		.dusk
		.unwrap_or(jiff::civil::Time::new(21, 30, 0, 0).unwrap());
	let day_temp = state
		.preview_day_temp
		.unwrap_or(config.nightlight.day.temperature);
	let day_bright = state
		.preview_day_bright
		.unwrap_or(config.nightlight.day.brightness);
	let night_temp = state
		.preview_night_temp
		.unwrap_or(config.nightlight.night.temperature);
	let night_bright = state
		.preview_night_bright
		.unwrap_or(config.nightlight.night.brightness);

	column![
		neo_card(
			row![
				container(icon())
					.width(58)
					.height(58)
					.padding(12)
					.style(|_| container::Style {
						background: Some(iced::Background::Color(COLORS.white)),
						border: Border {
							color: COLORS.border,
							width: 2.0,
							radius: 3.into(),
						},
						..Default::default()
					}),
				column![
					text("NIGHTLIGHT")
						.width(Length::Fill)
						.color(COLORS.text)
						.size(34)
						.weight(font::Weight::Bold)
						.stretch(font::Stretch::ExtraExpanded)
						.wrapping(text::Wrapping::WordOrGlyph),
					text("Save your eyes")
						.width(Length::Fill)
						.color(COLORS.text.scale_alpha(0.76))
						.size(14)
						.weight(font::Weight::Bold)
				]
				.spacing(4)
			]
			.align_y(Alignment::Center)
			.spacing(16)
		)
		.width(Length::Fill)
		.height(116)
		.background(accent_color())
		.padding(18),
		// Settings
		grid![
			slider_card(
				SliderCardArgs::new()
					.name("DAY TEMP")
					.value(day_temp)
					.display(format!("{}K", day_temp))
					.range(1000..=10_000)
					.step(10)
					.accent(COLORS.decorative.yellow)
					.handle(COLORS.decorative.pink)
					.background(COLORS.decorative.yellow90)
					.on_change_live(Message::PreviewDayTemp)
					.on_change(Message::UpdateDayTemp)
			),
				slider_card(
					SliderCardArgs::new()
						.name("DAY BRIGHT")
						.value(day_bright)
						.display(format!("{:.0}%", day_bright * 100.0))
						.range(0.0..=1.0)
						.step(0.01)
						.accent(COLORS.decorative.green)
						.handle(COLORS.decorative.yellow)
						.background(COLORS.decorative.green90)
						.on_change_live(Message::PreviewDayBright)
						.on_change(Message::UpdateDayBright)
			),
				slider_card(
					SliderCardArgs::new()
						.name("NIGHT TEMP")
						.value(night_temp)
						.display(format!("{}K", night_temp))
						.range(1000..=10_000)
						.step(10)
						.accent(COLORS.decorative.purple70)
						.handle(COLORS.decorative.blue)
						.background(COLORS.decorative.purple90)
						.on_change_live(Message::PreviewNightTemp)
						.on_change(Message::UpdateNightTemp)
			),
				slider_card(
					SliderCardArgs::new()
						.name("NIGHT BRIGHT")
						.value(night_bright)
						.display(format!(
							"{:.0}%",
							night_bright * 100.0
						))
						.range(0.0..=1.0)
						.step(0.01)
						.accent(COLORS.decorative.blue)
						.handle(COLORS.decorative.yellow)
						.background(COLORS.decorative.blue90)
						.on_change_live(Message::PreviewNightBright)
						.on_change(Message::UpdateNightBright)
			)
		]
		.height(Length::Shrink)
		.columns(2)
		.spacing(14),
		neo_card(
			column![
				row![
					container("")
						.height(Length::Fill)
						.width(8)
						.style(|_| container::Style {
							background: Some(iced::Background::Color(COLORS.decorative.pink)),
							border: Border {
								color: COLORS.border,
								width: 2.0,
								radius: 3.into(),
							},
							..Default::default()
						}),
					column![
						text("USE LOCATION")
							.color(COLORS.text)
							.size(18)
							.weight(font::Weight::Bold),
						text("Let geoclue and weather data determine dusk and dawn")
							.color(COLORS.text.scale_alpha(0.7))
							.size(13)
							.weight(font::Weight::Bold)
							.wrapping(text::Wrapping::Word)
					]
					.spacing(4)
					.width(Length::Fill),
					neo_toggle()
						.toggled(config.nightlight.use_location)
						.width(40)
						.height(18)
						.enabled(false),
				]
				.align_y(Alignment::Center)
				.spacing(12),
				container("")
					.height(2)
					.width(Length::Fill)
					.style(|_| container::Style {
						background: Some(iced::Background::Color(COLORS.black)),
						..Default::default()
					}),
				column![
					text("OR SET CUSTOM TIMES")
						.color(COLORS.text)
						.size(18)
						.weight(font::Weight::Bold),
					text("Set custom times for the night mode to work. Dusk is when it turns on the night settings, Dawn when it turns on the day settings.")
						.color(COLORS.text.scale_alpha(0.7))
						.size(13)
						.weight(font::Weight::Bold)
						.wrapping(text::Wrapping::Word)
				]
				.spacing(4)
				.width(Length::Fill),

				row![
					time_container("DAWN", dawn, COLORS.decorative.orange70, Message::DecreaseDawn, Message::IncreaseDawn),
					time_container("DUSK", dusk, COLORS.decorative.purple70, Message::DecreaseDusk, Message::IncreaseDusk)
				]
				.spacing(10),
			]
			.spacing(12)
		)
		.width(Length::Fill)
		.padding(16),
	]
	.spacing(16)
	.into()
}

struct SliderCardArgs<'a, T, D, F, L> {
	name: &'a str,
	value: T,
	value_display: D,
	range: std::ops::RangeInclusive<T>,
	step: T,
	accent: Color,
	handle_color: Color,
	background_color: Color,
	on_change: Option<F>,
	on_change_live: Option<L>,
}

impl<'a, T: Default, D: Default, F, L> SliderCardArgs<'a, T, D, F, L> {
	pub fn new() -> Self {
		Self {
			name: "<not set>",
			value: T::default(),
			value_display: D::default(),
			range: T::default()..=T::default(),
			step: T::default(),
			accent: COLORS.white,
			handle_color: COLORS.decorative.pink,
			background_color: COLORS.white,
			on_change: None,
			on_change_live: None,
		}
	}

	pub fn name(mut self, name: &'static str) -> Self {
		self.name = name;
		self
	}

	pub fn value(mut self, value: T) -> Self {
		self.value = value;
		self
	}

	pub fn range(mut self, range: std::ops::RangeInclusive<T>) -> Self {
		self.range = range;
		self
	}

	pub fn display(mut self, display: D) -> Self {
		self.value_display = display;
		self
	}

	pub fn step(mut self, step: T) -> Self {
		self.step = step;
		self
	}

	pub fn accent(mut self, color: Color) -> Self {
		self.accent = color;
		self
	}

	pub fn handle(mut self, color: Color) -> Self {
		self.handle_color = color;
		self
	}

	pub fn background(mut self, color: Color) -> Self {
		self.background_color = color;
		self
	}

	pub fn on_change(mut self, func: F) -> Self {
		self.on_change = Some(func);
		self
	}

	pub fn on_change_live(mut self, func: L) -> Self {
		self.on_change_live = Some(func);
		self
	}
}

fn slider_card<'a, T: Num + NumCast + AsPrimitive<f32> + Clone>(
	args: SliderCardArgs<
		'a,
		T,
		impl text::IntoFragment<'a>,
		impl Fn(T) -> Message + 'static,
		impl Fn(T) -> Message + 'static,
	>,
) -> Element<'a, Message> {
	let color = args.accent;
	neo_card(
		column![
			row![
				text(args.name)
					.width(Length::Fill)
					.color(COLORS.text)
					.size(18)
					.weight(font::Weight::Bold),
				container(
					text(args.value_display)
						.weight(font::Weight::Bold)
						.size(16)
						.color(COLORS.text)
						.center()
				)
				.center_x(86)
				.center_y(32)
				.style(move |_| container::Style {
					background: Some(iced::Background::Color(color)),
					border: Border {
						color: COLORS.border,
						width: 2.0,
						radius: 3.into(),
					},
					..Default::default()
				})
			]
			.spacing(8),
			neo_slider(args.range, args.value)
				.step(args.step)
				.running_color(args.accent)
				.handle_color(args.handle_color)
				.on_change_live_maybe(args.on_change_live)
				.on_change_maybe(args.on_change)
		]
		.spacing(12),
	)
	.width(Length::Fill)
	.height(112)
	.padding(16)
	.background(args.background_color)
	.into()
}

fn time_container<'a>(
	name: &'a str, time: jiff::civil::Time, color: Color, decrease: Message, increase: Message,
) -> Element<'a, Message> {
	container(
		row![
			text(name)
				.color(COLORS.text)
				.size(18)
				.weight(font::Weight::Bold),
			space::horizontal(),
			neo_button(svg(phosphor_icon!("minus")))
				.width(38)
				.on_press(decrease),
			neo_card(
				text(time.strftime("%H:%M").to_string())
					.color(COLORS.text)
					.size(18)
					.weight(font::Weight::Bold),
			)
			.width(82)
			.background(COLORS.white),
			neo_button(svg(phosphor_icon!("plus")))
				.width(38)
				.on_press(increase),
		]
		.spacing(6)
		.align_y(Alignment::Center)
		.height(Length::Fill),
	)
	.width(Length::Fill)
	.padding([4, 12])
	.center_y(54)
	.style(move |_| container::Style {
		background: Some(iced::Background::Color(color)),
		border: Border {
			color: COLORS.border,
			width: 2.0,
			radius: 3.into(),
		},
		..Default::default()
	})
	.into()
}
