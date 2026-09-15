use std::{future::Future, pin::Pin};

use config::ConfigFile;
use daemon::{DEFAULT_BUS_NAME, calendar, nightlight};
use futures::{StreamExt, pin_mut};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use tokio::sync::oneshot;

type Error = Box<dyn std::error::Error>;
type NightlightFuture = Pin<Box<dyn Future<Output = Result<(), Error>>>>;
type CalendarFuture = Pin<Box<dyn Future<Output = Result<(), Error>>>>;

#[tokio::main]
async fn main() -> Result<(), Error> {
	log::init!("daemon", "permafrostd")?;

	let config_path = ConfigFile::path()?;
	let (_doc, mut config) = ConfigFile::load_from_file(&config_path)?;
	let config_events = ConfigFile::watch_file(&config_path)?;
	pin_mut!(config_events);
	let connection = zbus::connection::Builder::session()?
		.name(DEFAULT_BUS_NAME)?
		.build()
		.await?;
	let (mut nightlight_shutdown, mut nightlight_task) =
		start_nightlight(connection.clone(), config.nightlight);
	let mut shutdown_signal = Box::pin(wait_for_shutdown_signal());

	let db = setup_db().await?;
	let (calendar_shutdown, mut calendar_task) = start_calendar(connection.clone(), db.clone());

	loop {
		tokio::select! {
			result = &mut shutdown_signal => {
				result?;
				let _ = nightlight_shutdown.send(());
				let _ = calendar_shutdown.send(());
				// nightlight_task.await?;

				let (res1, res2) = tokio::join!(
					nightlight_task,
					calendar_task,
				);
				res1?;
				res2?;

				break;
			}
			event = config_events.next() => {
				let Some(event) = event else {
					return Err("config watcher stopped".into());
				};
				event?;

				match ConfigFile::load_from_file(&config_path) {
					Ok((_doc, new_config)) => {
						log::info!("Reloading config");
						let _ = nightlight_shutdown.send(());
						nightlight_task.await?;

						config = new_config;
						(nightlight_shutdown, nightlight_task) =
							start_nightlight(connection.clone(), config.nightlight);
					}
					Err(error) => {
						log::error!("Failed to reload config: {error}");
					}
				}
			}
			// TODO: Don't shut down.
			result = &mut nightlight_task => {
				result?;
				return Err("nightlight service stopped unexpectedly".into());
			}
			result = &mut calendar_task => {
				result?;
				return Err("calendar service stopped unexpectedly".into());
			}
		}
	}

	Ok(())
}

fn start_nightlight(
	connection: zbus::Connection, config: config::Nightlight,
) -> (oneshot::Sender<()>, NightlightFuture) {
	let (shutdown_tx, shutdown_rx) = oneshot::channel();
	let task = Box::pin(nightlight::run(connection, config, async move {
		let _ = shutdown_rx.await;
		Ok(())
	}));

	(shutdown_tx, task)
}

// TODO: Allow for cal configuration, e.g. default reminders, db path with template expansion, etc.
fn start_calendar(
	connection: zbus::Connection, db: DatabaseConnection,
) -> (oneshot::Sender<()>, CalendarFuture) {
	let (shutdown_tx, shutdown_rx) = oneshot::channel();
	let task = Box::pin(calendar::run(connection, db, async move {
		let _ = shutdown_rx.await;
		Ok(())
	}));

	(shutdown_tx, task)
}

async fn wait_for_shutdown_signal() -> Result<(), Error> {
	#[cfg(unix)]
	{
		let mut terminate =
			tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

		tokio::select! {
			result = tokio::signal::ctrl_c() => result?,
			_ = terminate.recv() => (),
		}
	}

	#[cfg(not(unix))]
	{
		tokio::signal::ctrl_c().await?;
	}

	log::debug!("recieved shutdown event");

	Ok(())
}

async fn setup_db() -> Result<DatabaseConnection, sea_orm::DbErr> {
	let db_path = dirs::data_dir()
		.ok_or_else(|| sea_orm::DbErr::Custom("Failed to get data directory".to_string()))?
		.join("permafrostd");
	let _ = std::fs::create_dir_all(&db_path);
	let db_path = db_path.join("permafrostd.db");

	let db_url = format!("sqlite://{}", db_path.to_string_lossy());

	let mut opts = ConnectOptions::new(&db_url);
	opts.map_sqlx_sqlite_opts(|opts| opts.create_if_missing(true));

	let db = Database::connect(opts).await?;

	db.get_schema_registry("daemon::*").sync(&db).await?;

	Ok(db)
}
