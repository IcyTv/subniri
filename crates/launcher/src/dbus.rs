use async_channel::Sender;
use zbus::interface;

use crate::launcher::Message;

pub const SERVICE_NAME: &str = "de.icytv.subniri.Launcher";
pub const OBJECT_PATH: &str = "/de/icytv/subniri/Launcher";

pub struct DbusListener {
	pub tx: Sender<Message>,
}

impl DbusListener {
	pub async fn connect(tx: Sender<Message>) -> zbus::Result<zbus::Connection> {
		let listener = DbusListener { tx };

		zbus::connection::Builder::session()
			.and_then(|s| s.name(SERVICE_NAME))
			.and_then(|s| s.serve_at(OBJECT_PATH, listener))
			.map(zbus::connection::Builder::build)?
			.await
	}

	async fn send(&self, msg: Message) -> zbus::fdo::Result<()> {
		self.tx
			.send(msg)
			.await
			.map_err(|e| zbus::fdo::Error::Failed(format!("{e}")))
	}
}

#[interface(name = "de.icytv.subniri.Launcher")]
impl DbusListener {
	async fn open(&self) -> zbus::fdo::Result<()> {
		self.send(Message::Open).await
	}

	async fn close(&self) -> zbus::fdo::Result<()> {
		self.send(Message::Close).await
	}

	async fn exit(&self) -> zbus::fdo::Result<()> {
		self.send(Message::Exit).await
	}
}
