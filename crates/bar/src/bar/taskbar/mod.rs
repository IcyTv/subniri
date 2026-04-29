mod widgets;

use std::cell::RefCell;
use std::cmp::Ordering as StdOrdering;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::rc::Rc;

use futures::StreamExt;
use glib::clone;
use glib::object::Cast;
use gtk4::prelude::*;
use gtk4::{CustomSorter, Ordering as GtkOrdering, gdk, gio};
use lru::LruCache;
use niri_client::{NiriWindowLayout as WindowLayout, NiriWindowRaw as Window, NiriWorkspace as Workspace};

use widgets::{TaskbarItem, TaskbarItemKind};

#[derive(Clone)]
pub struct IconCache(Rc<RefCell<LruCache<String, Option<gio::Icon>>>>);

impl IconCache {
	pub fn new() -> Self {
		Self(Rc::new(RefCell::new(LruCache::new(unsafe {
			NonZeroUsize::new_unchecked(100)
		}))))
	}

	/// Get a cached icon based of the PID
	/// This function caches based off the `/proc/<pid>/cmdline` file.
	/// That means: If the window has no PID, we can't cache it (should only happen with the gnome
	/// portal, so not a huge deal)...
	///
	/// # Returns:
	/// - `None` if the underlying resolver needs to run
	/// - `Some(None)` if we have cached, that the app has no icon we can resolve
	/// - `Some(Some(icon))` if the icon can be resolved
	pub fn get(&self, window: &Window) -> Option<Option<gio::Icon>> {
		let cache_key = Self::cache_key(window)?;

		let mut icon_cache = self.0.borrow_mut();
		icon_cache.get(&cache_key).cloned()
	}

	pub fn insert(&self, window: &Window, icon: Option<gio::Icon>) {
		let Some(cache_key) = Self::cache_key(window) else {
			return;
		};

		let mut icon_cache = self.0.borrow_mut();
		let _ = icon_cache.push(cache_key, icon);
	}

	fn cache_key(window: &Window) -> Option<String> {
		let mut cache_key = String::new();
		if let Some(app_id) = &window.app_id {
			if !matches!(app_id.as_str(), "electron" | "python" | "java") {
				cache_key = app_id.clone();
			}
		}
		if let Some(pid) = window.pid
			&& cache_key.is_empty()
		{
			let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
				return None;
			};
			let cmdline = String::from_utf8_lossy(&cmdline);
			cache_key = cmdline.to_string()
		}

		if cache_key.contains("xwayland") {
			// TODO: For now we don't cache xwayland windows that don't properly advertise.
			// We could technically do the stunt we do in the actual icon resolution, but, since
			// this is rare, we simply re-resolve the icon...
			return None;
		}

		if !cache_key.is_empty() { Some(cache_key) } else { None }
	}
}

pub struct Taskbar {
	widget: gtk4::ListView,
}

impl Taskbar {
	pub fn new(monitor_index: i32, icon_cache: IconCache) -> Self {
		let item_factory = create_item_factory();
		let sorter = create_sorter();

		let store = gio::ListStore::new::<TaskbarItem>();

		let sort_model = gtk4::SortListModel::new(Some(store.clone()), Some(sorter));

		let selection_model = gtk4::NoSelection::new(Some(sort_model));

		let widget = gtk4::ListView::builder()
			.name("taskbar")
			.orientation(gtk4::Orientation::Horizontal)
			.model(&selection_model)
			.factory(&item_factory)
			.css_classes(["taskbar"])
			.build();

		glib::spawn_future_local(clone!(
			#[weak]
			store,
			async move {
				let monitor = gdk::Display::default()
					.expect("to have a default display")
					.monitors()
					.into_iter()
					.enumerate()
					.find_map(|(i, m)| if i as i32 == monitor_index { Some(m) } else { None })
					.unwrap()
					.unwrap()
					.downcast::<gdk::Monitor>()
					.unwrap();

				let mut workspace_tracker = WorkspaceTracker::new(&monitor);
				sync_workspace_items(&store, &workspace_tracker);
				update_workspace_focus(&store, workspace_tracker.focused_workspace_id());

				let mut event_stream = Box::pin(niri_client::event_stream());

				while let Some(event) = event_stream.next().await {
					use niri_client::Event::*;
					match event {
						WorkspacesChanged { workspaces } => {
							workspace_tracker.update(workspaces);
							sync_workspace_items(&store, &workspace_tracker);
							reconcile_window_items_with_workspaces(&store, &workspace_tracker);
							update_workspace_focus(&store, workspace_tracker.focused_workspace_id());
						}
						WindowsChanged { windows } => {
							rebuild_window_items(&store, &workspace_tracker, &windows, icon_cache.clone());
						}
						WindowOpenedOrChanged { window } => {
							handle_window_update(&store, &workspace_tracker, &window, icon_cache.clone());
						}
						WindowClosed { id } => {
							remove_window_item(&store, id);
						}
						WindowFocusChanged { id } => {
							update_window_focus(&store, id);
						}
						WindowLayoutsChanged { changes } => {
							for (id, layout) in changes {
								update_window_layout(&store, id, layout);
							}
						}
						WorkspaceActivated { id, .. } => {
							update_workspace_focus(&store, Some(id));
						}
						WorkspaceActiveWindowChanged {
							workspace_id,
							active_window_id,
						} => {
							update_workspace_focus(&store, Some(workspace_id));
							update_window_focus(&store, active_window_id);
						}
						_ => {}
					}
				}

				panic!("Niri IPC event stream ended unexpectedly");
			}
		));

		Self { widget }
	}

	pub fn widget(&self) -> &gtk4::Widget {
		self.widget.upcast_ref()
	}
}

struct WorkspaceTracker {
	allowed_outputs: HashSet<String>,
	workspaces: HashMap<u64, Workspace>,
	base_index: u8,
}

impl WorkspaceTracker {
	fn new(monitor: &gdk::Monitor) -> Self {
		let allowed_outputs = resolve_outputs_for_monitor(monitor);
		let initial_workspaces = niri_client::fetch_workspaces();
		let mut tracker = Self {
			allowed_outputs,
			workspaces: HashMap::new(),
			base_index: 0,
		};
		tracker.update(initial_workspaces);
		tracker
	}

	fn update(&mut self, workspaces: Vec<Workspace>) {
		self.workspaces = workspaces.into_iter().map(|ws| (ws.id, ws)).collect();
		self.recompute_base_index();
	}

	fn recompute_base_index(&mut self) {
		let min_idx = self
			.workspaces
			.values()
			.filter(|ws| self.workspace_is_visible(ws.id))
			.map(|ws| ws.idx)
			.min()
			.unwrap_or(0);
		self.base_index = min_idx;
	}

	fn workspace_details(&self, window: &Window) -> Option<(u64, u8)> {
		let workspace_id = window.workspace_id?;
		let idx = self.display_index(workspace_id)?;
		Some((workspace_id, idx))
	}

	fn visible_workspace_ids(&self) -> Vec<u64> {
		let mut ids: Vec<u64> = self
			.workspaces
			.values()
			.filter(|ws| self.workspace_is_visible(ws.id))
			.map(|ws| ws.id)
			.collect();
		ids.sort_by_key(|id| self.workspaces.get(id).map(|ws| ws.idx).unwrap_or(0));
		ids
	}

	fn display_index(&self, workspace_id: u64) -> Option<u8> {
		let ws = self.workspaces.get(&workspace_id)?;
		if !self.workspace_is_visible(workspace_id) {
			return None;
		}
		Some(ws.idx.saturating_sub(self.base_index))
	}

	fn workspace_is_visible(&self, workspace_id: u64) -> bool {
		if self.allowed_outputs.is_empty() {
			return true;
		}

		self.workspaces
			.get(&workspace_id)
			.and_then(|ws| ws.output.as_ref())
			.map(|output| self.allowed_outputs.contains(output))
			.unwrap_or(false)
	}

	fn workspace(&self, workspace_id: u64) -> Option<&Workspace> {
		self.workspaces.get(&workspace_id)
	}

	fn focused_workspace_id(&self) -> Option<u64> {
		self.visible_workspace_ids()
			.into_iter()
			.find(|id| self.workspaces.get(id).is_some_and(|ws| ws.is_focused))
	}

	fn display_index_for_workspace(&self, workspace_id: u64) -> u8 {
		self.display_index(workspace_id).unwrap_or(0)
	}
}

fn resolve_outputs_for_monitor(monitor: &gdk::Monitor) -> HashSet<String> {
	let mut allowed = HashSet::new();

	if let Some(connector) = monitor.connector() {
		let connector = connector.to_string();
		allowed.insert(connector.clone());
		let outputs = niri_client::fetch_outputs();
		if let Some(output) = outputs.get(&connector) {
			allowed.insert(output.name.clone());
		}
	}

	allowed
}

fn sync_workspace_items(store: &gio::ListStore, tracker: &WorkspaceTracker) {
	let mut keep: HashSet<u64> = HashSet::new();

	for workspace_id in tracker.visible_workspace_ids() {
		if let Some(workspace) = tracker.workspace(workspace_id) {
			let display_index = tracker.display_index_for_workspace(workspace_id);
			if let Some((index, item)) =
				find_taskbar_item(store, |item| item.is_workspace() && item.workspace_id() == workspace_id)
			{
				item.update_workspace(workspace, display_index);
				store.items_changed(index, 1, 1);
			} else {
				let item = TaskbarItem::new_workspace(workspace, display_index);
				store.append(&item);
			}
			keep.insert(workspace_id);
		}
	}

	store.retain(|obj| {
		if let Some(item) = obj.downcast_ref::<TaskbarItem>() {
			if item.is_workspace() {
				keep.contains(&item.workspace_id())
			} else {
				true
			}
		} else {
			false
		}
	});
}

fn reconcile_window_items_with_workspaces(store: &gio::ListStore, tracker: &WorkspaceTracker) {
	for index in (0..store.n_items()).rev() {
		if let Some(obj) = store.item(index)
			&& let Ok(item) = obj.downcast::<TaskbarItem>()
			&& item.is_window()
		{
			let workspace_id = item.workspace_id();
			if let Some(display_index) = tracker.display_index(workspace_id) {
				if let Some(widget) = item.window() {
					widget.set_workspace_index(display_index);
					widget.set_workspace_id(workspace_id);
					store.items_changed(index, 1, 1);
				}
			} else {
				store.remove(index);
			}
		}
	}
}

fn rebuild_window_items(store: &gio::ListStore, tracker: &WorkspaceTracker, windows: &[Window], icon_cache: IconCache) {
	remove_window_items(store);

	for window in windows {
		if let Some((workspace_id, display_index)) = tracker.workspace_details(window) {
			let item = TaskbarItem::new_window(window, workspace_id, display_index, icon_cache.clone());
			store.append(&item);
		}
	}
}

fn handle_window_update(store: &gio::ListStore, tracker: &WorkspaceTracker, window: &Window, icon_cache: IconCache) {
	let placement = tracker.workspace_details(window);

	if let Some((index, item)) = find_taskbar_item(store, |item| item.is_window() && item.window_id() == window.id) {
		if let Some((workspace_id, display_index)) = placement {
			item.update_window(window, workspace_id, display_index, icon_cache);
			store.items_changed(index, 1, 1);
		} else {
			store.remove(index);
		}
	} else if let Some((workspace_id, display_index)) = placement {
		let item = TaskbarItem::new_window(window, workspace_id, display_index, icon_cache);
		store.append(&item);
	}
}

fn remove_window_item(store: &gio::ListStore, window_id: u64) {
	if let Some((index, _)) = find_taskbar_item(store, |item| item.is_window() && item.window_id() == window_id) {
		store.remove(index);
	}
}

fn update_window_focus(store: &gio::ListStore, focused_window_id: Option<u64>) {
	for index in 0..store.n_items() {
		if let Some(obj) = store.item(index)
			&& let Ok(item) = obj.downcast::<TaskbarItem>()
			&& item.is_window()
		{
			if let Some(widget) = item.window() {
				widget.set_focused(focused_window_id == Some(widget.window_id()));
			}
		}
	}
}

fn update_workspace_focus(store: &gio::ListStore, focused_workspace_id: Option<u64>) {
	for index in 0..store.n_items() {
		if let Some(obj) = store.item(index)
			&& let Ok(item) = obj.downcast::<TaskbarItem>()
			&& item.is_workspace()
		{
			if let Some(widget) = item.workspace() {
				widget.set_focused(focused_workspace_id == Some(widget.workspace_id()));
			}
		}
	}
}

fn update_window_layout(store: &gio::ListStore, window_id: u64, layout: WindowLayout) {
	if let Some((index, item)) = find_taskbar_item(store, |item| item.is_window() && item.window_id() == window_id) {
		if let Some(widget) = item.window() {
			widget.refresh_from_layout(layout);
			store.items_changed(index, 1, 1);
		}
	}
}

fn remove_window_items(store: &gio::ListStore) {
	store.retain(|obj| {
		if let Some(item) = obj.downcast_ref::<TaskbarItem>() {
			item.is_workspace()
		} else {
			false
		}
	});
}

fn find_taskbar_item<F>(store: &gio::ListStore, mut predicate: F) -> Option<(u32, TaskbarItem)>
where
	F: FnMut(&TaskbarItem) -> bool,
{
	for index in 0..store.n_items() {
		if let Some(obj) = store.item(index)
			&& let Ok(item) = obj.downcast::<TaskbarItem>()
		{
			if predicate(&item) {
				return Some((index, item));
			}
		}
	}
	None
}

fn to_gtk_ordering(ordering: StdOrdering) -> GtkOrdering {
	match ordering {
		StdOrdering::Less => GtkOrdering::Smaller,
		StdOrdering::Equal => GtkOrdering::Equal,
		StdOrdering::Greater => GtkOrdering::Larger,
	}
}

fn create_item_factory() -> gtk4::SignalListItemFactory {
	let factory = gtk4::SignalListItemFactory::new();

	factory.connect_setup(|_, li| {
		let li = li.downcast_ref::<gtk4::ListItem>().expect("to be a ListItem");
		li.set_child(Some(&gtk4::Box::new(gtk4::Orientation::Horizontal, 0)));
	});
	factory.connect_bind(|_, li| {
		let list_item = li.downcast_ref::<gtk4::ListItem>().expect("Needs to be a ListItem");
		if let Some(item) = list_item.item().and_downcast::<TaskbarItem>() {
			list_item.set_child(item.widget().as_ref());
		}
	});
	factory.connect_unbind(|_, li| {
		let list_item = li.downcast_ref::<gtk4::ListItem>().expect("Needs to be a ListItem");
		list_item.set_child(None::<&gtk4::Widget>);
	});
	factory
}

fn create_sorter() -> CustomSorter {
	CustomSorter::new(|obj_a, obj_b| {
		let item_a = obj_a.downcast_ref::<TaskbarItem>().expect("TaskbarItem");
		let item_b = obj_b.downcast_ref::<TaskbarItem>().expect("TaskbarItem");

		let mut cmp = item_a.workspace_index().cmp(&item_b.workspace_index());
		if cmp != StdOrdering::Equal {
			return to_gtk_ordering(cmp);
		}

		cmp = item_a.kind().sort_value().cmp(&item_b.kind().sort_value());
		if cmp != StdOrdering::Equal {
			return to_gtk_ordering(cmp);
		}

		if item_a.kind() == TaskbarItemKind::Window && item_b.kind() == TaskbarItemKind::Window {
			cmp = item_a.column_index().cmp(&item_b.column_index());
			if cmp != StdOrdering::Equal {
				return to_gtk_ordering(cmp);
			}
			cmp = item_a.tile_index().cmp(&item_b.tile_index());
			if cmp != StdOrdering::Equal {
				return to_gtk_ordering(cmp);
			}
			cmp = item_a.window_id().cmp(&item_b.window_id());
			if cmp != StdOrdering::Equal {
				return to_gtk_ordering(cmp);
			}
		}

		cmp = item_a.workspace_id().cmp(&item_b.workspace_id());
		to_gtk_ordering(cmp)
	})
}
