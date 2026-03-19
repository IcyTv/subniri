use std::cell::RefCell;

use astal_notifd::prelude::*;
use astal_notifd::Notification;
use glib::{clone, Properties};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::CompositeTemplate;

glib::wrapper! {
	pub struct NotificationItem(ObjectSubclass<imp::NotificationItem>)
		@extends gtk4::Box, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Orientable;
}

impl NotificationItem {
	pub fn new(notification: &Notification) -> Self {
		glib::Object::builder()
			.property("notification", Some(notification.clone()))
			.build()
	}
}

mod imp {
	use super::*;

	#[derive(Default, Properties, CompositeTemplate)]
	#[template(file = "./src/popups/notifications/item.blp")]
	#[properties(wrapper_type = super::NotificationItem)]
	pub struct NotificationItem {
		#[property(get, construct_only)]
		notification: RefCell<Option<Notification>>,

		#[template_child]
		app_icon: gtk4::TemplateChild<gtk4::Image>,

		#[template_child]
		action_buttons: gtk4::TemplateChild<gtk4::Box>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for NotificationItem {
		type ParentType = gtk4::Box;
		type Type = super::NotificationItem;

		const NAME: &'static str = "NotificationItem";

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
			klass.bind_template_callbacks();
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for NotificationItem {
		fn constructed(&self) {
			self.parent_constructed();

			let Some(notification) = self.notification.borrow().clone() else {
				return;
			};

			let app_icon = notification.app_icon();
			if !app_icon.is_empty() {
				if app_icon.contains('/') {
					self.app_icon.set_from_file(Some(app_icon.as_str()));
				} else {
					self.app_icon.set_icon_name(Some(app_icon.as_str()));
				}
			} else {
				self.app_icon.set_icon_name(Some("dialog-information-symbolic"));
			}

			let actions = notification.actions();
			if actions.is_empty() {
				self.action_buttons.set_visible(false);
				return;
			}

			for action in actions {
				let action_id = action.id().to_string();
				let button = gtk4::Button::builder().label(action.label().as_str()).build();
				button.connect_clicked(clone!(
					#[weak]
					notification,
					#[strong]
					action_id,
					move |_| {
						notification.invoke(&action_id);
					}
				));
				self.action_buttons.append(&button);
			}
		}
	}

	impl WidgetImpl for NotificationItem {}
	impl BoxImpl for NotificationItem {}

	#[gtk4::template_callbacks]
	impl NotificationItem {
		#[template_callback]
		fn on_dismiss(&self) {
			if let Some(notification) = self.notification.borrow().as_ref() {
				notification.dismiss();
			}
		}
	}
}
