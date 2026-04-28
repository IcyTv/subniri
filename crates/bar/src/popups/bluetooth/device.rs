use std::cell::RefCell;

use astal_bluetooth::Device;
use astal_bluetooth::prelude::*;
use glib::{Properties, clone};
use gtk4::CompositeTemplate;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use icons::Icon;

glib::wrapper! {
	pub struct BluetoothDevice(ObjectSubclass<imp::BluetoothDevice>)
		@extends gtk4::Button, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Actionable, gtk4::Buildable, gtk4::Constraint, gtk4::ConstraintTarget;
}

impl BluetoothDevice {
	pub fn new(device: &Device) -> Self {
		glib::Object::builder().property("device", device.clone()).build()
	}
}

mod imp {
	use astal_bluetooth::prelude::DeviceExt;

	use super::*;

	#[derive(Default, Properties, CompositeTemplate)]
	#[template(file = "./src/popups/bluetooth/device.blp")]
	#[properties(wrapper_type = super::BluetoothDevice)]
	pub struct BluetoothDevice {
		#[property(get, construct_only)]
		device: RefCell<Option<Device>>,

		#[property(get, set)]
		address: RefCell<String>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for BluetoothDevice {
		type ParentType = gtk4::Button;
		type Type = super::BluetoothDevice;

		const NAME: &'static str = "BluetoothDevice";

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
			klass.bind_template_callbacks();
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for BluetoothDevice {
		fn constructed(&self) {
			self.parent_constructed();

			let device = self.device.borrow();
			let device = match &*device {
				Some(d) => d,
				None => {
					println!("Device is None in constructed()!");
					return;
				}
			};

			let obj = self.obj();
			device.bind_property("address", &*obj, "address").sync_create().build();

			// Update CSS class based on connection state
			let update_style = clone!(
				#[weak]
				obj,
				move |device: &Device| {
					obj.remove_css_class("connected");
					obj.remove_css_class("connecting");

					if device.is_connected() {
						obj.add_css_class("connected");
					} else if device.is_connecting() {
						obj.add_css_class("connecting");
					}
				}
			);

			// Set initial state
			update_style(device);

			// Update when connection state changes
			device.connect_connected_notify(clone!(
				#[strong]
				update_style,
				move |device| {
					update_style(device);
				}
			));
			device.connect_connecting_notify(move |device| {
				update_style(device);
			});
		}
	}

	impl WidgetImpl for BluetoothDevice {}
	impl ButtonImpl for BluetoothDevice {
		fn clicked(&self) {
			let device = self.device.borrow();
			if let Some(device) = device.as_ref() {
				let device_is_connected = device.is_connected() || device.is_connecting();

				if !device_is_connected {
					glib::spawn_future_local(clone!(
						#[weak]
						device,
						async move {
							match device.connect_device_future().await {
								Ok(_) => {
									println!("Connected to device {:?}", device);
								}
								Err(e) => {
									eprintln!("Failed to connect to device {:?}: {}", device, e);
								}
							}
						}
					));
				} else {
					let obj = self.obj();
					obj.remove_css_class("connected");

					glib::spawn_future_local(clone!(
						#[weak]
						device,
						#[weak]
						obj,
						async move {
							match device.disconnect_device_future().await {
								Ok(_) => {
									println!("Disconnected from device {:?}", device);
								}
								Err(e) => {
									eprintln!("Failed to disconnect from device {:?}: {}", device, e);
									// Restore connected state on error
									if device.is_connected() {
										obj.add_css_class("connected");
									}
								}
							}
						}
					));
				}
			}
		}
	}

	#[gtk4::template_callbacks]
	impl BluetoothDevice {
		#[template_callback]
		fn to_icon(&self, icon_name: Option<&str>) -> &'static str {
			match icon_name {
				Some("audio-headphones") => Icon::Headphones.name(),
				Some("audio-headset") => Icon::Headset.name(),
				_ => Icon::Bluetooth.name(),
			}
		}
	}
}
