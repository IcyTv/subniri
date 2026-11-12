#![allow(clippy::missing_errors_doc)]

use std::collections::{HashMap, HashSet};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// This is the main configuration file for the subniri desktop shell.
/// If you want to, you can edit settings from here, and they'll be automatically syncronized to
/// all components in the subniri shell.
/// You can also edit the settings in a graphical interface (`snowconf`), and they will be
/// synchronized to this file.
#[derive(Default, Debug, Clone, PartialEq, ConfigFile, ConfigFileSerialize, Validate)]
#[garde(allow_unvalidated)]
pub struct ConfigFile {
	/// Shared appearance selection for all Subniri interfaces.
	#[config(default)]
	#[garde(dive)]
	pub appearance: Appearance,
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
	/// Pair using snowconf and spotify.com/pair. No developer application, client ID or secret
	/// is required. Spotify Premium is required by the underlying session implementation.
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
	/// Fileberg's context-menu and file-transfer preferences.
	#[config(default)]
	#[garde(dive)]
	pub fileberg: Fileberg,
}

pub const FILEBERG_ACTION_IDS: &[&str] = &[
	"open",
	"open-with",
	"open-window",
	"cut",
	"copy",
	"advanced-copy",
	"advanced-move",
	"advanced-copy-auto",
	"advanced-copy-durable",
	"advanced-copy-buffered",
	"advanced-move-auto",
	"advanced-move-durable",
	"advanced-move-buffered",
	"duplicate",
	"rename",
	"trash",
	"tags",
	"properties",
	"new-folder",
	"paste",
	"sort",
	"view",
	"refresh",
];

#[derive(Default, Debug, Clone, PartialEq, Config, ConfigSerialize, Validate)]
pub struct Fileberg {
	/// Auto buffers only proven fixed local storage; other destinations are flushed.
	#[config(default)]
	#[garde(skip)]
	pub transfer_durability: FilebergDurability,

	/// Stable command IDs in user-defined order, shared across menu contexts.
	#[config(list_style = children)]
	#[garde(custom(validate_fileberg_favorites))]
	pub favorites: Vec<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Config, ConfigSerialize)]
pub enum FilebergDurability {
	#[default]
	Auto,
	Durable,
	Buffered,
}

impl FilebergDurability {
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Auto => "auto",
			Self::Durable => "durable",
			Self::Buffered => "buffered",
		}
	}
}

fn validate_fileberg_favorites(values: &[String], _ctx: &()) -> garde::Result {
	let mut seen = HashSet::new();
	for value in values {
		if !FILEBERG_ACTION_IDS.contains(&value.as_str()) {
			return Err(garde::Error::new(format!(
				"Unknown Fileberg action: {value}"
			)));
		}
		if !seen.insert(value) {
			return Err(garde::Error::new(format!(
				"Duplicate Fileberg favorite: {value}"
			)));
		}
	}
	Ok(())
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

	/// Apply a change to freshly loaded settings while serializing cooperating writers.
	pub fn update(change: impl FnOnce(&mut Self)) -> Result<(KdlDocument, Self), ConfigError> {
		let _lock = Self::lock_write_target()?;
		let (mut doc, mut config) = Self::load()?;
		change(&mut config);
		config.write(&mut doc)?;

		Ok((doc, config))
	}

	fn lock_write_target() -> Result<std::fs::File, ConfigError> {
		let target = Self::override_path().map_or_else(Self::path, Ok)?;
		let target = absolute_path(target)?;
		if let Some(parent) = target.parent() {
			std::fs::create_dir_all(parent)?;
		}
		let mut lock_path = target.into_os_string();
		lock_path.push(".lock");
		let lock = std::fs::OpenOptions::new()
			.create(true)
			.truncate(false)
			.read(true)
			.write(true)
			.open(PathBuf::from(lock_path))?;
		lock.lock()?;

		// Keep the lock file in place: unlinking it would let writers lock different inodes.
		Ok(lock)
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

				atomic_write(file, doc.to_string().as_bytes())?;

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
		let _lock = Self::lock_write_target()?;
		let declarative = Self::load_declarative()?;
		if let Some(path) = Self::override_path()
			&& path.exists()
		{
			std::fs::remove_file(&path)?;
			record_process_write(&path, String::new())?;
		}

		Ok(declarative)
	}

	#[must_use]
	#[allow(clippy::too_many_lines)]
	pub fn nix_diff(&self, declarative: &Self) -> String {
		let mut out = String::new();

		nix_assignment(
			&mut out,
			"services.subniri.settings.appearance.theme",
			self.appearance.theme,
			declarative.appearance.theme,
			|mode| format!("\"{}\"", mode.as_str()),
		);

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
			"services.subniri.settings.spotify.pollIntervalSeconds",
			self.spotify.poll_interval_seconds,
			declarative.spotify.poll_interval_seconds,
			|value| value.to_string(),
		);

		if self.system_menu.effective_rows() != declarative.system_menu.effective_rows() {
			out.push_str("services.subniri.settings.systemMenu.widgets = [];\n");
			out.push_str("services.subniri.settings.systemMenu.rows = [\n");
			for row in self.system_menu.effective_rows() {
				out.push_str("  [ ");
				for entry in row.0 {
					match entry {
						MenuEntry::Gap(width) => out.push_str(&format!("{{ gap = {width}; }} ")),
						MenuEntry::Widget(tile) => out.push_str(&format!(
							"{{ widget = {}; width = {}; height = {}; }} ",
							nix_string(system_menu_widget_name(&tile.widget)),
							tile.width,
							tile.height
						)),
					}
				}
				out.push_str("]\n");
			}
			out.push_str("];\n");
		}
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

		nix_assignment(
			&mut out,
			"services.subniri.settings.fileberg.transfer_durability",
			self.fileberg.transfer_durability,
			declarative.fileberg.transfer_durability,
			|value| nix_string(value.as_str()),
		);
		nix_assignment(
			&mut out,
			"services.subniri.settings.fileberg.favorites",
			&self.fileberg.favorites,
			&declarative.fileberg.favorites,
			|values| nix_list(values.iter().map(String::as_str)),
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

		// TODO: Should we format?
		doc.autoformat();

		let contents = doc.to_string();
		atomic_write(file, contents.as_bytes())?;
		record_process_write(file, contents)?;

		Ok(())
	}
}

fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
	let target = resolve_write_target(path)?;
	let permissions = match std::fs::metadata(&target) {
		Ok(metadata) => {
			// Rename alone only checks the directory permissions. Check the actual target too,
			// so atomic replacement cannot bypass a declarative file's read-only permissions.
			std::fs::OpenOptions::new().write(true).open(&target)?;
			Some(metadata.permissions())
		}
		Err(error) if error.kind() == io::ErrorKind::NotFound => None,
		Err(error) => return Err(error),
	};
	let parent = target
		.parent()
		.ok_or_else(|| io::Error::other("config path has no parent"))?;
	std::fs::create_dir_all(parent)?;
	let name = target
		.file_name()
		.ok_or_else(|| io::Error::other("config path has no filename"))?;
	let (temp_path, mut temp) = loop {
		let mut temp_name = name.to_os_string();
		temp_name.push(format!(
			".{}.{}.tmp",
			std::process::id(),
			TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
		));
		let temp_path = parent.join(temp_name);
		match std::fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.open(&temp_path)
		{
			Ok(temp) => break (temp_path, temp),
			Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
			Err(error) => return Err(error),
		}
	};

	let result = (|| {
		temp.write_all(contents)?;
		temp.flush()?;
		if let Some(permissions) = permissions {
			temp.set_permissions(permissions)?;
		}
		temp.sync_all()?;
		std::fs::rename(&temp_path, &target)
	})();
	if result.is_err() {
		// create_new succeeded, so this is exclusively our temporary file.
		let _ = std::fs::remove_file(&temp_path);
	}

	result
}

fn resolve_write_target(path: &Path) -> io::Result<PathBuf> {
	let mut target = absolute_path(path)?;
	for _ in 0..40 {
		match std::fs::symlink_metadata(&target) {
			Ok(metadata) if metadata.file_type().is_symlink() => {
				let link = std::fs::read_link(&target)?;
				target = if link.is_absolute() {
					link
				} else {
					target
						.parent()
						.ok_or_else(|| io::Error::other("symlink has no parent"))?
						.join(link)
				};
			}
			Ok(_) => return Ok(target),
			Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(target),
			Err(error) => return Err(error),
		}
	}

	Err(io::Error::other("too many config symlinks"))
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

fn system_menu_widget_name<'a>(widget: &'a SystemMenuWidgets) -> &'a str {
	match widget {
		SystemMenuWidgets::Wifi => "wifi",
		SystemMenuWidgets::Bluetooth => "bluetooth",
		SystemMenuWidgets::Speaker => "speaker",
		SystemMenuWidgets::Microphone => "microphone",
		SystemMenuWidgets::Vpn => "vpn",
		SystemMenuWidgets::Nightlight => "nightlight",
		SystemMenuWidgets::DarkMode => "dark-mode",
		SystemMenuWidgets::Homeassistant(entity_id) => entity_id,
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
	/// control Home Assistant entities from subniri.
	pub enabled: bool,
	/// The url of your homeassistant instance.
	/// Format "<http://homeassistant.local:8123>"
	pub url: Option<url::Url>,
	/// A list of entity IDs whose updates should remain active without a D-Bus observer. The legacy
	/// `tracked_devices` key is retained so existing configuration files continue to load.
	#[config(list_style = children)]
	pub tracked_devices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Config, ConfigSerialize, Validate)]
pub struct Spotify {
	/// Whether to enable the spotify integration or not.
	#[garde(skip)]
	pub enabled: bool,
	/// Deprecated: accepted for old configurations but ignored. Pair through snowconf instead.
	/// This compatibility-only field is no longer generated or exported.
	#[config(default, deprecated)]
	#[garde(skip)]
	pub client_id: String,
	/// Recovery refresh interval when push is disconnected, in seconds. Saved tracks normally
	/// update through WebSocket events; healthy connections reconcile at most every 15 minutes.
	/// Legacy values below 60 are accepted but clamped to 60 at runtime, with a warning.
	#[config(default = 60)]
	#[garde(range(min = 5, max = 3600))]
	pub poll_interval_seconds: u64,
}

impl Default for Spotify {
	fn default() -> Self {
		Self {
			enabled: false,
			client_id: String::new(),
			poll_interval_seconds: 60,
		}
	}
}

#[derive(Debug, Default, Clone, PartialEq, Config, ConfigSerialize, Validate)]
pub struct Appearance {
	#[config(default)]
	#[garde(skip)]
	pub theme: AppearanceMode,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Config)]
pub enum AppearanceMode {
	#[default]
	Light,
	Dark,
}

impl AppearanceMode {
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Light => "light",
			Self::Dark => "dark",
		}
	}

	pub fn from_name(name: &str) -> Option<Self> {
		match name {
			"light" => Some(Self::Light),
			"dark" => Some(Self::Dark),
			_ => None,
		}
	}
}

#[derive(Debug, PartialEq, Eq, Clone, Config, strum::EnumDiscriminants)]
#[strum_discriminants(derive(strum::VariantArray))]
pub enum SystemMenuWidgets {
	Wifi,
	Bluetooth,
	Speaker,
	Microphone,
	Vpn,
	Nightlight,
	DarkMode,
	#[config(other)]
	Homeassistant(String),
}

impl SystemMenuWidgets {
	pub const VARIANTS: &'static [Self] = &[
		Self::Wifi,
		Self::Bluetooth,
		Self::Speaker,
		Self::Microphone,
		Self::Vpn,
		Self::Nightlight,
		Self::DarkMode,
	];
}

mod system_menu;
pub use system_menu::{
	MenuEntry, MenuRow, MenuTile, PlacedTile, ResolvedMenu, resolve_rows, rows_from_tiles,
};

#[derive(Debug, Default, Clone, Eq, Validate)]
#[garde(allow_unvalidated)]
pub struct SystemMenu {
	/// Legacy ordered list. Migrated into rows of two medium tiles when edited.
	pub widgets: Vec<SystemMenuWidgets>,
	pub rows: Vec<MenuRow>,
}

impl PartialEq for SystemMenu {
	fn eq(&self, other: &Self) -> bool {
		self.effective_rows() == other.effective_rows()
	}
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

	#[test]
	fn appearance_defaults_and_round_trips() {
		let (_, defaults) = ConfigFile::parse_validated("").unwrap();
		assert_eq!(defaults.appearance.theme, AppearanceMode::Light);

		let (_, dark) = ConfigFile::parse_validated("appearance { theme \"dark\"; }").unwrap();
		let mut doc = KdlDocument::new();
		dark.apply_to_kdl_document(&mut doc).unwrap();
		let (_, restored) = ConfigFile::parse_validated(&doc.to_string()).unwrap();

		assert_eq!(restored.appearance.theme, AppearanceMode::Dark);
		assert!(
			dark.nix_diff(&defaults)
				.contains("appearance.theme = \"dark\";")
		);
		assert!(ConfigFile::parse_validated("appearance { theme \"unknown\"; }").is_err());
	}

	#[test]
	fn dark_mode_tile_round_trips_without_changing_default_layout() {
		let (_, defaults) = ConfigFile::parse_validated("").unwrap();

		assert!(defaults.system_menu.widgets.is_empty());
		assert!(defaults.system_menu.rows.is_empty());
		assert!(SystemMenuWidgets::VARIANTS.contains(&SystemMenuWidgets::DarkMode));

		let (_, configured) = ConfigFile::parse_validated(
			"system_menu { row { widget \"dark-mode\" width=2 height=1; }; }",
		)
		.unwrap();
		let mut doc = KdlDocument::new();
		configured.apply_to_kdl_document(&mut doc).unwrap();
		let (_, restored) = ConfigFile::parse_validated(&doc.to_string()).unwrap();

		assert_eq!(restored.system_menu, configured.system_menu);
		assert!(configured.nix_diff(&defaults).contains("dark-mode"));
	}

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
	fn update_preserves_fresh_fields_and_comments() {
		let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
		let _environment = TestEnvironment::new("transaction");
		let base = ConfigFile::path().unwrap();
		std::fs::write(&base, "// keep this comment\nspotify { enabled; }\n").unwrap();

		ConfigFile::update(|config| config.nightlight.day.temperature = 5300).unwrap();
		let (doc, config) = ConfigFile::update(|config| {
			config.fileberg.favorites = vec!["copy".into()];
		})
		.unwrap();

		assert!(config.spotify.enabled);
		assert_eq!(config.nightlight.day.temperature, 5300);
		assert_eq!(config.fileberg.favorites, ["copy"]);
		assert!(doc.to_string().contains("// keep this comment"));
		assert_eq!(ConfigFile::load().unwrap().1, config);
		assert!(
			std::fs::read_to_string(base)
				.unwrap()
				.contains("// keep this comment")
		);
	}

	#[test]
	fn update_without_override_creates_parent_and_base_lock() {
		let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
		let environment = TestEnvironment::new("base-transaction");
		let path = environment.root.join("nested/config.kdl");
		// ENV_LOCK excludes other tests while these process globals are changed.
		unsafe {
			std::env::remove_var(CONFIG_OVERRIDE_ENV);
			std::env::set_var("SUBNIRI_CONFIG_FILE", &path);
		}

		let (_, config) = ConfigFile::update(|config| config.spotify.enabled = true).unwrap();

		assert!(config.spotify.enabled);
		assert_eq!(ConfigFile::load().unwrap().1, config);
		assert!(path.with_file_name("config.kdl.lock").exists());
	}

	#[test]
	fn update_serializes_separate_lock_handles() {
		let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
		let _environment = TestEnvironment::new("serialization");
		let (entered_tx, entered_rx) = std::sync::mpsc::channel();
		let (release_tx, release_rx) = std::sync::mpsc::channel();
		let (second_tx, second_rx) = std::sync::mpsc::channel();

		std::thread::scope(|scope| {
			let first = scope.spawn(move || {
				ConfigFile::update(|config| {
					entered_tx.send(()).unwrap();
					release_rx.recv().unwrap();
					config.spotify.enabled = true;
				})
				.unwrap();
			});
			entered_rx.recv().unwrap();
			let second = scope.spawn(move || {
				ConfigFile::update(|config| {
					second_tx.send(()).unwrap();
					config.fileberg.favorites = vec!["copy".into()];
				})
				.unwrap();
			});

			let blocked = second_rx.recv_timeout(Duration::from_millis(100)).is_err();
			release_tx.send(()).unwrap();
			first.join().unwrap();
			second.join().unwrap();
			assert!(blocked);
		});

		let (_, config) = ConfigFile::load().unwrap();
		assert!(config.spotify.enabled);
		assert_eq!(config.fileberg.favorites, ["copy"]);
	}

	#[test]
	fn atomic_writes_keep_readers_on_complete_documents() {
		let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
		let environment = TestEnvironment::new("atomic-readers");
		let path = environment.root.join("atomic.kdl");
		let mut config = ConfigFile::default();
		let mut doc = KdlDocument::new();
		config.write_to_file(&mut doc, &path).unwrap();
		let done = std::sync::atomic::AtomicBool::new(false);
		let ready = std::sync::Barrier::new(2);

		std::thread::scope(|scope| {
			let reader = scope.spawn(|| {
				ConfigFile::parse_validated(&std::fs::read_to_string(&path).unwrap()).unwrap();
				ready.wait();
				while !done.load(Ordering::Acquire) {
					let contents = std::fs::read_to_string(&path).unwrap();
					assert!(!contents.is_empty());
					ConfigFile::parse_validated(&contents).unwrap();
				}
			});
			ready.wait();
			for index in 0..30 {
				config.spotify.enabled = index % 2 == 0;
				config.write_to_file(&mut doc, &path).unwrap();
			}
			done.store(true, Ordering::Release);
			reader.join().unwrap();
		});
	}

	#[test]
	fn clear_override_preserves_override_when_declarative_is_invalid() {
		let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
		let _environment = TestEnvironment::new("clear-invalid");
		ConfigFile::update(|config| config.spotify.enabled = true).unwrap();
		let path = ConfigFile::override_path().unwrap();
		let contents = std::fs::read_to_string(&path).unwrap();
		std::fs::write(ConfigFile::path().unwrap(), "invalid {").unwrap();

		assert!(ConfigFile::clear_override().is_err());
		assert_eq!(std::fs::read_to_string(path).unwrap(), contents);
	}

	#[cfg(unix)]
	#[test]
	fn atomic_writes_preserve_symlinks_permissions_and_readonly_targets() {
		use std::os::unix::fs::{PermissionsExt as _, symlink};

		let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
		let environment = TestEnvironment::new("symlink");
		let target = environment.root.join("target.kdl");
		let link = environment.root.join("link.kdl");
		std::fs::write(&target, "// original\n").unwrap();
		std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
		symlink("target.kdl", &link).unwrap();
		let mut doc = KdlDocument::new();

		ConfigFile::default()
			.write_to_file(&mut doc, &link)
			.unwrap();

		assert!(
			std::fs::symlink_metadata(&link)
				.unwrap()
				.file_type()
				.is_symlink()
		);
		assert_eq!(
			std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
			0o640
		);
		assert_eq!(
			ConfigFile::load_from_file(&target).unwrap().1,
			ConfigFile::default()
		);
		std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o440)).unwrap();
		let contents = std::fs::read_to_string(&target).unwrap();

		assert!(
			ConfigFile::default()
				.write_to_file(&mut doc, &link)
				.is_err()
		);
		assert_eq!(std::fs::read_to_string(&target).unwrap(), contents);
		assert!(
			std::fs::symlink_metadata(&link)
				.unwrap()
				.file_type()
				.is_symlink()
		);
	}

	#[test]
	fn fileberg_favorites_roundtrip_preserves_settings_and_override_semantics() {
		let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
		let _environment = TestEnvironment::new("fileberg");
		let (mut doc, mut config) = ConfigFile::load_declarative().unwrap();
		config.spotify.enabled = true;
		config.nightlight.day.temperature = 5300;
		config.fileberg.favorites = vec!["copy".into(), "new-folder".into(), "open".into()];
		config.write(&mut doc).unwrap();
		let (_, loaded) = ConfigFile::load().unwrap();
		assert_eq!(loaded.fileberg.favorites, config.fileberg.favorites);
		assert!(loaded.spotify.enabled);
		assert_eq!(loaded.nightlight.day.temperature, 5300);
		let (_, base) = ConfigFile::load_declarative().unwrap();
		assert!(base.fileberg.favorites.is_empty());
		assert!(
			loaded
				.nix_diff(&base)
				.contains("settings.fileberg.favorites = [\"copy\" \"new-folder\" \"open\"];")
		);
	}

	#[test]
	fn fileberg_favorites_validate_unique_known_command_ids() {
		let mut settings = Fileberg {
			favorites: vec!["open".into(), "open".into()],
			..Fileberg::default()
		};
		assert!(settings.validate().is_err());
		settings.favorites = vec!["not-an-action".into()];
		assert!(settings.validate().is_err());
		settings.favorites = vec![
			"open".into(),
			"tags".into(),
			"advanced-copy".into(),
			"advanced-move".into(),
		];
		assert!(settings.validate().is_ok());
	}

	#[test]
	fn fileberg_policy_leaf_favorites_validate_and_round_trip() {
		let ids = [
			"advanced-copy-auto",
			"advanced-copy-durable",
			"advanced-copy-buffered",
			"advanced-move-auto",
			"advanced-move-durable",
			"advanced-move-buffered",
		];
		let favorites = ids
			.iter()
			.map(|id| format!("\"{id}\""))
			.collect::<Vec<_>>()
			.join(" ");
		let input = format!("fileberg {{ favorites {favorites}; }}");
		let (_, config) = ConfigFile::parse_validated(&input).unwrap();
		let mut doc = KdlDocument::new();

		config.apply_to_kdl_document(&mut doc).unwrap();
		let (_, restored) = ConfigFile::parse_validated(&doc.to_string()).unwrap();

		assert_eq!(restored.fileberg.favorites, ids);
		assert!(
			ConfigFile::parse_validated(
				"fileberg { favorites \"advanced-copy-auto\" \"advanced-copy-auto\"; }"
			)
			.is_err()
		);
	}

	#[test]
	fn fileberg_durability_defaults_to_auto() {
		assert_eq!(
			Fileberg::default().transfer_durability,
			FilebergDurability::Auto
		);

		for input in ["", "fileberg {}", "fileberg { favorites \"copy\"; }"] {
			let (_, config) = ConfigFile::parse_validated(input).unwrap();

			assert_eq!(
				config.fileberg.transfer_durability,
				FilebergDurability::Auto
			);
		}
	}

	#[test]
	fn fileberg_durability_round_trips_and_exports_policy_changes() {
		let defaults = ConfigFile::default();

		for policy in [
			FilebergDurability::Auto,
			FilebergDurability::Durable,
			FilebergDurability::Buffered,
		] {
			let input = format!(
				"fileberg {{ transfer_durability \"{}\"; favorites \"advanced-copy\" \"advanced-move\"; }}",
				policy.as_str()
			);
			let (_, config) = ConfigFile::parse_validated(&input).unwrap();
			let mut doc = KdlDocument::new();

			config.apply_to_kdl_document(&mut doc).unwrap();
			let (_, restored) = ConfigFile::parse_validated(&doc.to_string()).unwrap();

			assert_eq!(restored.fileberg, config.fileberg);
			assert_eq!(restored.fileberg.transfer_durability, policy);
			assert!(!config.nix_diff(&config).contains("settings.fileberg"));

			let export = config.nix_diff(&defaults);
			assert!(export.contains(
				"services.subniri.settings.fileberg.favorites = [\"advanced-copy\" \"advanced-move\"];"
			));
			if policy == FilebergDurability::Auto {
				assert!(!export.contains("transfer_durability"));
			} else {
				assert!(export.contains(&format!(
					"services.subniri.settings.fileberg.transfer_durability = \"{}\";",
					policy.as_str()
				)));
				assert!(defaults.nix_diff(&config).contains(
					"services.subniri.settings.fileberg.transfer_durability = \"auto\";"
				));
			}
		}
	}

	#[test]
	fn fileberg_durability_rejects_invalid_policy() {
		for input in [
			"fileberg { transfer_durability \"unknown\"; }",
			"fileberg { transfer_durability 1; }",
		] {
			assert!(ConfigFile::parse_validated(input).is_err());
		}
	}

	#[test]
	fn fileberg_durability_preserves_base_and_override_semantics() {
		let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
		let _environment = TestEnvironment::new("fileberg-durability");
		let base_path = ConfigFile::path().unwrap();
		let base_contents = "fileberg { transfer_durability \"durable\"; }\n";
		std::fs::write(&base_path, base_contents).unwrap();

		assert!(!ConfigFile::override_active());
		assert_eq!(
			ConfigFile::load().unwrap().1.fileberg.transfer_durability,
			FilebergDurability::Durable
		);

		ConfigFile::update(|config| {
			config.fileberg.transfer_durability = FilebergDurability::Buffered;
		})
		.unwrap();
		let (_, loaded) = ConfigFile::load().unwrap();
		let (_, base) = ConfigFile::load_declarative().unwrap();

		assert!(ConfigFile::override_active());
		assert_eq!(
			loaded.fileberg.transfer_durability,
			FilebergDurability::Buffered
		);
		assert_eq!(
			base.fileberg.transfer_durability,
			FilebergDurability::Durable
		);
		assert_eq!(std::fs::read_to_string(&base_path).unwrap(), base_contents);
		assert!(
			loaded
				.nix_diff(&base)
				.contains("services.subniri.settings.fileberg.transfer_durability = \"buffered\";")
		);

		ConfigFile::clear_override().unwrap();

		assert!(!ConfigFile::override_active());
		assert_eq!(ConfigFile::load().unwrap().1, base);
	}

	#[test]
	fn override_is_created_loaded_and_cleared() {
		let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
		let _environment = TestEnvironment::new("lifecycle");
		let (mut doc, mut config) = ConfigFile::load_declarative().unwrap();
		assert_eq!(config.spotify.poll_interval_seconds, 60);

		config.spotify.enabled = true;
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
	fn spotify_poll_interval_defaults_and_validates() {
		let mut spotify = Spotify::default();
		assert_eq!(spotify.poll_interval_seconds, 60);
		assert!(spotify.validate().is_ok());

		spotify.poll_interval_seconds = 4;
		assert!(spotify.validate().is_err());
		spotify.poll_interval_seconds = 5;
		assert!(spotify.validate().is_ok());
		spotify.poll_interval_seconds = 3600;
		assert!(spotify.validate().is_ok());
		spotify.poll_interval_seconds = 3601;
		assert!(spotify.validate().is_err());
	}

	#[test]
	fn legacy_spotify_client_id_is_read_but_never_generated_or_exported() {
		use config_traits::{Config as _, ConfigSerialize as _};
		let doc: KdlDocument = "spotify {\n client_id \"old-client\"\n enabled\n}\n"
			.parse()
			.unwrap();
		let spotify = Spotify::from_kdl_node(doc.get("spotify").unwrap()).unwrap();
		assert_eq!(spotify.client_id, "old-client");
		assert!(spotify.enabled);
		let mut node = kdl::KdlNode::new("spotify");
		spotify.apply_to_kdl_node(&mut node).unwrap();
		assert!(!node.to_string().contains("client_id"));
		let mut config = ConfigFile::default();
		config.spotify = spotify;
		assert!(!config.nix_diff(&ConfigFile::default()).contains("clientId"));
	}

	#[test]
	fn nix_diff_contains_only_changed_assignments() {
		let base = ConfigFile::default();
		let mut effective = base.clone();
		effective.spotify.enabled = true;
		effective.spotify.client_id = "client\"${id}".to_string();
		effective.spotify.poll_interval_seconds = 30;

		assert_eq!(
			effective.nix_diff(&base),
			concat!(
				"services.subniri.settings.spotify.enable = true;\n",
				"services.subniri.settings.spotify.pollIntervalSeconds = 30;\n",
			)
		);
	}
}
