use std::sync::Once;

pub mod bar;
pub mod popups;
pub mod shell;

pub use bar::Bar;
pub use shell::notifications::NotificationsOverlay;

pub fn init_resources() {
	static RESOURCES_ONCE: Once = Once::new();

	RESOURCES_ONCE.call_once(|| {
		gtk4::gio::resources_register_include!("assets.gresource").expect("Failed to load bar assets");
	});
}
