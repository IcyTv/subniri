use gtk4::gdk;
use gtk4::prelude::{BoxExt as _, *};

mod bluetooth;
mod clock;
mod mediaplayer;
mod network;
mod notifications;
mod overview;
mod taskbar;
mod volume;

pub struct Bar {
	pub window: astal4::Window,
}

impl Bar {
	fn new(monitor_index: i32, monitor_width: i32) -> Self {
		let overview = overview::Overview::new();
		let taskbar = taskbar::Taskbar::new(monitor_index);
		let start_child = gtk4::Box::builder()
			.hexpand(true)
			.orientation(gtk4::Orientation::Horizontal)
			.spacing(8)
			.build();

		start_child.append(overview.widget());
		start_child.append(taskbar.widget());

		// let clock = clock::Clock::new();

		let mediaplayer = mediaplayer::MediaPlayerWidget::new();

		let volume = volume::Volume::new();
		let network = network::Network::new();
		let bluetooth = bluetooth::Bluetooth::new();
		let clock = clock::Clock::new();
		let notifications = notifications::Notifications::new();

		let end_box = gtk4::Box::builder()
			.hexpand(true)
			.orientation(gtk4::Orientation::Horizontal)
			.halign(gtk4::Align::End)
			.spacing(8)
			.build();

		end_box.append(volume.widget());
		end_box.append(network.widget());
		end_box.append(bluetooth.widget());
		end_box.append(clock.widget());
		end_box.append(&notifications);

		let center_box = gtk4::CenterBox::builder()
			.start_widget(&start_child)
			.center_widget(&mediaplayer)
			.end_widget(&end_box)
			.css_classes(["bar"])
			.build();

		let window = astal4::Window::builder()
			.layer(astal4::Layer::Top)
			.anchor(astal4::WindowAnchor::TOP)
			.exclusivity(astal4::Exclusivity::Exclusive)
			.child(&center_box)
			.keymode(astal4::Keymode::None)
			.name("bar")
			.css_classes(["bar"])
			.monitor(monitor_index)
			.width_request(monitor_width)
			.build();

		Self { window }
	}

	pub fn for_all_monitors(display: &gtk4::gdk::Display) -> Vec<Self> {
		display
			.monitors()
			.iter::<gdk::Monitor>()
			.map(|li| li.unwrap())
			.enumerate()
			.map(|(idx, monitor)| {
				let width = monitor.geometry().width();
				Bar::new(idx as i32, width)
			})
			.collect()
	}
}
