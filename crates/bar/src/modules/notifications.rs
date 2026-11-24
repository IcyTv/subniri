//! Notification presentation is a client of the daemon, never a second server.
use std::collections::{HashMap, HashSet};

use daemon_common::{Notification, NotificationsProxy};
use futures::StreamExt;
use iced::{
	Color, Element, Length, Rectangle, Subscription, Task, font,
	widget::{column, container, row, svg, text},
};
use neo_widgets::{
	phosphor_icon,
	style::Colors,
	widgets::{
		NeoCardStyle, NeoContentSurfaceStyle, NeoSurfaceStyle, neo_button, neo_card, neo_scrollable,
	},
};

mod body;
#[path = "../notifications/focus.rs"]
mod focus;
mod icons;
#[path = "../notifications/mod.rs"]
mod queue;

/// Fixed toast layer-surface width, shared with the layer-shell requests in `main.rs`.
pub(crate) const TOAST_WIDTH: u32 = 400;

/// Whether a natural stack height needs the scrolling view.
fn overflows(height: u32, maximum: u32) -> bool {
	height > maximum.max(1)
}

#[derive(Debug, Clone)]
pub enum Message {
	Snapshot(Vec<Notification>),
	Icon(u64, String, Option<neo_widgets::icons::ResolvedIcon>),
	Measured(String, Vec<(u32, u64)>, u32),
	Images(u64, u64, Vec<Option<iced::widget::image::Handle>>),
	Focus(Option<String>),
	Tick,
	Hover(String, u32, bool),
	Expire(u64, u32, u64),
	Expired(u64, u32, u64, Result<(), String>),
	Added(Notification),
	Removed(u32),
	Offline(String),
	Open(Rectangle),
	Dismiss(u32),
	Clear,
	Action(u32, String),
	Link(String),
	Done(Result<(), String>),
}

#[derive(Debug)]
pub struct Notifications {
	pub entries: HashMap<u32, Notification>,
	pub toasts: HashMap<String, queue::Queue>,
	focused: Option<String>,
	outputs: HashSet<String>,
	bar_output: Option<String>,
	waiting: Vec<Notification>,
	config: config::Notifications,
	unseen: HashSet<u32>,
	open: bool,
	online: bool,
	images: HashMap<u64, Vec<Option<iced::widget::image::Handle>>>,
	image_loading: bool,
	icons: HashMap<String, Option<neo_widgets::icons::ResolvedIcon>>,
	icon_loading: bool,
	measurements: HashMap<String, u32>,
	/// Outputs whose toast stack currently renders in the scrolling view.
	scrolling: HashSet<String>,
	transient_finished: HashSet<(u32, u64)>,
	expiry: HashMap<(u32, u64), (std::time::Instant, u32, bool)>,
	generation: u64,
}

impl Notifications {
	pub fn new(config: &config::Notifications) -> Self {
		Self {
			entries: HashMap::new(),
			toasts: Default::default(),
			focused: None,
			outputs: HashSet::new(),
			bar_output: None,
			waiting: Vec::new(),
			config: config.clone(),
			unseen: HashSet::new(),
			open: false,
			online: false,
			images: HashMap::new(),
			image_loading: false,
			icons: HashMap::new(),
			icon_loading: false,
			measurements: HashMap::new(),
			scrolling: HashSet::new(),
			transient_finished: HashSet::new(),
			expiry: HashMap::new(),
			generation: 0,
		}
	}

	pub fn output(&mut self, name: String, present: bool) {
		if present {
			self.outputs.insert(name);
			let waiting = std::mem::take(&mut self.waiting);
			for n in waiting {
				self.enqueue(&n, false);
			}
		} else {
			self.outputs.remove(&name);
			self.toasts.remove(&name);
			if self.focused.as_ref() == Some(&name) {
				self.focused = None;
			}
		}
	}

	fn enqueue(&mut self, n: &Notification, replacement: bool) {
		if replacement {
			for queue in self.toasts.values_mut() {
				queue.upsert(
					n,
					true,
					self.config.max_visible,
					self.config.default_timeout_ms,
				);
			}
			if let Some(waiting) = self.waiting.iter_mut().find(|old| old.id == n.id) {
				*waiting = n.clone();
			}
			return;
		}
		let targets: Vec<_> = match self.config.toast_placement {
			config::NotificationToastPlacement::AllOutputs => {
				self.outputs.iter().cloned().collect()
			}
			config::NotificationToastPlacement::BarOutput => self
				.bar_output
				.as_ref()
				.filter(|name| self.outputs.contains(*name))
				.cloned()
				.into_iter()
				.collect(),
			_ => self
				.focused
				.as_ref()
				.filter(|name| self.outputs.contains(*name))
				.cloned()
				.into_iter()
				.collect(),
		};
		if targets.is_empty() {
			self.waiting.push(n.clone());
		}
		for output in targets {
			self.toasts.entry(output).or_default().upsert(
				n,
				false,
				self.config.max_visible,
				self.config.default_timeout_ms,
			);
		}
	}

	fn toast_revision(&self, output: &str) -> Vec<(u32, u64)> {
		self.toasts
			.get(output)
			.map(|q| q.visible.iter().map(|t| (t.id, t.revision)).collect())
			.unwrap_or_default()
	}

	pub fn toast_height(&self, output: &str, maximum: u32) -> Option<u32> {
		// Width stays fixed at 400px. Keep the last natural measurement while
		// changed content is measured, including equal-height replacements.
		// Output-height changes only clamp this value; they need no new report.
		self.measurements
			.get(output)
			.map(|height| (*height).clamp(1, maximum.max(1)))
	}

	/// Cards fill the toast surface edge to edge. Each card's own hard-shadow
	/// gutter separates it from the next one, so the stack needs no spacing,
	/// separators, or backdrop of its own.
	fn toast_stack<'a>(&'a self, output: &'a str, colors: Colors) -> Element<'a, Message> {
		let mut cards = column![].spacing(0).width(Length::Fill);

		if let Some(queue) = self.toasts.get(output) {
			for toast in &queue.visible {
				if let Some(n) = self.entries.get(&toast.id) {
					cards = cards.push(
						iced::widget::mouse_area(self.card(n, colors, true))
							.on_enter(Message::Hover(output.into(), n.id, true))
							.on_exit(Message::Hover(output.into(), n.id, false)),
					);
				}
			}
		}

		cards.into()
	}

	/// Decide per output whether the toast stack scrolls, from the natural
	/// height and the output's current maximum. Either input may change on its
	/// own (a new measurement or an output resize). Switching rebuilds the card
	/// widgets, which can drop a pending `on_exit`, so hover is cleared for that
	/// output's visible toasts; no other timers are touched.
	pub fn sync_layouts(&mut self, maximum: impl Fn(&str) -> u32) {
		let outputs: Vec<String> = self.toasts.keys().cloned().collect();
		self.scrolling
			.retain(|output| self.toasts.contains_key(output));

		for output in outputs {
			let scrolling = self
				.measurements
				.get(&output)
				.is_some_and(|height| overflows(*height, maximum(&output)));
			if scrolling == self.scrolling.contains(&output) {
				continue;
			}

			if scrolling {
				self.scrolling.insert(output.clone());
			} else {
				self.scrolling.remove(&output);
			}
			if let Some(queue) = self.toasts.get_mut(&output) {
				let ids: Vec<_> = queue.visible.iter().map(|t| t.id).collect();
				for id in ids {
					queue.hover(id, false);
				}
			}
		}
	}

	pub fn view_toasts<'a>(&'a self, output: &'a str, colors: Colors) -> Element<'a, Message> {
		let revision = self.toast_revision(output);

		// The scrollable reserves a scrollbar gutter beside the cards, so only
		// stacks taller than the output get one; `sync_layout` owns that mode.
		// The probe is always the bare stack at the surface width, so
		// measurements do not depend on whether the rendered view scrolls.
		let stack: Element<'a, Message> = if self.scrolling.contains(output) {
			// Fully transparent viewport and track (neo_surface::draw always
			// paints a border-coloured shadow quad), so nothing opaque shows
			// through the cards' shadow corners; only the yellow handle draws.
			let clear = NeoSurfaceStyle {
				background: Color::TRANSPARENT,
				border: Color::TRANSPARENT,
				radius: 2.0,
				border_width: 0.0,
				shadow_width: 0.0,
			};
			let mut handle = NeoSurfaceStyle::with_colors(colors);
			handle.background = colors.decorative.yellow;
			handle.radius = 2.0;
			neo_scrollable(self.toast_stack(output, colors))
				.width(TOAST_WIDTH)
				.height(Length::Fill)
				.content_style(NeoContentSurfaceStyle {
					surface: clear,
					padding: 0.0.into(),
				})
				.track_style(clear)
				.handle_style(handle)
				.into()
		} else {
			self.toast_stack(output, colors)
		};

		neo_widgets::widgets::intrinsic_size(
			container(self.toast_stack(output, colors)).width(TOAST_WIDTH),
			container(stack).width(TOAST_WIDTH).height(Length::Fill),
			move |size| {
				Message::Measured(
					output.into(),
					revision.clone(),
					(size.height.ceil() as u32).max(1),
				)
			},
		)
	}

	pub fn enabled(&self) -> bool {
		self.config.enabled
	}

	pub fn configure(&mut self, config: &config::Notifications) {
		self.config = config.clone();
		for queue in self.toasts.values_mut() {
			queue.rebalance(config.max_visible);
		}
		if !config.enabled {
			self.toasts.clear();
			self.waiting.clear();
			self.unseen.clear();
			self.open = false;
		}
	}

	pub fn bar_output(&mut self, output: Option<String>) {
		self.bar_output = output;
		let waiting = std::mem::take(&mut self.waiting);
		for n in waiting {
			self.enqueue(&n, false);
		}
	}

	pub fn opened_on(&mut self, output: Option<String>) {
		let _ = output;
		self.opened();
	}

	pub fn opened(&mut self) {
		self.open = true;
		self.unseen.clear();
		self.toasts.clear();
		self.waiting.clear();
	}

	pub fn closed(&mut self) {
		self.open = false;
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Measured(output, revision, height) => {
				if revision == self.toast_revision(&output) {
					self.measurements.insert(output, height);
				}
			}
			Message::Icon(generation, source, icon) => {
				self.icon_loading = false;
				if generation == self.generation {
					self.icons.insert(source, icon);
				}
			}
			Message::Images(generation, body, handles) => {
				self.image_loading = false;
				if generation == self.generation {
					self.images.insert(body, handles);
				}
			}
			Message::Focus(output) => {
				self.focused = output;
				let waiting = std::mem::take(&mut self.waiting);
				for notification in waiting {
					self.enqueue(&notification, false);
				}
			}
			Message::Hover(output, id, hovered) => {
				if let Some(queue) = self.toasts.get_mut(&output) {
					queue.hover(id, hovered);
				}
			}
			Message::Tick => {
				let expired: HashSet<_> = self
					.toasts
					.values_mut()
					.flat_map(|q| q.tick(self.config.max_visible))
					.collect();
				self.transient_finished.extend(expired);
				let finished: Vec<_> = self
					.transient_finished
					.iter()
					.copied()
					.filter(|(id, revision)| {
						self.entries
							.get(id)
							.is_some_and(|n| n.revision == *revision)
							&& !self.toasts.values().any(|q| q.contains(*id))
							&& !self.waiting.iter().any(|n| n.id == *id)
					})
					.collect();
				for key in finished {
					self.transient_finished.remove(&key);
					self.expiry
						.entry(key)
						.or_insert((std::time::Instant::now(), 0, false));
				}
				if self.online {
					let generation = self.generation;
					return Task::batch(self.expiry.iter_mut().filter_map(
						|(&(id, revision), (due, _, running))| {
							if !*running && *due <= std::time::Instant::now() {
								*running = true;
								Some(Task::done(Message::Expire(generation, id, revision)))
							} else {
								None
							}
						},
					));
				}
			}

			Message::Snapshot(entries) => {
				self.entries = entries
					.into_iter()
					.filter(|n| !n.closed)
					.map(|n| (n.id, n))
					.collect();
				self.generation = self.generation.wrapping_add(1);
				self.transient_finished.clear();
				self.expiry.retain(|(id, revision), state| {
					state.2 = false;
					self.entries
						.get(id)
						.is_some_and(|n| n.revision == *revision)
				});
				self.unseen.clear();
				self.toasts.clear();
				self.waiting.clear();
				self.online = true;
			}
			Message::Added(notification) => {
				let replacement = self.entries.contains_key(&notification.id);
				if replacement {
					self.transient_finished
						.retain(|(id, _)| *id != notification.id);
					self.expiry.retain(|(id, _), _| *id != notification.id);
				}
				if !notification.closed {
					if self.config.enabled && !self.open {
						self.enqueue(&notification, replacement);
						if !replacement {
							self.unseen.insert(notification.id);
						}
					}
					self.entries.insert(notification.id, notification);
				}
			}
			Message::Removed(id) => {
				self.entries.remove(&id);
				self.transient_finished.retain(|(key, _)| *key != id);
				self.expiry.retain(|(key, _), _| *key != id);
				self.unseen.remove(&id);
				for queue in self.toasts.values_mut() {
					queue.remove(id, self.config.max_visible);
				}
				self.waiting.retain(|n| n.id != id);
			}
			Message::Offline(error) => {
				self.online = false;
				self.generation = self.generation.wrapping_add(1);
				self.transient_finished.clear();
				for state in self.expiry.values_mut() {
					state.2 = false;
				}
				self.toasts.clear();
				self.waiting.clear();
				log::warn!("Notifications disconnected: {error}");
			}
			Message::Link(uri) => {
				return Task::perform(
					async move {
						let url = url::Url::parse(&uri).map_err(|e| e.to_string())?;
						if !body::safe_uri(&uri) {
							return Err("Unsupported notification link".into());
						}
						tokio::process::Command::new("xdg-open")
							.arg(url.as_str())
							.status()
							.await
							.map_err(|e| e.to_string())
							.and_then(|status| {
								if status.success() {
									Ok(())
								} else {
									Err("URI opener failed".into())
								}
							})
					},
					Message::Done,
				);
			}
			Message::Expire(generation, id, revision) => {
				if generation != self.generation
					|| !self.online || !self
					.entries
					.get(&id)
					.is_some_and(|n| n.revision == revision)
				{
					return Task::none();
				}
				return Task::perform(
					async move {
						let result = async {
							let connection = zbus::Connection::session()
								.await
								.map_err(|e| e.to_string())?;
							let proxy = NotificationsProxy::new(&connection)
								.await
								.map_err(|e| e.to_string())?;
							proxy
								.expire_transient(id, revision)
								.await
								.map_err(|e| e.to_string())
						}
						.await;
						(generation, id, revision, result)
					},
					|(generation, id, revision, result)| {
						Message::Expired(generation, id, revision, result)
					},
				);
			}
			Message::Expired(generation, id, revision, result) => {
				if generation == self.generation {
					if result.is_ok() {
						self.expiry.remove(&(id, revision));
					} else if let Some((due, attempt, running)) =
						self.expiry.get_mut(&(id, revision))
					{
						*running = false;
						*attempt = attempt.saturating_add(1);
						*due = std::time::Instant::now()
							+ std::time::Duration::from_secs(1_u64 << (*attempt).min(5));
						log::warn!("Transient expiry failed: {:?}", result.err());
					}
				}
			}
			Message::Dismiss(_) | Message::Clear | Message::Action(_, _) => {
				return Task::perform(
					async move {
						let connection = zbus::Connection::session()
							.await
							.map_err(|e| e.to_string())?;
						let proxy = NotificationsProxy::new(&connection)
							.await
							.map_err(|e| e.to_string())?;
						match message {
							Message::Dismiss(id) => proxy.dismiss(id).await,
							Message::Clear => proxy.clear_all().await,
							Message::Action(id, key) => proxy.invoke_action(id, &key).await,
							_ => unreachable!(),
						}
						.map_err(|e| e.to_string())
					},
					Message::Done,
				);
			}
			Message::Done(Err(error)) => log::warn!("Notification operation failed: {error}"),
			_ => {}
		}
		let visible: HashSet<_> = self
			.toasts
			.values()
			.flat_map(|queue| queue.visible.iter().map(|toast| toast.id))
			.collect();
		let mut candidates: Vec<_> = self
			.entries
			.values()
			.filter(|n| n.app_icon.len() <= 4096 && icons::local_source(&n.app_icon))
			.collect();
		candidates.sort_by_key(|n| {
			(
				!visible.contains(&n.id),
				std::cmp::Reverse((n.created_at, n.id)),
			)
		});
		let mut seen = HashSet::new();
		let wanted: Vec<_> = candidates
			.into_iter()
			.map(|n| n.app_icon.clone())
			.filter(|source| seen.insert(source.clone()))
			.take(64)
			.collect();
		self.icons.retain(|source, _| wanted.contains(source));
		if !self.icon_loading {
			if let Some(source) = wanted
				.into_iter()
				.find(|source| !self.icons.contains_key(source))
			{
				self.icon_loading = true;
				let generation = self.generation;
				return Task::perform(
					async move {
						let input = source.clone();
						let icon = tokio::task::spawn_blocking(move || icons::load_local(&input))
							.await
							.unwrap_or_default();
						(generation, source, icon)
					},
					|(generation, source, icon)| Message::Icon(generation, source, icon),
				);
			}
		}

		let mut candidates: Vec<_> = self
			.entries
			.values()
			.filter(|n| n.body.contains("<img"))
			.collect();
		// Keep the newest retained bodies resident; older bodies yield capacity
		// to new arrivals, including cached decode failures. Stable IDs break ties.
		candidates.sort_by_key(|n| std::cmp::Reverse((n.created_at, n.id)));
		let wanted: HashSet<_> = candidates
			.iter()
			.take(64)
			.map(|n| body_key(&n.body))
			.collect();
		self.images.retain(|key, _| wanted.contains(key));
		if !self.image_loading {
			if let Some(body) =
				candidates.iter().take(64).map(|n| &n.body).find(|body| {
					body.contains("<img") && !self.images.contains_key(&body_key(body))
				}) {
				let key = body_key(body);
				let generation = self.generation;
				let body: String = body.chars().take(16_384).collect();
				self.image_loading = true;
				return Task::perform(
					async move {
						let source = body.clone();
						let images =
							tokio::task::spawn_blocking(move || body::load_images(&source))
								.await
								.unwrap_or_default();
						(generation, key, images)
					},
					|(generation, body, images)| Message::Images(generation, body, images),
				);
			}
		}
		Task::none()
	}

	pub fn subscription(&self) -> Subscription<Message> {
		let transport = Subscription::run(|| {
			async_stream::stream! {
				loop {
					let mut session = Box::pin(async_stream::stream! {
						let connection = match zbus::Connection::session().await { Ok(c) => c, Err(e) => { yield Message::Offline(e.to_string()); return; } };
						let proxy = match NotificationsProxy::new(&connection).await { Ok(p) => p, Err(e) => { yield Message::Offline(e.to_string()); return; } };
						let mut signals = match proxy.inner().receive_all_signals().await { Ok(s) => s, Err(e) => { yield Message::Offline(e.to_string()); return; } };
						let mut owners = match proxy.inner().receive_owner_changed().await { Ok(s) => s, Err(e) => { yield Message::Offline(e.to_string()); return; } };
						let snapshot = match proxy.get_all().await { Ok(s) => s, Err(e) => { yield Message::Offline(e.to_string()); return; } };
						let mut reconciled: HashMap<_, _> = snapshot.into_iter().map(|n| (n.id, n)).collect();
						// Signals queued during the snapshot are a quiet part of hydration.
						use futures::FutureExt;
						while let Some(Some(signal)) = signals.next().now_or_never() {
							match signal.header().member().map(|m| m.as_str()) {
								Some("NotificationAdded") => if let Ok((n,)) = signal.body().deserialize::<(Notification,)>() { reconciled.insert(n.id, n); },
								Some("NotificationRemoved") => if let Ok((id,)) = signal.body().deserialize::<(u32,)>() { reconciled.remove(&id); },
								_ => {}
							}
						}
						yield Message::Snapshot(reconciled.into_values().collect());
						loop {
							let incoming = tokio::select! { signal = signals.next() => signal, _ = owners.next() => { break; } };
							let Some(signal) = incoming else { break; };
							match signal.header().member().map(|m| m.as_str()) {
								Some("NotificationAdded") => if let Ok((notification,)) = signal.body().deserialize::<(Notification,)>() { yield Message::Added(notification); },
								Some("NotificationRemoved") => if let Ok((id,)) = signal.body().deserialize::<(u32,)>() { yield Message::Removed(id); },
								_ => {}
							}
						}
						yield Message::Offline("Daemon connection lost".into());
					});
					while let Some(message) = session.next().await { yield message; }
					tokio::time::sleep(std::time::Duration::from_secs(2)).await;
				}
			}
		});
		Subscription::batch([
			transport,
			Subscription::run(focus::stream).map(Message::Focus),
			iced::time::every(std::time::Duration::from_millis(100)).map(|_| Message::Tick),
		])
	}

	pub fn view(&self, colors: Colors) -> Element<'_, Message> {
		if !self.enabled() {
			return iced::widget::space().width(0).into();
		}
		let icon = if !self.unseen.is_empty() {
			phosphor_icon!("bell-ringing", "fill")
		} else if self.entries.is_empty() {
			phosphor_icon!("bell", "bold")
		} else {
			phosphor_icon!("bell", "fill")
		};
		neo_button(svg(icon).height(super::ICON_HEIGHT))
			.with_colors(colors)
			.height(super::MODULE_HEIGHT)
			.width(super::MODULE_HEIGHT)
			.radius(super::MODULE_RADIUS)
			.background(colors.decorative.blue)
			.on_press_with_bounds(Message::Open)
			.into()
	}

	pub fn view_popup(&self, colors: Colors) -> Element<'_, Message> {
		let mut entries: Vec<_> = self.entries.values().collect();
		entries.sort_by_key(|n| std::cmp::Reverse((n.created_at, n.id)));
		if self.config.center_layout == config::NotificationCenterLayout::GroupedByApp {
			entries.sort_by(|a, b| a.app_name.cmp(&b.app_name));
		}
		let mut cards = column![].spacing(10);
		let mut app = None;
		for notification in entries {
			if self.config.center_layout == config::NotificationCenterLayout::GroupedByApp
				&& app != Some(&notification.app_name)
			{
				cards = cards.push(
					text(&notification.app_name)
						.size(16)
						.color(colors.secondary),
				);
				app = Some(&notification.app_name);
			}
			cards = cards.push(self.card(notification, colors, false));
		}
		if self.entries.is_empty() {
			cards = cards.push(text("No notifications").color(colors.secondary));
		}
		let title = if self.online {
			"Notifications"
		} else {
			"Notifications · reconnecting"
		};
		neo_card(
			column![
				row![
					text(title).size(20).color(colors.text).width(Length::Fill),
					neo_button(text("Clear all"))
						.with_colors(colors)
						.on_press(Message::Clear)
				]
				.spacing(16),
				neo_scrollable(cards)
					.with_colors(colors)
					.height(Length::Shrink.max(520))
			]
			.spacing(16),
		)
		.with_colors(colors)
		.background(colors.body)
		.width(440)
		.radius(super::MODULE_RADIUS)
		.padding(16)
		.into()
	}

	pub fn card<'a>(
		&self, n: &'a Notification, colors: Colors, toast: bool,
	) -> Element<'a, Message> {
		let icon: Element<'a, Message> = match icons::resolve(n, self.icons.get(&n.app_icon)) {
			neo_widgets::icons::ResolvedIcon::Svg(handle) => {
				svg(handle).width(24).height(24).into()
			}
			neo_widgets::icons::ResolvedIcon::Image(handle) => {
				iced::widget::image(handle).width(24).height(24).into()
			}
		};
		let header =
			row![
				icon,
				text(&n.app_name)
					.size(14)
					.color(colors.secondary)
					.wrapping(text::Wrapping::None)
					.ellipsis(text::Ellipsis::End)
					.width(Length::Fill),
				neo_button(svg(phosphor_icon!("x", "bold")).width(12).height(12).style(
					move |_, _| svg::Style {
						color: Some(colors.text)
					}
				),)
				.with_colors(colors)
				.padding(4)
				.on_press(Message::Dismiss(n.id))
			]
			.spacing(10)
			.align_y(iced::Alignment::Center);

		let mut content = column![
			header,
			text(&n.summary)
				.size(17)
				.color(colors.text)
				.weight(font::Weight::Bold),
			body::view(
				&n.body,
				self.images
					.get(&body_key(&n.body))
					.map(Vec::as_slice)
					.unwrap_or(&[]),
				colors
			)
		]
		.spacing(8)
		.width(Length::Fill);

		let mut actions = row![].spacing(8);
		for action in n
			.actions
			.iter()
			.filter(|a| a.identifier != "default")
			.take(16)
		{
			actions = actions.push(
				neo_button(text(&action.display).size(14))
					.with_colors(colors)
					.on_press(Message::Action(n.id, action.identifier.clone())),
			);
		}
		if n.actions.iter().any(|a| a.identifier != "default") {
			content = content.push(actions);
		}

		// Toasts stand alone like other bar popups, so they use the popup body
		// colour; center cards sit inside that body, so they use surface.
		let mut style = NeoCardStyle::with_colors(colors);
		style.surface.background = if toast { colors.body } else { colors.surface };
		style.surface.radius = super::MODULE_RADIUS;
		style.padding = 14.0.into();
		if critical(n) {
			style.surface.border = colors.feedback.danger;
		}

		iced::widget::mouse_area(neo_card(content).style(style).width(Length::Fill))
			.on_press(Message::Action(n.id, "default".into()))
			.into()
	}
}

/// Mirrors the queue's critical-urgency rule (hint `urgency` == 2).
fn critical(n: &Notification) -> bool {
	n.hints.get("urgency").and_then(|v| u8::try_from(v).ok()) == Some(2)
}

fn body_key(body: &str) -> u64 {
	use std::hash::{Hash, Hasher};
	let mut hash = std::collections::hash_map::DefaultHasher::new();
	body.hash(&mut hash);
	hash.finish()
}
