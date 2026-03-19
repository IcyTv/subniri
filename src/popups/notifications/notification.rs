use std::cell::RefCell;

use astal_notifd::prelude::*;
use astal_notifd::Notifd;
use astal_tray::prelude::*;
use astal_tray::{Tray, TrayItem};
use glib::{clone, Properties};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gio, CompositeTemplate};

use super::item::NotificationItem;

glib::wrapper! {
	pub struct NotificationsPopup(ObjectSubclass<imp::NotificationsPopup>)
		@extends gtk4::Popover, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Buildable, gtk4::Constraint, gtk4::ConstraintTarget, gtk4::ShortcutManager, gtk4::Native;
}

impl NotificationsPopup {
	pub fn new() -> Self {
		glib::Object::builder().build()
	}
}

mod imp {
	use super::*;

	#[derive(Default, Properties, CompositeTemplate)]
	#[template(file = "./src/popups/notifications/notification.blp")]
	#[properties(wrapper_type = super::NotificationsPopup)]
	pub struct NotificationsPopup {
		#[template_child]
		notifications_list_view: gtk4::TemplateChild<gtk4::ListView>,
		#[template_child]
		empty_state: gtk4::TemplateChild<gtk4::Box>,
		#[template_child]
		tray_flow: gtk4::TemplateChild<gtk4::FlowBox>,
		#[template_child]
		tray_menu_box: gtk4::TemplateChild<gtk4::Box>,

		notification_store: RefCell<Option<gio::ListStore>>,
		inline_tray_menu: RefCell<Option<gtk4::PopoverMenu>>,
	}

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

			self.tray_flow.set_selection_mode(gtk4::SelectionMode::None);

			let store = gio::ListStore::new::<NotificationItem>();
			self.notification_store.replace(Some(store.clone()));

			let factory = gtk4::SignalListItemFactory::new();
			factory.connect_bind(|_, item| {
				let item = item.downcast_ref::<gtk4::ListItem>().expect("ListItem");
				if let Some(notification_item) = item.item().and_downcast::<NotificationItem>() {
					item.set_child(Some(&notification_item));
				}
			});
			factory.connect_unbind(|_, item| {
				let item = item.downcast_ref::<gtk4::ListItem>().expect("ListItem");
				item.set_child(None::<&gtk4::Widget>);
			});

			let selection_model = gtk4::NoSelection::new(Some(store.clone()));
			self.notifications_list_view.set_model(Some(&selection_model));
			self.notifications_list_view.set_factory(Some(&factory));

			let notifd = Notifd::default();
			let tray = Tray::default();

			self.refresh_notifications(&notifd);
			self.rebuild_tray_items(&tray);

			notifd.connect_notify_local(
				Some("notifications"),
				clone!(
					#[weak(rename_to = imp)]
					self,
					move |notifd, _| {
						imp.refresh_notifications(notifd);
					}
				),
			);

			tray.connect_item_added(clone!(
				#[weak(rename_to = imp)]
				self,
				#[weak]
				tray,
				move |_, _| {
					imp.rebuild_tray_items(&tray);
				}
			));
			tray.connect_item_removed(clone!(
				#[weak(rename_to = imp)]
				self,
				#[weak]
				tray,
				move |_, _| {
					imp.rebuild_tray_items(&tray);
				}
			));
		}
	}

	impl WidgetImpl for NotificationsPopup {}
	impl PopoverImpl for NotificationsPopup {}

	#[gtk4::template_callbacks]
	impl NotificationsPopup {
		#[template_callback]
		fn on_clear_all(&self) {
			let notifd = Notifd::default();
			for notification in notifd.notifications() {
				notification.dismiss();
			}
		}
	}

	impl NotificationsPopup {
		fn clear_inline_tray_menu(&self) {
			if let Some(menu) = self.inline_tray_menu.borrow_mut().take() {
				menu.popdown();
				self.tray_menu_box.remove(&menu);
			}
			self.tray_menu_box.set_visible(false);
		}

		fn set_inline_menu_anchor_from_button(&self, button: &gtk4::Button) {
			if let Some(rect) = button.compute_bounds(&self.tray_flow.get()) {
				let center_x = rect.x() + (rect.width() / 2.0);
				let menu_width = self.tray_menu_box.width() as f32;
				let menu_margin = (center_x - (menu_width / 2.0)).max(0.0).round() as i32;
				self.tray_menu_box.set_margin_start(menu_margin);
			}
		}

		fn event_position_in_root(widget: &impl IsA<gtk4::Widget>, x: f64, y: f64) -> (i32, i32) {
			let root_pos = widget
				.root()
				.and_downcast::<gtk4::Window>()
				.and_then(|root| widget.compute_point(&root, &gtk4::graphene::Point::new(x as f32, y as f32)))
				.map(|point| (point.x().round() as i32, point.y().round() as i32));

			if let Some(position) = root_pos {
				position
			} else {
				(widget.width() / 2, widget.height() / 2)
			}
		}

		fn refresh_notifications(&self, notifd: &Notifd) {
			let notifications = notifd.notifications();
			if let Some(store) = self.notification_store.borrow().as_ref() {
				store.remove_all();
				for notification in notifications {
					let row = NotificationItem::new(&notification);
					store.append(&row);
				}
			}

			self.empty_state.set_visible(notifd.notifications().is_empty());
		}

		fn rebuild_tray_items(&self, tray: &Tray) {
			while let Some(child) = self.tray_flow.first_child() {
				self.tray_flow.remove(&child);
			}

			for item in tray.items() {
				let widget = self.build_tray_widget(&item);
				self.tray_flow.insert(&widget, -1);
			}
		}

		fn build_tray_widget(&self, item: &TrayItem) -> gtk4::Widget {
			let button = gtk4::Button::builder()
				.css_classes(["tray-item"])
				.valign(gtk4::Align::Center)
				.build();

			let image = gtk4::Image::builder().pixel_size(16).build();
			image.set_from_gicon(&item.gicon());
			button.set_child(Some(&image));

			let tooltip = item.tooltip_text();
			if !tooltip.is_empty() {
				button.set_tooltip_text(Some(tooltip.as_str()));
			}

			if let Some(menu_model) = item.menu_model() {
				let action_group = item.action_group();
				let anchor_button = button.clone();

				button.connect_clicked(clone!(
					#[strong]
					item,
					#[strong]
					menu_model,
					#[strong]
					action_group,
					#[weak(rename_to = imp)]
					self,
					move |_| {
						item.about_to_show();

						imp.clear_inline_tray_menu();
						imp.set_inline_menu_anchor_from_button(&anchor_button);

						let menu = gtk4::PopoverMenu::from_model(Some(&menu_model));
						menu.set_has_arrow(false);

						if let Some(action_group) = action_group.as_ref() {
							menu.insert_action_group("dbusmenu", Some(action_group));
						}

						imp.tray_menu_box.append(&menu);
						imp.tray_menu_box.set_visible(true);
						menu.popup();
						imp.inline_tray_menu.replace(Some(menu));
					}
				));

				let secondary_click = gtk4::GestureClick::new();
				secondary_click.set_button(3);
				secondary_click.connect_released(clone!(
					#[weak]
					button,
					move |_, _, _, _| {
						button.emit_clicked();
					}
				));
				button.add_controller(secondary_click);
			} else {
				let weak_imp_left = self.downgrade();
				button.connect_clicked(clone!(
					#[strong]
					item,
					#[weak]
					button,
					move |_| {
						if let Some(imp) = weak_imp_left.upgrade() {
							imp.clear_inline_tray_menu();
						}
						let (x, y) = Self::widget_center_in_root(&button);
						item.activate(x, y);
						item.secondary_activate(x, y);
					}
				));

				let weak_imp_right = self.downgrade();
				let secondary_click = gtk4::GestureClick::new();
				secondary_click.set_button(3);
				secondary_click.connect_released(clone!(
					#[strong]
					item,
					#[weak]
					button,
					move |_, _, x, y| {
						if let Some(imp) = weak_imp_right.upgrade() {
							imp.clear_inline_tray_menu();
						}
						let (x, y) = Self::event_position_in_root(&button, x, y);
						item.secondary_activate(x, y);
					}
				));
				button.add_controller(secondary_click);
			}

			button.upcast()
		}

		fn widget_center_in_root(widget: &impl IsA<gtk4::Widget>) -> (i32, i32) {
			let center_x = (widget.width() / 2) as f64;
			let center_y = (widget.height() / 2) as f64;
			Self::event_position_in_root(widget, center_x, center_y)
		}
	}
}
