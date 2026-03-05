use astal_bluetooth::Bluetooth as Bt;
use astal_bluetooth::prelude::{BluetoothExt, DeviceExt};
use glib::clone;
use gtk4::prelude::*;
use gtk4::{ClosureExpression, PropertyExpression, glib};

use crate::icons;
use crate::popups::bluetooth::BluetoothPopup;

pub struct Bluetooth {
	widget: gtk4::Button,
}

impl Bluetooth {
	pub fn new() -> Self {
		let bt = Bt::default();

		let button_box = gtk4::Box::builder()
			.orientation(gtk4::Orientation::Horizontal)
			.spacing(6)
			.build();

		let icon = gtk4::Image::from_icon_name(icons::Icon::BluetoothOff.name());
		let default_label = format!("{}", bt.devices().into_iter().filter(|d| d.is_connected()).count());
		let label = gtk4::Label::builder().label(default_label).build();

		button_box.append(&icon);
		button_box.append(&label);

		let button = gtk4::Button::builder()
			.child(&button_box)
			.css_classes(["bar-button", "bluetooth-button"])
			.build();

		let is_powered_expr = PropertyExpression::new(Bt::static_type(), gtk4::Expression::NONE, "is-powered");
		let is_connected_expr = PropertyExpression::new(Bt::static_type(), gtk4::Expression::NONE, "is-connected");

		let is_powered_and_connected = ClosureExpression::new::<glib::GString>(
			&[&is_powered_expr, &is_connected_expr],
			glib::closure!(
				|_: Option<glib::Object>, is_powered: bool, is_connected: bool| -> glib::GString {
					if is_connected {
						// TODO get proper connected icon
						icons::Icon::Bluetooth.name().into()
					} else if is_powered {
						icons::Icon::Bluetooth.name().into()
					} else {
						icons::Icon::BluetoothOff.name().into()
					}
				}
			),
		);
		is_powered_and_connected.bind(&icon, "icon-name", Some(&bt));

		bt.connect_devices_notify(clone!(
			#[weak]
			label,
			move |bt| {
				let string = format!("{}", bt.devices().into_iter().filter(|d| d.is_connected()).count());

				label.set_label(&string);
			}
		));

		let popup = BluetoothPopup::new();
		popup.set_parent(&button);

		button.connect_clicked(clone!(
			#[weak]
			popup,
			move |_| {
				popup.popup();
			}
		));

		Self { widget: button }
	}

	pub fn widget(&self) -> &gtk4::Button {
		&self.widget
	}
}
