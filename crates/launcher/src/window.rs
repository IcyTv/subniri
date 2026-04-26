use astal4::{Exclusivity, Keymode, Layer, Window, WindowAnchor};
use glib::{Propagation, clone};
use gtk4::prelude::BoxExt as _;
use gtk4::{EventControllerKey, GestureClick, PickFlags, prelude::*};

use crate::launcher::LauncherWidget;

pub struct MainLauncherWindow {
	pub window: Window,
	pub launcher_widget: LauncherWidget,
}

impl MainLauncherWindow {
	pub fn new() -> Self {
		let launcher_widget = LauncherWidget::new();

		let center_box = gtk4::Box::builder()
			.halign(gtk4::Align::Center)
			.valign(gtk4::Align::Center)
			.css_classes(["backdrop"])
			.hexpand(true)
			.vexpand(true)
			.build();
		center_box.append(&launcher_widget);

		let main = gtk4::Box::builder()
			.halign(gtk4::Align::Fill)
			.valign(gtk4::Align::Fill)
			.vexpand(true)
			.hexpand(true)
			.build();
		main.append(&center_box);

		let window = Window::builder()
			.layer(Layer::Overlay)
			.anchor(WindowAnchor::all())
			.exclusivity(Exclusivity::Exclusive)
			.keymode(Keymode::Exclusive)
			.can_target(true)
			.can_focus(true)
			.sensitive(true)
			.name("avalaunch")
			.namespace("subniri-launcher")
			.css_classes(["avalaunch-window", "avalaunch"])
			.child(&main)
			.build();

		let gesture = GestureClick::new();
		gesture.connect_pressed(clone!(
			#[weak]
			main,
			#[weak]
			window,
			move |_, _, x, y| {
				let picked = main.pick(x, y, PickFlags::DEFAULT);

				if picked.as_ref().map_or(false, |picked| *picked == main) {
					window.set_visible(false);
				}
			}
		));

		main.add_controller(gesture);

		let key_event = EventControllerKey::builder().build();

		key_event.connect_key_pressed(clone!(
			#[weak_allow_none]
			window,
			move |_, val, _, _| {
				if val == gtk4::gdk::Key::Escape {
					if let Some(window) = window {
						window.set_visible(false);
						return Propagation::Stop;
					}
				}
				return Propagation::Proceed;
			}
		));
		window.add_controller(key_event);

		Self {
			window,
			launcher_widget,
		}
	}
}
