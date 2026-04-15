use std::cell::RefCell;
use std::time::Duration;

use glib::{clone, Properties};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, gio, glib};
use rand::seq::IndexedRandom;

static RESOURCE_PATH: &str = "/de/icytv/subniri/assets/gifs";
const MAX_GIF_SIZE: i32 = 200;

glib::wrapper! {
	pub struct LauncherImage(ObjectSubclass<self::LauncherImagePrivate>)
		@extends gtk4::Box, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl LauncherImage {
	pub fn new(icon_name: &str) -> Self {
		glib::Object::builder().property("image", icon_name).build()
	}

	pub fn random() -> Self {
		let children = gio::resources_enumerate_children(RESOURCE_PATH, gio::ResourceLookupFlags::NONE).unwrap();

		let gif_files: Vec<String> = children
			.iter()
			.filter(|name| name.ends_with(".gif"))
			.map(|name| name[..(name.len() - 4)].to_string())
			.collect();

		Self::new(gif_files.choose(&mut rand::rng()).expect("No gifs found"))
	}
}

#[derive(Debug, Default, Properties)]
#[properties(wrapper_type = LauncherImage)]
pub struct LauncherImagePrivate {
	#[property(get, set)]
	image: RefCell<String>,
	anim_task: RefCell<Option<glib::JoinHandle<()>>>,
}

#[glib::object_subclass]
impl ObjectSubclass for LauncherImagePrivate {
	type ParentType = gtk4::Box;
	type Type = LauncherImage;

	const NAME: &'static str = "LauncherImage";
}

#[glib::derived_properties]
impl ObjectImpl for LauncherImagePrivate {
	fn constructed(&self) {
		self.parent_constructed();

		let obj = self.obj();

		obj.add_css_class("launcher-gif");
		obj.set_halign(gtk4::Align::Start);
		obj.set_valign(gtk4::Align::Start);

		let image = gtk4::Image::builder()
			.pixel_size(MAX_GIF_SIZE)
			.halign(gtk4::Align::Start)
			.valign(gtk4::Align::Start)
			.build();
		image.set_size_request(MAX_GIF_SIZE, MAX_GIF_SIZE);
		image.set_overflow(gtk4::Overflow::Hidden);

		obj.append(&image);

		let update = glib::clone!(
			#[weak]
			obj,
			#[weak]
			image,
			#[upgrade_or_default]
			move || {
				let imp = obj.imp();
				if let Some(handle) = imp.anim_task.borrow_mut().take() {
					handle.abort();
				}

				let value = obj.property::<String>("image");
				let resource_path = format!("{}/{}.gif", RESOURCE_PATH, value);
				let uri = format!("resource:///{}", resource_path.trim_start_matches('/'));

				let file = gio::File::for_uri(&uri);
				let image_weak = image.downgrade();

				let handle = glib::MainContext::default().spawn_local(async move {
					let image = match glycin::Loader::new(file).load().await {
						Ok(img) => img,
						Err(err) => {
							eprintln!("Failed to load gif {}: {:?}", uri, err);
							return;
						}
					};

					let mut frames: Vec<(gdk::Texture, Duration)> = Vec::new();
					for _ in 0..512 {
						let frame = match image.next_frame().await {
							Ok(f) => f,
							Err(err) => {
								eprintln!("Failed to decode frame {}: {:?}", uri, err);
								return;
							}
						};

						if let Some(0) = frame.details().n_frame()
							&& !frames.is_empty()
						{
							break;
						}

						frames.push((frame.texture(), frame.delay().unwrap_or(Duration::from_millis(100))));
					}

					if frames.is_empty() {
						eprintln!("No frames decoded for {}", uri);
						return;
					}

					let mut i: isize = 0;
					let mut dir: isize = 1;

					loop {
						let Some(image) = image_weak.upgrade() else {
							return;
						};

						let (tex, delay) = &frames[i as usize];
						image.set_paintable(Some(tex));
						image.set_size_request(MAX_GIF_SIZE, MAX_GIF_SIZE);
						glib::timeout_future(*delay).await;

						if frames.len() > 1 {
							if i == (frames.len() as isize - 1) {
								dir = -1;
							} else if i == 0 {
								dir = 1;
							}
							i += dir;
						}
					}
				});

				*imp.anim_task.borrow_mut() = Some(handle);
			}
		);

		obj.connect_notify_local(
			Some("image"),
			clone!(
				#[strong]
				update,
				move |_, _| update()
			),
		);
		update();
	}
}

impl WidgetImpl for LauncherImagePrivate {}
impl BoxImpl for LauncherImagePrivate {}
