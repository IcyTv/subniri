use async_channel::{Receiver, Sender};
use zbus::interface;

#[derive(Clone, Copy)]
pub enum LauncherEvent {
	Launch,
	Hide,
}

pub struct LauncherManager {
	sender: Sender<LauncherEvent>,
}

impl LauncherManager {
	pub fn new() -> (Self, Receiver<LauncherEvent>) {
		let (sender, receiver) = async_channel::unbounded();

		(Self { sender }, receiver)
	}
}

#[interface(name = "de.icytv.subniri.Launcher")]
impl LauncherManager {
	async fn launch(&self) {
		let _ = self.sender.send(LauncherEvent::Launch).await;
	}

	async fn hide(&self) {
		let _ = self.sender.send(LauncherEvent::Hide).await;
	}
}
