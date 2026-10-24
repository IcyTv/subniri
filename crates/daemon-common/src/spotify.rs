use zbus::zvariant::OwnedObjectPath;

pub const SPOTIFY_PORT: u16 = 43817;
pub const SPOTIFY_REDIRECT_URL: &str =
	const_format::formatc!("http://127.0.0.1:{}/callback", SPOTIFY_PORT);

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

	async fn begin_authorization(&self) -> zbus::fdo::Result<String>;
	async fn complete_authorization(&self, code: &str, state: &str) -> zbus::fdo::Result<bool>;
	async fn cancel_authorization(&self) -> zbus::fdo::Result<()>;
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
	async fn saved(&self) -> bool;

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
		}
	}
}

impl std::fmt::Display for SpotifyStatus {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		formatter.write_str(self.as_str())
	}
}
