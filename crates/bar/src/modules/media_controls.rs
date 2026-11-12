use std::{
	cell::RefCell,
	collections::HashMap,
	error::Error,
	fmt,
	num::NonZeroUsize,
	rc::Rc,
	time::{Duration, Instant},
};

use async_channel::{Receiver, Sender};
use daemon_common::{DEFAULT_BUS_NAME, SpotifyProxy, SpotifyTrackProxy};
use futures::{Stream, StreamExt};
use iced::{
	Animation, ContentFit, Element, Font, Length, Subscription, Task,
	alignment::{Horizontal, Vertical},
	font,
	widget::{column, container, image, row, space, svg, text},
};
use mprizzle::{
	LoopStatus, Mpris, MprisError, MprisEvent, MprisPlayer, PlaybackStatus, PlayerError,
	PlayerIdentity,
};
use neo_widgets::{
	icons::{ResolvedIcon, background::resolve_icon},
	phosphor_icon,
	style::Colors,
	widgets::{NeoButton, neo_button, neo_card, neo_slider, spinner},
};
use reqwest::{Client, IntoUrl, redirect};
use utilities::Hashable;
use zbus::zvariant::OwnedObjectPath;

use crate::modules::{MODULE_HEIGHT, MODULE_RADIUS};

mod saved_track;
use saved_track::{Observation, SavedTrack, Source as SpotifySavedSource};

#[derive(Debug, Clone)]
pub enum Message {
	PlayerChanged(PlayerIdentity, PlayerSnapshot),
	PlayerDetached(PlayerIdentity),
	MprisDisconnected,
	PlayerUpdated(PlayerIdentity, PlayerSnapshot),
	PlayerPosition(PlayerIdentity, Duration),
	CyclePlayer,
	UpdateThumbnail(ThumbnailCacheKey, Option<image::Handle>),
	PlayPause,
	SkipPrevious,
	SkipNext,
	Redraw,
	FocusPlayer,
	ToggleSpotifySaved,
	CycleRepeat,
	ToggleShuffle,
	ModeWritten {
		identity: PlayerIdentity,
		generation: u64,
		action: ModeAction,
		result: Result<ModeAction, String>,
	},
	SpotifySavedChanged {
		track_id: String,
		generation: u64,
		path: Option<OwnedObjectPath>,
		source: SpotifySavedSource,
		result: Result<bool, String>,
	},
	ClosePopup,
	Noop,
}

#[derive(Debug, Clone)]
enum PlayerCommand {
	CyclePlayer,
	PlayPause,
	SkipPrevious,
	SkipNext,
	SetMode {
		identity: PlayerIdentity,
		generation: u64,
		action: ModeAction,
	},
}

#[derive(Debug, Clone, Copy)]
pub enum ModeAction {
	Repeat(LoopStatus),
	Shuffle(bool),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ThumbnailCacheKey {
	art_url: Option<String>,
	url: Option<String>,
}

impl From<(Option<String>, Option<String>)> for ThumbnailCacheKey {
	fn from((art_url, url): (Option<String>, Option<String>)) -> Self {
		Self { art_url, url }
	}
}

#[derive(Clone)]
pub struct MediaControls {
	active_player: Option<(PlayerIdentity, PlayerSnapshot)>,
	active_player_position: f32,
	thumbnail_cache: Rc<RefCell<lru::LruCache<ThumbnailCacheKey, Option<image::Handle>>>>,
	is_playing: Animation<bool>,
	saved_track: SavedTrack,
	cmd_rx: Hashable<Receiver<PlayerCommand>>,
	cmd_tx: Sender<PlayerCommand>,
	mode_generation: u64,
	repeat_pending: bool,
	shuffle_pending: bool,
}

impl fmt::Debug for MediaControls {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("MediaControls").finish_non_exhaustive()
	}
}

impl MediaControls {
	pub fn new() -> Self {
		let (cmd_tx, cmd_rx) = async_channel::unbounded();
		Self {
			active_player: None,
			active_player_position: 0.0,
			thumbnail_cache: Rc::new(RefCell::new(lru::LruCache::new(
				NonZeroUsize::new(8).unwrap_or(NonZeroUsize::MIN),
			))),
			is_playing: Animation::new(false).quick(),
			saved_track: SavedTrack::default(),
			cmd_rx: Hashable::new(cmd_rx),
			cmd_tx,
			mode_generation: 0,
			repeat_pending: false,
			shuffle_pending: false,
		}
	}

	pub fn subscription(&self) -> Subscription<Message> {
		let mpris_sub = Subscription::run_with(self.cmd_rx.clone(), |cmd_rx| {
			mpris_to_msg_stream((**cmd_rx).clone())
		});
		let spotify_sub = self.saved_track.observation().map(|observation| {
			Subscription::run_with(observation, |(track_id, generation)| {
				spotify_track_stream(track_id.clone(), *generation)
			})
		});
		let service_sub = match spotify_sub {
			Some(spotify_sub) => Subscription::batch([mpris_sub, spotify_sub]),
			None => mpris_sub,
		};
		if self.is_playing.is_animating(Instant::now()) {
			Subscription::batch([service_sub, iced::window::frames().map(|_| Message::Redraw)])
		} else {
			service_sub
		}
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::PlayerChanged(identity, snapshot) => {
				self.reset_modes();
				let art_url = snapshot.art_url.clone();
				let url = snapshot.url.clone();
				let spotify_track_id = spotify_track_id(url.as_deref());
				self.is_playing.go_mut(snapshot.is_playing, Instant::now());
				self.active_player_position = 0.0;
				self.active_player = Some((identity.clone(), snapshot));

				let mut tasks = Vec::with_capacity(3);
				tasks.push(iced_runtime::task::effect(iced_runtime::Action::Window(
					iced_runtime::window::Action::RedrawAll,
				)));

				let ck = (art_url, url).into();

				if !self.thumbnail_cache.borrow().contains(&ck) {
					let fetch_thumbnail_task =
						Task::future(async move { thumbnail_update_task(ck).await });
					tasks.push(fetch_thumbnail_task);
				}
				tasks.push(self.set_spotify_track(spotify_track_id));

				return Task::batch(tasks);
			}
			Message::PlayerDetached(identity)
				if self.active_player.as_ref().is_some_and(|p| p.0 == identity) =>
			{
				self.reset_modes();
				self.active_player = None;
				let spotify_task = self.set_spotify_track(None);
				self.active_player_position = 0.0;
				self.is_playing.go_mut(false, Instant::now());
				return spotify_task;
			}
			Message::MprisDisconnected => {
				self.reset_modes();
				self.active_player = None;
				let spotify_task = self.set_spotify_track(None);
				self.active_player_position = 0.0;
				self.is_playing.go_mut(false, Instant::now());
				return spotify_task;
			}
			Message::PlayerUpdated(identity, snapshot)
				if let Some(active_player) = &mut self.active_player
					&& active_player.0 == identity =>
			{
				let spotify_track_id = spotify_track_id(snapshot.url.as_deref());
				let tn_update = if snapshot.art_url != active_player.1.art_url
					|| snapshot.url != active_player.1.url
				{
					let ck = (snapshot.art_url.clone(), snapshot.url.clone()).into();
					if self.thumbnail_cache.borrow().contains(&ck) {
						None
					} else {
						Some(ck)
					}
				} else {
					None
				};

				self.is_playing.go_mut(snapshot.is_playing, Instant::now());
				active_player.1 = snapshot;

				let spotify_task = self.set_spotify_track(spotify_track_id);
				return if let Some(ck) = tn_update {
					Task::batch([
						Task::future(async move { thumbnail_update_task(ck).await }),
						spotify_task,
					])
				} else {
					spotify_task
				};
			}
			Message::PlayerPosition(id, pos)
				if self
					.active_player
					.as_ref()
					.is_some_and(|(aid, _)| *aid == id) =>
			{
				self.active_player_position = pos.as_secs_f32();
			}
			Message::CyclePlayer => {
				let _ = self.cmd_tx.send_blocking(PlayerCommand::CyclePlayer);
			}
			Message::PlayPause => {
				let _ = self.cmd_tx.send_blocking(PlayerCommand::PlayPause);
			}
			Message::SkipPrevious => {
				let _ = self.cmd_tx.send_blocking(PlayerCommand::SkipPrevious);
			}
			Message::SkipNext => {
				let _ = self.cmd_tx.send_blocking(PlayerCommand::SkipNext);
			}
			Message::CycleRepeat | Message::ToggleShuffle => {
				let Some((identity, snapshot)) = &self.active_player else {
					return Task::none();
				};
				if !snapshot.can_control {
					return Task::none();
				}

				let action = match message {
					Message::CycleRepeat if !self.repeat_pending => snapshot
						.loop_status
						.map(|status| ModeAction::Repeat(next_repeat(status))),
					Message::ToggleShuffle if !self.shuffle_pending => snapshot
						.shuffle
						.map(|shuffle| ModeAction::Shuffle(!shuffle)),
					_ => None,
				};
				if let Some(action) = action {
					let command = PlayerCommand::SetMode {
						identity: identity.clone(),
						generation: self.mode_generation,
						action,
					};
					if self.cmd_tx.send_blocking(command).is_ok() {
						match action {
							ModeAction::Repeat(_) => self.repeat_pending = true,
							ModeAction::Shuffle(_) => self.shuffle_pending = true,
						}
					}
				}
			}
			Message::ModeWritten {
				identity,
				generation,
				action,
				result,
			} => {
				if generation != self.mode_generation
					|| !self
						.active_player
						.as_ref()
						.is_some_and(|(active, _)| *active == identity)
				{
					return Task::none();
				}

				match action {
					ModeAction::Repeat(_) => self.repeat_pending = false,
					ModeAction::Shuffle(_) => self.shuffle_pending = false,
				}
				match result {
					Ok(confirmed) => {
						if let Some((_, snapshot)) = &mut self.active_player {
							match confirmed {
								ModeAction::Repeat(status) => snapshot.loop_status = Some(status),
								ModeAction::Shuffle(shuffle) => snapshot.shuffle = Some(shuffle),
							}
						}
					}
					Err(error) => log::warn!("Failed to change MPRIS mode: {error}"),
				}
			}
			Message::FocusPlayer => {
				let Some((player_id, player)) = self.active_player.as_ref() else {
					log::warn!("No active player to focus");
					return Task::none();
				};

				let desktop_entry = if player.desktop_entry.is_empty() {
					player_id.bus().to_string()
				} else {
					player.desktop_entry.clone()
				};

				let title = player.title.clone();

				return Task::future(async move {
					if let Err(e) = tokio::task::spawn_blocking(move || {
						try_focus_mpris_window(desktop_entry, title)
					})
					.await
					{
						log::warn!("Failed to focus mpris window: {e}");
					};

					Message::ClosePopup
				});
			}
			Message::ToggleSpotifySaved => {
				if let Some(mutation) = self.saved_track.toggle() {
					return spotify_saved_task(mutation);
				}
			}
			Message::SpotifySavedChanged {
				track_id,
				generation,
				path,
				source,
				result,
			} => {
				if let Err(error) = self.saved_track.apply(Observation {
					track_id,
					generation,
					path,
					source,
					result,
				}) {
					log::warn!("Failed to update Spotify saved state: {error}");
				}
			}
			Message::UpdateThumbnail(identity, thumbnail) => {
				self.thumbnail_cache.borrow_mut().put(identity, thumbnail);
			}
			Message::Redraw if self.is_playing.is_animating(Instant::now()) => {
				return iced_runtime::task::effect(iced_runtime::Action::Window(
					iced_runtime::window::Action::RedrawAll,
				));
			}
			_ => (),
		}

		Task::none()
	}

	fn set_spotify_track(&mut self, track_id: Option<String>) -> Task<Message> {
		self.saved_track.set_track(track_id);

		Task::none()
	}

	fn reset_modes(&mut self) {
		self.mode_generation = self.mode_generation.wrapping_add(1);
		self.repeat_pending = false;
		self.shuffle_pending = false;
	}

	pub fn view(&self, colors: Colors) -> NeoButton<'_, Message> {
		let background = self.is_playing.interpolate(
			colors.decorative.pink70,
			colors.decorative.pink,
			Instant::now(),
		);
		let foreground = colors.text_on(background);

		// let icon = phosphor_icon!("spotify-logo");
		const ICON_SIZE: u32 = super::ICON_HEIGHT as u32;
		let icon: Element<'_, ()> = if let Some(active_player) = &self.active_player {
			match resolve_icon(&active_player.1.desktop_entry, 32, 2) {
				ResolvedIcon::Svg(img) => svg(img).width(ICON_SIZE).height(ICON_SIZE).into(),
				ResolvedIcon::Image(img) => iced::widget::image(img)
					.width(ICON_SIZE)
					.height(ICON_SIZE)
					.into(),
			}
		} else {
			svg(phosphor_icon!("play-pause"))
				.style(move |_, _| svg::Style {
					color: Some(foreground),
				})
				.width(Length::Shrink)
				.height(ICON_SIZE)
				.into()
		};

		neo_button(
			container(
				row![
					icon.map(|()| Message::Noop),
					super::module_label(
						text(
							self.active_player
								.as_ref()
								.map_or("<nothing>", |p| p.1.title.as_str())
								.to_string()
						)
						.font(Font {
							weight: font::Weight::Bold,
							..Font::DEFAULT
						})
						.color(foreground)
						.size(18)
						.align_y(Vertical::Center)
						.ellipsis(text::Ellipsis::End)
						.wrapping(text::Wrapping::None)
					)
				]
				.spacing(5.)
				.align_y(Vertical::Center),
			)
			.width(Length::Shrink.max(300)),
		)
		.with_colors(colors)
		.height(MODULE_HEIGHT)
		.radius(MODULE_RADIUS)
		.background(background)
	}

	#[allow(clippy::too_many_lines)]
	pub fn view_popup(&self, colors: Colors) -> Element<'_, Message> {
		let icon_style = move |_: &iced::Theme, _| svg::Style {
			color: Some(colors.text),
		};

		let mut actions = column![
			neo_button(svg(phosphor_icon!("arrows-down-up")).style(icon_style))
				.with_colors(colors)
				.width(40)
				.height(40)
				.on_press(Message::CyclePlayer),
			neo_button(svg(phosphor_icon!("arrow-square-in")).style(icon_style))
				.with_colors(colors)
				.width(40)
				.height(40)
				.on_press(Message::FocusPlayer)
		]
		.align_x(Horizontal::Right)
		.width(Length::Fill)
		.spacing(8);

		if self.saved_track.observation().is_some() {
			let saved = self.saved_track.saved() == Some(true);
			let background = if saved {
				colors.decorative.pink
			} else {
				colors.surface
			};
			let icon = if self.saved_track.saved() == Some(true) {
				phosphor_icon!("heart", "fill")
			} else {
				phosphor_icon!("heart")
			};
			actions = actions.push(
				neo_button(svg(icon).style(move |_, _| svg::Style {
					color: Some(colors.text_on(background)),
				}))
				.with_colors(colors)
				.background(background)
				.width(40)
				.height(40)
				.enabled(self.saved_track.saved().is_some() && !self.saved_track.pending())
				.on_press(Message::ToggleSpotifySaved),
			);
		} else {
			actions = actions.push(space().width(40).height(40));
		}

		let art_row = row![space::horizontal(), self.get_thumbnail(colors), actions]
			.align_y(Vertical::Center);

		let title = if let Some(title) = self.active_player.as_ref().map(|p| p.1.title.clone()) {
			text(title)
		} else {
			text("<unknowwn>")
		};
		let title = title
			.font(Font {
				weight: font::Weight::Bold,
				..Default::default()
			})
			.size(24)
			.color(colors.text)
			.wrapping(text::Wrapping::WordOrGlyph)
			.width(Length::Shrink);

		let mut content =
			column![art_row, space::vertical().height(12), title].align_x(Horizontal::Center);

		if let Some(artists) = self.active_player.as_ref().map(|p| p.1.artists.as_slice()) {
			content = content.push(
				text(artists.join(", "))
					.font(Font {
						weight: font::Weight::Bold,
						..Default::default()
					})
					.size(16)
					.color(colors.secondary)
					.wrapping(text::Wrapping::WordOrGlyph)
					.width(Length::Shrink),
			);
		}
		content = content.push(space::vertical().height(12));

		if let Some((_id, snap)) = &self.active_player
			&& snap.can_control
		{
			content = content.push(
				neo_slider(0.0..=snap.length.as_secs_f32(), self.active_player_position)
					.with_colors(colors)
					.step(0.01_f32)
					.on_change(|_| Message::Noop),
			);
			content = content.push(space().height(2));

			let position_text = text(format!(
				"{:0>2}:{:0>2}",
				self.active_player_position as u32 / 60,
				self.active_player_position as u32 % 60
			))
			.color(colors.text)
			.size(12);
			let length_text = text(format!(
				"{:0>2}:{:0>2}",
				snap.length.as_secs() / 60,
				(snap.length.as_millis() * 1000) % 60
			))
			.color(colors.text)
			.size(12);

			content = content.push(row![position_text, space::horizontal(), length_text]);

			content = content.push(space::vertical().height(12));
		}

		// TODO: On disbled buttons, add a tooltip
		if let Some((_id, snap)) = &self.active_player
			&& snap.can_control
		{
			let repeat_background = if matches!(
				snap.loop_status,
				Some(LoopStatus::Playlist | LoopStatus::Track)
			) {
				colors.decorative.purple
			} else {
				colors.surface
			};
			let repeat_icon = if snap.loop_status == Some(LoopStatus::Track) {
				phosphor_icon!("repeat-once")
			} else {
				phosphor_icon!("repeat")
			};
			let repeat = neo_button(svg(repeat_icon).style(move |_, _| svg::Style {
				color: Some(colors.text_on(repeat_background)),
			}))
			.with_colors(colors)
			.background(repeat_background)
			.height(48)
			.width(48)
			.enabled(snap.loop_status.is_some() && !self.repeat_pending)
			.on_press(Message::CycleRepeat);
			let repeat_label = match snap.loop_status {
				Some(LoopStatus::None) => "Repeat off",
				Some(LoopStatus::Playlist) => "Repeat all",
				Some(LoopStatus::Track) => "Repeat one",
				None => "Repeat unavailable",
			};
			let repeat = iced::widget::tooltip(
				repeat,
				text(repeat_label).color(colors.text),
				iced::widget::tooltip::Position::Top,
			);
			let skip_prev = neo_button(svg(phosphor_icon!("skip-back")).style(icon_style))
				.with_colors(colors)
				.height(48)
				.width(48)
				.enabled(snap.can_previous)
				.on_press(Message::SkipPrevious);
			let pp_icon = if snap.is_playing {
				phosphor_icon!("pause")
			} else {
				phosphor_icon!("play")
			};
			let pp_enabled =
				(snap.is_playing && snap.can_pause) || (!snap.is_playing && snap.can_play);
			let play_pause = neo_button(svg(pp_icon).style(icon_style))
				.with_colors(colors)
				.height(48)
				.width(48)
				.enabled(pp_enabled)
				.on_press(Message::PlayPause);
			let skip_next = neo_button(svg(phosphor_icon!("skip-forward")).style(icon_style))
				.with_colors(colors)
				.height(48)
				.width(48)
				.enabled(snap.can_skip)
				.on_press(Message::SkipNext);
			let shuffle_background = if snap.shuffle == Some(true) {
				colors.decorative.green
			} else {
				colors.surface
			};
			let shuffle =
				neo_button(
					svg(phosphor_icon!("shuffle")).style(move |_, _| svg::Style {
						color: Some(colors.text_on(shuffle_background)),
					}),
				)
				.with_colors(colors)
				.background(shuffle_background)
				.height(48)
				.width(48)
				.enabled(snap.shuffle.is_some() && !self.shuffle_pending)
				.on_press(Message::ToggleShuffle);

			content = content.push(
				row![
					repeat,
					space::horizontal(),
					skip_prev,
					play_pause,
					skip_next,
					space::horizontal(),
					shuffle
				]
				.spacing(6),
			);
		}

		neo_card(content)
			.with_colors(colors)
			.width(400)
			.background(colors.body)
			.radius(MODULE_RADIUS)
			.into()
	}

	fn get_thumbnail(&self, colors: Colors) -> Element<'_, Message> {
		let content: Element<Message> = if let Some((_, snap)) = &self.active_player {
			let ck = ThumbnailCacheKey::from((snap.art_url.clone(), snap.url.clone()));
			if let Some(tn) = self.thumbnail_cache.borrow_mut().get(&ck) {
				if let Some(tn) = tn {
					image(tn)
						.width(Length::Fill)
						.height(Length::Fill)
						.content_fit(ContentFit::Cover)
						.expand(true)
						.into()
				} else {
					space().into()
				}
			} else {
				container(spinner().bar_color(colors.text).size(40.0))
					.align_x(Horizontal::Center)
					.align_y(Vertical::Center)
					.width(Length::Fill)
					.height(Length::Fill)
					.into()
			}
		} else {
			text("No active player")
				.color(colors.text)
				.font(Font {
					weight: font::Weight::Bold,
					..Default::default()
				})
				.into()
		};

		neo_card(content)
			.with_colors(colors)
			.width(150)
			.height(150)
			.padding(2)
			.into()
	}
}

fn next_repeat(status: LoopStatus) -> LoopStatus {
	match status {
		LoopStatus::None => LoopStatus::Playlist,
		LoopStatus::Playlist => LoopStatus::Track,
		LoopStatus::Track => LoopStatus::None,
	}
}

fn mpris_to_msg_stream(rx: Receiver<PlayerCommand>) -> impl Stream<Item = Message> {
	async_stream::stream! {
		loop {
			let mut mpris = match Mpris::new().await {
				Ok(mpris) => mpris,
				Err(error) => {
					log::warn!("Failed to connect to mpris, retrying: {error}");
					yield Message::MprisDisconnected;
					tokio::time::sleep(Duration::from_secs(5)).await;
					continue;
				}
			};
			let connection = mpris.connection();
			mpris.watch();

			let mut players = HashMap::<PlayerIdentity, MprisPlayer>::new();
			let mut current_player = None;

			loop {
				tokio::select! {
					() = connection.closed() => {
						// A completed mprizzle signal stream need not close its
						// event channel. Watch transport loss independently.
						log::warn!("MPRIS D-Bus connection closed, reconnecting");
						yield Message::MprisDisconnected;
						break;
					}
					event = mpris.recv() => {
						match event {
							Ok(Ok(event)) => {
								if let Some(msg) = manage_mpris_event(event, &mut players, &mut current_player).await {
									yield msg;
								}
							}
							Ok(Err(error)) => {
								log::warn!("MPRIS watch stream failed, reconnecting: {error}");
								yield Message::MprisDisconnected;
								break;
							}
							Err(error) => {
								log::warn!("MPRIS event channel closed, reconnecting: {error}");
								yield Message::MprisDisconnected;
								break;
							}
						}
					}
					event = rx.recv() => {
						if event.is_err() {
							return;
						}
						if let Some(msg) = handle_player_command(event, &mut players, &mut current_player).await {
							yield msg;
						}
					}
				}
			}

			drop(players);
			drop(mpris);
			drop(connection);
			tokio::time::sleep(Duration::from_secs(1)).await;
		}
	}
}

async fn handle_player_command(
	command: Result<PlayerCommand, async_channel::RecvError>,
	players: &mut HashMap<PlayerIdentity, MprisPlayer>,
	current_player: &mut Option<PlayerIdentity>,
) -> Option<Message> {
	let command = match command {
		Ok(c) => c,
		Err(e) => {
			log::warn!("Failed to receive player command: {e}");
			return None;
		}
	};

	match command {
		PlayerCommand::SetMode {
			identity,
			generation,
			action,
		} => {
			let write = async {
				if current_player.as_ref() != Some(&identity) {
					return Err("Player selection changed".to_string());
				}
				let player = players
					.get_mut(&identity)
					.ok_or_else(|| "Player disappeared".to_string())?;
				match action {
					ModeAction::Repeat(status) => {
						player
							.set_loop_status(status)
							.await
							.map_err(|error| error.to_string())?;
						player
							.loop_status()
							.await
							.map(ModeAction::Repeat)
							.map_err(|error| error.to_string())
					}
					ModeAction::Shuffle(shuffle) => {
						player
							.set_shuffle(shuffle)
							.await
							.map_err(|error| error.to_string())?;
						player
							.shuffle()
							.await
							.map(ModeAction::Shuffle)
							.map_err(|error| error.to_string())
					}
				}
			};
			let result = tokio::time::timeout(Duration::from_secs(5), write)
				.await
				.unwrap_or_else(|_| Err("MPRIS mode write timed out".to_string()));

			Some(Message::ModeWritten {
				identity,
				generation,
				action,
				result,
			})
		}
		PlayerCommand::CyclePlayer => {
			log::trace!("Cycling {} players", players.len());
			if players.is_empty() {
				log::warn!("No players to cycle");
				return None;
			}

			let mut players = players.iter().collect::<Vec<_>>();
			players.sort_by(|(left_id, _), (right_id, _)| left_id.bus().cmp(right_id.bus()));

			let next_player_index = current_player
				.as_ref()
				.and_then(|current_player_id| {
					players.iter().position(|(id, _)| *id == current_player_id)
				})
				.map_or(0, |index| (index + 1) % players.len());

			let Some(next_player) = players.get(next_player_index) else {
				log::warn!("Next player index was out of range");
				return None;
			};
			*current_player = Some(next_player.0.clone());

			return Some(Message::PlayerChanged(
				next_player.0.clone(),
				read_player_snapshot(next_player.1).await,
			));
		}
		PlayerCommand::PlayPause | PlayerCommand::SkipNext | PlayerCommand::SkipPrevious => {
			if players.is_empty() {
				log::warn!("No player");
				return None;
			}

			let Some(pid) = current_player.as_ref() else {
				log::warn!("No active player to interact with");
				return None;
			};

			let Some(player) = players.get_mut(pid) else {
				log::warn!("Active player not found");
				return None;
			};

			let res = match command {
				PlayerCommand::PlayPause => player.play_pause().await,
				PlayerCommand::SkipPrevious => player.previous().await,
				PlayerCommand::SkipNext => player.next().await,
				// TODO: Maybe turn these into PlayerActions or sth?
				PlayerCommand::CyclePlayer => unreachable!(),
				PlayerCommand::SetMode { .. } => unreachable!(),
			};

			if let Err(e) = res {
				log::warn!("Failed to execute action on player '{}': {e}", pid.bus());
			}

			None
		}
	}
}

async fn manage_mpris_event(
	event: MprisEvent, players: &mut HashMap<PlayerIdentity, MprisPlayer>,
	current_player: &mut Option<PlayerIdentity>,
) -> Option<Message> {
	match event {
		MprisEvent::PlayerAttached(player) => {
			let identity = player.identity().clone();
			let snapshot = read_player_snapshot(&player).await;
			players.insert(identity.clone(), player);
			if current_player.is_none() {
				*current_player = Some(identity.clone());
				Some(Message::PlayerChanged(identity, snapshot))
			} else {
				None
			}
		}
		MprisEvent::PlayerDetached(identity) => {
			let was_current_player = current_player.as_ref().is_some_and(|id| *id == identity);
			players.remove(&identity);

			if !was_current_player {
				return Some(Message::PlayerDetached(identity));
			}

			let mut remaining_players = players.iter().collect::<Vec<_>>();
			remaining_players
				.sort_by(|(left_id, _), (right_id, _)| left_id.bus().cmp(right_id.bus()));

			let Some((next_identity, next_player)) = remaining_players.first() else {
				*current_player = None;
				return Some(Message::PlayerDetached(identity));
			};

			*current_player = Some((*next_identity).clone());
			Some(Message::PlayerChanged(
				(*next_identity).clone(),
				read_player_snapshot(next_player).await,
			))
		}
		MprisEvent::PlayerPropertiesChanged(identity) | MprisEvent::PlayerSeeked(identity) => {
			if let Some(player) = players.get(&identity) {
				let snapshot = read_player_snapshot(player).await;
				Some(Message::PlayerUpdated(identity, snapshot))
			} else {
				None
			}
		}
		MprisEvent::PlayerPosition(identity, position) => {
			Some(Message::PlayerPosition(identity, position))
		}
	}
}

async fn thumbnail_update_task(ck: impl Into<ThumbnailCacheKey>) -> Message {
	let ck = ck.into();
	let thumbnail_url = resolve_thumbnail(ck.art_url.as_deref(), ck.url.as_deref()).await;
	match thumbnail_url {
		Ok(url) => Message::UpdateThumbnail(ck, url),
		Err(e) => {
			log::warn!("Error fetching thumbnail: {e}");
			Message::Noop
		}
	}
}

async fn resolve_thumbnail(
	art_url: Option<&str>, url: Option<&str>,
) -> Result<Option<image::Handle>, Box<dyn Error>> {
	if let Some(art_url) = art_url
		&& let Some(path) = art_url.strip_prefix("file://")
	{
		return Ok(Some(image::Handle::from_path(path)));
	}

	let client = Client::builder()
		.user_agent("subniri/0.1")
		.redirect(redirect::Policy::limited(5))
		.timeout(Duration::from_secs(2))
		.build()?;

	let mut urls = vec![];

	if let Some(art_url) = art_url {
		return load_image_url(&client, art_url).await.map(Some);
	}

	let Some(url) = url else {
		log::info!("No xesam::url field");
		return Ok(None);
	};

	let url = match url::Url::parse(url) {
		Ok(url) => url,
		Err(e) => {
			return Err(e.into());
		}
	};

	if let Some(id) = youtube_video_id(&url) {
		urls.extend_from_slice(&[
			format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"),
			format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"),
			format!("https://i.ytimg.com/vi/{id}/sddefault.jpg"),
			format!("https://i.ytimg.com/vi/{id}/maxresdefault.jpg"),
			format!("https://i.ytimg.com/vi/{id}/default.jpg"),
		]);
	}

	let base = url.origin().ascii_serialization();

	urls.extend_from_slice(&[
		format!("{base}/favicon.svg"),
		format!("{base}/favicon.png"),
		format!("{base}/apple-touch-icon.png"),
		format!("{base}/apple-touch-icon-precomposed.png"),
		format!("{base}/favicon.ico"),
	]);

	for url in urls {
		let Ok(res) = client.head(&url).send().await else {
			continue;
		};

		if res.status().is_success() {
			return load_image_url(&client, url).await.map(Some);
		}
	}

	Ok(None)
}

fn youtube_video_id(url: &url::Url) -> Option<String> {
	// normal watch URLs
	if let Some((_, v)) = url.query_pairs().find(|(k, _)| k == "v") {
		return Some(v.into_owned());
	}

	let path = url.path();

	// youtu.be/<id>
	if url.domain() == Some("youtu.be") {
		return path
			.trim_start_matches('/')
			.split('/')
			.next()
			.map(str::to_string);
	}

	// /shorts/<id>
	if let Some(id) = path.strip_prefix("/shorts/") {
		return id.split('/').next().map(str::to_string);
	}

	// /embed/<id>
	if let Some(id) = path.strip_prefix("/embed/") {
		return id.split('/').next().map(str::to_string);
	}

	None
}

async fn load_image_url(
	client: &reqwest::Client, url: impl IntoUrl,
) -> Result<image::Handle, Box<dyn Error>> {
	let resp = client.get(url).send().await?;

	let bytes = resp.bytes().await?;

	let handle = tokio::task::spawn_blocking(
		move || -> Result<image::Handle, Box<dyn Error + Send + Sync>> {
			let img = ::image::load_from_memory(&bytes)?;
			let img = img.resize_to_fill(300, 300, ::image::imageops::FilterType::Lanczos3);
			let rgba = img.into_rgba8();
			let (width, height) = rgba.dimensions();
			let pixels = rgba.into_raw();
			Ok(image::Handle::from_rgba(width, height, pixels))
		},
	)
	.await?
	.map_err(|e| e as Box<dyn Error>)?;

	Ok(handle)
}

fn spotify_track_id(url: Option<&str>) -> Option<String> {
	let id = if let Some(id) = url.and_then(|url| url.strip_prefix("spotify:track:")) {
		id.to_string()
	} else {
		let url = url::Url::parse(url?).ok()?;
		if url.host_str() != Some("open.spotify.com") {
			return None;
		}
		let mut segments = url.path_segments()?;
		if segments.next() != Some("track") {
			return None;
		}
		segments.next()?.to_string()
	};

	(id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())).then_some(id)
}

fn spotify_track_stream(track_id: String, generation: u64) -> impl Stream<Item = Message> {
	async_stream::stream! {
		'connection: loop {
			let connection = match zbus::Connection::session().await {
				Ok(connection) => connection,
				Err(error) => {
					yield spotify_saved_message(&track_id, generation, None, SpotifySavedSource::Initial, Err(error.to_string()));
					tokio::time::sleep(Duration::from_secs(5)).await;
					continue;
				}
			};
			let root = match SpotifyProxy::builder(&connection)
				.cache_properties(zbus::proxy::CacheProperties::No).build().await {
				Ok(proxy) => proxy,
				Err(error) => {
					yield spotify_saved_message(&track_id, generation, None, SpotifySavedSource::Initial, Err(error.to_string()));
					tokio::time::sleep(Duration::from_secs(5)).await;
					continue;
				}
			};
			let dbus = match zbus::fdo::DBusProxy::new(&connection).await {
				Ok(proxy) => proxy,
				Err(error) => {
					yield spotify_saved_message(&track_id, generation, None, SpotifySavedSource::Initial, Err(error.to_string()));
					tokio::time::sleep(Duration::from_secs(5)).await;
					continue;
				}
			};
			let mut owner_changes = match dbus.receive_name_owner_changed().await {
				Ok(changes) => changes,
				Err(error) => {
					yield spotify_saved_message(&track_id, generation, None, SpotifySavedSource::Initial, Err(error.to_string()));
					tokio::time::sleep(Duration::from_secs(5)).await;
					continue;
				}
			};
			let path = match root.get_track(&track_id).await {
				Ok(path) => path,
				Err(error) => {
					yield spotify_saved_message(&track_id, generation, None, SpotifySavedSource::Initial, Err(error.to_string()));
					tokio::time::sleep(Duration::from_secs(5)).await;
					continue;
				}
			};
			// Saved is genuinely unknown until Observe completes; subscribe lazily
			// rather than trying to preload an invented false value via GetAll.
			let track_builder = match SpotifyTrackProxy::builder(&connection)
				.cache_properties(zbus::proxy::CacheProperties::Lazily).path(path.clone()) {
				Ok(builder) => builder,
				Err(error) => {
					yield spotify_saved_message(&track_id, generation, None, SpotifySavedSource::Initial, Err(error.to_string()));
					tokio::time::sleep(Duration::from_secs(5)).await;
					continue;
				}
			};
			let track = match track_builder.build().await {
					Ok(proxy) => proxy,
					Err(error) => {
						yield spotify_saved_message(&track_id, generation, None, SpotifySavedSource::Initial, Err(error.to_string()));
						tokio::time::sleep(Duration::from_secs(5)).await;
						continue;
					}
			};
			let mut changes = track.receive_saved_changed().await;
			match track.observe().await {
				Ok(saved) => yield spotify_saved_message(
					&track_id,
					generation,
					Some(path.clone()),
					SpotifySavedSource::Initial,
					Ok(saved),
				),
				Err(error) => {
					yield spotify_saved_message(&track_id, generation, None, SpotifySavedSource::Initial, Err(error.to_string()));
					// A failed initial read does not remove daemon observation. Keep
					// listening for push while honoring its current (uncached) deadline.
					let retry_at = root.retry_at().await.unwrap_or(0);
					let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
					let deadline = tokio::time::Instant::now().checked_add(Duration::from_secs(saved_track::retry_delay_seconds(retry_at,now)));
					let retry = async move {
						if let Some(deadline) = deadline { tokio::time::sleep_until(deadline).await; }
						else { std::future::pending::<()>().await; }
					};
					tokio::pin!(retry);
					loop {
						tokio::select! {
							() = &mut retry => continue 'connection,
							change = changes.next() => {
								let Some(change) = change else { continue 'connection; };
								if let Ok(saved) = change.get().await {
									yield spotify_saved_message(&track_id,generation,Some(path.clone()),SpotifySavedSource::Initial,Ok(saved));
									break;
								}
							},
							owner = owner_changes.next() => {
								let Some(owner) = owner else { continue 'connection; };
								if owner.args().is_ok_and(|args|args.name().as_str()==DEFAULT_BUS_NAME.as_str()) { continue 'connection; }
							}
						}
					}
				}
			}

			loop {
				tokio::select! {
					change = changes.next() => {
						let Some(change) = change else {
							break;
						};
						let result = change.get().await.map_err(|error| error.to_string());
						yield spotify_saved_message(
							&track_id,
							generation,
							Some(path.clone()),
							SpotifySavedSource::Signal,
							result,
						);
					}
					owner_change = owner_changes.next() => {
						let Some(owner_change) = owner_change else {
							break;
						};
						if owner_change.args().is_ok_and(|args| args.name().as_str() == DEFAULT_BUS_NAME.as_str()) {
							break;
						}
					}
				}
			}
			tokio::time::sleep(Duration::from_secs(1)).await;
		}
	}
}

fn spotify_saved_task(command: saved_track::Mutation) -> Task<Message> {
	let saved_track::Mutation {
		track_id,
		path,
		generation,
		sequence: mutation,
		saved,
	} = command;
	Task::future(async move {
		let result = async {
			let connection = zbus::Connection::session().await?;
			let proxy = SpotifyTrackProxy::builder(&connection)
				.path(path.clone())?
				.build()
				.await?;
			proxy.set_saved(saved).await
		}
		.await
		.map_err(|error| error.to_string());
		spotify_saved_message(
			&track_id,
			generation,
			Some(path),
			SpotifySavedSource::Mutation(mutation),
			result,
		)
	})
}

fn spotify_saved_message(
	track_id: &str, generation: u64, path: Option<OwnedObjectPath>, source: SpotifySavedSource,
	result: Result<bool, String>,
) -> Message {
	Message::SpotifySavedChanged {
		track_id: track_id.to_string(),
		generation,
		path,
		source,
		result,
	}
}

#[cfg(test)]
mod tests {
	use super::{
		LoopStatus, ModeAction, PlayerCommand, PlayerIdentity, PlayerSnapshot, next_repeat,
	};
	use super::{MediaControls, Message, SpotifySavedSource};

	#[tokio::test]
	#[ignore = "requires a headless graphics renderer"]
	async fn media_action_positions_survive_heart_visibility()
	-> Result<(), super::super::layout_tests::Error> {
		use super::super::layout_tests;

		let mut renderer = layout_tests::renderer().await?;
		for (theme, colors) in [
			("light", neo_widgets::style::LIGHT),
			("dark", neo_widgets::style::DARK),
		] {
			let mut controls = MediaControls::new();
			let absent = layout_tests::capture(
				controls.view_popup(colors),
				&mut renderer,
				colors,
				&format!("media-no-heart-{theme}"),
			)?;
			controls.saved_track.set_track(Some("test-track".into()));
			let present = layout_tests::capture(
				controls.view_popup(colors),
				&mut renderer,
				colors,
				&format!("media-heart-{theme}"),
			)?;

			let slots = |snapshot: &layout_tests::Snapshot| {
				snapshot
					.nodes
					.iter()
					.copied()
					.filter(|bounds| bounds.width == 40.0 && bounds.height == 40.0)
					.collect::<Vec<_>>()
			};
			assert_eq!(
				slots(&absent),
				slots(&present),
				"action slots must stay fixed when the heart appears"
			);
		}
		Ok(())
	}

	#[test]
	fn repeat_cycles_off_playlist_track_off() {
		let playlist = next_repeat(LoopStatus::None);
		let track = next_repeat(playlist);

		assert_eq!(playlist, LoopStatus::Playlist);
		assert_eq!(track, LoopStatus::Track);
		assert_eq!(next_repeat(track), LoopStatus::None);
	}

	#[derive(Default)]
	struct ModePlayer {
		repeat: String,
		shuffle: bool,
		reject: bool,
	}

	#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
	impl ModePlayer {
		#[zbus(property)]
		fn can_control(&self) -> bool {
			true
		}

		#[zbus(property)]
		fn loop_status(&self) -> &str {
			&self.repeat
		}

		#[zbus(property)]
		fn set_loop_status(&mut self, status: String) -> zbus::fdo::Result<()> {
			if self.reject {
				return Err(zbus::fdo::Error::NotSupported("read-only repeat".into()));
			}
			self.repeat = status;
			Ok(())
		}

		#[zbus(property)]
		fn shuffle(&self) -> bool {
			self.shuffle
		}

		#[zbus(property)]
		fn set_shuffle(&mut self, shuffle: bool) -> zbus::fdo::Result<()> {
			if self.reject {
				return Err(zbus::fdo::Error::NotSupported("read-only shuffle".into()));
			}
			self.shuffle = shuffle;
			Ok(())
		}
	}

	#[tokio::test]
	#[ignore = "run explicitly under dbus-run-session with no real players"]
	async fn mode_commands_write_and_read_back_over_private_mpris()
	-> Result<(), Box<dyn std::error::Error>> {
		let bus = "org.mpris.MediaPlayer2.subniri_mode_test";
		let server = zbus::connection::Builder::session()?
			.name(bus)?
			.serve_at(
				"/org/mpris/MediaPlayer2",
				ModePlayer {
					repeat: "None".into(),
					..Default::default()
				},
			)?
			.build()
			.await?;
		let connection = zbus::Connection::session().await?;
		let identity = PlayerIdentity::new(bus.into())?;
		let player = super::MprisPlayer::new(&connection, identity.clone()).await?;
		let mut players = std::collections::HashMap::from([(identity.clone(), player)]);
		let mut current = Some(identity.clone());

		for action in [
			ModeAction::Repeat(LoopStatus::Playlist),
			ModeAction::Repeat(LoopStatus::Track),
			ModeAction::Repeat(LoopStatus::None),
			ModeAction::Shuffle(true),
			ModeAction::Shuffle(false),
		] {
			let command = PlayerCommand::SetMode {
				identity: identity.clone(),
				generation: 1,
				action,
			};
			let reply = super::handle_player_command(Ok(command), &mut players, &mut current).await;
			let Some(Message::ModeWritten {
				result: Ok(confirmed),
				..
			}) = reply
			else {
				panic!("expected confirmed mode, got {reply:?}");
			};
			assert_eq!(format!("{confirmed:?}"), format!("{action:?}"));
		}

		server
			.object_server()
			.interface::<_, ModePlayer>("/org/mpris/MediaPlayer2")
			.await?
			.get_mut()
			.await
			.reject = true;
		let command = PlayerCommand::SetMode {
			identity,
			generation: 1,
			action: ModeAction::Shuffle(true),
		};
		let reply = super::handle_player_command(Ok(command), &mut players, &mut current).await;
		assert!(matches!(
			reply,
			Some(Message::ModeWritten { result: Err(_), .. })
		));
		server.close().await?;

		Ok(())
	}

	#[test]
	fn modes_require_control_and_readable_state_and_serialize_each_action() {
		let mut controls = MediaControls::new();
		let identity =
			PlayerIdentity::new("org.mpris.MediaPlayer2.test".into()).expect("valid identity");
		controls.active_player = Some((identity.clone(), PlayerSnapshot::default()));

		let _ = controls.update(Message::CycleRepeat);
		let _ = controls.update(Message::ToggleShuffle);
		assert!(controls.cmd_rx.try_recv().is_err());

		let snapshot = &mut controls.active_player.as_mut().expect("player").1;
		snapshot.can_control = true;
		snapshot.loop_status = Some(LoopStatus::None);
		snapshot.shuffle = Some(false);
		let _ = controls.update(Message::CycleRepeat);
		let _ = controls.update(Message::CycleRepeat);
		assert!(controls.repeat_pending);
		assert!(matches!(
			controls.cmd_rx.try_recv(),
			Ok(PlayerCommand::SetMode {
				action: ModeAction::Repeat(LoopStatus::Playlist),
				..
			})
		));
		assert!(
			controls.cmd_rx.try_recv().is_err(),
			"duplicate pending action must not write"
		);

		let _ = controls.update(Message::ToggleShuffle);
		assert!(controls.shuffle_pending);
		assert!(matches!(
			controls.cmd_rx.try_recv(),
			Ok(PlayerCommand::SetMode {
				action: ModeAction::Shuffle(true),
				..
			})
		));

		let generation = controls.mode_generation;
		let _ = controls.update(Message::ModeWritten {
			identity: identity.clone(),
			generation,
			action: ModeAction::Repeat(LoopStatus::Playlist),
			result: Err("rejected".into()),
		});
		assert!(!controls.repeat_pending);
		assert_eq!(
			controls
				.active_player
				.as_ref()
				.expect("player")
				.1
				.loop_status,
			Some(LoopStatus::None)
		);

		controls.reset_modes();
		let _ = controls.update(Message::ModeWritten {
			identity,
			generation,
			action: ModeAction::Shuffle(true),
			result: Ok(ModeAction::Shuffle(true)),
		});
		assert_eq!(
			controls.active_player.as_ref().expect("player").1.shuffle,
			Some(false),
			"late reply cannot update a new player generation"
		);
	}

	#[test]
	fn saved_track_reconciliation_rejects_stale_and_mismatched_results()
	-> Result<(), Box<dyn std::error::Error>> {
		let mut controls = MediaControls::new();
		let _ = controls.set_spotify_track(Some("A".into()));
		let old = controls.saved_track.observation().ok_or("missing track")?.1;
		let _ = controls.set_spotify_track(Some("B".into()));
		let _ = controls.set_spotify_track(Some("A".into()));
		let generation = controls.saved_track.observation().ok_or("missing track")?.1;
		let path = zbus::zvariant::OwnedObjectPath::try_from("/track/A")?;
		let changed = |generation, source, result| Message::SpotifySavedChanged {
			track_id: "A".into(),
			generation,
			path: Some(path.clone()),
			source,
			result,
		};
		let _ = controls.update(changed(old, SpotifySavedSource::Initial, Ok(true)));
		assert_eq!(controls.saved_track.saved(), None);
		let _ = controls.update(changed(generation, SpotifySavedSource::Initial, Ok(false)));
		assert_eq!(controls.saved_track.saved(), Some(false));
		assert!(!controls.saved_track.pending());
		let task = controls.update(Message::ToggleSpotifySaved);
		assert!(task.units() > 0);
		assert_eq!(controls.update(Message::ToggleSpotifySaved).units(), 0);
		let mutation = 1; // First serialized mutation in this trace.
		let _ = controls.update(changed(
			generation,
			SpotifySavedSource::Initial,
			Err("late".into()),
		));
		assert_eq!(controls.saved_track.saved(), Some(false));
		assert!(controls.saved_track.pending());
		let _ = controls.update(changed(generation, SpotifySavedSource::Signal, Ok(true)));
		assert!(controls.saved_track.pending());
		let _ = controls.update(changed(
			generation,
			SpotifySavedSource::Mutation(mutation + 1),
			Ok(false),
		));
		assert_eq!(controls.saved_track.saved(), Some(true));
		assert!(controls.saved_track.pending());
		let _ = controls.update(changed(
			generation,
			SpotifySavedSource::Mutation(mutation),
			Err("write rejected".into()),
		));
		assert_eq!(controls.saved_track.saved(), Some(true));
		assert!(!controls.saved_track.pending());
		let _ = controls.set_spotify_track(Some("A".into()));
		assert_eq!(
			controls.saved_track.observation().ok_or("missing track")?.1,
			generation
		);
		let _ = controls.update(Message::MprisDisconnected);
		assert!(controls.saved_track.observation().is_none());
		assert_eq!(controls.saved_track.saved(), None);
		assert!(!controls.saved_track.pending());
		Ok(())
	}
	use super::spotify_track_id;

	struct ChildGuard(std::process::Child);

	impl Drop for ChildGuard {
		fn drop(&mut self) {
			let _ = self.0.kill();
			let _ = self.0.wait();
		}
	}

	#[test]
	#[ignore = "requires dbus-daemon; starts and stops only a private broker"]
	fn broker_loss_reaches_bar() -> Result<(), Box<dyn std::error::Error>> {
		use std::io::{BufRead, Read};
		use std::process::{Command, Stdio};
		let mut broker = ChildGuard(
			Command::new("dbus-daemon")
				.args(["--session", "--nofork", "--print-address=1"])
				.stdout(Stdio::piped())
				.spawn()?,
		);
		let mut address = String::new();
		std::io::BufReader::new(broker.0.stdout.take().ok_or("missing broker stdout")?)
			.read_line(&mut address)?;
		if !address.starts_with("unix:") {
			return Err("unexpected private broker address".into());
		}
		// Run the real subscription in another process. Never mutate the test
		// process's bus environment or stop the user's broker.
		let mut child = ChildGuard(
			Command::new(std::env::current_exe()?)
				.args([
					"--exact",
					"modules::media_controls::tests::broker_loss_child",
					"--ignored",
					"--nocapture",
				])
				.env("DBUS_SESSION_BUS_ADDRESS", address.trim())
				.env("SUBNIRI_MPRIS_TEST_CHILD", "1")
				.stdout(Stdio::piped())
				.spawn()?,
		);
		let mut output =
			std::io::BufReader::new(child.0.stdout.take().ok_or("missing child stdout")?);
		loop {
			let mut line = String::new();
			if output.read_line(&mut line)? == 0 {
				return Err("MPRIS test child exited before setup completed".into());
			}
			if line.contains("MPRIS_TEST_READY") {
				break;
			}
		}
		broker.0.kill()?;
		broker.0.wait()?;
		let mut rest = String::new();
		output.read_to_string(&mut rest)?;
		assert!(
			child.0.wait()?.success(),
			"broker-loss child failed: {rest}"
		);
		Ok(())
	}

	#[tokio::test]
	#[ignore = "subprocess helper for broker_loss_reaches_bar"]
	async fn broker_loss_child() -> Result<(), Box<dyn std::error::Error>> {
		use futures::StreamExt;
		use std::io::Write;
		if std::env::var_os("SUBNIRI_MPRIS_TEST_CHILD").is_none() {
			return Ok(());
		}
		let (_tx, rx) = async_channel::unbounded();
		let mut stream = Box::pin(super::mpris_to_msg_stream(rx));
		assert!(
			tokio::time::timeout(std::time::Duration::from_millis(200), stream.next())
				.await
				.is_err(),
			"empty private bus should not emit a player event during setup"
		);
		println!("MPRIS_TEST_READY");
		std::io::stdout().flush()?;
		let message =
			tokio::time::timeout(std::time::Duration::from_secs(2), stream.next()).await?;
		assert!(matches!(message, Some(super::Message::MprisDisconnected)));
		Ok(())
	}

	#[test]
	fn extracts_spotify_track_ids() {
		let id = "4iV5W9uYEdYUVa79Axb7Rh";
		assert_eq!(
			spotify_track_id(Some(&format!("spotify:track:{id}"))).as_deref(),
			Some(id)
		);
		assert_eq!(
			spotify_track_id(Some(&format!(
				"https://open.spotify.com/track/{id}?si=test"
			)))
			.as_deref(),
			Some(id)
		);
	}

	#[test]
	fn rejects_non_track_spotify_urls() {
		assert_eq!(
			spotify_track_id(Some(
				"https://open.spotify.com/episode/4iV5W9uYEdYUVa79Axb7Rh"
			)),
			None
		);
		assert_eq!(
			spotify_track_id(Some("spotify:local:artist:album:track")),
			None
		);
	}
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, Hash)]
pub struct PlayerSnapshot {
	title: String,
	artists: Vec<String>,
	art_url: Option<String>,
	url: Option<String>,
	desktop_entry: String,
	length: Duration,
	is_playing: bool,

	shuffle: Option<bool>,
	loop_status: Option<LoopStatus>,

	can_control: bool,
	can_skip: bool,
	can_previous: bool,
	can_seek: bool,
	can_play: bool,
	can_pause: bool,
}

#[allow(clippy::too_many_lines)]
async fn read_player_snapshot(player: &MprisPlayer) -> PlayerSnapshot {
	const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(3);

	match tokio::time::timeout(SNAPSHOT_TIMEOUT, read_player_snapshot_inner(player)).await {
		Ok(snapshot) => snapshot,
		Err(_) => {
			log::warn!(
				"Timed out reading snapshot for player '{}'",
				player.identity().bus()
			);
			PlayerSnapshot::default()
		}
	}
}

#[allow(clippy::too_many_lines)]
async fn read_player_snapshot_inner(player: &MprisPlayer) -> PlayerSnapshot {
	let Ok(metadata) = player.metadata().await else {
		log::warn!("Failed to get player metadata");
		return PlayerSnapshot::default();
	};

	let name = player.identity().bus();

	let title = metadata
		.title()
		.unwrap_or_else(|e| {
			log::warn!("Failed to get title for player '{name}': {e}");
			Some("<failed>".to_string())
		})
		.unwrap_or_else(|| "<unknown>".to_string());

	let artists = metadata
		.artists()
		.unwrap_or_else(|e| {
			log::warn!("Failed to get artists for player '{name}': {e}");
			Some(vec![])
		})
		.unwrap_or_else(std::vec::Vec::new);

	let art_url = metadata.art_url().unwrap_or_else(|e| {
		log::warn!("Failed to get player thumbnail for player '{name}': {e}");
		None
	});
	let url = metadata.url().unwrap_or_else(|e| {
		log::warn!("Failed to get player url for player '{name}': {e}");
		None
	});

	let length = metadata
		.length()
		.unwrap_or_else(|e| {
			log::warn!("Failed to get length of title for player '{name}': {e}");
			Some(Duration::default())
		})
		.unwrap_or_default();

	let (
		playback_status,
		desktop_entry,
		can_control,
		can_next,
		can_previous,
		can_seek,
		can_play,
		can_pause,
		shuffle,
		loop_status,
	) = tokio::join!(
		player.playback_status(),
		player.desktop_entry(),
		player.can_control(),
		player.can_next(),
		player.can_previous(),
		player.can_seek(),
		player.can_play(),
		player.can_pause(),
		player.shuffle(),
		player.loop_status(),
	);

	let is_playing = playback_status.map_or_else(
		|e| {
			log::warn!("Failed to get playback state for player '{name}': {e}");
			false
		},
		|status| status == PlaybackStatus::Playing,
	);

	let desktop_entry = desktop_entry.unwrap_or_else(|e| {
		log::warn!("Failed to get desktop entry for player '{name}': {e}");
		String::new()
	});

	let can_control = can_control.unwrap_or_else(|e| {
		log::warn!("Failed to get CanControl for player '{name}', assiming false: {e}");
		false
	});
	let can_skip = can_next.unwrap_or_else(|e| {
		log::warn!("Failed to get CanGoNext for player '{name}', assiming false: {e}");
		false
	});
	let can_previous = can_previous.unwrap_or_else(|e| {
		log::warn!("Failed to get CanGoPrevious for player '{name}', assiming false: {e}");
		false
	});
	let can_seek = can_seek.unwrap_or_else(|e| {
		log::warn!("Failed to get CanSeek for player '{name}', assiming false: {e}");
		false
	});
	let can_play = can_play.unwrap_or_else(|e| {
		log::warn!("Failed to get CanPlay for player '{name}', assiming false: {e}");
		false
	});
	let can_pause = can_pause.unwrap_or_else(|e| {
		log::warn!("Failed to get CanPause for player '{name}', assiming false: {e}");
		false
	});

	let shuffle = match shuffle {
		Ok(s) => Some(s),
		Err(MprisError::PlayerErr(PlayerError::FailedToGetProp(name, _))) if name == "Shuffle" => {
			None
		}
		Err(e) => {
			log::warn!(
				"Failed to get shuffle property for player '{name}', disabling shuffle: {e}"
			);
			None
		}
	};

	let loop_status = match loop_status {
		Ok(l) => Some(l),
		Err(MprisError::PlayerErr(PlayerError::FailedToGetProp(name, _)))
			if name == "LoopStatus" =>
		{
			None
		}
		Err(e) => {
			log::warn!("Failed to get loop status for player '{name}', disabling looping: {e}");
			None
		}
	};

	PlayerSnapshot {
		title,
		artists,
		art_url,
		url,
		desktop_entry,
		length,
		is_playing,

		shuffle,
		loop_status,

		can_control,
		can_skip,
		can_previous,
		can_seek,
		can_play,
		can_pause,
	}
}

fn try_focus_mpris_window(app_id: String, title: String) {
	let app_id = app_id.to_lowercase();
	let title = title.to_lowercase();

	let Ok(mut socket) = niri_ipc::socket::Socket::connect()
		.inspect_err(|e| log::warn!("Can't connect to niri: {e}"))
	else {
		return;
	};

	let windows = match socket.send(niri_ipc::Request::Windows) {
		Ok(Ok(niri_ipc::Response::Windows(windows))) => windows,
		Ok(Ok(_)) => {
			log::warn!("Unexpected response from niri when requesting windows");
			return;
		}
		Ok(Err(e)) => {
			log::warn!("Failed to get windows from niri: {e}");
			return;
		}
		Err(e) => {
			log::warn!("Failed to communicate with niri socket: {e}");
			return;
		}
	};

	let matching_windows: Vec<_> = windows
		.iter()
		.filter(|w| {
			if let Some(w_app_id) = &w.app_id {
				let w_app_id = w_app_id.to_lowercase();

				w_app_id == app_id || w_app_id.contains(&app_id) || app_id.contains(&w_app_id)
			} else {
				false
			}
		})
		.collect();

	if matching_windows.is_empty() {
		log::debug!("No matching windows found for app_id '{app_id}'");
		return;
	}

	if matching_windows.len() == 1 {
		focus_window(&mut socket, matching_windows[0].id);
		return;
	}

	if !title.is_empty() {
		if let Some(w) = matching_windows.iter().find(|w| {
			if let Some(w_title) = &w.title {
				let w_title = w_title.to_lowercase();
				w_title == title || w_title.contains(&title) || title.contains(&w_title)
			} else {
				false
			}
		}) {
			focus_window(&mut socket, w.id);
			return;
		}
	}

	// Fallback: Focus the first match
	focus_window(&mut socket, matching_windows[0].id);
}

fn focus_window(socket: &mut niri_ipc::socket::Socket, window_id: u64) {
	if let Err(e) = socket.send(niri_ipc::Request::Action(niri_ipc::Action::FocusWindow {
		id: window_id,
	})) {
		log::warn!("Failed to focus window {window_id}: {e}");
	}
}
