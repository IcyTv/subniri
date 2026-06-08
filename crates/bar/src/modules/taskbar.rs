use std::{cmp::Ordering, fmt, pin::pin, sync::Arc};

use futures::{
	Stream, StreamExt,
	stream::{self},
};
use iced::{
	Element, Length, Subscription,
	widget::{column, container, image, row, space, svg},
};
use neo_widgets::{
	icons::{ResolvedIcon, resolve_from_window},
	style::COLORS,
	widgets::{NeoButtonStyle, NeoContentSurfaceStyle, NeoSurfaceStyle, neo_button, neo_card},
};
use niri_ipc::{
	Action, Event, Reply, Request, Response, Window, Workspace, WorkspaceReferenceArg,
	socket::SOCKET_PATH_ENV,
};
use tokio::{
	io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader},
	net::UnixStream,
	sync::Mutex,
};

use crate::modules::{MODULE_HEIGHT, MODULE_RADIUS};

#[derive(Debug, Clone)]
pub enum Message {
	Event(Event),
	FocusWorkspace(u64),
	FocusWindow(u64),
}

#[derive(Clone)]
pub struct Taskbar {
	stream: Arc<Mutex<BufReader<UnixStream>>>,
	windows: Vec<Window>,
	workspaces: Vec<Workspace>,
}

impl fmt::Debug for Taskbar {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Taskbar").finish_non_exhaustive()
	}
}

impl Taskbar {
	pub async fn new() -> Result<Self, String> {
		let stream = get_stream().await?;
		let stream = Arc::new(Mutex::new(stream));

		Ok(Self {
			stream,
			windows: vec![],
			workspaces: vec![],
		})
	}

	pub fn subscription() -> Subscription<Message> {
		Subscription::run(|| {
			async_stream::stream! {
				let event_stream = match open_event_stream().await {
					Ok(event_stream) => event_stream,
					Err(error) => {
						log::warn!("Failed to open niri event stream: {error}");
						return;
					}
				};
				let mut event_stream = pin!(event_stream);

				while let Some(event) = event_stream.next().await {
					match event {
						Ok(event) => yield Message::Event(event),
						Err(e) => log::warn!("Error in niri event: {e}"),
					}
				}

				log::warn!("Niri socket disconnect");
			}
		})
	}

	pub fn update(&mut self, message: Message) {
		match message {
			Message::Event(Event::WindowsChanged { mut windows }) => {
				windows.sort_by(cmp_windows);
				self.windows = windows;
			}
			Message::Event(Event::WorkspacesChanged { mut workspaces }) => {
				workspaces.sort_by_key(|w| w.idx);
				self.workspaces = workspaces;
			}
			Message::Event(Event::WorkspaceActivated { id, focused }) => {
				let output = self
					.workspaces
					.iter()
					.find(|workspace| workspace.id == id)
					.and_then(|workspace| workspace.output.clone());
				for workspace in &mut self.workspaces {
					if workspace.id == id {
						workspace.is_active = true;
						workspace.is_focused = focused;
					} else {
						workspace.is_focused = false;
						if output.is_some() && workspace.output == output {
							workspace.is_active = false;
						}
					}
				}
			}
			Message::Event(Event::WindowOpenedOrChanged { window }) => {
				let index = self.windows.iter().position(|w| w.id == window.id);

				if window.is_focused
					&& let Some(current_focus) = self
						.windows
						.iter_mut()
						.find(|w| w.is_focused && w.id != window.id)
				{
					current_focus.is_focused = false;
				}

				if let Some(index) = index {
					if let Some(existing) = self.windows.get_mut(index) {
						*existing = window;
					}
				} else {
					let insert_idx = self
						.windows
						.binary_search_by(|w| cmp_windows(w, &window))
						.unwrap_or_else(|e| e);

					self.windows.insert(insert_idx, window);
				}
			}
			Message::Event(Event::WindowFocusChanged { id }) => {
				for window in &mut self.windows {
					window.is_focused = Some(window.id) == id;
				}
			}
			Message::Event(Event::WindowLayoutsChanged { changes }) => {
				for (window_id, layout) in changes {
					if let Some(window) = self.windows.iter_mut().find(|w| w.id == window_id) {
						window.layout = layout;
					}
				}
				self.windows.sort_by(cmp_windows);
			}
			Message::Event(Event::WindowClosed { id }) => {
				if let Some(index) = self.windows.iter().position(|w| w.id == id) {
					self.windows.remove(index);
				}
			}

			Message::FocusWorkspace(id) => {
				// TODO: I'm sure we can do better here... Maybe return a Task or sth?
				if let Err(error) = futures::executor::block_on(self.send(Request::Action(
					Action::FocusWorkspace {
						reference: WorkspaceReferenceArg::Id(id),
					},
				))) {
					log::warn!("Failed to focus workspace {id}: {error}");
				}
			}
			Message::FocusWindow(id) => {
				if let Err(error) = futures::executor::block_on(
					self.send(Request::Action(Action::FocusWindow { id })),
				) {
					log::warn!("Failed to focus window {id}: {error}");
				}
			}
			Message::Event(_) => (),
		}
	}

	pub fn view(&self, output_name: Option<&str>) -> Element<'_, Message> {
		let mut row = row![]
			.spacing(4)
			.padding([0, 8])
			.align_y(iced::Alignment::Center);

		for (index, workspace) in self.workspaces.iter().enumerate() {
			if output_name.is_none() || workspace.output.as_deref() != output_name {
				continue;
			}

			if workspace.is_active {
				row = row.push(workspace_btn(workspace, &[]));

				row = row.push(space().width(1));

				for window in &self.windows {
					if window.workspace_id != Some(workspace.id) {
						continue;
					}

					row = row.push(window_btn(window));
				}

				if index != self.workspaces.len() - 1 {
					row = row.push(space().width(1));
				}
			} else {
				row = row.push(workspace_btn(workspace, &self.windows));
			}
		}

		neo_card(row)
			.height(MODULE_HEIGHT)
			.radius(MODULE_RADIUS)
			.padding(0)
			.into()
	}
}

fn workspace_btn<'a>(workspace: &'a Workspace, windows: &'a [Window]) -> Element<'a, Message> {
	let content: Element<Message> = if workspace.is_active {
		"".into()
	} else {
		let windows = windows
			.iter()
			.filter(|w| w.workspace_id == Some(workspace.id))
			.take(4)
			.collect::<Vec<_>>();

		workspace_preview(&windows)
	};

	neo_button(content)
		.style(NeoButtonStyle {
			surface: NeoContentSurfaceStyle {
				surface: NeoSurfaceStyle {
					background: if workspace.is_active {
						COLORS.decorative.yellow
					} else {
						COLORS.white
					},
					shadow_width: 2.0,
					..Default::default()
				},
				padding: 4.0.into(),
			},
			..Default::default()
		})
		.height(MODULE_HEIGHT - 20.)
		.width(MODULE_HEIGHT - 20.)
		.on_press(Message::FocusWorkspace(workspace.id))
		.into()
}

fn workspace_preview<'a>(windows: &[&'a Window]) -> Element<'a, Message> {
	const IMAGE_SIZE: f32 = 8.0;
	const ICON_GAP: f32 = 2.0;
	const PREVIEW_SIZE: f32 = IMAGE_SIZE * 2.0 + ICON_GAP;

	let preview: Element<_> = match windows {
		[] => space()
			.width(Length::Fixed(PREVIEW_SIZE))
			.height(Length::Fixed(PREVIEW_SIZE))
			.into(),
		[window] => container(workspace_preview_icon(window))
			.width(Length::Fixed(PREVIEW_SIZE))
			.height(Length::Fixed(PREVIEW_SIZE))
			.center_x(Length::Fill)
			.center_y(Length::Fill)
			.into(),
		[window_a, window_b] => row![
			workspace_preview_icon(window_a),
			workspace_preview_icon(window_b)
		]
		.spacing(ICON_GAP)
		.into(),
		[window_a, window_b, window_c] => column![
			row![
				workspace_preview_icon(window_a),
				workspace_preview_icon(window_b)
			]
			.spacing(ICON_GAP),
			container(workspace_preview_icon(window_c))
				.width(Length::Fixed(PREVIEW_SIZE))
				.center_x(Length::Fill),
		]
		.spacing(ICON_GAP)
		.into(),
		[window_a, window_b, window_c, window_d] => column![
			row![
				workspace_preview_icon(window_a),
				workspace_preview_icon(window_b)
			]
			.spacing(ICON_GAP),
			row![
				workspace_preview_icon(window_c),
				workspace_preview_icon(window_d)
			]
			.spacing(ICON_GAP),
		]
		.spacing(ICON_GAP)
		.into(),
		_ => unreachable!(),
	};

	container(preview)
		.width(Length::Fill)
		.height(Length::Fill)
		.center_x(Length::Fill)
		.center_y(Length::Fill)
		.into()
}

fn workspace_preview_icon(window: &Window) -> Element<'_, Message> {
	const IMAGE_SIZE: Length = Length::Fixed(8.0);

	match resolve_from_window(window, 8, 1) {
		ResolvedIcon::Svg(handle) => svg(handle).width(IMAGE_SIZE).height(IMAGE_SIZE).into(),
		ResolvedIcon::Image(handle) => image(handle).width(IMAGE_SIZE).height(IMAGE_SIZE).into(),
	}
}

fn window_btn(window: &Window) -> Element<'_, Message> {
	let icon = resolve_from_window(window, 32, 2);
	let icon: Element<Message> = match icon {
		ResolvedIcon::Svg(handle) => svg(handle).into(),
		ResolvedIcon::Image(handle) => image(handle)
			.filter_method(image::FilterMethod::Linear)
			.expand(true)
			.into(),
	};

	neo_button(icon)
		.shadow_width(0.0)
		.padding(4.0)
		.height(MODULE_HEIGHT - 20.0)
		.width(MODULE_HEIGHT - 20.0)
		.background(if window.is_focused {
			COLORS.decorative.pink
		} else {
			COLORS.white
		})
		.on_press(Message::FocusWindow(window.id))
		.into()
}

impl Taskbar {
	async fn send(&mut self, request: Request) -> Result<Response, String> {
		let mut buf = serde_json::to_string(&request).map_err(|e| format!("{e}"))?;
		buf.push('\n');

		self.stream
			.lock()
			.await
			.write_all(buf.as_bytes())
			.await
			.map_err(|e| format!("{e}"))?;

		buf.clear();

		self.stream
			.lock()
			.await
			.read_line(&mut buf)
			.await
			.map_err(|e| format!("{e}"))?;

		serde_json::from_str::<'_, Reply>(&buf)
			.map_err(|e| format!("{e}"))?
			.map_err(|e| e.clone())
	}
}

async fn get_stream() -> Result<BufReader<UnixStream>, String> {
	let socket_path = std::env::var_os(SOCKET_PATH_ENV).ok_or("failed to get niri socket path")?;

	let stream = UnixStream::connect(socket_path)
		.await
		.map_err(|e| format!("{e}"))?;
	Ok(BufReader::new(stream))
}

async fn open_event_stream() -> Result<impl Stream<Item = Result<Event, io::Error>>, String> {
	let mut stream = get_stream().await?;

	let mut buf = serde_json::to_string(&Request::EventStream).map_err(|e| format!("{e}"))?;
	buf.push('\n');

	stream
		.write_all(buf.as_bytes())
		.await
		.map_err(|e| format!("{e}"))?;

	buf.clear();

	stream
		.read_line(&mut buf)
		.await
		.map_err(|e| format!("{e}"))?;

	let reply = serde_json::from_str::<'_, Reply>(&buf)
		.map_err(|e| format!("{e}"))?
		.map_err(|e| e.clone())?;

	match reply {
		Response::Handled => (),
		r => return Err(format!("Invalid response: {r:?}")),
	}

	Ok(stream::unfold(
		(stream, buf),
		|(mut stream, mut buf)| async move {
			buf.clear();

			let event = stream
				.read_line(&mut buf)
				.await
				.and_then(|_| serde_json::from_str(&buf).map_err(From::from));

			Some((event, (stream, buf)))
		},
	))
}

fn cmp_windows(a: &Window, b: &Window) -> Ordering {
	win_sort_key(a).cmp(&win_sort_key(b))
}

fn win_sort_key(w: &Window) -> impl Ord {
	// TODO what about tabbed? Can niri do tabbed?
	let (col, tile) = w.layout.pos_in_scrolling_layout.unwrap_or_default();
	(w.workspace_id, col, tile, w.id)
}
