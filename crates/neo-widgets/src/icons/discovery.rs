use std::time::Duration;

use daemon_common::{DEFAULT_BUS_NAME, DISCOVERY_OBJECT_PATH, DiscoveryProxy};
use iced::{
	Subscription,
	futures::{SinkExt, StreamExt},
};

#[derive(Debug, Clone, Copy)]
pub struct DiscoveryChange {
	pub applications: bool,
	pub icons: bool,
}

/// Reconnect after service replacement and invalidate on each new connection:
/// revisions belong to a daemon lifetime, not a persistent global counter.
pub fn discovery_updates() -> Subscription<DiscoveryChange> {
	Subscription::run(|| {
		iced::stream::channel(16, |mut output| async move {
			loop {
				if let Err(error) = observe(&mut output).await {
					log::debug!("Discovery connection unavailable: {error}");
				}
				tokio::time::sleep(Duration::from_secs(2)).await;
			}
		})
	})
}

async fn observe(
	output: &mut iced::futures::channel::mpsc::Sender<DiscoveryChange>,
) -> zbus::Result<()> {
	let connection = zbus::Connection::session().await?;
	let proxy = DiscoveryProxy::builder(&connection)
		.destination(DEFAULT_BUS_NAME)?
		.path(DISCOVERY_OBJECT_PATH)?
		.build()
		.await?;
	let mut changes = proxy.receive_changed().await?;
	let mut owners = proxy.inner().receive_owner_changed().await?;
	let mut latest = proxy.state().await?;
	let initial = DiscoveryChange {
		applications: true,
		icons: true,
	};
	super::invalidate_discovery(initial);
	if output.send(initial).await.is_err() {
		return Ok(());
	}

	loop {
		tokio::select! {
			_ = owners.next() => return Ok(()),
			change = changes.next() => {
				let Some(change) = change else { return Ok(()); };
				let state = change.args()?.state;
				// Ignore an older signal queued during the initial state read.
				let change = DiscoveryChange {
					applications: state.applications > latest.applications,
					icons: state.icons > latest.icons,
				};
				latest.applications = latest.applications.max(state.applications);
				latest.icons = latest.icons.max(state.icons);
				if change.applications || change.icons {
					super::invalidate_discovery(change);
					if output.send(change).await.is_err() { return Ok(()); }
				}
			}
		}
	}
}
