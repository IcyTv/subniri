use iced::{
	Element, Length, Subscription,
	widget::{Grid, column, container, row, rule, text},
};
use jiff::ToSpan;
use neo_widgets::{
	style::COLORS,
	widgets::{neo_button, neo_card, neo_tumbler},
};

#[derive(Debug)]
pub struct Calendar {
	current_date: jiff::Zoned,

	selected_date: jiff::civil::Date,
	selected_month: i8,
	selected_year: i16,
}

#[derive(Debug, Clone, Copy)]
pub enum Message {
	Tick,

	Closed,

	OnDateSelected(jiff::civil::Date),
	OnMonthForward,
	OnMonthBackward,
	OnYearForward,
	OnYearBackward,
}

impl Calendar {
	pub fn new() -> Self {
		let current_date = jiff::Zoned::now();

		Self {
			selected_date: current_date.date(),
			selected_month: current_date.month(),
			selected_year: current_date.year(),

			current_date,
		}
	}

	pub fn update(&mut self, message: Message) {
		match message {
			Message::Tick => {
				self.current_date = jiff::Zoned::now();
			}
			Message::Closed => {
				self.selected_date = self.current_date.date();
				self.selected_month = self.current_date.month();
				self.selected_year = self.current_date.year();
			}
			Message::OnDateSelected(day) => {
				self.selected_date = day;
				self.selected_month = day.month();
				self.selected_year = day.year();
			}
			Message::OnMonthForward => {
				self.selected_month += 1;

				if self.selected_month > 12 {
					self.selected_month = 1;
					self.selected_year = self.selected_year.saturating_add(1);
				}
			}
			Message::OnMonthBackward => {
				self.selected_month -= 1;

				if self.selected_month < 1 {
					self.selected_month = 12;
					self.selected_year = self.selected_year.saturating_sub(1);
				}
			}
			Message::OnYearForward => {
				self.selected_year = self.selected_year.saturating_add(1);
			}
			Message::OnYearBackward => {
				self.selected_year = self.selected_year.saturating_sub(1);
			}
		}
	}

	pub fn view(&self) -> Element<'_, Message> {
		let date = jiff::civil::date(self.selected_year, self.selected_month, 1)
			.to_zoned(self.current_date.time_zone().clone())
			.unwrap();

		let month_begin = date.first_of_month().unwrap();
		let leading_days = month_begin.weekday().to_monday_zero_offset() as i32;

		let grid_begin = month_begin.date().checked_sub(leading_days.days()).unwrap();

		let rows = if leading_days + month_begin.days_in_month() as i32 > 35 {
			6
		} else {
			5
		};

		let days = (0..7 * rows).map(|cell| {
			let date = grid_begin.checked_add((cell as i64).days()).unwrap();

			let is_current_month =
				date.month() == month_begin.month() && date.year() == month_begin.year();
			let is_current_day = date == self.current_date.date();
			let is_selected_day = date == self.selected_date;

			let text_color = if is_selected_day {
				COLORS.decorative.purple
			} else {
				COLORS.text
			};
			let text_style = text::Style {
				color: Some(text_color),
			};

			let widget: Element<_> = if is_current_day {
				column![
					text(date.day().to_string()).style(move |_| text_style),
					rule::horizontal(2).style(|_| rule::Style {
						color: COLORS.decorative.purple,
						fill_mode: rule::FillMode::Full,
						snap: true,
						radius: 0.0.into(),
					})
				]
				.spacing(0)
				.into()
			} else if is_current_month {
				text(date.day().to_string())
					.style(move |_| text_style)
					.into()
			} else {
				text(date.day().to_string())
					.style(|_| text::Style {
						color: Some(COLORS.text.scale_alpha(0.5)),
						..Default::default()
					})
					.into()
			};

			neo_button(widget)
				.on_press(Message::OnDateSelected(date))
				.into()
		});

		let month_name = jiff::fmt::strtime::format("%B", &month_begin).unwrap_or_else(|error| {
			log::warn!("Failed to format month name: {error}");
			"Unknown".to_string()
		});

		column![
			row![
				neo_tumbler(
					container(text(month_name).weight(iced::font::Weight::Bold))
						.center_x(Length::Fill),
					Message::OnMonthForward,
					Message::OnMonthBackward,
					0.0
				)
				.width(Length::Fill),
				// space::horizontal(),
				neo_tumbler(
					text(format!("{}", self.selected_year)).weight(iced::font::Weight::Bold),
					Message::OnYearForward,
					Message::OnYearBackward,
					5.0,
				),
			],
			Grid::with_children(days).columns(7)
		]
		.spacing(10)
		.width(iced::Length::Fill)
		.into()
	}
}
