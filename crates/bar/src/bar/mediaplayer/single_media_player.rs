use std::cell::RefCell;
use std::time::Duration;

use astal_mpris::prelude::*;
use astal_mpris::{PlaybackStatus, Player};
use glib::{ControlFlow, Properties, clone};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{CompositeTemplate, EventControllerMotion, glib};
use lazy_regex::regex;

use super::button_state::ButtonState;

glib::wrapper! {
	pub struct SingleMediaPlayerWidget(ObjectSubclass<imp::SingleMediaPlayerWidget>)
		@extends gtk4::Box, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Orientable, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl SingleMediaPlayerWidget {
	pub fn new(player: &Player) -> Self {
		glib::Object::builder()
			.property("switcher-icon-name", icons::Icon::ArrowUpDown.name())
			.property("player", player)
			.property("player-icon", player_icon_name(player))
			.build()
	}
}

mod imp {
	use std::cell::Cell;
	use std::rc::Rc;
	use std::sync::OnceLock;

	use glib::subclass::Signal;

	use super::*;
	use crate::popups::mediaplayer::MediaPlayerPopup;

	#[derive(Properties, Default, CompositeTemplate)]
	#[template(file = "./src/bar/mediaplayer/single_media_player.blp")]
	#[properties(wrapper_type = super::SingleMediaPlayerWidget)]
	pub struct SingleMediaPlayerWidget {
		#[property(get, construct_only)]
		player: RefCell<Player>,

		#[property(get, set)]
		playing_title: RefCell<String>,

		#[property(get, set)]
		button_state: RefCell<ButtonState>,

		#[property(get, construct_only)]
		switcher_icon_name: RefCell<String>,
		#[property(get, construct_only)]
		player_icon: RefCell<String>,

		#[template_child]
		switcher_button_stack: TemplateChild<gtk4::Stack>,
		#[template_child]
		title_label: TemplateChild<gtk4::Label>,
		#[template_child]
		player_button: TemplateChild<gtk4::Button>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for SingleMediaPlayerWidget {
		type ParentType = gtk4::Box;
		type Type = super::SingleMediaPlayerWidget;

		const NAME: &'static str = "SingleMediaPlayerWidget";

		fn class_init(klass: &mut Self::Class) {
			Self::bind_template(klass);
			Self::bind_template_callbacks(klass);
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for SingleMediaPlayerWidget {
		fn constructed(&self) {
			self.parent_constructed();

			let obj = self.obj();

			let hover_controller = EventControllerMotion::new();
			hover_controller.connect_enter(clone!(
				#[weak]
				obj,
				move |_, _, _| {
					obj.set_button_state(ButtonState::SwitcherIcon);
				}
			));
			hover_controller.connect_leave(clone!(
				#[weak]
				obj,
				move |_| {
					obj.set_button_state(ButtonState::PlayerIcon);
				}
			));
			self.switcher_button_stack.add_controller(hover_controller);

			let widget = obj.upcast_ref::<gtk4::Widget>();
			let popup = MediaPlayerPopup::new(&self.player.borrow());
			popup.set_parent(widget);
			popup.set_autohide(false);

			let hover_count = Rc::new(Cell::new(0u32));
			let popdown_timeout: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
			let full_hover_controller = EventControllerMotion::new();
			full_hover_controller.connect_enter(clone!(
				#[weak]
				popup,
				#[strong]
				hover_count,
				#[strong]
				popdown_timeout,
				move |_, _, _| {
					let new = hover_count.get() + 1;
					hover_count.set(new);

					let _ = popdown_timeout.borrow_mut().take();

					if new == 1 {
						popup.popup();
					}
				}
			));
			full_hover_controller.connect_leave(clone!(
				#[weak]
				popup,
				#[strong]
				hover_count,
				#[strong]
				popdown_timeout,
				move |_| {
					let new = hover_count.get().saturating_sub(1);
					hover_count.set(new);

					if new == 0 {
						if let Some(id) = popdown_timeout.borrow_mut().take() {
							id.remove();
						}

						let timeout_ref = popdown_timeout.clone();
						let popup = popup.clone();
						let source_id = glib::timeout_add_local(Duration::from_millis(120), move || {
							popup.popdown();
							ControlFlow::Break
						});
						timeout_ref.borrow_mut().replace(source_id);
					}
				}
			));
			obj.add_controller(full_hover_controller);

			let popup_hover_controller = EventControllerMotion::new();
			popup_hover_controller.connect_enter(clone!(
				#[weak]
				popup,
				#[strong]
				hover_count,
				#[strong]
				popdown_timeout,
				move |_, _, _| {
					let new = hover_count.get() + 1;
					hover_count.set(new);

					if let Some(id) = popdown_timeout.borrow_mut().take() {
						id.remove();
					}

					if new == 1 {
						popup.popup();
					}
				}
			));
			popup_hover_controller.connect_leave(clone!(
				#[weak]
				popup,
				#[strong]
				hover_count,
				#[strong]
				popdown_timeout,
				move |_| {
					let new = hover_count.get().saturating_sub(1);
					hover_count.set(new);

					if new == 0 {
						if let Some(id) = popdown_timeout.borrow_mut().take() {
							id.remove();
						}

						let timeout_ref = popdown_timeout.clone();
						let popup = popup.clone();
						let source_id = glib::timeout_add_local(Duration::from_millis(120), move || {
							popup.popdown();
							ControlFlow::Break
						});
						timeout_ref.borrow_mut().replace(source_id);
					}
				}
			));
			popup.add_controller(popup_hover_controller);

			let player = self.player.borrow();
			let title_label = &*self.title_label;
			player
				.bind_property("title", title_label, "label")
				.sync_create()
				.build();

			player
				.bind_property("playback-status", &*self.player_button, "active")
				.transform_to(|_, status: PlaybackStatus| Some(status == PlaybackStatus::Playing))
				.sync_create()
				.build();
		}

		fn signals() -> &'static [Signal] {
			static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
			SIGNALS.get_or_init(|| vec![Signal::builder("player-changed").build()])
		}
	}

	impl WidgetImpl for SingleMediaPlayerWidget {}
	impl BoxImpl for SingleMediaPlayerWidget {}

	#[gtk4::template_callbacks]
	impl SingleMediaPlayerWidget {
		#[template_callback]
		fn on_player_switch_clicked(&self) {
			println!("Switching player");
			let obj = self.obj();
			obj.emit_by_name::<()>("player-changed", &[]);
		}

		#[template_callback]
		fn on_title_button_clicked(&self) {
			let player = self.player.borrow();
			if player.can_pause() && player.playback_status() == PlaybackStatus::Playing {
				player.pause();
			} else if player.can_play() && player.playback_status() != PlaybackStatus::Playing {
				player.play();
			}
		}
	}
}

fn player_icon_name(player: &Player) -> &'static str {
	match player.bus_name().as_str() {
		bn if bn.ends_with("spotify") => icons::Icon::Spotify.name(),
		bn if regex!(r#"^org.mpris.MediaPlayer2.firefox.instance_.*$"#).is_match(bn) => icons::Icon::Firefox.name(),
		_ => "audio-x-generic",
	}
}
