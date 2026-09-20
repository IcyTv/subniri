use std::time::Duration;

use config::ConfigFile;
use daemon::{calendar, nightlight, spotify};
use daemon_common::DEFAULT_BUS_NAME;
use futures::{StreamExt, pin_mut};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use tokio::sync::{oneshot, watch};

type Error = Box<dyn std::error::Error>;

#[tokio::main]
async fn main() -> Result<(), Error> {
	log::init!("daemon", "permafrostd")?;

	let (_doc, mut config) = ConfigFile::load()?;
	let connection = zbus::connection::Builder::session()?
		.name(DEFAULT_BUS_NAME)?
		.build()
		.await?;
	let db = setup_db().await?;
	let (shutdown_tx, _) = watch::channel(false);
	let (nightlight_config_tx, _) = watch::channel(config.nightlight.clone());
	let (spotify_config_tx, _) = watch::channel(config.spotify.clone());
	let mut nightlight_supervisor = Box::pin(supervise_nightlight(
		connection.clone(),
		nightlight_config_tx.subscribe(),
		shutdown_tx.subscribe(),
	));
	let mut calendar_supervisor = Box::pin(supervise_calendar(
		connection.clone(),
		db.clone(),
		shutdown_tx.subscribe(),
	));
	let mut spotify_supervisor = Box::pin(supervise_spotify(
		connection.clone(),
		spotify_config_tx.subscribe(),
		shutdown_tx.subscribe(),
	));
	let mut shutdown_signal = Box::pin(wait_for_shutdown_signal());

	'daemon: loop {
		let config_events = match ConfigFile::watch() {
			Ok(events) => events,
			Err(error) => {
				log::error!("Failed to watch config: {error}; retrying");
				tokio::select! {
					result = &mut shutdown_signal => {
						result?;
						break 'daemon;
					}
					() = tokio::time::sleep(Duration::from_secs(2)) => continue,
					() = &mut nightlight_supervisor => {
						log::error!("Nightlight supervisor stopped; restarting it");
						nightlight_supervisor = Box::pin(supervise_nightlight(
							connection.clone(), nightlight_config_tx.subscribe(), shutdown_tx.subscribe()
						));
						continue;
					}
					() = &mut calendar_supervisor => {
						log::error!("Calendar supervisor stopped; restarting it");
						calendar_supervisor = Box::pin(supervise_calendar(
							connection.clone(), db.clone(), shutdown_tx.subscribe()
						));
						continue;
					}
					() = &mut spotify_supervisor => {
						log::error!("Spotify supervisor stopped; restarting it");
						spotify_supervisor = Box::pin(supervise_spotify(
							connection.clone(), spotify_config_tx.subscribe(), shutdown_tx.subscribe()
						));
						continue;
					}
				}
			}
		};
		pin_mut!(config_events);

		loop {
			tokio::select! {
				result = &mut shutdown_signal => {
					result?;
					break 'daemon;
				}
				event = config_events.next() => {
					let Some(event) = event else {
						log::error!("Config watcher stopped; restarting it");
						break;
					};
					if let Err(error) = event {
						log::error!("Config watch error: {error}");
						continue;
					}

					match ConfigFile::load() {
						Ok((_doc, new_config)) => {
							log::info!("Reloading config");
							if new_config.nightlight != config.nightlight {
								let _ = nightlight_config_tx.send(new_config.nightlight.clone());
							}
							if new_config.spotify != config.spotify {
								let _ = spotify_config_tx.send(new_config.spotify.clone());
							}
							config = new_config;
						}
						Err(error) => log::error!("Failed to reload config: {error}"),
					}
				}
				() = &mut nightlight_supervisor => {
					log::error!("Nightlight supervisor stopped; restarting it");
					nightlight_supervisor = Box::pin(supervise_nightlight(
						connection.clone(), nightlight_config_tx.subscribe(), shutdown_tx.subscribe()
					));
				}
				() = &mut calendar_supervisor => {
					log::error!("Calendar supervisor stopped; restarting it");
					calendar_supervisor = Box::pin(supervise_calendar(
						connection.clone(), db.clone(), shutdown_tx.subscribe()
					));
				}
				() = &mut spotify_supervisor => {
					log::error!("Spotify supervisor stopped; restarting it");
					spotify_supervisor = Box::pin(supervise_spotify(
						connection.clone(), spotify_config_tx.subscribe(), shutdown_tx.subscribe()
					));
				}
			}
		}
	}

	let _ = shutdown_tx.send(true);
	tokio::join!(
		nightlight_supervisor,
		calendar_supervisor,
		spotify_supervisor
	);
	db.close().await?;

	Ok(())
}

async fn supervise_nightlight(
	connection: zbus::Connection, mut config: watch::Receiver<config::Nightlight>,
	mut shutdown: watch::Receiver<bool>,
) {
	let mut backoff = Duration::from_secs(1);
	loop {
		if *shutdown.borrow() {
			return;
		}
		let current_config = config.borrow_and_update().clone();
		let (service_shutdown_tx, service_shutdown_rx) = oneshot::channel();
		let service = nightlight::run(connection.clone(), current_config, async move {
			let _ = service_shutdown_rx.await;
			Ok(())
		});
		tokio::pin!(service);

		let failed = tokio::select! {
			result = &mut service => {
				log_service_result("nightlight", result);
				true
			}
			changed = config.changed() => {
				let _ = service_shutdown_tx.send(());
				log_service_cleanup("nightlight", service.await);
				if changed.is_err() {
					return;
				}
				false
			}
			changed = shutdown.changed() => {
				let _ = service_shutdown_tx.send(());
				log_service_cleanup("nightlight", service.await);
				if changed.is_err() || *shutdown.borrow_and_update() {
					return;
				}
				false
			}
		};
		if *shutdown.borrow() {
			return;
		}
		if !failed {
			backoff = Duration::from_secs(1);
			continue;
		}

		tokio::select! {
			() = tokio::time::sleep(backoff) => {}
			changed = config.changed() => {
				if changed.is_err() {
					return;
				}
			}
			changed = shutdown.changed() => {
				if changed.is_err() || *shutdown.borrow_and_update() {
					return;
				}
			}
		}
		backoff = (backoff * 2).min(Duration::from_secs(30));
	}
}

async fn supervise_calendar(
	connection: zbus::Connection, db: DatabaseConnection, mut shutdown: watch::Receiver<bool>,
) {
	let mut backoff = Duration::from_secs(1);
	loop {
		if *shutdown.borrow() {
			return;
		}
		let (service_shutdown_tx, service_shutdown_rx) = oneshot::channel();
		let service = calendar::run(connection.clone(), db.clone(), async move {
			let _ = service_shutdown_rx.await;
			Ok(())
		});
		tokio::pin!(service);
		tokio::select! {
			result = &mut service => log_service_result("calendar", result),
			changed = shutdown.changed() => {
				let _ = service_shutdown_tx.send(());
				log_service_cleanup("calendar", service.await);
				if changed.is_err() || *shutdown.borrow_and_update() {
					return;
				}
			}
		}

		tokio::select! {
			() = tokio::time::sleep(backoff) => {}
			changed = shutdown.changed() => {
				if changed.is_err() || *shutdown.borrow_and_update() {
					return;
				}
			}
		}
		backoff = (backoff * 2).min(Duration::from_secs(30));
	}
}

async fn supervise_spotify(
	connection: zbus::Connection, config: watch::Receiver<config::Spotify>,
	mut shutdown: watch::Receiver<bool>,
) {
	let mut backoff = Duration::from_secs(1);
	while !*shutdown.borrow() {
		let result = spotify::run(connection.clone(), config.clone(), shutdown.clone()).await;
		if *shutdown.borrow() {
			log_service_cleanup("Spotify", result);
			return;
		}
		log_service_result("Spotify", result);
		tokio::select! {
			() = tokio::time::sleep(backoff) => {}
			changed = shutdown.changed() => {
				if changed.is_err() || *shutdown.borrow_and_update() {
					return;
				}
			}
		}
		backoff = (backoff * 2).min(Duration::from_secs(30));
	}
}

fn log_service_result(name: &str, result: Result<(), impl std::fmt::Display>) {
	match result {
		Ok(()) => log::warn!("{name} service stopped unexpectedly"),
		Err(error) => log::error!("{name} service failed: {error}"),
	}
}

fn log_service_cleanup(name: &str, result: Result<(), impl std::fmt::Display>) {
	if let Err(error) = result {
		log::error!("{name} service failed while stopping: {error}");
	}
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
