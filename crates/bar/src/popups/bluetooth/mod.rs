mod device;

use std::cell::RefCell;

use astal_bluetooth::Bluetooth;
use glib::{Properties, clone};
use gtk4::gio::ListStore;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{CompositeTemplate, SignalListItemFactory};

glib::wrapper! {
	pub struct BluetoothPopup(ObjectSubclass<imp::BluetoothPopup>)
		@extends gtk4::Popover, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Buildable, gtk4::Constraint, gtk4::ConstraintTarget, gtk4::ShortcutManager, gtk4::Native;
}

impl BluetoothPopup {
	pub fn new() -> Self {
		glib::Object::builder().build()
	}
}

mod imp {

	use astal_bluetooth::prelude::{AdapterExt, BluetoothExt, DeviceExt};
	use gtk4::SortListModel;

	use super::*;

	#[derive(Default, Clone, Copy)]
	struct AdapterState {
		discoverable: bool,
		pairable:     bool,
		discovering:  bool,
	}

	#[derive(Default, Properties, CompositeTemplate)]
	#[template(file = "./src/popups/bluetooth/bluetooth.blp")]
	#[properties(wrapper_type = super::BluetoothPopup)]
	pub struct BluetoothPopup {
		#[template_child]
		list_view:      TemplateChild<gtk4::ListView>,
		previous_state: RefCell<Option<AdapterState>>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for BluetoothPopup {
		type ParentType = gtk4::Popover;
		type Type = super::BluetoothPopup;

		const NAME: &'static str = "BluetoothPopup";

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
			// klass.bind_template_callbacks();
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for BluetoothPopup {
		fn constructed(&self) {
			self.parent_constructed();
			let obj = self.obj();

			let bt = Bluetooth::default();

			let model = ListStore::builder().build();
			let factory = SignalListItemFactory::new();

			factory.connect_setup(move |_, _item| {});
			factory.connect_bind(move |_, item| {
				let list_item = item
					.downcast_ref::<gtk4::ListItem>()
					.expect("The item is not a ListItem");

				let bt_device = list_item.item().and_downcast::<device::BluetoothDevice>().unwrap();
				list_item.set_child(Some(&bt_device));
			});
			factory.connect_unbind(move |_, item| {
				let list_item = item
					.downcast_ref::<gtk4::ListItem>()
					.expect("The item is not a ListItem");
				list_item.set_child(None::<&gtk4::Widget>);
			});

			let add_devices = clone!(
				#[weak]
				model,
				move |bt: &Bluetooth| {
					model.remove_all();
					for device in bt.devices() {
						let item = device::BluetoothDevice::new(&device);
						model.append(&item);
					}
				}
			);

			add_devices(&bt);
			bt.connect_device_added(clone!(
				#[weak]
				model,
				move |_bt, device| {
					let item = device::BluetoothDevice::new(device);
					model.append(&item);
				}
			));
			bt.connect_device_removed(clone!(
				#[weak]
				model,
				move |_bt, device| {
					for i in 0..model.n_items() {
						let item = model.item(i).unwrap();
						let bt_device = item
							.downcast_ref::<device::BluetoothDevice>()
							.expect("Item is not a BluetoothDevice");
						if bt_device.address() == device.address() {
							model.remove(i);
							break;
						}
					}
				}
			));

			let sort_model = SortListModel::builder().model(&model).build();

			let selection_model = gtk4::NoSelection::new(Some(sort_model));

			self.list_view.set_model(Some(&selection_model));
			self.list_view.set_factory(Some(&factory));

			// obj.bind_property("visible", &bt, "")
			obj.connect_notify_local(
				Some("visible"),
				clone!(
					#[weak]
					bt,
					move |obj, _| {
						if let Some(adapter) = bt.adapter() {
							let imp = obj.imp();
							if obj.is_visible() {
								// Store previous state
								let state = AdapterState {
									discoverable: adapter.is_discoverable(),
									pairable:     adapter.is_pairable(),
									discovering:  adapter.is_discovering(),
								};
								*imp.previous_state.borrow_mut() = Some(state);

								let _ = adapter.start_discovery();
								adapter.set_discoverable(true);
								adapter.set_pairable(true);
							} else {
								// Restore previous state
								if let Some(state) = *imp.previous_state.borrow() {
									adapter.set_discoverable(state.discoverable);
									adapter.set_pairable(state.pairable);
									if !state.discovering {
										let _ = adapter.stop_discovery();
									}
								}

								*imp.previous_state.borrow_mut() = None;
							}
						}
					}
				),
			);
		}
	}

	impl WidgetImpl for BluetoothPopup {}
	impl PopoverImpl for BluetoothPopup {}
}
