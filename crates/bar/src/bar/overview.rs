use glib::clone;
use gtk4::Image;
use gtk4::prelude::*;

use crate::popups::launcher::LauncherPopup;

// const NIXOS_ICON: &[u8] = include_bytes!("./NixOS.png");

pub struct Overview {
	widget: gtk4::Button,
}

impl Overview {
	pub fn new() -> Self {
		let image = Image::from_icon_name(icons::Icon::Nixos.name());
		image.set_pixel_size(24);

		let button = gtk4::Button::builder()
			.width_request(24)
			.height_request(24)
			.child(&image)
			.build();

		let launcher = LauncherPopup::new();
		launcher.set_parent(&button);

		button.connect_clicked(clone!(
			#[weak]
			launcher,
			move |_| launcher.popup()
		));

		Self { widget: button }
	}

	pub fn widget(&self) -> &gtk4::Widget {
		self.widget.upcast_ref()
	}
}
