use std::cell::RefCell;

use astal4::prelude::*;
use glib::Properties;
use gtk4::subclass::prelude::*;
use gtk4::CompositeTemplate;

use icons::Icon;

glib::wrapper! {
	pub struct FolderButton(ObjectSubclass<imp::FolderButton>)
		@extends gtk4::Button, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Actionable, gtk4::Buildable, gtk4::Constraint, gtk4::ConstraintTarget, gtk4::ShortcutManager, gtk4::Root, gtk4::Native;
}

impl FolderButton {
	pub fn new(icon: Icon, label: &str) -> Self {
		glib::Object::builder()
			.property("icon-name", icon.name())
			.property("label", label)
			.build()
	}
}

mod imp {
	use super::*;

	#[derive(Default, Properties, CompositeTemplate)]
	#[template(file = "./src/popups/launcher/folder_button.blp")]
	#[properties(wrapper_type = super::FolderButton)]
	pub struct FolderButton {
		#[property(get, set)]
		pub icon_name: RefCell<String>,
		#[property(get, set)]
		pub label: RefCell<String>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for FolderButton {
		type ParentType = gtk4::Button;
		type Type = super::FolderButton;

		const NAME: &'static str = "FolderButton";

		fn class_init(klass: &mut Self::Class) {
			Self::bind_template(klass);
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for FolderButton {
		fn constructed(&self) {
			self.parent_constructed();
		}
	}

	impl WidgetImpl for FolderButton {}

	impl ButtonImpl for FolderButton {}
}
