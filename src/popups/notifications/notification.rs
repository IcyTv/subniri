use std::cell::RefCell;

use glib::Properties;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{CompositeTemplate, SignalListItemFactory};

use crate::icons::Icon;

glib::wrapper! {
	pub struct NotificationsPopup(ObjectSubclass<imp::NotificationsPopup>)
		@extends gtk4::Popover, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Buildable, gtk4::Constraint, gtk4::ConstraintTarget, gtk4::ShortcutManager, gtk4::Native;
}

mod imp {

	use super::*;

	#[derive(Default, Properties, CompositeTemplate)]
	#[template(file = "./src/popups/notifications/notification.blp")]
	#[properties(wrapper_type = super::NotificationsPopup)]
	pub struct NotificationsPopup {}

	#[glib::object_subclass]
	impl ObjectSubclass for NotificationsPopup {
		type ParentType = gtk4::Popover;
		type Type = super::NotificationsPopup;

		const NAME: &'static str = "NotificationsPopup";

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
			klass.bind_template_callbacks();
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for NotificationsPopup {
		fn constructed(&self) {
			self.parent_constructed();

			let factory = SignalListItemFactory::new();
		}
	}

	#[gtk4::template_callbacks]
	impl NotificationsPopup {}

	impl WidgetImpl for NotificationsPopup {}
	impl PopoverImpl for NotificationsPopup {}
}
