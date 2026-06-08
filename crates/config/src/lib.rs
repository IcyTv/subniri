#![allow(clippy::missing_errors_doc)]

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use async_stream::stream;
use config_macros::{Config, ConfigFile, ConfigFileSerialize, ConfigSerialize};
use config_traits::ConfigFileValidateExt as _;
use config_traits::{ConfigError, ConfigFileSerialize};
use futures::Stream;
use garde::Validate;
use jiff::civil::Time;
pub use kdl::KdlDocument;
use notify::{
	EventKind, RecursiveMode, Watcher,
	event::{CreateKind, ModifyKind, RemoveKind},
};

const CONFIG_WATCH_DEBOUNCE: Duration = Duration::from_millis(250);

static PROCESS_WRITES: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();

/// This is the main configuration file for the subniri desktop shell.
/// If you want to, you can edit settings from here, and they'll be automatically syncronized to
/// all components in the subniri shell.
/// You can also edit the settings in a graphical interface (`snowconf`), and they will be
/// synchronized to this file.
#[derive(Default, Debug, Clone, ConfigFile, ConfigFileSerialize, Validate)]
#[garde(allow_unvalidated)]
pub struct ConfigFile {
	/// Configuration for controlling the behavior of the nightlight.
	/// Temperature controls the color temperature (like with a light bulb) in [K]elvin. Lower means
	/// more red.
	/// Brightness controls the brighness of the screen. You probably won't want to go below ~0.2,
	/// because it becomes very hard to see.
	#[config(default)]
	#[garde(dive)]
	pub nightlight: Nightlight,
	/// Settings for controlling homeassistant.
	/// If you don't know what homeassistant is, please ignore this configuration. It's not for you.
	/// Otherwise, you'll need to set a url, and then log in in the Settings app.
	/// The connection keys are stored in your system's secret store.
	#[config(default)]
	#[garde(dive)]
	pub homeassistant: Homeassistant,
	/// Settings for controlling Spotify.
	/// Enabling this will let you do things, like adding a song to your library from the bar, at
	/// the cost of some networking overhead.
	/// To connect to spotify, you'll need a client id and a client secret by creating an
	/// application at <https://developer.spotify.com/dashboard> and setting the client id and
	/// client secret in the settings (Or manually adding them to the key store). Then you need to
	/// log in to your Spotify account.
	/// As far as I undertand, this will only work using a Spotify Premium account.
	/// Connection details and secrets are stored in your system's secret store.
	#[config(default)]
	#[garde(dive)]
	pub spotify: Spotify,
	/// Settings for controlling the System Menu (the bar module with the four squares)
	#[config(default)]
	#[garde(dive)]
	pub system_menu: SystemMenu,
}

impl ConfigFile {
	pub fn path() -> Result<PathBuf, ConfigError> {
		std::env::var("SUBNIRI_CONFIG_FILE")
			.map(std::path::PathBuf::from)
			.ok()
			.or_else(|| Some(dirs::config_dir()?.join("subniri/config.kdl")))
			.ok_or_else(|| {
				std::io::Error::new(std::io::ErrorKind::NotFound, "No config file found").into()
			})
	}

	pub fn load() -> Result<(KdlDocument, Self), ConfigError> {
		Self::load_from_file(Self::path()?)
	}

	pub fn watch() -> Result<impl Stream<Item = Result<(), ConfigError>>, ConfigError> {
		Self::watch_file(Self::path()?)
	}

	pub fn watch_file(
		file: impl AsRef<Path>,
	) -> Result<impl Stream<Item = Result<(), ConfigError>>, ConfigError> {
		let config_path = absolute_path(file)?;
		let watched_dir = config_path
			.parent()
			.ok_or_else(|| std::io::Error::other("config path has no parent directory"))?
			.to_path_buf();
		let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

		let mut watcher =
			notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
				let _ = tx.send(event);
			})
			.map_err(notify_error)?;

		watcher
			.watch(&watched_dir, RecursiveMode::NonRecursive)
			.map_err(notify_error)?;

		Ok(stream! {
			let _watcher = watcher;

			while let Some(event) = rx.recv().await {
				match event {
					Ok(event) if is_config_change(&event, &config_path) => {}
					Ok(_) => continue,
					Err(error) => {
						yield Err(notify_error(error));
						continue;
					}
				}

				tokio::time::sleep(CONFIG_WATCH_DEBOUNCE).await;
				let mut changed = true;

				while let Ok(event) = rx.try_recv() {
					match event {
						Ok(event) => {
							changed |= is_config_change(&event, &config_path);
						}
						Err(error) => yield Err(notify_error(error)),
					}
				}

				if !changed {
					continue;
				}

				match current_file_contents(&config_path) {
					Ok(contents) if is_process_write(&config_path, &contents) => (),
					Ok(_) => yield Ok(()),
					Err(error) => yield Err(error),
				}
			}
		})
	}

	pub fn load_from_file<P: AsRef<Path>>(file: P) -> Result<(KdlDocument, Self), ConfigError> {
		let file = file.as_ref();
		match std::fs::read_to_string(file) {
			Ok(doc) => {
				let config = Self::parse_validated(&doc).map_err(|e| {
					std::io::Error::new(
						std::io::ErrorKind::InvalidData,
						format!("Failed to parse config file: {e}"),
					)
				})?;
				Ok((config.0, config.1))
			}
			Err(e) if e.kind() == io::ErrorKind::NotFound => {
				let config = Self::default();
				let mut doc = KdlDocument::new();
				config.apply_to_kdl_document(&mut doc)?;
				doc.autoformat();

				if let Some(parent) = file.parent() {
					std::fs::create_dir_all(parent)?;
				}

				std::fs::write(file, doc.to_string())?;

				Ok((doc, config))
			}
			Err(e) => Err(e.into()),
		}
	}

	pub fn write(&self, doc: &mut KdlDocument) -> Result<(), ConfigError> {
		self.write_to_file(doc, Self::path()?)
	}

	pub fn write_to_file(
		&self, doc: &mut KdlDocument, file: impl AsRef<Path>,
	) -> Result<(), ConfigError> {
		let file = file.as_ref();

		// NOTE: This is mostly a safety net, so we don't do something stupid when programatically
		// changing the config
		#[cfg(debug_assertions)]
		if let Err(e) = self.validate() {
			return Err(ConfigError::Validation {
				error: Box::new(e),
				src: None,
				span: None,
			});
		}

		self.apply_to_kdl_document(doc)?;

		// TODO: Should we want/need to check for existing edits?

		// TODO: Should we format?
		doc.autoformat();

		let contents = doc.to_string();
		std::fs::write(file, &contents)?;
		record_process_write(file, contents)?;

		Ok(())
	}
}

fn is_config_change(event: &notify::Event, config_path: &Path) -> bool {
	let relevant_kind = matches!(
		event.kind,
		EventKind::Any
			| EventKind::Create(CreateKind::Any | CreateKind::File)
			| EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Name(_))
			| EventKind::Remove(RemoveKind::Any | RemoveKind::File)
	);

	relevant_kind
		&& event
			.paths
			.iter()
			.filter_map(|path| absolute_path(path).ok())
			.any(|path| path == config_path)
}

fn current_file_contents(path: &Path) -> Result<String, ConfigError> {
	match std::fs::read_to_string(path) {
		Ok(contents) => Ok(contents),
		Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
		Err(error) => Err(error.into()),
	}
}

fn record_process_write(path: &Path, contents: String) -> Result<(), ConfigError> {
	let path = absolute_path(path)?;
	let mut writes = PROCESS_WRITES
		.get_or_init(|| Mutex::new(HashMap::new()))
		.lock()
		.map_err(|_| std::io::Error::other("config write tracking lock poisoned"))?;

	writes.insert(path, contents);
	Ok(())
}

fn is_process_write(path: &Path, contents: &str) -> bool {
	let Ok(mut writes) = PROCESS_WRITES
		.get_or_init(|| Mutex::new(HashMap::new()))
		.lock()
	else {
		return false;
	};

	if writes
		.get(path)
		.is_some_and(|last_write| last_write == contents)
	{
		writes.remove(path);
		true
	} else {
		false
	}
}

fn absolute_path(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
	let path = path.as_ref();
	if path.is_absolute() {
		Ok(path.to_path_buf())
	} else {
		Ok(std::env::current_dir()?.join(path))
	}
}

fn notify_error(error: notify::Error) -> ConfigError {
	std::io::Error::other(error).into()
}

#[derive(Debug, Config, Clone, ConfigSerialize, Validate)]
#[garde(allow_unvalidated)]
pub struct Nightlight {
	/// If the nightlight integration should be enabled.
	pub enabled: bool,

	/// Whether you want to use a location provider (geoclue2) and a weather API to determine the dawn
	/// and dusk times based of the real times that the sun sets.
	/// This setting can't be combined with manually setting `dawn` or `dusk`
	#[config(default = false)]
	#[garde(custom(verify_use_location_dusk_dawn(&self.dusk, &self.dawn)))]
	pub use_location: bool,

	/// Time (usually in the morning), when the nightlight should change to the `day` settings.
	/// The format can be anything close to `"HH:MM"`
	// TODO: Allow for default = None, when use_location and not specified...
	#[config(default = unsafe { Time::new(7, 0, 0, 0).unwrap_unchecked() })]
	#[garde(custom(verify_dusk_dawn_use_location("dawn", self.use_location)))]
	pub dawn: Option<Time>,
	/// Time (usually in the evening), when the nightlight should change to the `night` settings.
	/// The format can be anything close to `"HH:MM"`
	#[config(default = unsafe { Time::new(20, 0, 0, 0).unwrap_unchecked() })]
	#[garde(custom(verify_dusk_dawn_use_location("dusk", self.use_location)))]
	pub dusk: Option<Time>,

	/// Settings for daytime (after dawn, before dusk)
	#[config(default = NightlightSetting::day())]
	#[garde(dive)]
	pub day: NightlightSetting,
	/// Settings for night time (after dusk, before dawn)
	#[config(default = NightlightSetting::night())]
	#[garde(dive)]
	pub night: NightlightSetting,

	/// Debounce delay for applying gamma table changes, in milliseconds.
	/// This prevents rapid slider updates from overwhelming the compositor.
	#[config(default = 500)]
	#[garde(range(min = 0, max = 10_000))]
	pub debounce_ms: u64,
}

#[allow(clippy::ref_option)]
fn verify_use_location_dusk_dawn<'a>(
	dusk: &'a Option<Time>, dawn: &'a Option<Time>,
) -> impl FnOnce(&bool, &()) -> garde::Result + 'a {
	move |value, ()| {
		if *value && dusk.is_some() {
			Err(garde::Error::new(
				"`use_location` and `dusk` are mutually exclusive",
			))
		} else if *value && dawn.is_some() {
			Err(garde::Error::new(
				"`use_location` and `dawn` are mutually exclusive",
			))
		} else {
			Ok(())
		}
	}
}

fn verify_dusk_dawn_use_location(
	name: &'static str, use_location: bool,
) -> impl FnOnce(&Option<Time>, &()) -> garde::Result {
	move |value, ()| {
		if use_location && value.is_some() {
			Err(garde::Error::new(format!(
				"`use_location` and `{name}` are mutually exclusive"
			)))
		} else {
			Ok(())
		}
	}
}

impl Default for Nightlight {
	fn default() -> Self {
		Self {
			enabled: false,
			use_location: false,
			dawn: Some(unsafe { jiff::civil::Time::new(7, 0, 0, 0).unwrap_unchecked() }),
			dusk: Some(unsafe { jiff::civil::Time::new(20, 0, 0, 0).unwrap_unchecked() }),
			night: NightlightSetting::night(),
			day: NightlightSetting::day(),
			debounce_ms: 500,
		}
	}
}

#[derive(Debug, Config, Clone, ConfigSerialize, Validate)]
pub struct NightlightSetting {
	/// Temperature of the light in [K]elvin. Basically the lower the number, the redder the light.
	/// Normal daytime temperature is 6500.
	/// Range [1000-10000]
	#[garde(range(min = 1000, max = 10000))]
	pub temperature: u32,
	/// Brightness of the light.
	/// Range [0.1-1.0]
	#[garde(range(min = 0.1, max = 1.0))]
	pub brightness: f64,
}

impl Default for NightlightSetting {
	fn default() -> Self {
		Self::day()
	}
}

impl NightlightSetting {
	#[must_use]
	pub const fn day() -> Self {
		Self {
			temperature: 6500,
			brightness: 1.0,
		}
	}

	#[must_use]
	pub const fn night() -> Self {
		Self {
			temperature: 2500,
			brightness: 0.7,
		}
	}
}

#[derive(Debug, Default, Clone, Config, ConfigSerialize, Validate)]
#[garde(allow_unvalidated)]
pub struct Homeassistant {
	/// Should the homeassistant integration be enabled? If it's disabled, you won't be able to
	/// control your homeassistant-controlled devices from subniri.
	pub enabled: bool,
	/// The url of your homeassistant instance.
	/// Format "<http://homeassistant.local:8123>"
	pub url: Option<url::Url>,
	/// A list of device id's that you want to be able to control from subniri. If you go to the
	/// settings (`snowconf`), you'll be able to add all devices. But be aware, that this might
	/// require more resources from your system.
	#[config(list_style = children)]
	pub tracked_devices: Vec<String>,
}

#[derive(Debug, Default, Clone, Config, ConfigSerialize, Validate)]
#[garde(allow_unvalidated)]
pub struct Spotify {
	/// Whether to enable the spotify integration or not.
	enabled: bool,
}

#[derive(Debug, Clone, Config)]
pub enum SystemMenuWidgets {
	Wifi,
	Bluetooth,
	Speaker,
	Microphone,
	Vpn,
	Nightlight,
}

#[derive(Debug, Default, Clone, Config, ConfigSerialize, Validate)]
#[garde(allow_unvalidated)]
pub struct SystemMenu {
	/// Widgets to be displayed in the system menu. These put into 2 columns by the order they
	/// appear in this list.
	/// So: `a b c d`
	/// Turns into:
	///     `a b`
	///     `c d`
	pub widgets: Vec<SystemMenuWidgets>,
}
