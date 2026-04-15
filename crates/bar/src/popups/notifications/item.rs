use std::cell::RefCell;

use astal_notifd::prelude::*;
use astal_notifd::{Notification, Urgency};
use glib::{Properties, clone};
use gtk4::CompositeTemplate;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

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

			let obj = self.obj();

			let Some(notification) = self.notification.borrow().clone() else {
				return;
			};

			match notification.urgency() {
				Urgency::Low => obj.add_css_class("notification-level-low"),
				Urgency::Critical => obj.add_css_class("notification-level-critical"),
				_ => obj.add_css_class("notification-level-normal"),
			}

			self.set_best_app_icon(&notification);

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
		fn set_best_app_icon(&self, notification: &Notification) {
			if let Some(icon) = resolve_notification_icon(notification) {
				self.app_icon.set_from_gicon(&icon);
			} else {
				self.app_icon.set_icon_name(Some("dialog-information-symbolic"));
			}
		}

		#[template_callback]
		fn on_dismiss(&self) {
			if let Some(notification) = self.notification.borrow().as_ref() {
				notification.dismiss();
			}
		}
	}
}

pub fn resolve_notification_icon(notification: &Notification) -> Option<gtk4::gio::Icon> {
	let icon_theme = gtk4::gdk::Display::default().map(|display| gtk4::IconTheme::for_display(&display));

	for candidate in [notification.app_icon(), notification.image()] {
		if let Some(icon) = icons::resolve_icon_candidate(candidate.as_str(), icon_theme.as_ref()) {
			return Some(icon);
		}
	}

	let desktop_entry = notification.desktop_entry();
	if !desktop_entry.is_empty() {
		if let Some(icon) = icons::resolve_desktop_entry_icon(desktop_entry.as_str()) {
			return Some(icon);
		}

		if let Some(icon) = icons::resolve_icon_candidate(desktop_entry.as_str(), icon_theme.as_ref()) {
			return Some(icon);
		}
	}

	let app_name = notification.app_name();
	if !app_name.is_empty() {
		let normalized = app_name.to_ascii_lowercase().replace(' ', "-");
		if let Some(icon) = icons::resolve_icon_candidate(&normalized, icon_theme.as_ref()) {
			return Some(icon);
		}
	}

	None
}
