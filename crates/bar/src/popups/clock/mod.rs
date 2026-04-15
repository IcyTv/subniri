mod weather;

use std::cell::RefCell;

use astal_io::Time;
use astal_io::prelude::*;
use glib::{DateTime, Properties, clone};
use gtk4::CompositeTemplate;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use icons::Icon;

glib::wrapper! {
	pub struct ClockPopup(ObjectSubclass<imp::ClockPopup>)
		@extends gtk4::Popover, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Buildable, gtk4::Constraint, gtk4::ConstraintTarget, gtk4::ShortcutManager, gtk4::Native;
}

impl ClockPopup {
	pub fn new(time: &Time) -> Self {
		glib::Object::builder()
			.property("timer", time)
			.property("backwards", Icon::ChevronLeft.name())
			.property("forwards", Icon::ChevronRight.name())
			.build()
	}
}

mod imp {
	use super::*;

	#[derive(Default, Properties, CompositeTemplate)]
	#[template(file = "./src/popups/clock/clock.blp")]
	#[properties(wrapper_type = super::ClockPopup)]
	pub struct ClockPopup {
		#[property(get, set)]
		current_time: RefCell<String>,
		#[property(get, construct_only)]
		timer: RefCell<Time>,

		// Icons
		#[property(get, set)]
		backwards: RefCell<String>,
		#[property(get, set)]
		forwards: RefCell<String>,

		#[template_child]
		weather_box: TemplateChild<gtk4::Box>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for ClockPopup {
		type ParentType = gtk4::Popover;
		type Type = super::ClockPopup;

		const NAME: &'static str = "ClockPopup";

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
			klass.bind_template_callbacks();
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for ClockPopup {
		fn constructed(&self) {
			self.parent_constructed();

			let obj = self.obj();

			let timer = &*self.timer.borrow();
			timer.connect_now(clone!(
				#[weak]
				obj,
				move |_| {
					if let Ok(time) = DateTime::now_local() {
						let formatted = time.format("%T").unwrap_or_else(|_| "00:00:00".into());
						obj.set_current_time(formatted);
					} else {
						obj.set_current_time("00:00:00");
					}
				}
			));

			let weather_display = weather::WeatherDisplay::new();
			self.weather_box.append(&weather_display);
		}
	}

	impl WidgetImpl for ClockPopup {}
	impl PopoverImpl for ClockPopup {}

	#[gtk4::template_callbacks]
	impl ClockPopup {}
}
