/// Effective session appearance and the outcome of its latest persistence attempt.
/// A nonempty error does not mean the selection was rejected: `theme` remains active.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, zvariant::Type)]
pub struct AppearanceState {
	pub theme: String,
	pub saving: bool,
	pub error: String,
	pub revision: u64,
}

#[zbus::interface(
	name = "de.icytv.subniri.Appearance",
	proxy(gen_blocking = false, visibility = "pub")
)]
pub trait Appearance {
	async fn state(&self) -> AppearanceState;

	async fn select(&self, theme: &str) -> zbus::fdo::Result<()>;

	async fn retry_saving(&self) -> zbus::fdo::Result<()>;

	#[zbus(signal)]
	async fn changed(
		context: &zbus::object_server::SignalEmitter<'_>, state: AppearanceState,
	) -> zbus::Result<()>;
}
