use std::{future::Future, pin::Pin};

use config::ConfigFile;
use daemon::nightlight;
use futures::{StreamExt, pin_mut};
use tokio::sync::oneshot;

type Error = Box<dyn std::error::Error>;
type NightlightFuture = Pin<Box<dyn Future<Output = Result<(), Error>>>>;

#[tokio::main]
async fn main() -> Result<(), Error> {
	let _ = pretty_env_logger::try_init();

	let config_path = ConfigFile::path()?;
	let (_doc, mut config) = ConfigFile::load_from_file(&config_path)?;
	let config_events = ConfigFile::watch_file(&config_path)?;
	pin_mut!(config_events);
	let (mut nightlight_shutdown, mut nightlight_task) = start_nightlight(config.nightlight);
	let mut shutdown_signal = Box::pin(wait_for_shutdown_signal());

	loop {
		tokio::select! {
			result = &mut shutdown_signal => {
				result?;
				let _ = nightlight_shutdown.send(());
				nightlight_task.await?;
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
						(nightlight_shutdown, nightlight_task) = start_nightlight(config.nightlight);
					}
					Err(error) => {
						log::error!("Failed to reload config: {error}");
					}
				}
			}
			result = &mut nightlight_task => {
				result?;
				return Err("nightlight service stopped unexpectedly".into());
			}
		}
	}

	Ok(())
}

fn start_nightlight(config: config::Nightlight) -> (oneshot::Sender<()>, NightlightFuture) {
	let (shutdown_tx, shutdown_rx) = oneshot::channel();
	let task = Box::pin(nightlight::run(config, async move {
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
