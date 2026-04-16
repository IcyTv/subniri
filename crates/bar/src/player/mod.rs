use std::cell::RefCell;

use astal_mpris::prelude::{MprisExt as _, PlayerExt as _};
use astal_mpris::{Mpris, PlaybackStatus, Player};
use async_channel::{Receiver, Sender};
use glib::Properties;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use lazy_regex::regex;

use crate::dbus::PlayerCommand;

glib::wrapper! {
	pub struct PlayerModel(ObjectSubclass<imp::PlayerModel>);
}

impl PlayerModel {
	pub fn new() -> Self {
		glib::Object::builder().build()
	}

	pub fn players(&self) -> gio::ListStore {
		self.imp().players.borrow().as_ref().unwrap().clone()
	}

	pub fn replace_players(&self, new_players: &[Player]) {
		let players = self.players();
		players.remove_all();

		for player in new_players {
			players.append(player);
		}
	}

	pub fn active_player(&self) -> Option<Player> {
		let active_bus_name = self.active_bus_name();
		if active_bus_name.is_empty() {
			return None;
		}

		let players = self.players();
		for index in 0..players.n_items() {
			let player = players.item(index).and_then(|o| o.downcast::<Player>().ok());
			if let Some(player) = player
				&& player.bus_name().as_str() == active_bus_name
			{
				return Some(player);
			}
		}

		None
	}

	pub fn cycle_active_player(&self) {
		let players = self.players();
		let n_items = players.n_items();
		if n_items == 0 {
			self.set_active_bus_name(String::new());
			return;
		}

		let current = self.active_bus_name();
		let mut current_index = None;
		for index in 0..n_items {
			let player = players.item(index).and_then(|o| o.downcast::<Player>().ok());
			if let Some(player) = player
				&& player.bus_name().as_str() == current
			{
				current_index = Some(index);
				break;
			}
		}

		let next_index = current_index.map_or(0, |idx| (idx + 1) % n_items);
		if let Some(player) = players.item(next_index).and_then(|o| o.downcast::<Player>().ok()) {
			self.set_active_bus_name(player.bus_name().to_string());
		}
	}
}

mod imp {
	use super::*;

	#[derive(Properties, Default)]
	#[properties(wrapper_type = super::PlayerModel)]
	pub struct PlayerModel {
		#[property(get, set = Self::set_active_bus_name, explicit_notify, name = "active-bus-name")]
		active_bus_name: RefCell<String>,
		pub(super) players: RefCell<Option<gio::ListStore>>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for PlayerModel {
		type Type = super::PlayerModel;
		const NAME: &'static str = "BarPlayerModel";
	}

	#[glib::derived_properties]
	impl ObjectImpl for PlayerModel {
		fn constructed(&self) {
			self.parent_constructed();
			let players = gio::ListStore::builder().item_type(Player::static_type()).build();
			self.players.replace(Some(players));
		}
	}

	impl PlayerModel {
		fn set_active_bus_name(&self, active_bus_name: String) {
			if *self.active_bus_name.borrow() == active_bus_name {
				return;
			}

			self.active_bus_name.replace(active_bus_name);
			self.obj().notify_active_bus_name();
		}
	}
}

pub fn channel() -> (Sender<PlayerCommand>, Receiver<PlayerCommand>) {
	async_channel::unbounded()
}

pub fn spawn_controller(model: PlayerModel, recv: Receiver<PlayerCommand>) {
	glib::spawn_future_local(async move {
		let mpris = Mpris::default();

		refresh_model_from_mpris(&model, &mpris);
		mpris.connect_players_notify({
			let model = model.clone();
			move |mpris| {
				refresh_model_from_mpris(&model, mpris);
			}
		});

		while let Ok(command) = recv.recv().await {
			handle_command(&model, command);
		}
	});
}

fn refresh_model_from_mpris(model: &PlayerModel, mpris: &Mpris) {
	let mut new_players = valid_mpris_players(mpris);
	new_players.sort_by_key(player_to_key);

	model.replace_players(&new_players);

	if new_players.is_empty() {
		model.set_active_bus_name(String::new());
		return;
	}

	let active = model.active_bus_name();
	let has_active = !active.is_empty() && new_players.iter().any(|player| player.bus_name().as_str() == active);
	if has_active {
		return;
	}

	let fallback = new_players
		.iter()
		.find(|player| player.playback_status() == PlaybackStatus::Playing && player.can_control())
		.or_else(|| new_players.iter().find(|player| player.can_control()))
		.or_else(|| new_players.first())
		.map(|player| player.bus_name().to_string())
		.unwrap_or_default();

	model.set_active_bus_name(fallback);
}

fn handle_command(model: &PlayerModel, command: PlayerCommand) {
	match command {
		PlayerCommand::Cycle => model.cycle_active_player(),
		PlayerCommand::TogglePlayPause => {
			if let Some(player) = model.active_player() {
				if player.can_pause() && player.playback_status() == PlaybackStatus::Playing {
					player.pause();
				} else if player.can_play() {
					player.play();
				}
			}
		}
		PlayerCommand::Next => {
			if let Some(player) = model.active_player()
				&& player.can_go_next()
			{
				player.next();
			}
		}
		PlayerCommand::Previous => {
			if let Some(player) = model.active_player()
				&& player.can_go_previous()
			{
				player.previous();
			}
		}
		PlayerCommand::SetActiveByBusName(bus_name) => {
			model.set_active_bus_name(bus_name.unwrap_or_default());
		}
	}
}

fn valid_mpris_players(mpris: &Mpris) -> Vec<Player> {
	mpris
		.players()
		.into_iter()
		.filter(|player| player_to_key(player) < usize::MAX)
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
