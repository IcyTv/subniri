use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use zbus::object_server::SignalEmitter;

use daemon_common::{
	CALENDAR_INTERFACE, CALENDAR_OBJECT_PATH,
	calendar::{AddCalendarEventDto, CalendarEventDto},
};

mod entity;

pub async fn run<F>(
	connection: zbus::Connection, db: DatabaseConnection, shutdown_signal: F,
) -> Result<(), Box<dyn std::error::Error>>
where
	F: Future<Output = Result<(), Box<dyn std::error::Error>>>,
{
	let service = CalendarInterface { db };

	log::info!(
		"Serving calendar service on D-Bus at {}",
		CALENDAR_OBJECT_PATH
	);

	let added = connection
		.object_server()
		.at(CALENDAR_OBJECT_PATH, service.clone())
		.await?;
	if !added {
		return Err("calendar D-Bus interface is already registered".into());
	}

	tokio::select! {
		result = shutdown_signal => {
			result?;
			log::info!("Shutting down calendar service");

			connection
				.object_server()
				.remove::<CalendarInterface, _>(CALENDAR_OBJECT_PATH)
				.await?;
		}
		// TODO: Use tokio scheduler to do notifications, alerts, reminders, etc.
	}

	Ok(())
}

#[derive(Clone)]
pub struct CalendarInterface {
	db: DatabaseConnection,
}

#[zbus::interface(name = CALENDAR_INTERFACE)]
impl CalendarInterface {
	async fn get_events(
		&self, start: chrono::NaiveDateTime, end: chrono::NaiveDateTime,
	) -> zbus::fdo::Result<Vec<CalendarEventDto>> {
		let models = entity::Entity::find()
			.filter(entity::Column::StartTime.lte(end))
			.filter(entity::Column::EndTime.gte(start))
			.all(&self.db)
			.await
			.map_err(|e| zbus::fdo::Error::Failed(format!("Failed to query events: {}", e)))?;

		Ok(models.into_iter().map(CalendarEventDto::from).collect())
	}

	async fn add_event(
		&self, event: AddCalendarEventDto, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
	) -> zbus::fdo::Result<()> {
		let model: entity::ActiveModel = event.into();

		let res = model
			.insert(&self.db)
			.await
			.map_err(|e| zbus::fdo::Error::Failed(format!("Failed to insert event: {}", e)))?;

		emitter.event_added(res.into()).await.map_err(|e| {
			zbus::fdo::Error::Failed(format!("Failed to emit event_added signal: {}", e))
		})?;

		Ok(())
	}

	#[zbus(signal)]
	async fn event_added(
		signal_emitter: &SignalEmitter<'_>, event: CalendarEventDto,
	) -> zbus::Result<()>;
}

impl From<AddCalendarEventDto> for entity::ActiveModel {
	fn from(value: AddCalendarEventDto) -> Self {
		let description = if value.description.is_empty() {
			None
		} else {
			Some(value.description)
		};
		entity::Model {
			id: uuid::Uuid::new_v4(),
			title: value.title,
			description,
			start_time: value.start,
			end_time: value.end,
			is_all_day: value.is_all_day,
			color: None,
			reminder_minutes: None,
		}
		.into()
	}
}

impl From<entity::Model> for CalendarEventDto {
	fn from(value: entity::Model) -> Self {
		CalendarEventDto {
			id: value.id,
			title: value.title,
			start: value.start_time,
			end: value.end_time,
			is_all_day: value.is_all_day,
			description: value.description.unwrap_or_default(),
		}
	}
}
