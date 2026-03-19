use astal_notifd::prelude::*;
use astal_notifd::Notifd;
use glib::{clone, Properties};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::CompositeTemplate;

use std::cell::RefCell;

use crate::icons::Icon;
use crate::popups::notifications::NotificationsPopup;

glib::wrapper! {
	pub struct Notifications(ObjectSubclass<imp::Notifications>)
		@extends gtk4::Button, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Actionable, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Notifications {
	pub fn new() -> Self {
		glib::Object::builder()
			.property("has-unread", false)
			.property("bell-icon-name", Icon::Bell.name())
			.build()
	}
}

mod imp {
	use super::*;

	#[derive(CompositeTemplate, Default, Properties)]
	#[properties(wrapper_type = super::Notifications)]
	#[template(file = "./src/bar/notifications/notifications.blp")]
	pub struct Notifications {
		#[property(get, set)]
		has_unread: RefCell<bool>,
		#[property(get, set)]
		bell_icon_name: RefCell<String>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for Notifications {
		const NAME: &'static str = "Notifications";
		type Type = super::Notifications;
		type ParentType = gtk4::Button;

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for Notifications {
		fn constructed(&self) {
			self.parent_constructed();
			let notifd = Notifd::default();

			let obj = self.obj();
			let popup = NotificationsPopup::new();
			popup.set_parent(&*obj);

			obj.connect_clicked(clone!(
				#[weak]
				popup,
				move |_| {
					popup.popup();
				}
			));

			obj.set_has_unread(!notifd.notifications().is_empty());
			notifd.connect_notify_local(
				None,
				clone!(
					#[weak]
					obj,
					move |notifd, _| {
						obj.set_has_unread(notifd.notifications().len() > 0);
					}
				),
			);

			obj.bind_property("has-unread", &*obj, "bell-icon-name")
				.transform_to(|_, has_unread: bool| {
					Some(if has_unread {
						Icon::BellRingFilled.name()
					} else {
						Icon::Bell.name()
					})
				})
				.build();
		}
	}

	impl WidgetImpl for Notifications {}
	impl ButtonImpl for Notifications {}
}
