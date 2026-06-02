use zbus::proxy;

#[proxy(
	interface = "de.icytv.subniri.Launcher",
	default_path = "/de/icytv/subniri/Launcher",
	default_service = "de.icytv.subniri.Launcher"
)]
pub trait Launcher {
	fn open(&self) -> zbus::Result<()>;
	fn close(&self) -> zbus::Result<()>;
	fn exit(&self) -> zbus::Result<()>;
}
