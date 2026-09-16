use std::{collections::HashMap, sync::Arc};

use chrono::{Datelike, Timelike};
use daemon::calendar::{CalendarEventDto, CalendarProxy};
use futures::StreamExt;
use iced::{
	Element, Length, Subscription,
	alignment::Vertical,
	widget::{Grid, column, container, row, rule, space, stack, svg, text},
};
use jiff::ToSpan;
use neo_widgets::{
	phosphor_icon,
	style::COLORS,
	widgets::{neo_button, neo_card, neo_tumbler},
};

#[derive(Debug)]
pub struct Calendar {
	current_date: jiff::Zoned,

	selected_date: jiff::civil::Date,
	selected_month: i8,
	selected_year: i16,

	events: HashMap<jiff::civil::Date, Vec<Arc<CalendarEventDto>>>,
}

#[derive(Debug, Clone)]
pub enum Message {
	Tick,

	Closed,

	OnDateSelected(jiff::civil::Date),
	OnMonthForward,
	OnMonthBackward,
	OnYearForward,
	OnYearBackward,

	EventsUpdated(Vec<CalendarEventDto>),
	EventAdded(CalendarEventDto),
}

impl Calendar {
	pub fn new() -> Self {
		let current_date = jiff::Zoned::now();

		Self {
			selected_date: current_date.date(),
			selected_month: current_date.month(),
			selected_year: current_date.year(),

			current_date,

			events: HashMap::new(),
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

			Message::EventsUpdated(events) => {
				// self.events = events;
				for event in events {
					let start = chrono_naive_to_zoned(&event.start).unwrap();
					let end = chrono_naive_to_zoned(&event.end).unwrap();
					let event = Arc::new(event);

					for day in days_between(&start, &end) {
						self.events
							.entry(day)
							.or_insert_with(Vec::new)
							.push(event.clone());
					}
				}
			}
			Message::EventAdded(event) => {
				let start = chrono_naive_to_zoned(&event.start).unwrap();
				let end = chrono_naive_to_zoned(&event.end).unwrap();
				let event = Arc::new(event);

				for day in days_between(&start, &end) {
					self.events
						.entry(day)
						.or_insert_with(Vec::new)
						.push(event.clone());
				}
			}
		}
	}

	pub fn subscription(&self) -> Subscription<Message> {
		Subscription::run_with(
			(self.selected_month, self.selected_year),
			move |(month, year)| {
				let month = *month;
				let year = *year;
				async_stream::stream! {
					let connection = zbus::Connection::session().await.unwrap();
					let proxy = CalendarProxy::new(&connection).await.unwrap();

					let start = jiff::civil::date(year, month, 1)
						.to_zoned(jiff::tz::TimeZone::system())
						.unwrap();
					let end = start.checked_add(1.months()).unwrap();

					let start = zoned_to_chrono_naive(&start).unwrap();
					let end = zoned_to_chrono_naive(&end).unwrap();

					let all = proxy.get_events(start, end).await.unwrap();

					yield Message::EventsUpdated(all);


					let mut stream = proxy.receive_event_added().await.unwrap();

					while let Some(signal) = stream.next().await {
						let event = signal.args().unwrap();
						yield Message::EventAdded(event.event);
					}
				}
			},
		)
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

		let mut calendar_cells: Vec<Element<'_, Message>> =
			Vec::with_capacity(((rows + 1) * 8) as usize);
		for weekday in ["Wk", "Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"] {
			calendar_cells.push(
				neo_card(
					container(text(weekday).weight(iced::font::Weight::Bold))
						.center_x(Length::Fill),
				)
				.width(Length::Fill)
				.padding([2, 0])
				.background(COLORS.decorative.purple90)
				.into(),
			);
		}

		for week in 0..rows {
			let week_begin = grid_begin.checked_add((week as i64).weeks()).unwrap();
			let week_number = iso_week_number(week_begin);
			calendar_cells.push(
				neo_card(
					container(text(format!("{week_number:02}")).style(|_| text::Style {
						color: Some(COLORS.text.scale_alpha(0.5)),
						..Default::default()
					}))
					.center_x(Length::Fill),
				)
				.width(Length::Fill)
				.padding([2, 0])
				.background(COLORS.decorative.blue90)
				.into(),
			);

			for day in 0..7 {
				let date = week_begin.checked_add((day as i64).days()).unwrap();

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

				let event_indicator: Element<Message> = if self.events.contains_key(&date) {
					svg(phosphor_icon!("circle", "fill"))
						.width(6.0)
						.height(6.0)
						.style(|_, _| svg::Style {
							color: Some(COLORS.decorative.purple),
							..Default::default()
						})
						.into()
				} else {
					space().into()
				};

				let event_overlay = row![
					space().width(Length::Fill),
					event_indicator,
					space().width(Length::Fill),
				]
				.width(Length::Fill)
				.height(Length::Fill)
				.align_y(Vertical::Top);

				let underline: Element<Message> = if is_current_day {
					rule::horizontal(2)
						.style(|_| rule::Style {
							color: COLORS.decorative.purple,
							fill_mode: rule::FillMode::Full,
							snap: true,
							radius: 0.0.into(),
						})
						.into()
				} else {
					space().into()
				};

				let day = text(date.day().to_string()).style(move |_| {
					if is_current_month {
						text_style
					} else {
						text::Style {
							color: Some(COLORS.text.scale_alpha(0.5)),
							..Default::default()
						}
					}
				});

				let widget: Element<_> = stack![
					container(day)
						.width(Length::Fill)
						.height(28.0)
						.center_x(Length::Fill)
						.center_y(Length::Fill),
					event_overlay,
					container(underline)
						.width(Length::Fill)
						.height(Length::Fill)
						.align_y(Vertical::Bottom),
				]
				.into();

				calendar_cells.push(
					neo_button(widget)
						.on_press(Message::OnDateSelected(date))
						.into(),
				);
			}
		}

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
			Grid::with_children(calendar_cells).columns(8)
		]
		.spacing(10)
		.width(iced::Length::Fill)
		.into()
	}
}

fn zoned_to_chrono_naive(zoned: &jiff::Zoned) -> Option<chrono::NaiveDateTime> {
	// 1. Get the timezone-naive "civil" datetime from Jiff
	let civil = zoned.datetime();

	// 2. Build Chrono's NaiveDate and NaiveTime components
	let chrono_date = chrono::NaiveDate::from_ymd_opt(
		civil.year() as i32,
		civil.month() as u32,
		civil.day() as u32,
	)?;

	let chrono_time = chrono::NaiveTime::from_hms_nano_opt(
		civil.hour() as u32,
		civil.minute() as u32,
		civil.second() as u32,
		civil.subsec_nanosecond() as u32,
	)?;

	// 3. Combine into a NaiveDateTime
	Some(chrono::NaiveDateTime::new(chrono_date, chrono_time))
}

fn chrono_naive_to_zoned(naive: &chrono::NaiveDateTime) -> Option<jiff::Zoned> {
	// 1. Build Jiff's civil date and time components
	let civil_date = jiff::civil::date(naive.year() as i16, naive.month() as i8, naive.day() as i8);
	let civil_time = jiff::civil::time(
		naive.hour() as i8,
		naive.minute() as i8,
		naive.second() as i8,
		naive.nanosecond() as i32,
	);
	let civil_datetime = jiff::civil::DateTime::from_parts(civil_date, civil_time);
	// 2. Convert to Jiff's Zoned using the system timezone
	civil_datetime.to_zoned(jiff::tz::TimeZone::system()).ok()
}

fn days_between(start: &jiff::Zoned, end: &jiff::Zoned) -> impl Iterator<Item = jiff::civil::Date> {
	let start = start.date();
	let end = end.date();

	start.series(1.day()).take_while(move |date| *date <= end)
}

fn iso_week_number(date: jiff::civil::Date) -> u32 {
	chrono::NaiveDate::from_ymd_opt(date.year() as i32, date.month() as u32, date.day() as u32)
		.map_or(0, |date| date.iso_week().week())
}
