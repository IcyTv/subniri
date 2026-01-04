use std::cell::RefCell;

use astal_bluetooth::Device;
use astal_bluetooth::prelude::*;
use glib::{Properties, clone};
use gtk4::CompositeTemplate;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::icons::Icon;

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
	use std::ffi::c_void;

	use astal_bluetooth::prelude::DeviceExt;
	use async_channel::Sender;
	use glib::ffi::GError;
	use glib::gobject_ffi::GObject;
	use glib::translate::ToGlibPtr;
	use gtk4::gio::ffi::{GAsyncReadyCallback, GAsyncResult};

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
							match device.connect_device().await {
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
							match device.disconnect_device().await {
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

	trait BtDeviceExt {
		async fn connect_device(&self) -> Result<(), Box<dyn std::error::Error>>;
		async fn disconnect_device(&self) -> Result<(), Box<dyn std::error::Error>>;
	}

	struct ContextData {
		tx:      Sender<ConnectionResult>,
		finish:
			unsafe extern "C" fn(*mut astal_bluetooth_sys::AstalBluetoothDevice, *mut GAsyncResult, *mut *mut GError),
		_device: Device,
	}

	enum ConnectionResult {
		Success,
		Error(String),
	}

	impl BtDeviceExt for Device {
		async fn connect_device(&self) -> Result<(), Box<dyn std::error::Error>> {
			let callback = GAsyncReadyCallback::Some(callback);

			let (tx, rx) = async_channel::bounded(1);
			let user_data = Box::new(ContextData {
				tx,
				finish: astal_bluetooth_sys::astal_bluetooth_device_connect_device_finish,
				_device: self.clone(),
			});
			let user_data_ptr = Box::into_raw(user_data);

			// SAFETY: the strong ref stored in ContextData keeps the device alive until the callback
			// runs; the borrowed pointer returned by to_glib_none() is valid for this call.
			unsafe {
				astal_bluetooth_sys::astal_bluetooth_device_connect_device(
					self.to_glib_none().0,
					callback,
					user_data_ptr as *mut c_void,
				);
			}

			match rx.recv().await {
				Ok(ConnectionResult::Success) => Ok(()),
				Ok(ConnectionResult::Error(msg)) => Err(Box::new(std::io::Error::other(msg))),
				Err(e) => Err(Box::new(e)),
			}
		}

		async fn disconnect_device(&self) -> Result<(), Box<dyn std::error::Error>> {
			let callback = GAsyncReadyCallback::Some(callback);

			let (tx, rx) = async_channel::bounded(1);
			let user_data = Box::new(ContextData {
				tx,
				finish: astal_bluetooth_sys::astal_bluetooth_device_disconnect_device_finish,
				_device: self.clone(),
			});
			let user_data_ptr = Box::into_raw(user_data);

			unsafe {
				astal_bluetooth_sys::astal_bluetooth_device_disconnect_device(
					self.to_glib_none().0,
					callback,
					user_data_ptr as *mut c_void,
				);
			}

			match rx.recv().await {
				Ok(ConnectionResult::Success) => Ok(()),
				Ok(ConnectionResult::Error(msg)) => Err(Box::new(std::io::Error::other(msg))),
				Err(e) => Err(Box::new(e)),
			}
		}
	}

	unsafe extern "C" fn callback(source_object: *mut GObject, result: *mut GAsyncResult, user_data: *mut c_void) {
		if user_data.is_null() {
			eprintln!("User data is null in on_connect");
			return;
		}

		println!("on_connect called");

		// SAFETY: We allocated this with Box::into_raw() and GIO guarantees the callback
		// is invoked exactly once (see GAsyncReadyCallback documentation in gio(3)).
		// Taking ownership here ensures proper cleanup.
		let user_data = unsafe { Box::from_raw(user_data as *mut ContextData) };

		let finish = user_data.finish;

		let mut error: *mut GError = std::ptr::null_mut();

		// SAFETY: Calling the finish function for the async operation with valid pointers.
		// - source_object is the same AstalBluetoothDevice* passed to the original async call,
		//   guaranteed by GIO to be passed back to the callback (see GAsyncReadyCallback in gio(3))
		// - result is a valid GAsyncResult* provided by GIO
		// - error is an out-parameter that will be set if the operation failed
		unsafe {
			finish(
				source_object as *mut astal_bluetooth_sys::AstalBluetoothDevice,
				result,
				(&mut error) as *mut *mut GError,
			);
		}

		if error.is_null() {
			let _ = user_data.tx.send_blocking(ConnectionResult::Success);
		} else {
			// SAFETY: error is a valid GError pointer set by the finish function.
			// We must read the message and then free the error.
			let message = unsafe {
				let c_str = std::ffi::CStr::from_ptr((*error).message);
				let message = c_str.to_string_lossy().into_owned();
				// Free the GError as per GLib memory management (see g_error_free in glib(3))
				glib::ffi::g_error_free(error);
				message
			};

			let _ = user_data.tx.send_blocking(ConnectionResult::Error(message));
		}
	}
}
