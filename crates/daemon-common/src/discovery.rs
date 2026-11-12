/// Independent revisions let clients retain caches unaffected by a change.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, zvariant::Type,
)]
pub struct DiscoveryState {
	pub applications: u64,
	pub icons: u64,
}

#[zbus::interface(
	name = "de.icytv.subniri.Discovery",
	proxy(gen_blocking = false, visibility = "pub")
)]
pub trait Discovery {
	async fn state(&self) -> DiscoveryState;

	#[zbus(signal)]
	async fn changed(
		context: &zbus::object_server::SignalEmitter<'_>, state: DiscoveryState,
	) -> zbus::Result<()>;
}
