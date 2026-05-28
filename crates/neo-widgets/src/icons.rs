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

pub fn default_icon() -> ResolvedIcon {
	ResolvedIcon::Svg(phosphor_icon!("question", "bold"))
}

pub fn resolve_icon(app_id: impl AsRef<str>, size: u32, scale: u32) -> ResolvedIcon {
	resolve_with(|resolver| resolver.get_icon_by_app_id(app_id, size, scale))
}

pub fn resolve_from_desktop_entry(
	desktop_entry: impl AsRef<str>, size: u32, scale: u32,
) -> ResolvedIcon {
	resolve_with(|resolver| resolver.get_icon_by_desktop_entry(desktop_entry, size, scale))
}

pub fn resolve_from_window(window: &niri_ipc::Window, size: u32, scale: u32) -> ResolvedIcon {
	resolve_with(|resolver| resolver.get_icon_by_window(window, size, scale))
}

fn resolve_with(
	lookup: impl FnOnce(&mut ApplicationIconResolver) -> Option<IconFile>,
) -> ResolvedIcon {
	let resolver = icon_resolver();
	let mut resolver = resolver.lock().unwrap();

	lookup(&mut resolver)
		.map(ResolvedIcon::from_icon_file)
		.unwrap_or_else(default_icon)
}

pub fn phosphor_icon(icon: &str, variant: &str) -> svg::Handle {
	let path = if icon.contains('/') {
		PathBuf::from(icon)
	} else {
		let base = std::env::var("PHOSPHOR_ICONS").expect("PHOSPHOR_ICONS must be set");

		PathBuf::from(base)
			.join(variant)
			.join(format!("{icon}-{variant}.svg"))
	};

	svg::Handle::from_path(path)
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
			return icon.clone().to_opt();
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
			return icon.clone().to_opt();
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
		if let Some(icon) = self.get_icon_by_app_id(app_id, size, scale) {
			return Some(icon);
		}

		let pid = window.pid?;

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
	AppId,
	DesktopEntry,
}

#[derive(Clone)]
enum IconLookupResult {
	Found(IconFile),
	Missing,
}

impl IconLookupResult {
	fn to_opt(self) -> Option<IconFile> {
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
		std::env::var_os("NIX_STORE_DIR")
			.map(PathBuf::from)
			.unwrap_or_else(|| PathBuf::from("/nix/store"))
	})
}

fn nix_output_root_for_path(path: &Path) -> Option<PathBuf> {
	let store = nix_store_dir();
	let rel = path.strip_prefix(store).ok()?;

	let mut components = rel.components();
	let output_name = components.next()?.as_os_str();

	Some(store.join(output_name))
}
