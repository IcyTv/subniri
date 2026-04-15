use std::cell::RefCell;
use std::rc::Rc;

use astal_network::prelude::NetworkExt;
use astal_network::{Primary, Wired};
use glib::clone;
use gtk4::PropertyExpression;
use gtk4::prelude::*;

pub struct Network {
	widget: gtk4::Button,
}

impl Network {
	pub fn new() -> Self {
		let button_box = gtk4::Box::builder()
			.name("network")
			.orientation(gtk4::Orientation::Horizontal)
			.spacing(4)
			.build();

		let widget = gtk4::Button::builder()
			.css_classes(["bar-button", "network-button"])
			.child(&button_box)
			.build();

		let nw = astal_network::Network::default();

		if let Some(nw) = nw {
			let icon = gtk4::Image::from_icon_name(icons::Icon::WifiOff.name());
			let label = gtk4::Label::builder().build();

			button_box.append(&icon);
			button_box.append(&label);

			nw.bind_property("primary", &icon, "icon-name")
				.transform_to(|_, primary: Primary| {
					let icon_name = match primary {
						Primary::Wifi => icons::Icon::Wifi.name(),
						Primary::Wired => icons::Icon::Network.name(),
						_ => icons::Icon::WifiOff.name(),
					};
					Some(icon_name)
				})
				.sync_create()
				.build();

			let current_binding = Rc::new(RefCell::new(None::<gtk4::ExpressionWatch>));

			let bind_network_name = clone!(
				#[weak]
				label,
				#[strong]
				current_binding,
				move |nw: &astal_network::Network| {
					let current = current_binding.borrow_mut().take();
					if let Some(binding) = current {
						binding.unwatch();
					}

					let binding = match (nw.primary(), nw.wired(), nw.wifi()) {
						(Primary::Wired, Some(wired), _) => {
							let device_expr =
								PropertyExpression::new(Wired::static_type(), gtk4::Expression::NONE, "device");
							let device_name_expr =
								PropertyExpression::new(nm_rs::Device::static_type(), Some(device_expr), "interface");

							let binding = device_name_expr.bind(&label, "label", Some(&wired));

							Some(binding)
						}
						(Primary::Wifi, _, Some(wifi)) => {
							let device_expr = PropertyExpression::new(
								astal_network::Wifi::static_type(),
								gtk4::Expression::NONE,
								"device",
							);
							let device_name_expr =
								PropertyExpression::new(nm_rs::Device::static_type(), Some(device_expr), "interface");
							let binding = device_name_expr.bind(&label, "label", Some(&wifi));
							Some(binding)
						}
						_ => None,
					};

					*current_binding.borrow_mut() = binding;
				}
			);

			bind_network_name(&nw);

			nw.connect_notify_local(
				None,
				clone!(
					#[strong]
					bind_network_name,
					move |nw, _| {
						bind_network_name(nw);
					}
				),
			);
		}

		Self { widget }
	}

	pub fn widget(&self) -> &gtk4::Button {
		&self.widget
	}
}
