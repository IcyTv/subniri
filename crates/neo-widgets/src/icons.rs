#![allow(dead_code)]

#[macro_export]
macro_rules! phosphor_icon {
	($name:literal,$variant:literal) => {
		iced::widget::svg::Handle::from_memory(include_bytes!(concat!(
			env!("PHOSPHOR_ICONS"),
			"/",
			$variant,
			"/",
			$name,
			"-",
			$variant,
			".svg"
		)))
	};

	($name:literal) => {
		phosphor_icon!($name, "bold")
	};
}

use std::{
	collections::{HashMap, HashSet},
	path::{Path, PathBuf},
	sync::{Arc, Mutex, OnceLock},
};

use freedesktop_desktop_entry::{
	DesktopEntry, Iter, Locale, default_paths, find_app_by_id, unicase::Ascii,
};
use iced::{
	Element,
	widget::{image, svg},
};
use icon::{FileType, IconFile, IconSearch, Icons};

pub mod background;

static ICON_RESOLVER: OnceLock<Arc<Mutex<ApplicationIconResolver>>> = OnceLock::new();

fn icon_resolver() -> &'static Arc<Mutex<ApplicationIconResolver>> {
	ICON_RESOLVER.get_or_init(|| Arc::new(Mutex::new(ApplicationIconResolver::new())))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedIcon {
	Svg(svg::Handle),
	Image(image::Handle),
}

impl ResolvedIcon {
	#[allow(clippy::needless_pass_by_value)]
	#[must_use]
	pub fn from_icon_file(icon: IconFile) -> Self {
		match icon.file_type() {
			FileType::Svg => Self::Svg(svg::Handle::from_path(icon.path())),
			FileType::Png => Self::Image(image::Handle::from_path(icon.path())),
			FileType::Xpm => default_icon(),
		}
	}

	pub fn into_element<Message>(self) -> Element<'static, Message>
	where
		Message: 'static,
	{
		match self {
			Self::Svg(handle) => svg(handle).into(),
			Self::Image(handle) => image(handle).into(),
		}
	}
}

#[must_use]
pub fn default_icon() -> ResolvedIcon {
	ResolvedIcon::Svg(phosphor_icon!("question", "bold"))
}

pub fn resolve_icon(app_id: impl AsRef<str>, size: u32, scale: u32) -> ResolvedIcon {
	resolve_with(|resolver| resolver.get_icon_by_app_id(app_id, size, scale))
}

pub fn resolve_icon_name(icon_name: impl AsRef<str>, size: u32, scale: u32) -> ResolvedIcon {
	try_resolve_icon_name(icon_name, size, scale).unwrap_or_else(default_icon)
}

pub fn resolve_icon_name_in_path(
	icon_name: impl AsRef<str>, icon_path: impl AsRef<Path>, size: u32, scale: u32,
) -> ResolvedIcon {
	try_resolve_icon_name_in_path(icon_name, icon_path, size, scale).unwrap_or_else(default_icon)
}

pub fn try_resolve_icon_name(
	icon_name: impl AsRef<str>, size: u32, scale: u32,
) -> Option<ResolvedIcon> {
	let resolver = icon_resolver();
	let Ok(mut resolver) = resolver.lock() else {
		return None;
	};
	resolver
		.get_icon_by_name(icon_name.as_ref(), size, scale)
		.map(ResolvedIcon::from_icon_file)
}

pub fn try_resolve_icon_name_in_path(
	icon_name: impl AsRef<str>, icon_path: impl AsRef<Path>, size: u32, scale: u32,
) -> Option<ResolvedIcon> {
	let resolver = icon_resolver();
	let Ok(mut resolver) = resolver.lock() else {
		return None;
	};
	resolver.add_icon_dir(icon_path);
	resolver
		.get_icon_by_name(icon_name.as_ref(), size, scale)
		.map(ResolvedIcon::from_icon_file)
}

pub fn resolve_from_desktop_entry(
	desktop_entry: impl AsRef<str>, size: u32, scale: u32,
) -> ResolvedIcon {
	resolve_with(|resolver| resolver.get_icon_by_desktop_entry(desktop_entry, size, scale))
}

#[must_use]
pub fn resolve_from_window(window: &niri_ipc::Window, size: u32, scale: u32) -> ResolvedIcon {
	resolve_with(|resolver| resolver.get_icon_by_window(window, size, scale))
}

fn resolve_with(
	lookup: impl FnOnce(&mut ApplicationIconResolver) -> Option<IconFile>,
) -> ResolvedIcon {
	let resolver = icon_resolver();
	let Ok(mut resolver) = resolver.lock() else {
		return default_icon();
	};

	lookup(&mut resolver).map_or_else(default_icon, ResolvedIcon::from_icon_file)
}

struct ApplicationIconResolver {
	additional_icon_dirs: Vec<PathBuf>,
	application_dirs: Vec<PathBuf>,

	desktop_entries: Vec<DesktopEntry>,
	icons: Icons,

	cache: HashMap<IconLookupKey, IconLookupResult>,
	generation: u64,

	known_roots: HashSet<PathBuf>,
}

impl ApplicationIconResolver {
	fn get_icon_by_name(&mut self, name: &str, size: u32, scale: u32) -> Option<IconFile> {
		let lookup = IconLookupKey {
			kind: IconLookupKind::Name,
			name: name.to_string(),
			generation: self.generation,
			size,
			scale,
		};
		if let Some(icon) = self.cache.get(&lookup) {
			return icon.clone().into_opt();
		}
		// Missing tray names otherwise scan theme paths on every view rebuild.
		// Keep misses as well as hits until the search-path generation changes.
		let icon = self.find_icon(name, size, scale);
		self.cache.insert(lookup, icon.clone().into());
		icon
	}

	pub fn new() -> Self {
		let application_dirs = default_paths().collect::<Vec<_>>();
		let desktop_entries = Iter::new(application_dirs.clone().into_iter())
			.entries::<Locale>(None)
			.collect::<Vec<_>>();

		let icons = IconSearch::new().search().icons();

		Self {
			additional_icon_dirs: vec![],
			application_dirs,
			desktop_entries,
			icons,
			cache: HashMap::new(),
			generation: 0,
			known_roots: HashSet::new(),
		}
	}

	pub fn get_icon_by_app_id(
		&mut self, app_id: impl AsRef<str>, size: u32, scale: u32,
	) -> Option<IconFile> {
		let lookup = IconLookupKey {
			kind: IconLookupKind::AppId,
			name: app_id.as_ref().to_string(),
			generation: self.generation,
			size,
			scale,
		};

		if let Some(icon) = self.cache.get(&lookup) {
			return icon.clone().into_opt();
		}

		let icon = find_app_by_id(&self.desktop_entries, Ascii::new(app_id.as_ref()))
			.and_then(|desktop_entry| desktop_entry.icon())
			.and_then(|icon_name| self.find_icon(icon_name, size, scale));

		self.cache.insert(lookup, icon.clone().into());
		icon
	}

	pub fn get_icon_by_desktop_entry(
		&mut self, desktop_entry: impl AsRef<str>, size: u32, scale: u32,
	) -> Option<IconFile> {
		log::trace!(
			"Getting icon for {} at {}x{}",
			desktop_entry.as_ref(),
			size,
			scale
		);

		let lookup = IconLookupKey {
			kind: IconLookupKind::DesktopEntry,
			name: desktop_entry.as_ref().to_string(),
			generation: self.generation,
			size,
			scale,
		};

		if let Some(icon) = self.cache.get(&lookup) {
			return icon.clone().into_opt();
		}

		let id = desktop_entry
			.as_ref()
			.strip_suffix(".desktop")
			.unwrap_or(desktop_entry.as_ref());
		let id = Ascii::new(id);

		let icon = self
			.desktop_entries
			.iter()
			.find(|de| de.matches_id(id))
			.and_then(|desktop_entry| desktop_entry.icon())
			.and_then(|icon_name| self.find_icon(icon_name, size, scale));

		self.cache.insert(lookup, icon.clone().into());
		icon
	}

	fn get_icon_by_window(
		&mut self, window: &niri_ipc::Window, size: u32, scale: u32,
	) -> Option<IconFile> {
		let app_id = window.app_id.as_ref()?;
		self.get_icon_by_window_identity(app_id, window.pid, size, scale)
	}

	fn get_icon_by_window_identity(
		&mut self, app_id: &str, pid: Option<i32>, size: u32, scale: u32,
	) -> Option<IconFile> {
		if let Some(icon) = self.get_icon_by_app_id(app_id, size, scale) {
			return Some(icon);
		}

		let pid = pid?;

		let exe_path = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
		let root = nix_output_root_for_path(&exe_path)?;
		self.ensure_output_root(root);

		self.get_icon_by_app_id(app_id, size, scale)
	}

	fn ensure_output_root(&mut self, root: impl AsRef<Path>) {
		let root = root.as_ref();

		if !self.known_roots.insert(root.to_path_buf()) {
			return;
		}

		let icon_dirs = [root.join("share/icons"), root.join("share/pixmaps")];
		let icon_dirs_exist = icon_dirs.iter().any(|d| d.is_dir());

		let app_dirs = [root.join("share/applications")];
		let app_dirs_exist = app_dirs.iter().any(|d| d.is_dir());

		if app_dirs_exist {
			self.application_dirs
				.extend(app_dirs.into_iter().filter(|p| p.is_dir()));
			self.rebuild_desktop_entries();
		}

		if icon_dirs_exist {
			self.additional_icon_dirs
				.extend(icon_dirs.into_iter().filter(|p| p.is_dir()));
			self.rebuild_icons_cache();
		}

		if icon_dirs_exist || app_dirs_exist {
			self.generation = self.generation.wrapping_add(1);
			self.sweep();
		}
	}

	fn add_icon_dir(&mut self, path: impl AsRef<Path>) {
		let path = path.as_ref();
		if !path.is_dir() || self.additional_icon_dirs.iter().any(|dir| dir == path) {
			return;
		}

		self.additional_icon_dirs.push(path.to_path_buf());
		self.rebuild_icons_cache();
		self.generation = self.generation.wrapping_add(1);
		self.sweep();
	}

	fn find_icon(&self, icon_name: &str, size: u32, scale: u32) -> Option<IconFile> {
		let icon =
			path_icon(icon_name).or_else(|| self.icons.find_default_icon(icon_name, size, scale));
		let icon = icon.filter(is_supported_icon);

		log::trace!("Got icon: {icon:?}");
		icon
	}

	fn rebuild_desktop_entries(&mut self) {
		self.desktop_entries = Iter::new(self.application_dirs.clone().into_iter())
			.entries::<Locale>(None)
			.collect();
	}

	fn rebuild_icons_cache(&mut self) {
		self.icons = IconSearch::new()
			.add_directories(&self.additional_icon_dirs)
			.search()
			.icons();
	}

	fn sweep(&mut self) {
		let generation = self.generation;
		self.cache.retain(|key, _| key.generation == generation);
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct IconLookupKey {
	kind: IconLookupKind,
	name: String,
	size: u32,
	scale: u32,
	generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum IconLookupKind {
	Name,
	AppId,
	DesktopEntry,
}

#[derive(Clone)]
enum IconLookupResult {
	Found(IconFile),
	Missing,
}

impl IconLookupResult {
	fn into_opt(self) -> Option<IconFile> {
		match self {
			Self::Found(icon) => Some(icon),
			Self::Missing => None,
		}
	}
}

impl From<Option<IconFile>> for IconLookupResult {
	fn from(value: Option<IconFile>) -> Self {
		match value {
			Some(icon) => Self::Found(icon),
			None => Self::Missing,
		}
	}
}

fn path_icon(icon_name: &str) -> Option<IconFile> {
	let path = Path::new(icon_name);

	if path.is_absolute() || icon_name.contains('/') {
		IconFile::from_path(path).filter(|icon| icon.path().is_file())
	} else {
		None
	}
}

fn is_supported_icon(icon: &IconFile) -> bool {
	matches!(icon.file_type(), FileType::Png | FileType::Svg)
}

fn nix_store_dir() -> &'static Path {
	static STORE_PATH: OnceLock<PathBuf> = OnceLock::new();

	STORE_PATH.get_or_init(|| {
		std::env::var_os("NIX_STORE_DIR").map_or_else(|| PathBuf::from("/nix/store"), PathBuf::from)
	})
}

fn nix_output_root_for_path(path: &Path) -> Option<PathBuf> {
	let store = nix_store_dir();
	let rel = path.strip_prefix(store).ok()?;

	let mut components = rel.components();
	let output_name = components.next()?.as_os_str();

	Some(store.join(output_name))
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::{
		fs,
		time::{SystemTime, UNIX_EPOCH},
	};

	fn fixture() -> (ApplicationIconResolver, PathBuf) {
		let root = PathBuf::from("/tmp/opencode").join(format!(
			"tray-icon-cache-{}-{}",
			std::process::id(),
			SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap()
				.as_nanos()
		));
		fs::create_dir_all(root.join("hicolor/24x24/apps")).unwrap();
		fs::write(root.join("hicolor/index.theme"),
			"[Icon Theme]\nName=Fixture\nDirectories=24x24/apps\n\n[24x24/apps]\nSize=24\nType=Fixed\nContext=Applications\n").unwrap();
		let resolver = ApplicationIconResolver {
			additional_icon_dirs: vec![root.clone()],
			application_dirs: vec![],
			desktop_entries: vec![],
			icons: IconSearch::new_from(vec![root.clone()]).search().icons(),
			cache: HashMap::new(),
			generation: 0,
			known_roots: HashSet::new(),
		};
		(resolver, root)
	}

	#[test]
	fn named_icon_misses_are_cached_until_theme_invalidation() {
		let (mut resolver, root) = fixture();
		assert!(resolver.get_icon_by_name("sonarr-missing", 24, 1).is_none());
		assert_eq!(
			resolver.cache.len(),
			1,
			"missing tray names must be cached too"
		);
		fs::write(root.join("hicolor/24x24/apps/sonarr-missing.svg"), "<svg/>").unwrap();
		assert!(
			resolver.get_icon_by_name("sonarr-missing", 24, 1).is_none(),
			"a repeated miss must not search the filesystem again"
		);
		let extra = root.join("extra");
		fs::create_dir(&extra).unwrap();
		resolver.add_icon_dir(&extra);
		assert!(
			resolver.get_icon_by_name("sonarr-missing", 24, 1).is_some(),
			"theme generation changes must invalidate negative entries"
		);
	}

	#[test]
	fn named_icon_hits_are_cached_per_size_and_scale() {
		let (mut resolver, root) = fixture();
		let path = root.join("hicolor/24x24/apps/spotify.svg");
		fs::write(&path, "<svg/>").unwrap();
		assert!(resolver.get_icon_by_name("spotify", 24, 1).is_some());
		fs::remove_file(&path).unwrap();
		assert!(
			resolver.get_icon_by_name("spotify", 24, 1).is_some(),
			"a cached hit must not search the filesystem again"
		);
		assert!(resolver.get_icon_by_name("spotify", 32, 1).is_none());
		assert!(resolver.get_icon_by_name("spotify", 24, 2).is_none());
		assert_eq!(resolver.cache.len(), 3);
		let extra = root.join("extra");
		fs::create_dir(&extra).unwrap();
		resolver.add_icon_dir(&extra);
		assert!(resolver.get_icon_by_name("spotify", 24, 1).is_none());
		assert_eq!(
			resolver.cache.len(),
			1,
			"obsolete generations must be swept"
		);
	}
}
