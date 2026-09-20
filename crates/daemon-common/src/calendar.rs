use crate::{CALENDAR_INTERFACE, CALENDAR_OBJECT_PATH, DEFAULT_BUS_NAME};

#[zbus::proxy(
	interface = CALENDAR_INTERFACE,
	default_service = DEFAULT_BUS_NAME,
	default_path = CALENDAR_OBJECT_PATH
)]
pub trait Calendar {
	fn get_events(
		&self, start: chrono::NaiveDateTime, end: chrono::NaiveDateTime,
	) -> zbus::Result<Vec<CalendarEventDto>>;
	fn add_event(&self, event: AddCalendarEventDto) -> zbus::Result<()>;

	#[zbus(signal)]
	fn event_added(&self, event: CalendarEventDto) -> zbus::Result<()>;
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, zbus::zvariant::Type)]
pub struct AddCalendarEventDto {
	pub title: String,
	pub start: chrono::NaiveDateTime,
	pub end: chrono::NaiveDateTime,
	pub is_all_day: bool,
	pub description: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, zbus::zvariant::Type)]
pub struct CalendarEventDto {
	pub id: uuid::Uuid,
	pub title: String,
	pub start: chrono::NaiveDateTime,
	pub end: chrono::NaiveDateTime,
	pub is_all_day: bool,
	pub description: String,
}
