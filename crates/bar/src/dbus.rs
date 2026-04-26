use async_channel::Sender;
use zbus::interface;

#[derive(Clone, Debug)]
pub enum PlayerCommand {
	Cycle,
	TogglePlayPause,
	Next,
	Previous,
	SetActiveByBusName(Option<String>),
}

pub struct DbusManager {
	send: Sender<PlayerCommand>,
}

impl DbusManager {
	pub fn new(send: Sender<PlayerCommand>) -> Self {
		Self { send }
	}
}

#[interface(name = "de.icytv.subniri.Bar1")]
impl DbusManager {
	async fn cycle_player(&self) {
		let _ = self.send.send(PlayerCommand::Cycle).await;
	}

	async fn toggle_play_pause(&self) {
		let _ = self.send.send(PlayerCommand::TogglePlayPause).await;
	}

	async fn next(&self) {
		let _ = self.send.send(PlayerCommand::Next).await;
	}

	async fn previous(&self) {
		let _ = self.send.send(PlayerCommand::Previous).await;
	}
}
