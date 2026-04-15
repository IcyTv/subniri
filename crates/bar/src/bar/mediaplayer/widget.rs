use std::cell::RefCell;

use astal_mpris::prelude::{MprisExt, PlayerExt};
use astal_mpris::{Mpris, PlaybackStatus, Player};
use glib::{Properties, clone};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{CompositeTemplate, glib};
use lazy_regex::regex;

use super::single_media_player;

glib::wrapper! {
	pub struct MediaPlayerWidget(ObjectSubclass<imp::MediaPlayerWidget>)
		@extends gtk4::Box, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Orientable, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl MediaPlayerWidget {
	pub fn new() -> Self {
		glib::Object::builder().build()
	}
}

mod imp {
	use gtk4::gio::ListStore;

	use super::*;

	#[derive(Properties, Default, CompositeTemplate)]
	#[template(file = "./src/bar/mediaplayer/mediaplayer.blp")]
	#[properties(wrapper_type = super::MediaPlayerWidget)]
	pub struct MediaPlayerWidget {
		#[property(get, set)]
		players:          RefCell<Option<ListStore>>,
		#[property(get, construct_only)]
		player_selection: RefCell<Option<gtk4::SingleSelection>>,

		#[template_child]
		media_player_stack: TemplateChild<gtk4::Stack>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for MediaPlayerWidget {
		type ParentType = gtk4::Box;
		type Type = super::MediaPlayerWidget;

		const NAME: &'static str = "MediaPlayerWidget";

		fn class_init(klass: &mut Self::Class) {
			Self::bind_template(klass);
			// Self::bind_template_callbacks(klass);
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for MediaPlayerWidget {
		fn constructed(&self) {
			self.parent_constructed();

			let mpris = Mpris::default();

			let (players, player_selection) = player_store();
			self.players.replace(Some(players.clone()));
			self.player_selection.replace(Some(player_selection.clone()));

			let update_players = clone!(
				#[weak]
				players,
				move |mpris: &Mpris| {
					let mut new_players = valid_mpris_players(mpris);
					new_players.sort_by_key(player_to_key);

					players.remove_all();

					for player in new_players {
						players.append(&player);
					}
				}
			);

			update_players(&mpris);
			mpris.connect_players_notify(update_players);

			let stack = &self.media_player_stack;
			player_selection.connect_selected_item_notify(clone!(
				#[weak]
				stack,
				move |sel| {
					if let Some(player) = sel.selected_item().and_then(|o| o.downcast::<Player>().ok()) {
						let bus_name = player.bus_name();
						// Only set visible child if it exists in the stack
						if stack.child_by_name(&bus_name).is_some() {
							stack.set_visible_child_name(&bus_name);
						}
					}
				}
			));

			let obj = self.obj();
			let weak_obj = obj.downgrade();
			players.connect_items_changed(clone!(
				#[weak]
				stack,
				move |list, _position, _removed, _added| {
					// TODO: We recreate the whole stack every time we get a new list. This is
					// inefficient. Maybe we can do better...
					// 1. Clear stack
					while let Some(child) = stack.first_child() {
						stack.remove(&child);
					}

					// 2. Re-add all players
					for index in 0..list.n_items() {
						let item = list.item(index).and_then(|o| o.downcast::<Player>().ok());
						if let Some(player) = item {
							let player_widget = single_media_player::SingleMediaPlayerWidget::new(&player);
							let obj = weak_obj.clone();
							player_widget.connect_local("player-changed", false, move |_| {
								println!("Switching to next player");
								// obj.imp().next_player();
								if let Some(obj) = obj.upgrade() {
									obj.imp().next_player();
								}
								None
							});
							stack.add_named(&player_widget, Some(&player.bus_name()));
						}
					}
				}
			));
		}
	}

	impl WidgetImpl for MediaPlayerWidget {}
	impl BoxImpl for MediaPlayerWidget {}

	impl MediaPlayerWidget {
		pub fn next_player(&self) {
			let selection = self.player_selection.borrow();
			let selection = selection.as_ref().unwrap();
			let n_items = selection.n_items();
			println!("Number of players: {n_items}");
			if n_items == 0 {
				return;
			}

			let current = selection.selected();

			let next = if current == gtk4::INVALID_LIST_POSITION {
				0
			} else {
				(current + 1) % n_items
			};

			selection.set_selected(next);
		}
	}

	pub(super) fn player_store() -> (ListStore, gtk4::SingleSelection) {
		let player_store = ListStore::builder().item_type(Player::static_type()).build();
		let player_selection = gtk4::SingleSelection::builder()
			.model(&player_store)
			.autoselect(true)
			.selected(0)
			.build();

		(player_store, player_selection)
	}
}

fn valid_mpris_players(mpris: &Mpris) -> Vec<Player> {
	mpris
		.players()
		.into_iter()
		.filter(|p| player_to_key(p) < usize::MAX)
		.collect()
}

fn player_to_key(player: &Player) -> usize {
	let mut base_key = match (
		player.bus_name().as_str(),
		player.title().as_str(),
		player.can_control(),
	) {
		(_, "", _) | (_, _, false) => return usize::MAX,
		(bn, ..) if bn.ends_with("spotify") => 100,
		(bn, ..) if regex!(r#"^org.mpris.MediaPlayer2.firefox.instance_.*$"#).is_match(bn) => 200,
		_ => usize::MAX - 1000,
	};

	if player.playback_status() == PlaybackStatus::Playing {
		base_key -= 10;
	}

	base_key
}
