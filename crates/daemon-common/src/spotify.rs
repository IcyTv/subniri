use zbus::zvariant::OwnedObjectPath;

#[derive(
	Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, zbus::zvariant::Type,
)]
pub struct SpotifyPairing {
	pub verification_uri: String,
	pub user_code: String,
	pub expires_at: u64,
}

#[zbus::interface(
	name = "de.icytv.subniri.Spotify",
	proxy(
		default_service = "de.icytv.subniri.Daemon",
		default_path = "/de/icytv/subniri/Spotify",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait Spotify {
	#[zbus(property)]
	async fn status(&self) -> SpotifyStatus;

	#[zbus(property)]
	async fn authenticated(&self) -> bool;

	async fn begin_pairing(
		&self, #[zbus(header)] header: zbus::message::Header<'_>,
	) -> zbus::fdo::Result<SpotifyPairing>;
	async fn cancel_pairing(
		&self, #[zbus(header)] header: zbus::message::Header<'_>,
	) -> zbus::fdo::Result<()>;
	#[zbus(property)]
	async fn retry_at(&self) -> u64;
	async fn get_track(&self, track_id: &str) -> zbus::fdo::Result<OwnedObjectPath>;
}

#[zbus::interface(
	name = "de.icytv.subniri.Spotify.Track",
	proxy(
		default_service = "de.icytv.subniri.Daemon",
		gen_blocking = false,
		visibility = "pub"
	)
)]
pub trait SpotifyTrack {
	#[zbus(property(emits_changed_signal = "const"))]
	fn track_id(&self) -> String;

	#[zbus(property(emits_changed_signal = "invalidates"))]
	async fn saved(&self) -> zbus::fdo::Result<bool>;

	/// Registers interest even if the initial saved value is temporarily unknown.
	/// Keep the connection/property subscription alive for later push updates;
	/// Unobserve or disconnect releases that interest.
	async fn observe(
		&self, #[zbus(header)] header: zbus::message::Header<'_>,
		#[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
	) -> zbus::fdo::Result<bool>;
	async fn unobserve(
		&self, #[zbus(header)] header: zbus::message::Header<'_>,
	) -> zbus::fdo::Result<()>;
	async fn refresh(
		&self, #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
	) -> zbus::fdo::Result<bool>;
	async fn set_saved(
		&self, saved: bool, #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
	) -> zbus::fdo::Result<bool>;
}

#[derive(
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
	serde::Serialize,
	serde::Deserialize,
	zvariant::Type,
	zvariant::Value,
	zvariant::OwnedValue,
)]
#[zvariant(signature = "s")]
pub enum SpotifyStatus {
	Unconfigured,
	Connecting,
	Disabled,
	Ready,
	AuthenticationRequired,
	Authorizing,
	Error,
	RateLimited,
	Unavailable,
}

impl SpotifyStatus {
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Unconfigured => "Unconfigured",
			Self::Connecting => "Connecting",
			Self::Disabled => "Disabled",
			Self::Ready => "Ready",
			Self::AuthenticationRequired => "Authentication required",
			Self::Authorizing => "Authorizing",
			Self::Error => "Error",
			Self::RateLimited => "Rate limited; waiting to retry",
			Self::Unavailable => "Temporarily unavailable; reconnecting",
		}
	}
}

impl std::fmt::Display for SpotifyStatus {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		formatter.write_str(self.as_str())
	}
}
