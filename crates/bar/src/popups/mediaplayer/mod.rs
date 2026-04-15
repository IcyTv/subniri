use std::cell::RefCell;

use astal_mpris::prelude::PlayerExt;
use astal_mpris::{Loop, Player, Shuffle};
use glib::{Properties, clone};
use gtk4::CompositeTemplate;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use icons::Icon;

glib::wrapper! {
	pub struct MediaPlayerPopup(ObjectSubclass<imp::MediaPlayerPopup>)
		@extends gtk4::Popover, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Buildable, gtk4::Constraint, gtk4::ConstraintTarget, gtk4::ShortcutManager, gtk4::Native;
}

impl MediaPlayerPopup {
	pub fn new(player: &Player) -> Self {
		glib::Object::builder()
			.property("player", player)
			.property("shuffle-icon", Icon::Shuffle.name())
			.property("back-icon", Icon::SkipBack.name())
			.property("forward-icon", Icon::SkipForward.name())
			.property("repeat-icon", Icon::Repeat.name())
			.build()
	}
}

mod imp {

	use super::*;

	#[derive(Default, Properties, CompositeTemplate)]
	#[template(file = "./src/popups/mediaplayer/mediaplayer.blp")]
	#[properties(wrapper_type = super::MediaPlayerPopup)]
	pub struct MediaPlayerPopup {
		#[property(get, construct_only)]
		player: RefCell<Player>,

		#[property(get, set)]
		shuffle_icon: RefCell<String>,
		#[property(get, set)]
		back_icon: RefCell<String>,
		#[property(get, set)]
		play_icon: RefCell<String>,
		#[property(get, set)]
		forward_icon: RefCell<String>,
		#[property(get, set)]
		repeat_icon: RefCell<String>,

		#[property(get, set)]
		title_text: RefCell<String>,
		#[property(get, set)]
		artist_text: RefCell<String>,
		#[property(get, set)]
		album_text: RefCell<String>,

		#[property(get, set)]
		playback_duration: RefCell<f64>,
		#[property(get, set)]
		playback_progress: RefCell<f64>,

		#[property(get, set)]
		shuffle_supported: RefCell<bool>,
		#[property(get, set)]
		is_shuffle_active: RefCell<bool>,
		#[property(get, set)]
		loop_supported: RefCell<bool>,
		#[property(get, set)]
		is_repeat_active: RefCell<bool>,

		#[property(get, set)]
		cover_image: RefCell<Option<gtk4::gdk::Paintable>>,

		#[template_child]
		overlay: TemplateChild<gtk4::Overlay>,
		#[template_child]
		content_box: TemplateChild<gtk4::Box>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for MediaPlayerPopup {
		type ParentType = gtk4::Popover;
		type Type = super::MediaPlayerPopup;

		const NAME: &'static str = "MediaPlayerPopup";

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
			klass.bind_template_callbacks();
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for MediaPlayerPopup {
		fn constructed(&self) {
			self.parent_constructed();

			self.overlay.set_measure_overlay(&*self.content_box, true);

			let obj = self.obj();

			let player = self.player.borrow();

			player.bind_property("title", &*obj, "title-text").sync_create().build();

			player.bind_property("album", &*obj, "album-text").sync_create().build();

			player
				.bind_property("artist", &*obj, "artist-text")
				.sync_create()
				.build();

			// Check if shuffle is supported
			let shuffle_status = player.shuffle_status();
			let shuffle_supported = shuffle_status != Shuffle::Unsupported;
			obj.set_shuffle_supported(shuffle_supported);

			if shuffle_supported {
				player
					.bind_property("shuffle-status", &*obj, "is-shuffle-active")
					.transform_to(|_, status: Shuffle| Some(status == Shuffle::On))
					.transform_from(|_, shuf: bool| Some(if shuf { Shuffle::On } else { Shuffle::Off }))
					.sync_create()
					.bidirectional()
					.build();
			}

			// Check if loop is supported
			let loop_status = player.loop_status();
			let loop_supported = loop_status != Loop::Unsupported;
			obj.set_loop_supported(loop_supported);

			if loop_supported {
				player
					.bind_property("loop-status", &*obj, "is-repeat-active")
					.transform_to(|_, status: Loop| Some(matches!(status, Loop::Track | Loop::Playlist)))
					.transform_from(|_, looping: bool| Some(if looping { Loop::Playlist } else { Loop::None }))
					.sync_create()
					.bidirectional()
					.build();
			}

			player
				.bind_property("playback-status", &*obj, "play-icon")
				.transform_to(|_, status: astal_mpris::PlaybackStatus| {
					Some(match status {
						astal_mpris::PlaybackStatus::Playing => Icon::Pause.name(),
						_ => Icon::Play.name(),
					})
				})
				.sync_create()
				.build();

			player
				.bind_property("length", &*obj, "playback-duration")
				.sync_create()
				.build();

			// Manually set initial position without triggering the reverse binding
			// obj.set_playback_progress(player.position());

			// Bind position bidirectionally, but manually sync initial value
			// to avoid YouTube position jumping bug
			player
				.bind_property("position", &*obj, "playback-progress")
				// .bidirectional()
				.sync_create()
				.build();

			let cover_art = clone!(
				#[weak]
				obj,
				move |player: &Player| {
					let cover_art = player.cover_art();

					if cover_art.is_empty() {
						return;
					}

					let file = gtk4::gio::File::for_path(cover_art);
					glib::spawn_future_local(async move {
						let image = glycin::Loader::new(file).load().await;
						let image = match image {
							Ok(img) => img,
							Err(_) => {
								eprintln!("Failed to load cover art");
								return;
							}
						};
						let texture = image.next_frame().await;
						let texture = match texture {
							Ok(tex) => tex.texture(),
							Err(_) => {
								eprintln!("Failed to get texture from cover art");
								return;
							}
						};

						let paintable = texture.upcast::<gtk4::gdk::Paintable>();

						let imp = obj.imp();
						let mut cover_image = imp.cover_image.borrow_mut();
						cover_image.replace(paintable);
					});
				}
			);

			cover_art(&player);
			player.connect_cover_art_notify(cover_art);
		}
	}

	impl WidgetImpl for MediaPlayerPopup {}
	impl PopoverImpl for MediaPlayerPopup {}

	#[gtk4::template_callbacks]
	impl MediaPlayerPopup {
		#[template_callback]
		fn on_play_pause(&self) {
			let player = self.player.borrow();
			if player.can_pause() && player.playback_status() == astal_mpris::PlaybackStatus::Playing {
				player.pause();
			} else if player.can_play() && player.playback_status() != astal_mpris::PlaybackStatus::Playing {
				player.play();
			}
		}

		#[template_callback]
		fn on_previous_track(&self) {
			let player = self.player.borrow();
			if player.can_go_previous() {
				player.previous();
			}
		}

		#[template_callback]
		fn on_next_track(&self) {
			let player = self.player.borrow();
			if player.can_go_next() {
				player.next();
			}
		}
	}
}
