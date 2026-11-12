use config::ConfigFile;
use iced::{
	Alignment, Border, Color, Element, Length, Task,
	widget::{Svg, column, container, grid, row, space, svg, text},
};
use iced_runtime::core::font;
use jiff::ToSpan;
use neo_widgets::{
	phosphor_icon,
	style::Colors,
	widgets::{neo_button, neo_card, neo_slider, neo_toggle},
};
use num_traits::{AsPrimitive, Num, NumCast};

#[derive(Clone, Copy, Debug)]
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

	ToggleNightlight(bool),

	UpdateConfig,
}

#[derive(Clone, Default)]
pub struct Nightlight {
	day_temp: Option<u32>,
	day_bright: Option<f64>,
	night_temp: Option<u32>,
	night_bright: Option<f64>,
}

impl Nightlight {
	pub fn accent_color(colors: Colors) -> Color {
		colors.decorative.orange
	}

	pub fn icon<'a>() -> Svg<'a> {
		svg(phosphor_icon!("lightbulb-filament"))
	}

	pub fn update(&mut self, config: &mut ConfigFile, message: Message) -> Task<Message> {
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
				self.day_temp = Some(temp);
				return Task::none();
			}
			Message::PreviewDayBright(bright) => {
				self.day_bright = Some(bright);
				return Task::none();
			}
			Message::PreviewNightTemp(temp) => {
				self.night_temp = Some(temp);
				return Task::none();
			}
			Message::PreviewNightBright(bright) => {
				self.night_bright = Some(bright);
				return Task::none();
			}
			Message::UpdateDayTemp(temp) => {
				self.day_temp = None;
				config.nightlight.day.temperature = temp;
			}
			Message::UpdateDayBright(bright) => {
				self.day_bright = None;
				config.nightlight.day.brightness = bright;
			}
			Message::UpdateNightTemp(temp) => {
				self.night_temp = None;
				config.nightlight.night.temperature = temp;
			}
			Message::UpdateNightBright(bright) => {
				self.night_bright = None;
				config.nightlight.night.brightness = bright;
			}
			Message::ToggleNightlight(on) => {
				config.nightlight.enabled = on;
			}
			Message::UpdateConfig => return Task::none(),
		}

		Task::done(Message::UpdateConfig)
	}

	#[allow(clippy::too_many_lines)]
	pub fn view<'a>(&'a self, config: &'a ConfigFile, colors: Colors) -> Element<'a, Message> {
		let dawn = config
			.nightlight
			.dawn
			.unwrap_or_else(|| fallback_time(7, 0));

		let dusk = config
			.nightlight
			.dusk
			.unwrap_or_else(|| fallback_time(21, 30));
		let day_temp = self.day_temp.unwrap_or(config.nightlight.day.temperature);
		let day_bright = self.day_bright.unwrap_or(config.nightlight.day.brightness);
		let night_temp = self
			.night_temp
			.unwrap_or(config.nightlight.night.temperature);
		let night_bright = self
			.night_bright
			.unwrap_or(config.nightlight.night.brightness);

		column![
		neo_card(
			row![
				container(Self::icon())
					.width(crate::sizing::HEADER_ICON)
					.height(crate::sizing::HEADER_ICON)
					.padding(crate::sizing::HEADER_ICON_PADDING)
					.style(move |_| container::Style {
						background: Some(iced::Background::Color(colors.surface)),
						border: Border {
							color: colors.border,
							width: 2.0,
							radius: 3.into(),
						},
						..Default::default()
					}),
				column![
					text("NIGHTLIGHT")
						.width(Length::Fill)
						.color(colors.on_accent)
						.size(crate::sizing::HEADER_TITLE)
						.weight(font::Weight::Bold)
						.stretch(font::Stretch::ExtraExpanded)
						.wrapping(text::Wrapping::WordOrGlyph),
					text("Save your eyes")
						.width(Length::Fill)
						.color(colors.on_accent.scale_alpha(0.76))
						.size(14)
						.weight(font::Weight::Bold),
				]
				.spacing(4),

				space::horizontal(),
				neo_toggle()
					.with_colors(colors)
					.width(40)
					.height(18)
					.toggled(config.nightlight.enabled)
					.on_toggled(Message::ToggleNightlight)
			]
			.align_y(Alignment::Center)
			.spacing(16)
		)
		.with_colors(colors)
		.width(Length::Fill)
		.height(crate::sizing::HEADER_HEIGHT)
		.background(Self::accent_color(colors))
		.padding(crate::sizing::HEADER_PADDING),
		// Settings
		grid![
			slider_card(
				SliderCardArgs::new(colors)
					.name("DAY TEMP")
					.value(day_temp)
					.display(format!("{day_temp}K"))
					.range(1000..=10_000)
					.step(10)
					.accent(colors.decorative.yellow)
					.handle(colors.decorative.pink)
					.background(colors.surface)
					.on_change_live(Message::PreviewDayTemp)
					.on_change(Message::UpdateDayTemp)
			),
				slider_card(
					SliderCardArgs::new(colors)
						.name("DAY BRIGHT")
						.value(day_bright)
						.display(format!("{:.0}%", day_bright * 100.0))
						.range(0.0..=1.0)
						.step(0.01)
						.accent(colors.decorative.green)
						.handle(colors.decorative.yellow)
						.background(colors.surface)
						.on_change_live(Message::PreviewDayBright)
						.on_change(Message::UpdateDayBright)
			),
				slider_card(
					SliderCardArgs::new(colors)
						.name("NIGHT TEMP")
						.value(night_temp)
						.display(format!("{night_temp}K"))
						.range(1000..=10_000)
						.step(10)
						.accent(colors.decorative.purple70)
						.handle(colors.decorative.blue)
						.background(colors.surface)
						.on_change_live(Message::PreviewNightTemp)
						.on_change(Message::UpdateNightTemp)
			),
				slider_card(
					SliderCardArgs::new(colors)
						.name("NIGHT BRIGHT")
						.value(night_bright)
						.display(format!(
							"{:.0}%",
							night_bright * 100.0
						))
						.range(0.0..=1.0)
						.step(0.01)
						.accent(colors.decorative.blue)
						.handle(colors.decorative.yellow)
						.background(colors.surface)
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
						.style(move |_| container::Style {
							background: Some(iced::Background::Color(colors.decorative.pink)),
							border: Border {
								color: colors.border,
								width: 2.0,
								radius: 3.into(),
							},
							..Default::default()
						}),
					column![
						text("USE LOCATION")
							.color(colors.text)
							.size(18)
							.weight(font::Weight::Bold),
						text("Let geoclue and weather data determine dusk and dawn")
							.color(colors.text.scale_alpha(0.7))
							.size(13)
							.weight(font::Weight::Bold)
							.wrapping(text::Wrapping::Word)
					]
					.spacing(4)
					.width(Length::Fill),
					neo_toggle()
						.with_colors(colors)
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
					.style(move |_| container::Style {
						background: Some(iced::Background::Color(colors.border)),
						..Default::default()
					}),
				column![
					text("OR SET CUSTOM TIMES")
						.color(colors.text)
						.size(18)
						.weight(font::Weight::Bold),
					text("Set custom times for the night mode to work. Dusk is when it turns on the night settings, Dawn when it turns on the day settings.")
						.color(colors.text.scale_alpha(0.7))
						.size(13)
						.weight(font::Weight::Bold)
						.wrapping(text::Wrapping::Word)
				]
				.spacing(4)
				.width(Length::Fill),

				row![
					time_container("DAWN", dawn, colors.decorative.orange70, Message::DecreaseDawn, Message::IncreaseDawn, colors),
					time_container("DUSK", dusk, colors.decorative.purple70, Message::DecreaseDusk, Message::IncreaseDusk, colors)
				]
				.spacing(10),
			]
			.spacing(12)
		)
		.with_colors(colors)
		.width(Length::Fill)
		.padding(16),
	]
	.spacing(16)
	.into()
	}
}

struct SliderCardArgs<'a, T, D, F, L> {
	colors: Colors,
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

impl<T: Default, D: Default, F, L> SliderCardArgs<'_, T, D, F, L> {
	pub fn new(colors: Colors) -> Self {
		Self {
			colors,
			name: "<not set>",
			value: T::default(),
			value_display: D::default(),
			range: T::default()..=T::default(),
			step: T::default(),
			accent: colors.surface,
			handle_color: colors.decorative.pink,
			background_color: colors.surface,
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

fn slider_card<'a, T>(
	args: SliderCardArgs<
		'a,
		T,
		impl text::IntoFragment<'a>,
		impl Fn(T) -> Message + 'static,
		impl Fn(T) -> Message + 'static,
	>,
) -> Element<'a, Message>
where
	T: Num + NumCast + AsPrimitive<f32> + Clone,
	f32: AsPrimitive<T>,
{
	let color = args.accent;
	let colors = args.colors;

	neo_card(
		column![
			row![
				text(args.name)
					.width(Length::Fill)
					.color(colors.text)
					.size(18)
					.weight(font::Weight::Bold),
				container(
					text(args.value_display)
						.weight(font::Weight::Bold)
						.size(16)
						.color(colors.on_accent)
						.center()
				)
				.center_x(86)
				.center_y(32)
				.style(move |_| container::Style {
					background: Some(iced::Background::Color(color)),
					border: Border {
						color: colors.border,
						width: 2.0,
						radius: 3.into(),
					},
					..Default::default()
				})
			]
			.spacing(8),
			neo_slider(args.range, args.value)
				.with_colors(colors)
				.step(args.step)
				.running_color(args.accent)
				.handle_color(args.handle_color)
				.on_change_live_maybe(args.on_change_live)
				.on_change_maybe(args.on_change)
		]
		.spacing(12),
	)
	.with_colors(colors)
	.width(Length::Fill)
	.height(crate::sizing::CARD_HEIGHT)
	.padding(crate::sizing::CARD_PADDING)
	.background(args.background_color)
	.into()
}

fn fallback_time(hour: i8, minute: i8) -> jiff::civil::Time {
	match jiff::civil::Time::new(hour, minute, 0, 0) {
		Ok(time) => time,
		Err(error) => {
			log::warn!("Invalid fallback time {hour}:{minute}: {error}");
			jiff::civil::Time::default()
		}
	}
}

fn time_container(
	name: &str, time: jiff::civil::Time, color: Color, decrease: Message, increase: Message,
	colors: Colors,
) -> Element<'_, Message> {
	container(
		row![
			text(name)
				.color(colors.text_on(color))
				.size(18)
				.weight(font::Weight::Bold),
			space::horizontal(),
			neo_button(svg(phosphor_icon!("minus")).style(move |_, _| svg::Style {
				color: Some(colors.text)
			}))
			.with_colors(colors)
			.width(38)
			.on_press(decrease),
			neo_card(
				text(time.strftime("%H:%M").to_string())
					.color(colors.text)
					.size(18)
					.weight(font::Weight::Bold),
			)
			.with_colors(colors)
			.width(82)
			.background(colors.surface),
			neo_button(svg(phosphor_icon!("plus")).style(move |_, _| svg::Style {
				color: Some(colors.text)
			}))
			.with_colors(colors)
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
			color: colors.border,
			width: 2.0,
			radius: 3.into(),
		},
		..Default::default()
	})
	.into()
}
