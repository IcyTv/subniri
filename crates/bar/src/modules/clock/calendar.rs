use std::{collections::HashMap, sync::Arc, time::Duration};

use chrono::Datelike;
use daemon_common::calendar::{CalendarEventDto, CalendarProxy};
use futures::StreamExt;
use iced::{
	Element, Length, Subscription,
	alignment::Vertical,
	widget::{Grid, column, container, row, rule, scrollable, space, text},
};
use jiff::ToSpan;
use neo_widgets::{
	style::Colors,
	widgets::{
		NeoButtonStyle, NeoContentSurfaceStyle, NeoSurfaceStyle, neo_button,
		neo_tumbler_with_colors as tumbler,
	},
};

#[derive(Debug)]
pub struct Calendar {
	current_date: jiff::Zoned,

	selected_date: jiff::civil::Date,
	selected_month: i8,
	selected_year: i16,

	events: HashMap<jiff::civil::Date, Vec<Arc<CalendarEventDto>>>,
	service_error: Option<String>,
	loaded: bool,
	agenda_expanded: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
	Tick,

	Closed,

	OnDateSelected(jiff::civil::Date),
	CollapseAgenda,
	OnMonthForward,
	OnMonthBackward,
	OnYearForward,
	OnYearBackward,

	EventsUpdated(Vec<CalendarEventDto>),
	EventAdded(CalendarEventDto),
	Unavailable(String),
	ForMonth(i16, i8, Box<Message>),
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
			loaded: false,
			agenda_expanded: false,
		}
	}

	pub fn update(&mut self, message: Message) {
		match message {
			Message::ForMonth(year, month, message) => {
				if (year, month) == (self.selected_year, self.selected_month) {
					self.update(*message);
				}
			}
			Message::Tick => {
				self.current_date = jiff::Zoned::now();
			}
			Message::Closed => {
				self.agenda_expanded = false;
				self.select_date(self.current_date.date());
			}
			Message::OnDateSelected(day) => {
				self.select_date(day);
				self.agenda_expanded = true;
			}
			Message::CollapseAgenda => self.agenda_expanded = false,
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
				self.loaded = true;
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
			if let Ok(selected) = jiff::civil::Date::new(
				date.year(),
				date.month(),
				self.selected_date.day().min(date.days_in_month()),
			) {
				self.select_date(selected);
			}
		}
	}

	fn select_date(&mut self, date: jiff::civil::Date) {
		if (date.year(), date.month()) != (self.selected_year, self.selected_month) {
			self.events.clear();
			self.loaded = false;
			self.service_error = None;
		}

		self.selected_date = date;
		self.selected_year = date.year();
		self.selected_month = date.month();
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
									yield Message::ForMonth(year, month, Box::new(message));
								}
								Err(error) => {
									log::warn!("Calendar service unavailable: {error}");
									yield Message::ForMonth(year, month, Box::new(Message::Unavailable(error)));
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

	pub fn view(&self, colors: Colors) -> Element<'_, Message> {
		let Ok(month_begin) = jiff::civil::Date::new(self.selected_year, self.selected_month, 1)
		else {
			return text("Calendar date unavailable").color(colors.text).into();
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
				container(
					container(
						text(weekday)
							.weight(iced::font::Weight::Bold)
							.color(colors.text),
					)
					.center_x(Length::Fill),
				)
				.width(Length::Fill)
				.padding([2, 0])
				.into(),
			);
		}

		for week in 0..rows {
			let week_number = month_begin
				.checked_add((week * 7 - leading_days).days())
				.map_or(0, iso_week_number);
			calendar_cells.push(
				container(
					container(
						text(format!("{week_number:02}")).style(move |_| text::Style {
							color: Some(colors.text.scale_alpha(0.5)),
							..Default::default()
						}),
					)
					.center_x(Length::Fill),
				)
				.width(Length::Fill)
				.padding([2, 0])
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
					colors.on_accent
				} else if !is_current_month {
					colors.muted
				} else {
					colors.text
				};
				let text_style = text::Style {
					color: Some(text_color),
				};

				let underline: Element<Message> =
					if !is_selected_day && self.events.contains_key(&date) {
						rule::horizontal(2)
							.style(move |_| rule::Style {
								color: colors.decorative.purple,
								fill_mode: rule::FillMode::Full,
								snap: true,
								radius: 0.0.into(),
							})
							.into()
					} else {
						space().into()
					};

				let day = text(date.day().to_string())
					.size(19)
					.wrapping(text::Wrapping::None)
					.weight(if is_current_day {
						iced::font::Weight::Bold
					} else {
						iced::font::Weight::Normal
					})
					.style(move |_| text_style);

				let widget: Element<_> = column![
					container(day)
						.width(Length::Fill)
						.height(28.0)
						.center_x(Length::Fill)
						.center_y(Length::Fill),
					container(underline).width(Length::Fill).height(2),
				]
				.into();

				calendar_cells.push(
					neo_button(widget)
						.with_colors(colors)
						.style(day_style(colors, is_selected_day))
						.width(Length::Fill)
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
					.color(colors.text)
					.size(12)
					.into()
			},
		);

		let mut content = column![
			row![
				tumbler(
					container(
						text(month_name)
							.weight(iced::font::Weight::Bold)
							.color(colors.text)
					)
					.center_x(Length::Fill),
					Message::OnMonthForward,
					Message::OnMonthBackward,
					0.0,
					colors,
				)
				.width(Length::Fill),
				// space::horizontal(),
				tumbler(
					text(format!("{}", self.selected_year))
						.weight(iced::font::Weight::Bold)
						.color(colors.text),
					Message::OnYearForward,
					Message::OnYearBackward,
					5.0,
					colors,
				),
			],
			status,
			Grid::with_children(calendar_cells).columns(8)
		];
		if self.agenda_expanded {
			content = content.push(self.view_agenda(colors));
		}

		content.spacing(10).width(Length::Fill).into()
	}

	fn view_agenda(&self, colors: Colors) -> Element<'_, Message> {
		let header = row![
			text(self.selected_date.to_string())
				.color(colors.text)
				.weight(iced::font::Weight::Bold),
			space::horizontal(),
			neo_button(text("Collapse").color(colors.text))
				.with_colors(colors)
				.on_press(Message::CollapseAgenda),
		]
		.align_y(Vertical::Center);
		let mut agenda = column![header].spacing(8);
		let status = self.agenda_status();
		if let Some(status) = status {
			return agenda.push(text(status).color(colors.secondary)).into();
		}

		let events = self.agenda_events();
		let count = events.len();
		let rows = column(events.into_iter().map(|event| {
			let time = event_time(event);
			container(column![
				text(&event.title)
					.size(15)
					.weight(iced::font::Weight::Bold)
					.color(colors.text)
					.ellipsis(text::Ellipsis::End)
					.wrapping(text::Wrapping::None),
				text(time).size(12).color(colors.secondary),
			])
			.height(48)
			.width(Length::Fill)
			.into()
		}));
		agenda = agenda.push(scrollable(rows).height((count.min(4) * 48) as f32));

		agenda.into()
	}

	fn agenda_status(&self) -> Option<&'static str> {
		if self.service_error.is_some() {
			Some("Calendar unavailable")
		} else if !self.loaded {
			Some("Loading events…")
		} else if !self.events.contains_key(&self.selected_date) {
			Some("No events")
		} else {
			None
		}
	}

	fn agenda_events(&self) -> Vec<&CalendarEventDto> {
		let mut events: Vec<_> = self
			.events
			.get(&self.selected_date)
			.into_iter()
			.flatten()
			.map(Arc::as_ref)
			.collect();
		events.sort_by_key(|event| (event.start, event.end, event.title.as_str()));

		events
	}
}

fn event_time(event: &CalendarEventDto) -> String {
	if event.is_all_day {
		"All day".to_string()
	} else {
		format!(
			"{}–{}",
			event.start.format("%H:%M"),
			event.end.format("%H:%M")
		)
	}
}

fn day_style(colors: Colors, selected: bool) -> NeoButtonStyle {
	let background = if selected {
		colors.decorative.purple
	} else {
		colors.body
	};
	let highlighted = if selected { background } else { colors.raised };
	let surface = NeoSurfaceStyle {
		background,
		border_width: 0.0,
		shadow_width: 0.0,
		..NeoSurfaceStyle::with_colors(colors)
	};

	NeoButtonStyle {
		surface: NeoContentSurfaceStyle {
			surface,
			padding: 4.into(),
		},
		hovered: NeoSurfaceStyle {
			background: highlighted,
			..surface
		},
		focused: NeoContentSurfaceStyle {
			surface: NeoSurfaceStyle {
				background: highlighted,
				..surface
			},
			padding: 4.into(),
		},
		disabled_background: colors.body,
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

fn calendar_range(
	year: i16, month: i8,
) -> Result<(chrono::NaiveDateTime, chrono::NaiveDateTime), String> {
	// The API uses naive local datetimes. No timezone-midnight resolution is
	// needed for a month query, including on days with a local clock change.
	let month_start = chrono::NaiveDate::from_ymd_opt(i32::from(year), month as u32, 1)
		.and_then(|date| date.and_hms_opt(0, 0, 0))
		.ok_or_else(|| "Invalid calendar month".to_string())?;
	let start = month_start
		.checked_sub_signed(chrono::Duration::days(i64::from(
			month_start.weekday().num_days_from_monday(),
		)))
		.ok_or_else(|| "Calendar grid is out of range".to_string())?;
	let end = start
		.checked_add_signed(chrono::Duration::days(42))
		.ok_or_else(|| "Calendar month is out of range".to_string())?;

	Ok((start, end))
}

fn calendar_events(year: i16, month: i8) -> impl futures::Stream<Item = Result<Message, String>> {
	async_stream::try_stream! {
		let (start, end) = calendar_range(year, month)?;
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

	#[test]
	fn event_queries_cover_adjacent_month_dates_in_every_visible_grid() {
		for year in [2024, 2025, 2026] {
			for month in 1..=12 {
				let (start, end) = super::calendar_range(year, month).expect("valid range");
				let first = jiff::civil::Date::new(year, month, 1).expect("valid month");
				let leading = i32::from(first.weekday().to_monday_zero_offset());
				for offset in 0..42 {
					let day = first
						.checked_add(jiff::ToSpan::days(offset - leading))
						.expect("valid grid date");
					let day = chrono::NaiveDate::from_ymd_opt(
						i32::from(day.year()),
						day.month() as u32,
						day.day() as u32,
					)
					.expect("valid date");
					assert!(day >= start.date() && day < end.date());
				}
			}
		}
	}

	#[test]
	fn agenda_distinguishes_loading_failure_and_empty_and_sorts_local_times() {
		let mut calendar = Calendar::new();
		calendar.update(Message::OnDateSelected(jiff::civil::date(2026, 1, 1)));
		assert_eq!(calendar.agenda_status(), Some("Loading events…"));

		calendar.update(Message::Unavailable("offline".into()));
		assert_eq!(calendar.agenda_status(), Some("Calendar unavailable"));
		calendar.update(Message::EventsUpdated(vec![]));
		assert_eq!(calendar.agenda_status(), Some("No events"));

		let mut late = event(2026).expect("valid event");
		late.title = "Lunch".into();
		let mut early = late.clone();
		early.title = "Morning".into();
		early.start -= chrono::Duration::hours(3);
		early.end = early.start + chrono::Duration::minutes(30);
		calendar.events.insert(
			calendar.selected_date,
			vec![std::sync::Arc::new(late), std::sync::Arc::new(early)],
		);
		let events = calendar.agenda_events();

		assert_eq!(calendar.agenda_status(), None);
		assert_eq!(
			events
				.iter()
				.map(|event| event.title.as_str())
				.collect::<Vec<_>>(),
			["Morning", "Lunch"]
		);
		assert_eq!(super::event_time(events[0]), "09:00–09:30");
		let mut all_day = events[0].clone();
		all_day.is_all_day = true;
		assert_eq!(super::event_time(&all_day), "All day");
	}

	#[test]
	fn agenda_starts_compact_and_selection_expands_until_collapsed() {
		let mut calendar = Calendar::new();
		assert!(!calendar.agenda_expanded);

		calendar.update(Message::OnDateSelected(jiff::civil::date(2026, 1, 31)));
		assert!(calendar.agenda_expanded);
		assert!(!calendar.loaded);

		calendar.update(Message::EventsUpdated(vec![]));
		assert!(calendar.loaded);
		calendar.update(Message::CollapseAgenda);
		assert!(!calendar.agenda_expanded);
		calendar.update(Message::OnDateSelected(jiff::civil::date(2026, 1, 30)));
		assert!(calendar.agenda_expanded);
		calendar.update(Message::Closed);
		assert!(!calendar.agenda_expanded);
	}

	#[test]
	fn month_and_year_navigation_carry_and_clamp_selected_day() {
		let mut calendar = Calendar::new();
		calendar.update(Message::OnDateSelected(jiff::civil::date(2024, 1, 31)));
		calendar.update(Message::EventsUpdated(vec![]));

		calendar.update(Message::OnMonthForward);
		assert_eq!(calendar.selected_date, jiff::civil::date(2024, 2, 29));
		assert!(!calendar.loaded);
		calendar.update(Message::OnYearForward);
		assert_eq!(calendar.selected_date, jiff::civil::date(2025, 2, 28));

		calendar.update(Message::ForMonth(
			2024,
			1,
			Box::new(Message::EventsUpdated(vec![])),
		));
		assert!(
			!calendar.loaded,
			"old month data must not mark current month loaded"
		);
	}

	#[test]
	fn both_themes_build_compact_and_expanded_agendas() {
		let mut calendar = Calendar::new();
		for colors in [neo_widgets::style::LIGHT, neo_widgets::style::DARK] {
			let _ = calendar.view(colors);
		}

		calendar.update(Message::OnDateSelected(jiff::civil::date(2026, 1, 1)));
		calendar.update(Message::EventsUpdated(vec![
			event(2026).expect("valid event"),
		]));
		for colors in [neo_widgets::style::LIGHT, neo_widgets::style::DARK] {
			let _ = calendar.view(colors);
		}
	}

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
			matches!(message, Some(Message::ForMonth(_, _, message)) if matches!(*message, Message::Unavailable(_))),
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
		let colors = neo_widgets::style::Colors::for_theme(config::AppearanceMode::Light);
		let _ = calendar.view(colors);
		calendar.update(Message::OnDateSelected(jiff::civil::date(-9999, 1, 1)));
		calendar.update(Message::OnMonthBackward);
		calendar.update(Message::OnYearBackward);
		assert_eq!(
			(calendar.selected_year, calendar.selected_month),
			(-9999, 1)
		);
		let _ = calendar.view(colors);
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
		assert!(
			matches!(first, Some(Message::ForMonth(_, _, message)) if matches!(*message, Message::Unavailable(_)))
		);
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
			matches!(next, Some(Message::ForMonth(_, _, message)) if matches!(*message, Message::EventsUpdated(_))),
			"subscription must reconnect and read a fresh snapshot"
		);
		Ok(())
	}
}
