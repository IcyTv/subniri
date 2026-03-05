use std::cell::RefCell;

use astal_wp::prelude::*;
use astal_wp::{Direction, Endpoint};
use glib::{GString, Properties};
use gtk4::CompositeTemplate;
use gtk4::subclass::prelude::*;

use crate::icons::Icon;

glib::wrapper! {
	pub struct DeviceWidget(ObjectSubclass<imp::DeviceWidget>)
		@extends gtk4::ToggleButton, gtk4::Button, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Actionable, gtk4::Buildable, gtk4::Constraint, gtk4::ConstraintTarget;
}

impl DeviceWidget {
	pub fn new(device: &Endpoint) -> Self {
		glib::Object::builder()
			.property("endpoint", Some(device.clone()))
			.build()
	}
}

mod imp {

	use super::*;

	#[derive(Default, Properties, CompositeTemplate)]
	#[template(file = "./src/popups/volume/device.blp")]
	#[properties(wrapper_type = super::DeviceWidget)]
	pub struct DeviceWidget {
		#[property(get, construct_only)]
		pub endpoint: RefCell<Option<Endpoint>>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for DeviceWidget {
		type ParentType = gtk4::ToggleButton;
		type Type = super::DeviceWidget;

		const NAME: &'static str = "DeviceWidget";

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
			klass.bind_template_callbacks();
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for DeviceWidget {
		fn constructed(&self) {
			self.parent_constructed();

			let endpoint = self.endpoint.borrow();
			let endpoint = match &*endpoint {
				Some(ep) => ep,
				None => return,
			};
			let obj = self.obj();

			endpoint
				.bind_property("is-default", &*obj, "active")
				.bidirectional()
				.sync_create()
				.build();
		}
	}

	impl WidgetImpl for DeviceWidget {}
	impl ButtonImpl for DeviceWidget {}
	impl ToggleButtonImpl for DeviceWidget {}

	#[gtk4::template_callbacks]
	impl DeviceWidget {
		#[template_callback]
		fn to_icon(&self, direction: Direction) -> GString {
			match direction {
				Direction::Input => Icon::Mic.name().into(),
				Direction::Output => Icon::Volume2.name().into(),
				_ => Icon::VolumeOff.name().into(),
			}
		}
	}
}
