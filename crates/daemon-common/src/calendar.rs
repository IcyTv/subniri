#[zbus::interface(
	name = "de.icytv.subniri.Calendar",
	proxy(
		default_service = "de.icytv.subniri.Daemon",
		default_path = "/de/icytv/subniri/Calendar",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait Calendar {
	async fn get_events(
		&self, start: chrono::NaiveDateTime, end: chrono::NaiveDateTime,
	) -> zbus::fdo::Result<Vec<CalendarEventDto>>;
	async fn add_event(
		&self, event: AddCalendarEventDto,
		#[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
	) -> zbus::fdo::Result<()>;

	#[zbus(signal)]
	async fn event_added(
		emitter: &zbus::object_server::SignalEmitter<'_>, event: CalendarEventDto,
	) -> zbus::Result<()>;
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
