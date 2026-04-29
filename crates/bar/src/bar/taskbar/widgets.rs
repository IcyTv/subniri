use glib::Properties;
use glib::subclass::InitializingObject;
use gtk4::CompositeTemplate;
use gtk4::Widget;
use gtk4::gio;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use niri_client::{Niri, NiriWindowLayout as WindowLayout, NiriWindowRaw as NiriWindow, NiriWorkspace as Workspace};
use std::cell::RefCell;
use std::path::Path;
use std::path::PathBuf;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::AtomEnum;
use x11rb::protocol::xproto::ConnectionExt;

use super::IconCache;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TaskbarItemKind {
	Workspace,
	Window,
}

impl TaskbarItemKind {
	pub fn sort_value(self) -> i32 {
		match self {
			TaskbarItemKind::Workspace => 0,
			TaskbarItemKind::Window => 1,
		}
	}
}

glib::wrapper! {
	pub struct TaskbarItem(ObjectSubclass<taskbar_item_imp::TaskbarItem>);
}

impl TaskbarItem {
	pub fn new_workspace(workspace: &Workspace, display_index: u8) -> Self {
		let widget = NiriWorkspaceWidget::from_workspace(workspace, display_index);
		Self::from_widget(widget.upcast::<Widget>(), TaskbarItemKind::Workspace)
	}

	pub fn new_window(window: &NiriWindow, workspace_id: u64, display_index: u8, icon_cache: IconCache) -> Self {
		let widget = NiriWindowWidget::from_window(display_index, workspace_id, window, icon_cache);
		Self::from_widget(widget.upcast::<Widget>(), TaskbarItemKind::Window)
	}

	fn from_widget(widget: Widget, kind: TaskbarItemKind) -> Self {
		glib::Object::builder()
			.property("object", widget)
			.property("item-kind", kind.sort_value())
			.build()
	}

	pub fn is_window(&self) -> bool {
		self.kind() == TaskbarItemKind::Window
	}

	pub fn is_workspace(&self) -> bool {
		self.kind() == TaskbarItemKind::Workspace
	}

	pub fn kind(&self) -> TaskbarItemKind {
		match self.item_kind() {
			0 => TaskbarItemKind::Workspace,
			_ => TaskbarItemKind::Window,
		}
	}

	pub fn window(&self) -> Option<NiriWindowWidget> {
		self.object().and_then(|obj| obj.downcast::<NiriWindowWidget>().ok())
	}

	pub fn workspace(&self) -> Option<NiriWorkspaceWidget> {
		self.object().and_then(|obj| obj.downcast::<NiriWorkspaceWidget>().ok())
	}

	pub fn widget(&self) -> Option<Widget> {
		self.object()
	}

	pub fn workspace_id(&self) -> u64 {
		if let Some(window) = self.window() {
			window.workspace_id()
		} else if let Some(workspace) = self.workspace() {
			workspace.workspace_id()
		} else {
			0
		}
	}

	pub fn workspace_index(&self) -> i32 {
		if let Some(window) = self.window() {
			window.workspace_index() as i32
		} else if let Some(workspace) = self.workspace() {
			workspace.workspace_index() as i32
		} else {
			0
		}
	}

	pub fn column_index(&self) -> i32 {
		self.window().map(|w| w.column_index()).unwrap_or(-1)
	}

	pub fn tile_index(&self) -> i32 {
		self.window().map(|w| w.tile_index()).unwrap_or(-1)
	}

	pub fn window_id(&self) -> u64 {
		self.window().map(|w| w.window_id()).unwrap_or(0)
	}

	pub fn update_workspace(&self, workspace: &Workspace, display_index: u8) {
		if let Some(widget) = self.workspace() {
			widget.refresh_from_workspace(workspace, display_index);
		}
	}

	pub fn update_window(&self, window: &NiriWindow, workspace_id: u64, display_index: u8, icon_cache: IconCache) {
		if let Some(widget) = self.window() {
			widget.refresh_from_window(display_index, workspace_id, window, icon_cache);
		}
	}
}

mod taskbar_item_imp {
	use super::*;

	#[derive(Properties, Default)]
	#[properties(wrapper_type = super::TaskbarItem)]
	pub struct TaskbarItem {
		#[property(get, set)]
		object: RefCell<Option<gtk4::Widget>>,
		#[property(name = "item-kind", get, set, default = 0)]
		item_kind: RefCell<i32>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for TaskbarItem {
		type ParentType = glib::Object;
		type Type = super::TaskbarItem;
		const NAME: &'static str = "TaskbarItem";
	}

	#[glib::derived_properties]
	impl ObjectImpl for TaskbarItem {
		fn constructed(&self) {
			self.parent_constructed();
		}
	}
}

glib::wrapper! {
	pub struct NiriWindowWidget(ObjectSubclass<niri_window_imp::NiriWindowWidget>)
		@extends gtk4::Button, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Actionable, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl NiriWindowWidget {
	pub fn from_window(workspace_index: u8, workspace_id: u64, window: &NiriWindow, icon_cache: IconCache) -> Self {
		let icon = Self::icon_for_window(window, icon_cache);
		let (column, tile) = Self::position_for_window(window);

		let widget: Self = glib::Object::builder()
			.property("icon", Some(icon))
			.property("title", window.title.clone().unwrap_or_default())
			.property("workspace-index", workspace_index)
			.property("workspace-id", workspace_id)
			.property("window-id", window.id)
			.property("column-index", column)
			.property("tile-index", tile)
			.build();

		if window.is_focused {
			widget.add_css_class("focused");
		}

		widget
	}

	pub fn refresh_from_layout(&self, layout: WindowLayout) {
		let (column, tile) = layout.pos_in_scrolling_layout.unwrap_or_default();
		self.set_column_index(column as i32);
		self.set_tile_index(tile as i32);
	}

	pub fn refresh_from_window(
		&self, workspace_index: u8, workspace_id: u64, window: &NiriWindow, icon_cache: IconCache,
	) {
		self.set_workspace_index(workspace_index);
		self.set_workspace_id(workspace_id);

		let (column, tile) = Self::position_for_window(window);
		self.set_column_index(column);
		self.set_tile_index(tile);

		let title = window.title.as_deref().unwrap_or_default();
		self.set_title(title);

		let icon = Self::icon_for_window(window, icon_cache);
		self.set_icon(icon);
	}

	pub fn set_focused(&self, focused: bool) {
		if focused {
			self.add_css_class("focused");
		} else {
			self.remove_css_class("focused");
		}
	}

	fn icon_for_window(window: &NiriWindow, icon_cache: IconCache) -> gio::Icon {
		resolve_app_icon_from_window(window, icon_cache)
			.unwrap_or_else(|| gio::Icon::for_string(icons::Icon::FileTerminal.name()).unwrap())
	}

	pub fn position_for_window(window: &NiriWindow) -> (i32, i32) {
		let pos = window.layout.pos_in_scrolling_layout.unwrap_or_default();
		(pos.0 as i32, pos.1 as i32)
	}
}

// TODO: For better consolidation, move the inserts (and maybe the get) of the cache into a wrapper
// function or just leave it to the calling function...
// TODO: In fact we might just want to store a raw gio::Icon (without the Option) in the icon cache,
// and just use the default icon for unresolved icons...
fn resolve_app_icon_from_window(window: &NiriWindow, icon_cache: IconCache) -> Option<gio::Icon> {
	if let Some(cached) = icon_cache.get(window) {
		return cached;
	}

	if let Some(icon) = window
		.app_id
		.as_ref()
		.and_then(|app_id| icons::resolve_app_icon_from_app_id(&app_id))
	{
		icon_cache.insert(window, Some(icon.clone()));
		return Some(icon);
	}

	if let Some(pid) = window.pid {
		let exe_path = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
		let exe_str = exe_path
			.as_ref()
			.map(|p| p.to_string_lossy().into_owned())
			.unwrap_or_default();

		let cmdline_bytes = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
		let args: Vec<&[u8]> = cmdline_bytes.split(|&b| b == 0).filter(|s| !s.is_empty()).collect();
		// Check if it's an Xwayland app
		let is_x11 = exe_str.to_ascii_lowercase().contains("xwayland")
			|| args
				.iter()
				.any(|arg| arg.to_ascii_lowercase().windows(8).any(|w| w == b"xwayland"));

		let true_exe_path = if is_x11 {
			if let Some(app_id) = &window.app_id {
				find_executable_for_x11_app(app_id).ok()
			} else {
				None
			}
		} else {
			exe_path.clone()
		};

		// Collect candidate paths: the true executable, and any absolute paths in cmdline
		let mut candidate_paths = Vec::new();
		if let Some(path) = true_exe_path {
			candidate_paths.push(path);
		}

		for arg in &args {
			let path_str = String::from_utf8_lossy(arg);
			let path = PathBuf::from(path_str.as_ref());
			if path.is_absolute() && path.exists() {
				candidate_paths.push(path);
			}
		}

		let candidates = window
			.app_id
			.as_ref()
			.map_or(vec![], |app_id| icons::app_id_candidates(app_id));

		// Try to resolve using local prefix (e.g. Nix store, /opt, ~/.local)
		for candidate in &candidate_paths {
			let roots = find_app_roots_from_path(candidate);
			for root in roots {
				if let Some(df) = find_desktop_file_for_root_folder(&root, &candidates) {
					let kf = glib::KeyFile::new();
					match kf.load_from_file(&df, glib::KeyFileFlags::NONE) {
						Ok(_) => (),
						Err(e) => {
							eprintln!("Failed to parse keyfile: {e}");
							continue;
						}
					};
					if let Ok(icon_str) = kf.string("Desktop Entry", "Icon") {
						if let Some(icon) = resolve_local_app_icon(&root, &icon_str) {
							icon_cache.insert(window, Some(icon.clone()));
							return Some(icon);
						}
					}
				}
			}
		}
	}

	icon_cache.insert(window, None);
	None
}

fn find_app_roots_from_path(path: &Path) -> Vec<PathBuf> {
	let mut roots = Vec::new();
	let mut current = Some(path);

	while let Some(parent) = current.and_then(|p| p.parent()) {
		let share_apps = parent.join("share").join("applications");
		if share_apps.is_dir() {
			roots.push(parent.to_path_buf());
		}
		current = Some(parent);
	}

	roots
}

fn find_desktop_file_for_root_folder(root_folder: &Path, candidates: &[String]) -> Option<PathBuf> {
	let applications_share_folder = root_folder.join("share").join("applications");

	let mut all_desktops = Vec::new();

	if let Ok(entries) = std::fs::read_dir(&applications_share_folder) {
		for entry in entries.flatten() {
			let path = entry.path();
			if path.extension().map_or(false, |e| e == "desktop") {
				all_desktops.push(path);
			}
		}
	}

	if all_desktops.is_empty() {
		return None;
	}

	if !candidates.is_empty() {
		// 1. Exact match
		for path in &all_desktops {
			if let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) {
				if candidates.iter().any(|c| c == file_stem) {
					return Some(path.clone());
				}
			}
		}

		// 2. Reverse-DNS suffix match
		for path in &all_desktops {
			if let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) {
				if candidates.iter().any(|c| file_stem.ends_with(&format!(".{c}"))) {
					return Some(path.clone());
				}
			}
		}
	}

	// 3. Smart Fallback: Score them based on likeliness of being the main app
	let mut scored_desktops: Vec<_> = all_desktops
		.into_iter()
		.map(|path| {
			let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
			let mut penalty_score = 100; // Lower is better

			// Huge boost if it at least contains the candidate name
			if candidates.iter().any(|c| file_stem.contains(c)) {
				penalty_score -= 50;
			}

			// Penalize MIME-type and generic handlers
			if file_stem.contains('_') {
				penalty_score += 30; // Heavy penalty for things like 'krita_csv'
			}
			if file_stem.contains('-') {
				penalty_score += 10; // Light penalty for things like 'krita-painter'
			}

			// Break ties with length (shorter is usually the main app, assuming no underscores)
			penalty_score += file_stem.len();

			(penalty_score, path)
		})
		.collect();

	// Sort by lowest penalty score
	scored_desktops.sort_by_key(|k| k.0);

	// Return the winner
	scored_desktops.into_iter().next().map(|(_, p)| p)
}

fn resolve_local_app_icon(root_folder: &Path, icon_str: &str) -> Option<gio::Icon> {
	let path = PathBuf::from(icon_str);

	if path.is_absolute() && path.exists() {
		let file = gio::File::for_path(&path);
		return Some(gio::FileIcon::new(&file).upcast::<gio::Icon>());
	}

	const ICON_SIZES: &[&str] = &["scalable", "256x256", "128x128", "64x64", "48x48", "32x32"];
	const ICON_EXTENSIONS: &[&str] = &["svg", "png", "xpm"];

	for size in ICON_SIZES {
		let apps_folder = root_folder
			.join("share")
			.join("icons")
			.join("hicolor")
			.join(size)
			.join("apps");

		if !apps_folder.exists() {
			continue;
		}

		// Exact Match
		for ext in ICON_EXTENSIONS {
			let icon = apps_folder.join(icon_str).with_extension(ext);
			if icon.exists() {
				let file = gio::File::for_path(icon);
				return Some(gio::FileIcon::new(&file).upcast::<gio::Icon>());
			}
		}
	}

	let pixmaps_folder = root_folder.join("share").join("pixmaps");
	if pixmaps_folder.exists() {
		// Exact Match
		for ext in ICON_EXTENSIONS {
			let icon = pixmaps_folder.join(icon_str).with_extension(ext);
			if icon.exists() {
				let file = gio::File::for_path(icon);
				return Some(gio::FileIcon::new(&file).upcast::<gio::Icon>());
			}
		}
	}

	None
}

fn find_executable_for_x11_app(app_id: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
	let (conn, screen_num) = x11rb::connect(None)?;
	let screen = &conn.setup().roots[screen_num];
	let root_window = screen.root;

	let net_win_pid_atom = conn.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
	let wm_class_atom = AtomEnum::WM_CLASS;

	let tree = conn.query_tree(root_window)?.reply()?;

	for window in tree.children {
		let class_cookie = conn.get_property(false, window, wm_class_atom, AtomEnum::STRING, 0, 1024)?;
		let class_reply = class_cookie.reply()?;

		if let Some(class_val) = class_reply.value8() {
			let class_val: Vec<_> = class_val.collect();
			let class_str = String::from_utf8_lossy(&class_val);

			if class_str.contains(app_id) {
				let pid_cookie = conn.get_property(false, window, net_win_pid_atom, AtomEnum::CARDINAL, 0, 1)?;
				let pid_reply = pid_cookie.reply()?;

				if let Some(mut pid_val) = pid_reply.value32() {
					if let Some(pid) = pid_val.next() {
						let proc_path = format!("/proc/{}/exe", pid);
						let exe_path = std::fs::read_link(proc_path)?;

						return Ok(exe_path);
					}
				}
			}
		}
	}

	Err("Window or PID not found".into())
}

mod niri_window_imp {

	use super::*;

	#[derive(Properties, Default, CompositeTemplate)]
	#[template(file = "src/bar/taskbar/niri_window_widget.blp")]
	#[properties(wrapper_type = super::NiriWindowWidget)]
	pub struct NiriWindowWidget {
		#[property(get, construct_only)]
		window_id: RefCell<u64>,
		#[property(get, set)]
		pub icon: RefCell<Option<gio::Icon>>,
		#[property(get, set)]
		title: RefCell<String>,
		#[property(get, set)]
		workspace_index: RefCell<u8>,
		#[property(get, set)]
		workspace_id: RefCell<u64>,
		#[property(get, set)]
		column_index: RefCell<i32>,
		#[property(get, set)]
		tile_index: RefCell<i32>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for NiriWindowWidget {
		type ParentType = gtk4::Button;
		type Type = super::NiriWindowWidget;

		const NAME: &'static str = "NiriWindowWidget";

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for NiriWindowWidget {
		fn constructed(&self) {
			self.parent_constructed();
		}
	}

	impl WidgetImpl for NiriWindowWidget {}

	impl ButtonImpl for NiriWindowWidget {
		fn clicked(&self) {
			Niri::new().activate_window(*self.window_id.borrow());
		}
	}
}

glib::wrapper! {
	pub struct NiriWorkspaceWidget(ObjectSubclass<niri_workspace_imp::NiriWorkspaceWidget>)
		@extends gtk4::Button, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Actionable, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl NiriWorkspaceWidget {
	pub fn new_null() -> Self {
		glib::Object::builder()
			.property("icon", None::<String>)
			.property("workspace-id", 0u64)
			.property("display-mode", "workspace-index")
			.build()
	}

	pub fn from_workspace(workspace: &Workspace, display_index: u8) -> Self {
		let widget: Self = glib::Object::builder()
			.property("icon", workspace.name.clone())
			.property("workspace-id", workspace.id)
			.property("workspace-index", display_index)
			.property(
				"display-mode",
				if workspace.name.is_some() {
					"workspace-icon"
				} else {
					"workspace-index"
				},
			)
			.build();
		widget.set_focused(workspace.is_focused);
		widget
	}

	pub fn refresh_from_workspace(&self, workspace: &Workspace, display_index: u8) {
		self.set_workspace_id(workspace.id);
		self.set_workspace_index(display_index);
		if let Some(name) = &workspace.name {
			self.set_icon(name.clone());
		} else {
			self.set_property("icon", None::<String>);
		}
		self.set_display_mode(if workspace.name.is_some() {
			"workspace-icon"
		} else {
			"workspace-index"
		});
		self.set_focused(workspace.is_focused);
	}

	pub fn set_focused(&self, focused: bool) {
		if focused {
			self.add_css_class("focused");
		} else {
			self.remove_css_class("focused");
		}
	}
}

mod niri_workspace_imp {

	use super::*;

	#[derive(Properties, Default, CompositeTemplate)]
	#[properties(wrapper_type = super::NiriWorkspaceWidget)]
	#[template(file = "src/bar/taskbar/niri_workspace_widget.blp")]
	pub struct NiriWorkspaceWidget {
		#[property(get, set)]
		pub icon: RefCell<Option<String>>,
		#[property(get, set)]
		workspace_id: RefCell<u64>,
		#[property(get, set)]
		workspace_index: RefCell<u8>,
		#[property(get, set)]
		display_mode: RefCell<String>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for NiriWorkspaceWidget {
		type ParentType = gtk4::Button;
		type Type = super::NiriWorkspaceWidget;

		const NAME: &'static str = "NiriWorkspaceWidget";

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
		}

		fn instance_init(obj: &InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for NiriWorkspaceWidget {
		fn constructed(&self) {
			self.parent_constructed();
		}
	}

	impl WidgetImpl for NiriWorkspaceWidget {}

	impl ButtonImpl for NiriWorkspaceWidget {
		fn clicked(&self) {
			Niri::new().activate_workspace(*self.workspace_id.borrow());
		}
	}
}
