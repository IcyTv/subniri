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
use daemon_common::SpotifyProxy;
use futures::Stream;
use iced::{
	Animation, Element, Font, Length, Padding, Subscription, Task,
	alignment::{Horizontal, Vertical},
	font,
	widget::{column, container, image, row, space, svg, text},
};
use mprizzle::{
	LoopStatus, Mpris, MprisError, MprisEvent, MprisPlayer, PlaybackStatus, PlayerError,
	PlayerIdentity,
};
use neo_widgets::{
	icons::{ResolvedIcon, resolve_icon},
	phosphor_icon,
	style::COLORS,
	widgets::{NeoButton, neo_button, neo_card, neo_slider, spinner},
};
use reqwest::{Client, IntoUrl, redirect};
use utilities::Hashable;

use crate::modules::{MODULE_HEIGHT, MODULE_RADIUS};

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
	SpotifySavedChanged {
		track_id: String,
		generation: u64,
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
	spotify_track_id: Option<String>,
	spotify_saved: Option<bool>,
	spotify_pending: bool,
	spotify_generation: u64,
	cmd_rx: Hashable<Receiver<PlayerCommand>>,
	cmd_tx: Sender<PlayerCommand>,
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
			spotify_track_id: None,
			spotify_saved: None,
			spotify_pending: false,
			spotify_generation: 0,
			cmd_rx: Hashable::new(cmd_rx),
			cmd_tx,
		}
	}

	pub fn subscription(&self) -> Subscription<Message> {
		let mpris_sub = Subscription::run_with(self.cmd_rx.clone(), |cmd_rx| {
			mpris_to_msg_stream((**cmd_rx).clone())
		});
		if self.is_playing.is_animating(Instant::now()) {
			Subscription::batch([mpris_sub, iced::window::frames().map(|_| Message::Redraw)])
		} else {
			mpris_sub
		}
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::PlayerChanged(identity, snapshot) => {
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
				self.active_player = None;
				let spotify_task = self.set_spotify_track(None);
				self.active_player_position = 0.0;
				self.is_playing.go_mut(false, Instant::now());
				return spotify_task;
			}
			Message::MprisDisconnected => {
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
				let Some(track_id) = self.spotify_track_id.clone() else {
					return Task::none();
				};
				let Some(saved) = self.spotify_saved else {
					return Task::none();
				};
				if self.spotify_pending {
					return Task::none();
				}
				self.spotify_pending = true;
				return spotify_saved_task(track_id, self.spotify_generation, Some(!saved));
			}
			Message::SpotifySavedChanged {
				track_id,
				generation,
				result,
			} => {
				if generation != self.spotify_generation
					|| self.spotify_track_id.as_deref() != Some(&track_id)
				{
					return Task::none();
				}
				self.spotify_pending = false;
				match result {
					Ok(saved) => self.spotify_saved = Some(saved),
					Err(error) => {
						self.spotify_saved = None;
						log::warn!("Failed to update Spotify saved state: {error}");
					}
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
		if self.spotify_track_id == track_id {
			return Task::none();
		}
		self.spotify_generation = self.spotify_generation.wrapping_add(1);
		self.spotify_track_id = track_id.clone();
		self.spotify_saved = None;
		self.spotify_pending = track_id.is_some();

		track_id.map_or_else(Task::none, |track_id| {
			spotify_saved_task(track_id, self.spotify_generation, None)
		})
	}

	pub fn view(&self) -> NeoButton<'_, Message> {
		// let icon = phosphor_icon!("spotify-logo");
		const ICON_SIZE: u32 = 32;
		let icon: Element<'_, ()> = if let Some(active_player) = &self.active_player {
			match resolve_icon(&active_player.1.desktop_entry, 32, 2) {
				ResolvedIcon::Svg(img) => svg(img).width(Length::Shrink).height(ICON_SIZE).into(),
				ResolvedIcon::Image(img) => iced::widget::image(img)
					.width(Length::Shrink)
					.height(ICON_SIZE)
					.into(),
			}
		} else {
			svg(phosphor_icon!("play-pause"))
				.width(Length::Shrink)
				.height(ICON_SIZE)
				.into()
		};

		neo_button(
			container(
				row![
					icon.map(|()| Message::Noop),
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
					.color(COLORS.text)
					.size(18)
					.align_y(Vertical::Center)
					.ellipsis(text::Ellipsis::End)
					.wrapping(text::Wrapping::None)
				]
				.spacing(5.)
				.align_y(Vertical::Center),
			)
			.width(Length::Shrink.max(300)),
		)
		.padding(Padding {
			left: 8.0,
			right: 12.0,
			top: 2.0,
			bottom: 2.0,
		})
		.height(MODULE_HEIGHT)
		.radius(MODULE_RADIUS)
		.background(self.is_playing.interpolate(
			COLORS.decorative.pink70,
			COLORS.decorative.pink,
			Instant::now(),
		))
	}

	#[allow(clippy::too_many_lines)]
	pub fn view_popup(&self) -> Element<'_, Message> {
		let mut actions = column![
			neo_button(svg(phosphor_icon!("arrows-down-up")))
				.width(40)
				.height(40)
				.on_press(Message::CyclePlayer),
			neo_button(svg(phosphor_icon!("arrow-square-in")))
				.width(40)
				.height(40)
				.on_press(Message::FocusPlayer)
		]
		.align_x(Horizontal::Right)
		.width(Length::Fill)
		.spacing(8);
		if self.spotify_track_id.is_some() {
			let icon = if self.spotify_saved == Some(true) {
				phosphor_icon!("heart", "fill")
			} else {
				phosphor_icon!("heart")
			};
			actions = actions.push(
				neo_button(svg(icon))
					.width(40)
					.height(40)
					.enabled(self.spotify_saved.is_some() && !self.spotify_pending)
					.on_press(Message::ToggleSpotifySaved),
			);
		}
		let art_row =
			row![space::horizontal(), self.get_thumbnail(), actions].align_y(Vertical::Center);
		let title = (if let Some(title) = self.active_player.as_ref().map(|p| p.1.title.clone()) {
			text(title)
		} else {
			text("<unknowwn>")
		})
		.font(Font {
			weight: font::Weight::Bold,
			..Default::default()
		})
		.size(24)
		.color(COLORS.text)
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
					.color(COLORS.text.scale_alpha(0.7))
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
					.step(0.01)
					.on_change(|_| Message::Noop),
			);
			content = content.push(space().height(2));

			let position_text = text(format!(
				"{:0>2}:{:0>2}",
				self.active_player_position as u32 / 60,
				self.active_player_position as u32 % 60
			))
			.color(COLORS.text)
			.size(12);
			let length_text = text(format!(
				"{:0>2}:{:0>2}",
				snap.length.as_secs() / 60,
				(snap.length.as_millis() * 1000) % 60
			))
			.color(COLORS.text)
			.size(12);

			content = content.push(row![position_text, space::horizontal(), length_text]);

			content = content.push(space::vertical().height(12));
		}

		// TODO: On disbled buttons, add a tooltip
		if let Some((_id, snap)) = &self.active_player
			&& snap.can_control
		{
			let repeat = neo_button(svg(phosphor_icon!("repeat")))
				.height(48)
				.width(48)
				.enabled(snap.loop_status.is_some());
			let skip_prev = neo_button(svg(phosphor_icon!("skip-back")))
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
			let play_pause = neo_button(svg(pp_icon))
				.height(48)
				.width(48)
				.enabled(pp_enabled)
				.on_press(Message::PlayPause);
			let skip_next = neo_button(svg(phosphor_icon!("skip-forward")))
				.height(48)
				.width(48)
				.enabled(snap.can_skip)
				.on_press(Message::SkipNext);
			let shuffle = neo_button(svg(phosphor_icon!("shuffle")))
				.height(48)
				.width(48)
				.enabled(snap.shuffle.is_some());

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
			.width(400)
			.background(self.is_playing.interpolate(
				COLORS.decorative.pink70,
				COLORS.decorative.pink,
				Instant::now(),
			))
			.radius(MODULE_RADIUS)
			.into()
	}

	fn get_thumbnail(&self) -> Element<'_, Message> {
		let content: Element<Message> = if let Some((_, snap)) = &self.active_player {
			let ck = ThumbnailCacheKey::from((snap.art_url.clone(), snap.url.clone()));
			if let Some(tn) = self.thumbnail_cache.borrow_mut().get(&ck) {
				if let Some(tn) = tn {
					image(tn)
						.width(Length::Fill)
						.height(Length::Fill)
						.expand(true)
						.into()
				} else {
					space().into()
				}
			} else {
				container(spinner().bar_color(COLORS.black).size(40.0))
					.align_x(Horizontal::Center)
					.align_y(Vertical::Center)
					.width(Length::Fill)
					.height(Length::Fill)
					.into()
			}
		} else {
			text("No active player")
				.color(COLORS.text)
				.font(Font {
					weight: font::Weight::Bold,
					..Default::default()
				})
				.into()
		};

		neo_card(content).width(150).height(150).padding(0).into()
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
			mpris.watch();

			let mut players = HashMap::<PlayerIdentity, MprisPlayer>::new();
			let mut current_player = None;

			loop {
				tokio::select! {
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
						if let Some(msg) = handle_player_command(event, &mut players, &mut current_player).await {
							yield msg;
						}
					}
				}
			}

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

fn spotify_saved_task(track_id: String, generation: u64, saved: Option<bool>) -> Task<Message> {
	Task::future(async move {
		let result = async {
			let connection = zbus::Connection::session().await?;
			let proxy = SpotifyProxy::new(&connection).await?;
			if let Some(saved) = saved {
				proxy.set_track_saved(&track_id, saved).await
			} else {
				proxy.track_saved(&track_id).await
			}
		}
		.await
		.map_err(|error| error.to_string());
		Message::SpotifySavedChanged {
			track_id,
			generation,
			result,
		}
	})
}

#[cfg(test)]
mod tests {
	use super::spotify_track_id;

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
