use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use astal4::prelude::*;
use astal_notifd::prelude::*;
use astal_notifd::{Notifd, Notification, Urgency};
use gtk4::prelude::{BoxExt, DisplayExt, GtkWindowExt, ListModelExt, WidgetExt};

use crate::popups::notifications::NotificationItem;

const TOAST_TOP_OFFSET: i32 = 68;
const TOAST_SIDE_MARGIN: i32 = 12;
const TOAST_WINDOW_WIDTH: i32 = 380;
const DEFAULT_TIMEOUT_SECONDS: u32 = 10;

pub struct NotificationsOverlay {
	pub window: astal4::Window,
	stack: gtk4::Box,
	rows: RefCell<HashMap<u32, gtk4::Widget>>,
}

impl NotificationsOverlay {
	pub fn new_primary(display: &gtk4::gdk::Display) -> Option<Rc<Self>> {
		let monitor_index = primary_monitor_index(display)?;

		let stack = gtk4::Box::builder()
			.orientation(gtk4::Orientation::Vertical)
			.spacing(8)
			.halign(gtk4::Align::End)
			.valign(gtk4::Align::Start)
			.css_classes(["notification-toast-stack"])
			.build();

		let window = astal4::Window::builder()
			.layer(astal4::Layer::Top)
			.anchor(astal4::WindowAnchor::TOP | astal4::WindowAnchor::RIGHT)
			.exclusivity(astal4::Exclusivity::Ignore)
			.keymode(astal4::Keymode::None)
			.can_target(true)
			.sensitive(true)
			.focus_on_click(false)
			.monitor(monitor_index)
			.name("notification-toasts")
			.namespace("notification-toasts")
			.margin_top(TOAST_TOP_OFFSET)
			.margin_right(TOAST_SIDE_MARGIN)
			.width_request(TOAST_WINDOW_WIDTH)
			.child(&stack)
			.css_classes(["notification-toast-window"])
			.visible(false)
			.build();

		let overlay = Rc::new(Self {
			window,
			stack,
			rows: RefCell::new(HashMap::new()),
		});

		overlay.bind_notifications();
		overlay.window.present();

		Some(overlay)
	}

	fn bind_notifications(self: &Rc<Self>) {
		let notifd = Notifd::default();
		self.sync_notifications(&notifd);
		let overlay = Rc::clone(self);

		notifd.connect_notify_local(None, move |notifd, _| {
			overlay.sync_notifications(notifd);
		});
	}

	fn sync_notifications(&self, notifd: &Notifd) {
		let notifications = notifd.notifications();
		let mut next_ids = HashMap::new();

		for notification in notifications {
			if notification.urgency() == Urgency::Low {
				continue;
			}

			let id = notification.id();
			next_ids.insert(id, notification);
		}

		let current_ids: Vec<u32> = self.rows.borrow().keys().copied().collect();
		for id in current_ids {
			if !next_ids.contains_key(&id)
				&& let Some(widget) = self.rows.borrow_mut().remove(&id)
			{
				self.stack.remove(&widget);
			}
		}

		let mut ordered_ids: Vec<u32> = next_ids.keys().copied().collect();
		ordered_ids.sort_unstable();
		ordered_ids.reverse();

		for id in ordered_ids {
			if let Some(notification) = next_ids.get(&id)
				&& !self.rows.borrow().contains_key(&id)
			{
				self.add_toast(notification);
			}
		}

		self.update_window_visibility();
	}

	fn add_toast(&self, notification: &Notification) {
		let id = notification.id();
		let notification_for_resolved = notification.clone();
		let urgency = notification.urgency();
		let item = NotificationItem::new(notification);
		item.add_css_class("notification-toast");
		item.add_css_class("notification-level-toast");

		let widget = item.upcast::<gtk4::Widget>();
		self.rows.borrow_mut().insert(id, widget.clone());
		self.stack.prepend(&widget);
		self.update_window_visibility();

		let rows = self.rows.clone();
		let stack = self.stack.clone();
		let window = self.window.clone();
		let toast_widget = widget.clone();
		notification_for_resolved.connect_resolved(move |_, _| {
			rows.borrow_mut().remove(&id);
			if let Some(parent) = toast_widget.parent().and_downcast::<gtk4::Box>() {
				parent.remove(&toast_widget);
			}
			window.set_visible(stack.first_child().is_some());
		});

		if urgency != Urgency::Critical {
			let timeout_ms = notification.expire_timeout();
			let timeout_seconds = if timeout_ms > 0 {
				(timeout_ms as u32).div_ceil(1000)
			} else {
				DEFAULT_TIMEOUT_SECONDS
			};

			let stack = self.stack.clone();
			let window = self.window.clone();
			let toast_widget = widget.clone();

			glib::timeout_add_seconds_local_once(timeout_seconds, move || {
				if let Some(parent) = toast_widget.parent().and_downcast::<gtk4::Box>() {
					parent.remove(&toast_widget);
				}
				window.set_visible(stack.first_child().is_some());
			});
		}
	}

	fn update_window_visibility(&self) {
		self.window.set_visible(self.stack.first_child().is_some());
	}
}

fn primary_monitor_index(display: &gtk4::gdk::Display) -> Option<i32> {
	if display.monitors().n_items() > 0 {
		Some(0)
	} else {
		None
	}
}
