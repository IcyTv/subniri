use std::{collections::HashMap, sync::Arc, time::Duration};

use chrono::Datelike;
use daemon_common::calendar::{CalendarEventDto, CalendarProxy};
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
	service_error: Option<String>,
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
	Unavailable(String),
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
			service_error: None,
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
				self.shift_month(1);
			}
			Message::OnMonthBackward => {
				self.shift_month(-1);
			}
			Message::OnYearForward => {
				self.shift_month(12);
			}
			Message::OnYearBackward => {
				self.shift_month(-12);
			}

			Message::EventsUpdated(events) => {
				self.service_error = None;
				self.events.clear();
				for event in events {
					self.insert_event(event);
				}
			}
			Message::EventAdded(event) => {
				self.insert_event(event);
			}
			Message::Unavailable(error) => self.service_error = Some(error),
		}
	}

	fn shift_month(&mut self, months: i32) {
		if let Ok(date) = jiff::civil::Date::new(self.selected_year, self.selected_month, 1)
			.and_then(|date| date.checked_add(months.months()))
		{
			self.selected_year = date.year();
			self.selected_month = date.month();
		}
	}

	fn insert_event(&mut self, event: CalendarEventDto) {
		let (Some(start), Some(end)) = (
			chrono_date(event.start.date()),
			chrono_date(event.end.date()),
		) else {
			log::warn!("Ignoring calendar event with an unsupported date");
			return;
		};
		if event.end < event.start {
			log::warn!("Ignoring calendar event ending before its start");
			return;
		}
		let Ok(month) = jiff::civil::Date::new(self.selected_year, self.selected_month, 1) else {
			return;
		};
		// Bound work to the visible grid, even for a remotely supplied event
		// spanning thousands of years. Replayed signals upsert by stable ID.
		self.events.retain(|_, events| {
			events.retain(|existing| existing.id != event.id);
			!events.is_empty()
		});
		let leading = i32::from(month.weekday().to_monday_zero_offset());
		let event = Arc::new(event);
		for offset in 0..42 {
			if let Ok(date) = month.checked_add((offset - leading).days())
				&& date >= start
				&& date <= end
			{
				self.events.entry(date).or_default().push(event.clone());
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
					let mut backoff = Duration::from_secs(1);
					loop {
						let mut events = Box::pin(calendar_events(year, month));
						while let Some(message) = events.next().await {
							match message {
								Ok(message) => {
									backoff = Duration::from_secs(1);
									yield message;
								}
								Err(error) => {
									log::warn!("Calendar service unavailable: {error}");
									yield Message::Unavailable(error);
									break;
								}
							}
						}
						// Drop the old connection/streams before retrying.
						drop(events);
						tokio::time::sleep(backoff).await;
						backoff = (backoff * 2).min(Duration::from_secs(30));
					}
				}
			},
		)
	}

	pub fn view(&self) -> Element<'_, Message> {
		let Ok(month_begin) = jiff::civil::Date::new(self.selected_year, self.selected_month, 1)
		else {
			return text("Calendar date unavailable").into();
		};
		let leading_days = month_begin.weekday().to_monday_zero_offset() as i32;

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
			let week_number = month_begin
				.checked_add((week * 7 - leading_days).days())
				.map_or(0, iso_week_number);
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
				let Ok(date) = month_begin.checked_add((week * 7 + day - leading_days).days())
				else {
					calendar_cells.push(space().into());
					continue;
				};

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

		let month_name = jiff::fmt::strtime::format("%B", month_begin).unwrap_or_else(|error| {
			log::warn!("Failed to format month name: {error}");
			"Unknown".to_string()
		});
		let status: Element<'_, Message> = self.service_error.as_ref().map_or_else(
			|| space().into(),
			|error| {
				text(format!("Calendar unavailable: {error}"))
					.size(12)
					.into()
			},
		);

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
			status,
			Grid::with_children(calendar_cells).columns(8)
		]
		.spacing(10)
		.width(iced::Length::Fill)
		.into()
	}
}

fn chrono_date(date: chrono::NaiveDate) -> Option<jiff::civil::Date> {
	jiff::civil::Date::new(
		i16::try_from(date.year()).ok()?,
		i8::try_from(date.month()).ok()?,
		i8::try_from(date.day()).ok()?,
	)
	.ok()
}

async fn request<T, E: std::fmt::Display>(
	future: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, String> {
	tokio::time::timeout(Duration::from_secs(5), future)
		.await
		.map_err(|_| "Calendar request timed out".to_string())?
		.map_err(|error| error.to_string())
}

fn calendar_events(year: i16, month: i8) -> impl futures::Stream<Item = Result<Message, String>> {
	async_stream::try_stream! {
		// The API uses naive local datetimes. No timezone-midnight resolution is
		// needed for a month query, including on days with a local clock change.
		let start = chrono::NaiveDate::from_ymd_opt(i32::from(year), month as u32, 1)
			.and_then(|date| date.and_hms_opt(0, 0, 0))
			.ok_or_else(|| "Invalid calendar month".to_string())?;
		let end = start.checked_add_months(chrono::Months::new(1))
			.ok_or_else(|| "Calendar month is out of range".to_string())?;
		let connection = request(zbus::Connection::session()).await?;
		let proxy = request(CalendarProxy::new(&connection)).await?;
		// Subscribe before reading so events during the initial fetch are not lost.
		let mut events = request(proxy.receive_event_added()).await?;
		yield Message::EventsUpdated(request(proxy.get_events(start, end)).await?);
		let mut refresh = tokio::time::interval(Duration::from_secs(15));
		refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
		refresh.tick().await;
		loop {
			let signal = tokio::select! {
				signal = events.next() => Some(signal),
				_ = refresh.tick() => None,
			};
			if let Some(signal) = signal {
				let signal = signal.ok_or_else(|| "Calendar event stream closed".to_string())?;
				match signal.args() {
					Ok(args) => yield Message::EventAdded(args.event),
					Err(error) => log::warn!("Ignoring malformed calendar event: {error}"),
				}
			} else {
				// Also refresh after a daemon owner change without an EventAdded signal.
				yield Message::EventsUpdated(request(proxy.get_events(start, end)).await?);
			}
		}
	}
}

fn iso_week_number(date: jiff::civil::Date) -> u32 {
	chrono::NaiveDate::from_ymd_opt(date.year() as i32, date.month() as u32, date.day() as u32)
		.map_or(0, |date| date.iso_week().week())
}

#[cfg(test)]
mod tests {
	use super::{Calendar, Message};
	use daemon_common::calendar::CalendarEventDto;

	fn event(year: i32) -> Result<CalendarEventDto, Box<dyn std::error::Error>> {
		let date = chrono::NaiveDate::from_ymd_opt(year, 1, 1)
			.and_then(|date| date.and_hms_opt(12, 0, 0))
			.ok_or("invalid test date")?;
		Ok(CalendarEventDto {
			id: Default::default(),
			title: "test".into(),
			start: date,
			end: date,
			is_all_day: false,
			description: String::new(),
		})
	}

	#[test]
	fn out_of_range_event_does_not_panic() -> Result<(), Box<dyn std::error::Error>> {
		let mut calendar = Calendar::new();
		calendar.update(Message::EventsUpdated(vec![event(10_000)?]));
		assert!(calendar.events.is_empty());
		Ok(())
	}

	#[test]
	fn repeated_snapshot_replaces_instead_of_duplicating_events()
	-> Result<(), Box<dyn std::error::Error>> {
		let mut calendar = Calendar::new();
		let event = event(2026)?;
		calendar.update(Message::OnDateSelected(jiff::civil::date(2026, 1, 1)));
		calendar.update(Message::EventsUpdated(vec![event.clone()]));
		calendar.update(Message::EventsUpdated(vec![event]));
		assert_eq!(calendar.events.values().map(Vec::len).sum::<usize>(), 1);
		calendar.update(Message::EventsUpdated(vec![]));
		assert!(calendar.events.is_empty());
		Ok(())
	}

	#[tokio::test]
	#[ignore = "run explicitly under dbus-run-session with no daemon"]
	async fn absent_daemon_does_not_panic() -> Result<(), Box<dyn std::error::Error>> {
		use futures::StreamExt;
		let calendar = Calendar::new();
		let recipe = iced::advanced::subscription::into_recipes(calendar.subscription())
			.into_iter()
			.next()
			.ok_or("missing calendar subscription")?;
		let mut stream = recipe.stream(futures::stream::pending().boxed());
		let message =
			tokio::time::timeout(std::time::Duration::from_secs(2), stream.next()).await?;
		assert!(
			matches!(message, Some(Message::Unavailable(_))),
			"calendar must report unavailability"
		);
		Ok(())
	}

	#[test]
	fn navigation_and_view_are_safe_at_date_limits() {
		let mut calendar = Calendar::new();
		calendar.update(Message::OnDateSelected(jiff::civil::date(9999, 12, 1)));
		calendar.update(Message::OnMonthForward);
		calendar.update(Message::OnYearForward);
		assert_eq!(
			(calendar.selected_year, calendar.selected_month),
			(9999, 12)
		);
		let _ = calendar.view();
		calendar.update(Message::OnDateSelected(jiff::civil::date(-9999, 1, 1)));
		calendar.update(Message::OnMonthBackward);
		calendar.update(Message::OnYearBackward);
		assert_eq!(
			(calendar.selected_year, calendar.selected_month),
			(-9999, 1)
		);
		let _ = calendar.view();
	}

	#[test]
	fn event_work_is_bounded_and_replayed_signals_upsert() -> Result<(), Box<dyn std::error::Error>>
	{
		let mut calendar = Calendar::new();
		calendar.update(Message::OnDateSelected(jiff::civil::date(2026, 1, 1)));
		let mut event = event(-9999)?;
		event.end = chrono::NaiveDate::from_ymd_opt(9999, 12, 31)
			.and_then(|date| date.and_hms_opt(0, 0, 0))
			.ok_or("invalid end")?;
		calendar.update(Message::EventAdded(event.clone()));
		calendar.update(Message::EventAdded(event));
		assert_eq!(calendar.events.len(), 42);
		assert!(calendar.events.values().all(|events| events.len() == 1));
		Ok(())
	}

	struct TestCalendar;

	impl daemon_common::calendar::Calendar for TestCalendar {
		async fn get_events(
			&self, _: chrono::NaiveDateTime, _: chrono::NaiveDateTime,
		) -> zbus::fdo::Result<Vec<CalendarEventDto>> {
			Ok(vec![])
		}

		async fn add_event(
			&self, _: daemon_common::calendar::AddCalendarEventDto,
			_: zbus::object_server::SignalEmitter<'_>,
		) -> zbus::fdo::Result<()> {
			Ok(())
		}
	}

	#[tokio::test]
	#[ignore = "run explicitly under dbus-run-session with no daemon"]
	async fn subscription_recovers_when_daemon_appears() -> Result<(), Box<dyn std::error::Error>> {
		use futures::StreamExt;
		let calendar = Calendar::new();
		let recipe = iced::advanced::subscription::into_recipes(calendar.subscription())
			.into_iter()
			.next()
			.ok_or("missing subscription")?;
		let mut stream = recipe.stream(futures::stream::pending().boxed());
		let first = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next()).await?;
		assert!(matches!(first, Some(Message::Unavailable(_))));
		let connection = zbus::connection::Builder::session()?
			.name(daemon_common::DEFAULT_BUS_NAME)?
			.serve_at(
				daemon_common::CALENDAR_OBJECT_PATH,
				daemon_common::calendar::CalendarServer(TestCalendar),
			)?
			.build()
			.await?;
		let next = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next()).await?;
		connection.close().await?;
		assert!(
			matches!(next, Some(Message::EventsUpdated(_))),
			"subscription must reconnect and read a fresh snapshot"
		);
		Ok(())
	}
}
