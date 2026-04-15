use std::cell::RefCell;

use glib::subclass::InitializingObject;
use glib::Properties;
use gtk4::gio;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::CompositeTemplate;
use gtk4::Widget;
use niri_client::{Niri, NiriWindowLayout as WindowLayout, NiriWindowRaw as NiriWindow, NiriWorkspace as Workspace};

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

	pub fn new_window(window: &NiriWindow, workspace_id: u64, display_index: u8) -> Self {
		let widget = NiriWindowWidget::from_window(display_index, workspace_id, window);
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

	pub fn update_window(&self, window: &NiriWindow, workspace_id: u64, display_index: u8) {
		if let Some(widget) = self.window() {
			widget.refresh_from_window(display_index, workspace_id, window);
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
	pub fn from_window(workspace_index: u8, workspace_id: u64, window: &NiriWindow) -> Self {
		let icon = Self::icon_for_window(window);
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

	pub fn refresh_from_window(&self, workspace_index: u8, workspace_id: u64, window: &NiriWindow) {
		self.set_workspace_index(workspace_index);
		self.set_workspace_id(workspace_id);

		let (column, tile) = Self::position_for_window(window);
		self.set_column_index(column);
		self.set_tile_index(tile);

		let title = window.title.as_deref().unwrap_or_default();
		self.set_title(title);

		let icon = Self::icon_for_window(window);
		self.set_icon(icon);
	}

	pub fn set_focused(&self, focused: bool) {
		if focused {
			self.add_css_class("focused");
		} else {
			self.remove_css_class("focused");
		}
	}

	fn icon_for_window(window: &NiriWindow) -> gio::Icon {
		window
			.app_id
			.as_ref()
			.and_then(Self::get_icon_for_app_id)
			.unwrap_or_else(|| gio::Icon::for_string(icons::Icon::FileTerminal.name()).unwrap())
	}

	pub fn position_for_window(window: &NiriWindow) -> (i32, i32) {
		let pos = window.layout.pos_in_scrolling_layout.unwrap_or_default();
		(pos.0 as i32, pos.1 as i32)
	}

	fn get_icon_for_app_id(app_id: impl AsRef<str>) -> Option<gio::Icon> {
		icons::resolve_app_icon_from_app_id(app_id.as_ref())
	}
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
