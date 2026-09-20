#![allow(clippy::missing_errors_doc)]

use std::collections::{HashMap, HashSet};
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
const CONFIG_OVERRIDE_ENV: &str = "SUBNIRI_CONFIG_OVERRIDE_FILE";

static PROCESS_WRITES: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();

/// This is the main configuration file for the subniri desktop shell.
/// If you want to, you can edit settings from here, and they'll be automatically syncronized to
/// all components in the subniri shell.
/// You can also edit the settings in a graphical interface (`snowconf`), and they will be
/// synchronized to this file.
#[derive(Default, Debug, Clone, PartialEq, ConfigFile, ConfigFileSerialize, Validate)]
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
	/// Settings for controlling the launcher
	#[config(default)]
	#[garde(dive)]
	pub launcher: Launcher,

	/// Settings for controlling file indexing.
	/// File indexing builds a database of files, projects, etc. on your system, which allows for
	/// quick lookup of items in that database, for example from the launcher, at the cost of some
	/// background resources.
	#[config(default)]
	#[garde(dive)]
	pub indexing: Indexing,
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

	pub fn override_path() -> Option<PathBuf> {
		std::env::var(CONFIG_OVERRIDE_ENV)
			.ok()
			.filter(|path| !path.is_empty())
			.map(PathBuf::from)
	}

	pub fn active_path() -> Result<PathBuf, ConfigError> {
		Ok(Self::override_path()
			.filter(|path| path.exists())
			.unwrap_or(Self::path()?))
	}

	#[must_use]
	pub fn override_active() -> bool {
		Self::override_path().is_some_and(|path| path.exists())
	}

	pub fn load() -> Result<(KdlDocument, Self), ConfigError> {
		Self::load_from_file(Self::active_path()?)
	}

	pub fn load_declarative() -> Result<(KdlDocument, Self), ConfigError> {
		Self::load_from_file(Self::path()?)
	}

	pub fn watch() -> Result<impl Stream<Item = Result<(), ConfigError>>, ConfigError> {
		let mut paths = vec![Self::path()?];
		if let Some(override_path) = Self::override_path() {
			paths.push(override_path);
		}
		watch_files(paths)
	}

	pub fn watch_file(
		file: impl AsRef<Path>,
	) -> Result<impl Stream<Item = Result<(), ConfigError>>, ConfigError> {
		watch_files([file.as_ref().to_path_buf()])
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
		if let Some(override_path) = Self::override_path() {
			let (declarative_doc, declarative_config) = Self::load_declarative()?;
			if *self == declarative_config {
				if override_path.exists() {
					std::fs::remove_file(&override_path)?;
					record_process_write(&override_path, String::new())?;
				}
				*doc = declarative_doc;
				return Ok(());
			}

			return self.write_to_file(doc, override_path);
		}

		self.write_to_file(doc, Self::path()?)
	}

	pub fn clear_override() -> Result<(KdlDocument, Self), ConfigError> {
		if let Some(path) = Self::override_path()
			&& path.exists()
		{
			std::fs::remove_file(&path)?;
			record_process_write(&path, String::new())?;
		}

		Self::load_declarative()
	}

	#[must_use]
	#[allow(clippy::too_many_lines)]
	pub fn nix_diff(&self, declarative: &Self) -> String {
		let mut out = String::new();

		nix_assignment(
			&mut out,
			"services.subniri.settings.nightlight.enable",
			self.nightlight.enabled,
			declarative.nightlight.enabled,
			|value| value.to_string(),
		);
		nix_assignment(
			&mut out,
			"services.subniri.settings.nightlight.useLocation",
			self.nightlight.use_location,
			declarative.nightlight.use_location,
			|value| value.to_string(),
		);
		nix_assignment(
			&mut out,
			"services.subniri.settings.nightlight.dawn",
			self.nightlight.dawn,
			declarative.nightlight.dawn,
			nix_time,
		);
		nix_assignment(
			&mut out,
			"services.subniri.settings.nightlight.dusk",
			self.nightlight.dusk,
			declarative.nightlight.dusk,
			nix_time,
		);
		nix_assignment(
			&mut out,
			"services.subniri.settings.nightlight.day.temperature",
			self.nightlight.day.temperature,
			declarative.nightlight.day.temperature,
			|value| value.to_string(),
		);
		nix_assignment(
			&mut out,
			"services.subniri.settings.nightlight.day.brightness",
			self.nightlight.day.brightness,
			declarative.nightlight.day.brightness,
			|value| value.to_string(),
		);
		nix_assignment(
			&mut out,
			"services.subniri.settings.nightlight.night.temperature",
			self.nightlight.night.temperature,
			declarative.nightlight.night.temperature,
			|value| value.to_string(),
		);
		nix_assignment(
			&mut out,
			"services.subniri.settings.nightlight.night.brightness",
			self.nightlight.night.brightness,
			declarative.nightlight.night.brightness,
			|value| value.to_string(),
		);
		nix_assignment(
			&mut out,
			"services.subniri.settings.nightlight.debounceMs",
			self.nightlight.debounce_ms,
			declarative.nightlight.debounce_ms,
			|value| value.to_string(),
		);

		nix_assignment(
			&mut out,
			"services.subniri.settings.homeassistant.enable",
			self.homeassistant.enabled,
			declarative.homeassistant.enabled,
			|value| value.to_string(),
		);
		nix_assignment(
			&mut out,
			"services.subniri.settings.homeassistant.url",
			self.homeassistant.url.as_ref(),
			declarative.homeassistant.url.as_ref(),
			|value| value.map_or_else(|| "null".to_string(), |url| nix_string(url.as_str())),
		);
		nix_assignment(
			&mut out,
			"services.subniri.settings.homeassistant.trackedDevices",
			&self.homeassistant.tracked_devices,
			&declarative.homeassistant.tracked_devices,
			|values| nix_list(values.iter().map(String::as_str)),
		);

		nix_assignment(
			&mut out,
			"services.subniri.settings.spotify.enable",
			self.spotify.enabled,
			declarative.spotify.enabled,
			|value| value.to_string(),
		);
		nix_assignment(
			&mut out,
			"services.subniri.settings.spotify.clientId",
			self.spotify.client_id.as_str(),
			declarative.spotify.client_id.as_str(),
			nix_string,
		);

		nix_assignment(
			&mut out,
			"services.subniri.settings.systemMenu.widgets",
			&self.system_menu.widgets,
			&declarative.system_menu.widgets,
			|widgets| nix_list(widgets.iter().copied().map(system_menu_widget_name)),
		);
		nix_assignment(
			&mut out,
			"services.subniri.settings.launcher.providers",
			&self.launcher.providers,
			&declarative.launcher.providers,
			|providers| nix_list(providers.iter().copied().map(launcher_provider_name)),
		);

		let fuzzy = &self.launcher.fuzzy_search;
		let declarative_fuzzy = &declarative.launcher.fuzzy_search;
		for (name, value, base) in [
			("min_chars", fuzzy.min_chars, declarative_fuzzy.min_chars),
			(
				"short.chars",
				fuzzy.short_query_chars,
				declarative_fuzzy.short_query_chars,
			),
			(
				"short.distance",
				fuzzy.short_max_distance,
				declarative_fuzzy.short_max_distance,
			),
			(
				"medium.chars",
				fuzzy.medium_query_chars,
				declarative_fuzzy.medium_query_chars,
			),
			(
				"medium.distance",
				fuzzy.medium_max_distance,
				declarative_fuzzy.medium_max_distance,
			),
			(
				"long.distance",
				fuzzy.long_max_distance,
				declarative_fuzzy.long_max_distance,
			),
		] {
			nix_assignment(
				&mut out,
				&format!("services.subniri.settings.launcher.fuzzy_search.{name}"),
				value,
				base,
				|value| value.to_string(),
			);
		}

		nix_assignment(
			&mut out,
			"services.subniri.icepickd.enable",
			self.indexing.enabled,
			declarative.indexing.enabled,
			|value| value.to_string(),
		);

		out
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

#[allow(clippy::needless_pass_by_value)]
fn nix_assignment<T: PartialEq>(
	out: &mut String, name: &str, value: T, declarative: T, render: impl FnOnce(T) -> String,
) {
	if value != declarative {
		out.push_str(name);
		out.push_str(" = ");
		out.push_str(&render(value));
		out.push_str(";\n");
	}
}

fn nix_time(value: Option<Time>) -> String {
	value.map_or_else(
		|| "null".to_string(),
		|time| nix_string(&time.strftime("%H:%M").to_string()),
	)
}

fn nix_string(value: &str) -> String {
	format!(
		"\"{}\"",
		value
			.replace('\\', "\\\\")
			.replace('"', "\\\"")
			.replace("${", "\\${")
			.replace('\n', "\\n")
			.replace('\r', "\\r")
			.replace('\t', "\\t")
	)
}

fn nix_list<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
	format!(
		"[{}]",
		values
			.into_iter()
			.map(nix_string)
			.collect::<Vec<_>>()
			.join(" ")
	)
}

const fn system_menu_widget_name(widget: SystemMenuWidgets) -> &'static str {
	match widget {
		SystemMenuWidgets::Wifi => "wifi",
		SystemMenuWidgets::Bluetooth => "bluetooth",
		SystemMenuWidgets::Speaker => "speaker",
		SystemMenuWidgets::Microphone => "microphone",
		SystemMenuWidgets::Vpn => "vpn",
		SystemMenuWidgets::Nightlight => "nightlight",
	}
}

const fn launcher_provider_name(provider: LauncherProvider) -> &'static str {
	match provider {
		LauncherProvider::Calculator => "calculator",
		LauncherProvider::Applications => "applications",
		LauncherProvider::Files => "files",
		LauncherProvider::Nix => "nix",
	}
}

fn watch_files(
	files: impl IntoIterator<Item = impl AsRef<Path>>,
) -> Result<impl Stream<Item = Result<(), ConfigError>>, ConfigError> {
	let config_paths = files
		.into_iter()
		.map(absolute_path)
		.collect::<Result<HashSet<_>, _>>()?;
	let watched_dirs = config_paths
		.iter()
		.map(|path| {
			path.parent()
				.map(Path::to_path_buf)
				.ok_or_else(|| std::io::Error::other("config path has no parent directory"))
		})
		.collect::<Result<HashSet<_>, _>>()?;
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

	let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
		let _ = tx.send(event);
	})
	.map_err(notify_error)?;

	for watched_dir in watched_dirs {
		watcher
			.watch(&watched_dir, RecursiveMode::NonRecursive)
			.map_err(notify_error)?;
	}

	Ok(stream! {
		let _watcher = watcher;

		while let Some(event) = rx.recv().await {
			let mut changed_paths = match event {
				Ok(event) => config_change_paths(&event, &config_paths),
				Err(error) => {
					yield Err(notify_error(error));
					continue;
				}
			};
			if changed_paths.is_empty() {
				continue;
			}

			tokio::time::sleep(CONFIG_WATCH_DEBOUNCE).await;

			while let Ok(event) = rx.try_recv() {
				match event {
					Ok(event) => changed_paths.extend(config_change_paths(&event, &config_paths)),
					Err(error) => yield Err(notify_error(error)),
				}
			}

			let mut external_change = false;
			for path in changed_paths {
				match current_file_contents(&path) {
					Ok(contents) if is_process_write(&path, &contents) => {}
					Ok(_) => external_change = true,
					Err(error) => yield Err(error),
				}
			}

			if external_change {
				yield Ok(());
			}
		}
	})
}

fn config_change_paths(event: &notify::Event, config_paths: &HashSet<PathBuf>) -> HashSet<PathBuf> {
	let relevant_kind = matches!(
		event.kind,
		EventKind::Any
			| EventKind::Create(CreateKind::Any | CreateKind::File)
			| EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Name(_))
			| EventKind::Remove(RemoveKind::Any | RemoveKind::File)
	);

	if !relevant_kind {
		return HashSet::new();
	}

	event
		.paths
		.iter()
		.filter_map(|path| absolute_path(path).ok())
		.filter(|path| config_paths.contains(path))
		.collect()
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

#[derive(Debug, Config, Clone, PartialEq, ConfigSerialize, Validate)]
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

#[derive(Debug, Config, Clone, PartialEq, ConfigSerialize, Validate)]
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

#[derive(Debug, Default, Clone, PartialEq, Config, ConfigSerialize, Validate)]
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

#[derive(Debug, Default, Clone, PartialEq, Config, ConfigSerialize, Validate)]
#[garde(allow_unvalidated)]
pub struct Spotify {
	/// Whether to enable the spotify integration or not.
	pub enabled: bool,
	/// The client ID of the user's Spotify application.
	#[config(default)]
	pub client_id: String,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Config, strum::VariantArray)]
pub enum SystemMenuWidgets {
	Wifi,
	Bluetooth,
	Speaker,
	Microphone,
	Vpn,
	Nightlight,
}

#[derive(Debug, Default, Clone, PartialEq, Config, ConfigSerialize, Validate)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Config)]
pub enum LauncherProvider {
	Calculator,
	Applications,
	Files,
	Nix,
}

impl LauncherProvider {
	#[must_use]
	pub fn all() -> Vec<Self> {
		vec![Self::Calculator, Self::Applications, Self::Files]
	}
}

#[derive(Debug, Clone, PartialEq, Config, ConfigSerialize, Validate)]
#[garde(allow_unvalidated)]
pub struct Launcher {
	/// Providers to activate for the launcher. Their results will show up as entries in the
	/// launcher.
	#[config(default = LauncherProvider::all())]
	pub providers: Vec<LauncherProvider>,
	/// Settings for typo-tolerant launcher search.
	#[config(default)]
	#[garde(dive)]
	pub fuzzy_search: LauncherTypoSearch,
}

impl Default for Launcher {
	fn default() -> Self {
		Self {
			providers: LauncherProvider::all(),
			fuzzy_search: LauncherTypoSearch::default(),
		}
	}
}

#[derive(Debug, Clone, PartialEq, Config, ConfigSerialize, Validate)]
#[garde(allow_unvalidated)]
pub struct LauncherTypoSearch {
	/// Minimum query length before typo matching is enabled.
	#[config(default = 3)]
	#[garde(range(min = 0, max = 32))]
	pub min_chars: u32,
	/// Query length where the short-query typo distance applies.
	#[config(default = 4)]
	#[garde(range(min = 0, max = 64))]
	pub short_query_chars: u32,
	/// Maximum edit distance for short typo queries.
	#[config(default = 1)]
	#[garde(range(min = 0, max = 8))]
	pub short_max_distance: u32,
	/// Query length where the medium-query typo distance applies.
	#[config(default = 7)]
	#[garde(range(min = 0, max = 64))]
	pub medium_query_chars: u32,
	/// Maximum edit distance for medium typo queries.
	#[config(default = 2)]
	#[garde(range(min = 0, max = 8))]
	pub medium_max_distance: u32,
	/// Maximum edit distance for long typo queries.
	#[config(default = 3)]
	#[garde(range(min = 0, max = 8))]
	pub long_max_distance: u32,
}

impl Default for LauncherTypoSearch {
	fn default() -> Self {
		Self {
			min_chars: 3,
			short_query_chars: 4,
			short_max_distance: 1,
			medium_query_chars: 7,
			medium_max_distance: 2,
			long_max_distance: 3,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Config, ConfigSerialize, Validate)]
#[garde(allow_unvalidated)]
pub struct Indexing {
	#[config(default = true)]
	pub enabled: bool,
}

impl Default for Indexing {
	fn default() -> Self {
		Self { enabled: true }
	}
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]

	use std::{
		ffi::OsString,
		sync::{Mutex, OnceLock},
	};

	use super::*;

	static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

	struct TestEnvironment {
		root: PathBuf,
		old_config: Option<OsString>,
		old_override: Option<OsString>,
	}

	impl TestEnvironment {
		fn new(name: &str) -> Self {
			let root = std::env::temp_dir().join(format!(
				"subniri-config-{name}-{}-{}",
				std::process::id(),
				std::time::SystemTime::now()
					.duration_since(std::time::UNIX_EPOCH)
					.unwrap_or_default()
					.as_nanos()
			));
			std::fs::create_dir_all(&root).unwrap();
			let config = root.join("config.kdl");
			let config_override = root.join("config.override.kdl");
			let old_config = std::env::var_os("SUBNIRI_CONFIG_FILE");
			let old_override = std::env::var_os(CONFIG_OVERRIDE_ENV);

			// Tests are serialized by ENV_LOCK, so no other thread observes these process globals.
			unsafe {
				std::env::set_var("SUBNIRI_CONFIG_FILE", config);
				std::env::set_var(CONFIG_OVERRIDE_ENV, config_override);
			}

			Self {
				root,
				old_config,
				old_override,
			}
		}
	}

	impl Drop for TestEnvironment {
		fn drop(&mut self) {
			// Tests are serialized by ENV_LOCK, so restoring these process globals is safe.
			unsafe {
				match &self.old_config {
					Some(value) => std::env::set_var("SUBNIRI_CONFIG_FILE", value),
					None => std::env::remove_var("SUBNIRI_CONFIG_FILE"),
				}
				match &self.old_override {
					Some(value) => std::env::set_var(CONFIG_OVERRIDE_ENV, value),
					None => std::env::remove_var(CONFIG_OVERRIDE_ENV),
				}
			}
			let _ = std::fs::remove_dir_all(&self.root);
		}
	}

	#[test]
	fn override_is_created_loaded_and_cleared() {
		let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
		let _environment = TestEnvironment::new("lifecycle");
		let (mut doc, mut config) = ConfigFile::load_declarative().unwrap();

		config.spotify.client_id = "0123456789abcdef0123456789abcdef".to_string();
		config.write(&mut doc).unwrap();

		assert!(ConfigFile::override_active());
		assert_eq!(ConfigFile::load().unwrap().1, config);
		assert_eq!(
			ConfigFile::load_declarative().unwrap().1.spotify.client_id,
			""
		);

		let (_, declarative) = ConfigFile::clear_override().unwrap();
		assert!(!ConfigFile::override_active());
		assert_eq!(ConfigFile::load().unwrap().1, declarative);
	}

	#[test]
	fn matching_config_removes_override() {
		let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
		let _environment = TestEnvironment::new("matching");
		let (mut doc, mut config) = ConfigFile::load_declarative().unwrap();

		config.spotify.enabled = true;
		config.write(&mut doc).unwrap();
		assert!(ConfigFile::override_active());

		let (base_doc, base) = ConfigFile::load_declarative().unwrap();
		doc = base_doc;
		config = base;
		config.write(&mut doc).unwrap();
		assert!(!ConfigFile::override_active());
	}

	#[test]
	fn nix_diff_contains_only_changed_assignments() {
		let base = ConfigFile::default();
		let mut effective = base.clone();
		effective.spotify.enabled = true;
		effective.spotify.client_id = "client\"${id}".to_string();

		assert_eq!(
			effective.nix_diff(&base),
			concat!(
				"services.subniri.settings.spotify.enable = true;\n",
				"services.subniri.settings.spotify.clientId = \"client\\\"\\${id}\";\n",
			)
		);
	}
}
