#[zbus::interface(
	name = "de.icytv.subniri.Launcher",
	proxy(
		default_path = "/de/icytv/subniri/Launcher",
		default_service = "de.icytv.subniri.Launcher",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait Launcher {
	async fn open(&self) -> zbus::fdo::Result<()>;
	async fn close(&self) -> zbus::fdo::Result<()>;
	async fn exit(&self) -> zbus::fdo::Result<()>;
}
