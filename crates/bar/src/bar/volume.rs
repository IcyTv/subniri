use std::cell::RefCell;
use std::rc::Rc;

use astal_wp::{Audio, Wp};
use glib::clone;
use glib::object::ObjectExt;
use gtk4::prelude::*;

use crate::popups::volume::VolumePopup;

pub struct Volume {
	widget: gtk4::Button,
}

impl Volume {
	pub fn new() -> Self {
		let button_box = gtk4::Box::builder()
			.name("wireplumber")
			.orientation(gtk4::Orientation::Horizontal)
			.spacing(4)
			.build();

		let volume_icon = gtk4::Image::from_icon_name(icons::Icon::VolumeOff.name());
		volume_icon.set_pixel_size(24);
		let label = gtk4::Label::builder().label("0%").build();

		button_box.append(&volume_icon);
		button_box.append(&label);

		let widget = gtk4::Button::builder()
			.css_classes(["volume", "bar-button"])
			.child(&button_box)
			.build();

		let wp = Wp::default();
		let audio = wp.audio();

		let current_binding = Rc::new(RefCell::new(None::<glib::Binding>));
		let current_icon_binding = Rc::new(RefCell::new(None::<glib::Binding>));

		let popup = VolumePopup::new();
		popup.set_parent(&widget);

		widget.connect_clicked(clone!(
			#[weak]
			popup,
			move |_| {
				popup.popup();
			}
		));

		let changed_default_speaker = clone!(
			#[strong]
			current_binding,
			#[weak]
			label,
			#[weak]
			volume_icon,
			move |audio: &Audio| {
				if let Some(b) = current_binding.borrow_mut().take() {
					b.unbind();
				}
				if let Some(b) = current_icon_binding.borrow_mut().take() {
					b.unbind();
				}

				let speaker = audio.default_speaker();

				let binding = speaker
					.bind_property("volume", &label, "label")
					.transform_to(|_, volume: f64| Some(format!("{:.0}%", volume * 100.0)))
					.sync_create()
					.build();
				let icon_binding = speaker
					.bind_property("volume", &volume_icon, "icon-name")
					.transform_to(|_, volume: f64| {
						let icon_name = match volume {
							0.0 => icons::Icon::VolumeX,
							0.0..=0.2 => icons::Icon::Volume,
							0.2..=0.7 => icons::Icon::Volume1,
							_ => icons::Icon::Volume2,
						}
						.name();
						Some(icon_name.to_string())
					})
					.sync_create()
					.build();

				*current_binding.borrow_mut() = Some(binding);
				*current_icon_binding.borrow_mut() = Some(icon_binding);
			}
		);

		changed_default_speaker(&audio);

		audio.connect_notify_local(Some("default-speaker"), move |audio, _| changed_default_speaker(audio));

		Self { widget }
	}

	pub fn widget(&self) -> &gtk4::Button {
		&self.widget
	}
}
