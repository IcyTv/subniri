use std::cell::RefCell;

use astal_mpris::Player;
use astal_mpris::prelude::PlayerExt;
use async_channel::Sender;
use glib::{Properties, clone};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{CompositeTemplate, glib};

use crate::dbus::PlayerCommand;
use crate::player::PlayerModel;

use super::single_media_player;

glib::wrapper! {
	pub struct MediaPlayerWidget(ObjectSubclass<imp::MediaPlayerWidget>)
		@extends gtk4::Box, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Orientable, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl MediaPlayerWidget {
	pub fn new(model: PlayerModel, command_send: Sender<PlayerCommand>) -> Self {
		let obj: Self = glib::Object::builder().build();
		obj.imp().bind_to_model(model, command_send);
		obj
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
		players: RefCell<Option<ListStore>>,
		#[property(get, construct_only)]
		player_selection: RefCell<Option<gtk4::SingleSelection>>,

		#[template_child]
		media_player_stack: TemplateChild<gtk4::Stack>,

		model: RefCell<Option<PlayerModel>>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for MediaPlayerWidget {
		type ParentType = gtk4::Box;
		type Type = super::MediaPlayerWidget;

		const NAME: &'static str = "MediaPlayerWidget";

		fn class_init(klass: &mut Self::Class) {
			Self::bind_template(klass);
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for MediaPlayerWidget {
		fn constructed(&self) {
			self.parent_constructed();

			let (players, player_selection) = player_store();
			self.players.replace(Some(players.clone()));
			self.player_selection.replace(Some(player_selection.clone()));

			let stack = &self.media_player_stack;
			player_selection.connect_selected_item_notify(clone!(
				#[weak]
				stack,
				move |sel| {
					if let Some(player) = sel.selected_item().and_then(|o| o.downcast::<Player>().ok()) {
						let bus_name = player.bus_name();
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
					while let Some(child) = stack.first_child() {
						stack.remove(&child);
					}

					for index in 0..list.n_items() {
						let item = list.item(index).and_then(|o| o.downcast::<Player>().ok());
						if let Some(player) = item {
							let player_widget = single_media_player::SingleMediaPlayerWidget::new(&player);
							let obj = weak_obj.clone();
							player_widget.connect_local("player-changed", false, move |_| {
								if let Some(obj) = obj.upgrade() {
									obj.imp().on_local_cycle_requested();
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
		pub fn bind_to_model(&self, model: PlayerModel, command_send: Sender<PlayerCommand>) {
			self.model.replace(Some(model.clone()));

			let players = self.players.borrow();
			let players = players.as_ref().unwrap();
			let model_players = model.players();
			players.remove_all();
			for index in 0..model_players.n_items() {
				if let Some(player) = model_players.item(index).and_then(|o| o.downcast::<Player>().ok()) {
					players.append(&player);
				}
			}
			self.sync_selection_from_model();

			model_players.connect_items_changed(clone!(
				#[weak(rename_to = obj)]
				self.obj(),
				move |list, _position, _removed, _added| {
					let imp = obj.imp();
					let players = imp.players.borrow();
					let players = players.as_ref().unwrap();

					players.remove_all();
					for index in 0..list.n_items() {
						if let Some(player) = list.item(index).and_then(|o| o.downcast::<Player>().ok()) {
							players.append(&player);
						}
					}

					imp.sync_selection_from_model();
				}
			));

			model.connect_active_bus_name_notify(clone!(
				#[weak(rename_to = obj)]
				self.obj(),
				move |_| {
					obj.imp().sync_selection_from_model();
				}
			));

			let sender = command_send.clone();
			let selection = self.player_selection.borrow();
			let selection = selection.as_ref().unwrap();
			selection.connect_selected_item_notify(move |sel| {
				let bus_name = sel
					.selected_item()
					.and_then(|o| o.downcast::<Player>().ok())
					.map(|player| player.bus_name().to_string());

				let _ = sender.try_send(PlayerCommand::SetActiveByBusName(bus_name));
			});
		}

		fn on_local_cycle_requested(&self) {
			if let Some(model) = self.model.borrow().as_ref() {
				model.cycle_active_player();
			}
		}

		fn sync_selection_from_model(&self) {
			let model = self.model.borrow();
			let Some(model) = model.as_ref() else {
				return;
			};

			let active = model.active_bus_name();
			let selection = self.player_selection.borrow();
			let selection = selection.as_ref().unwrap();

			if active.is_empty() {
				if selection.selected() != gtk4::INVALID_LIST_POSITION {
					selection.set_selected(gtk4::INVALID_LIST_POSITION);
				}
				return;
			}

			let players = self.players.borrow();
			let players = players.as_ref().unwrap();
			for index in 0..players.n_items() {
				let player = players.item(index).and_then(|o| o.downcast::<Player>().ok());
				if let Some(player) = player
					&& player.bus_name().as_str() == active
				{
					if selection.selected() != index {
						selection.set_selected(index);
					}
					return;
				}
			}
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
