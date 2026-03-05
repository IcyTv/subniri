use astal_io::Time;
use astal_io::prelude::TimeExt;
use gtk4::glib::object::Cast;
use gtk4::glib::{DateTime, clone};
use gtk4::prelude::*;

use crate::icons;
use crate::popups::clock::ClockPopup;

pub struct Clock {
	container: gtk4::Widget,
}

impl Clock {
	pub fn new() -> Self {
		let button = gtk4::Button::builder()
			.name("clock")
			.css_classes(["clock", "bar-button"])
			.build();
		let container = gtk4::Box::builder()
			.name("clock")
			.spacing(6)
			.orientation(gtk4::Orientation::Horizontal)
			.build();
		let calendar_icon = gtk4::Image::from_icon_name(icons::Icon::Calendar.name());
		let date_label = gtk4::Label::builder().build();
		let clock_icon = gtk4::Image::from_icon_name(icons::Icon::Clock.name());
		let time_label = gtk4::Label::builder().build();

		container.append(&calendar_icon);
		container.append(&date_label);
		container.append(&clock_icon);
		container.append(&time_label);

		button.set_child(Some(&container));

		let timer = Time::interval(1000, None);

		let popup = ClockPopup::new(&timer);
		popup.set_parent(&button);

		button.connect_clicked(glib::clone!(
			#[weak]
			popup,
			move |_| {
				popup.popup();
			}
		));

		timer.connect_now(clone!(
			#[weak]
			date_label,
			move |_| {
				let Ok(time) = DateTime::now_local() else { return };
				let Ok(date) = time.format("%a %b %d") else { return };

				date_label.set_markup(&date);

				let Ok(time) = time.format("%H:%M") else { return };
				time_label.set_markup(&time);
			}
		));

		Self {
			container: button.upcast(),
		}
	}

	pub fn widget(&self) -> &gtk4::Widget {
		self.container.upcast_ref()
	}
}
