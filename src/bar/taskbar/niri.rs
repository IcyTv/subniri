use std::collections::{BTreeMap, HashMap};
use std::ops::Deref;

use async_channel::{Receiver, Sender};
use futures::Stream;
use niri_ipc::socket::Socket;
use niri_ipc::{Action, Event, Output, Request, Response, Window as NiriWindow, WindowLayout, Workspace};

// TODO: Should this really be a struct?
#[derive(Clone, Copy)]
pub struct Niri {}

impl Niri {
	pub fn new() -> Self {
		Self {}
	}

	pub fn activate_window(&self, id: u64) {
		let mut socket = Socket::connect().unwrap();
		let reply = socket.send(Request::Action(Action::FocusWindow { id })).unwrap();

		if !matches!(reply, Ok(Response::Handled)) {
			eprintln!("Failed to focus window {id}: {reply:?}");
		}
	}

	pub fn activate_workspace(&self, id: u64) {
		let mut socket = Socket::connect().unwrap();
		let reply = socket
			.send(Request::Action(Action::FocusWorkspace {
				reference: niri_ipc::WorkspaceReferenceArg::Id(id),
			}))
			.unwrap();
		if !matches!(reply, Ok(Response::Handled)) {
			eprintln!("Failed to focus workspace {id}: {reply:?}");
		}
	}

	pub fn outputs(&self) -> HashMap<String, Output> {
		let mut socket = Socket::connect().unwrap();
		let reply = socket.send(Request::Outputs).unwrap();
		match reply {
			Ok(Response::Outputs(outputs)) => outputs,
			_ => HashMap::new(),
		}
	}

	pub fn window_stream(&self) -> WindowStream {
		WindowStream::new()
	}

	/// Returns a stream of workspace changes.
	pub fn workspace_stream(&self) -> impl Stream<Item = Vec<Workspace>> + use<> {
		let mut socket = Socket::connect().unwrap();
		let reply = socket.send(Request::EventStream).unwrap();
		if !matches!(reply, Ok(Response::Handled)) {
			panic!("Failed to request event stream: {reply:?}");
		}

		let mut next = socket.read_events();
		async_stream::stream! {
			loop {
				match next() {
					Ok(Event::WorkspacesChanged { workspaces }) => {
						yield workspaces;
					}
					Ok(_) => (),
					Err(e) => {
						eprintln!("Niri IPC error reading from event stream: {e}");
					}
				}
			}
		}
	}
}

pub struct WindowStream {
	rx: Receiver<Vec<Window>>,
}

impl WindowStream {
	pub fn new() -> Self {
		let (tx, rx) = async_channel::unbounded();
		std::thread::spawn(move || Self::window_stream(tx));

		Self { rx }
	}

	pub async fn next(&self) -> Option<Vec<Window>> {
		self.rx.recv().await.ok()
	}

	fn window_stream(tx: Sender<Vec<Window>>) -> ! {
		let mut socket = Socket::connect().unwrap();
		let reply = socket.send(Request::EventStream).unwrap();
		if !matches!(reply, Ok(Response::Handled)) {
			panic!("Failed to request event stream: {reply:?}");
		}

		let mut recv_event = socket.read_events();

		let mut state = WindowSet::new();
		loop {
			while let Ok(event) = recv_event() {
				if let Some(windows) = state.with_event(event) {
					tx.send_blocking(windows).unwrap();
				}
			}
			eprintln!("Event stream disconnected, reconnecting...");
		}
	}
}

/// The toplevel window set within Niri, updated via the Niri event stream.
pub struct WindowSet(Option<Inner>);

impl WindowSet {
	pub fn new() -> Self {
		Self(None)
	}

	pub fn with_event(&mut self, event: Event) -> Option<Vec<Window>> {
		match event {
			Event::WindowsChanged { windows } => match self.0.take() {
				Some(Inner::WorkspacesOnly(workspaces)) => {
					self.0 = Some(Inner::Ready(NiriState::new(windows, workspaces)));
				}
				Some(Inner::WindowsOnly(_)) | None => {
					self.0 = Some(Inner::WindowsOnly(windows));
				}
				Some(Inner::Ready(mut state)) => {
					state.replace_windows(windows);
					self.0 = Some(Inner::Ready(state));
				}
			},
			Event::WorkspacesChanged { workspaces } => match self.0.take() {
				Some(Inner::WindowsOnly(windows)) => {
					self.0 = Some(Inner::Ready(NiriState::new(windows, workspaces)));
				}
				Some(Inner::WorkspacesOnly(_)) | None => {
					self.0 = Some(Inner::WorkspacesOnly(workspaces));
				}
				Some(Inner::Ready(mut state)) => {
					state.replace_workspaces(workspaces);
					self.0 = Some(Inner::Ready(state));
				}
			},
			Event::WindowClosed { id } => {
				if let Some(Inner::Ready(state)) = &mut self.0 {
					state.remove_window(id);
				} else {
					// tracing::warn!(%self, "unexpected state for WindowClosed event");
				}
			}
			Event::WindowOpenedOrChanged { window } => {
				if let Some(Inner::Ready(state)) = &mut self.0 {
					state.upsert_window(window);
				} else {
					// tracing::warn!(%self, "unexpected state for WindowOpenedOrChanged event");
				}
			}
			Event::WindowFocusChanged { id } => {
				if let Some(Inner::Ready(state)) = &mut self.0 {
					state.set_focus(id);
				} else {
					// tracing::warn!(%self, "unexpected state for WindowFocusChanged event");
				}
			}
			Event::WindowLayoutsChanged { changes } => {
				if let Some(Inner::Ready(state)) = &mut self.0 {
					for (window_id, layout) in changes.into_iter() {
						state.update_window_layout(window_id, layout);
					}
				}
			}
			_ => {}
		}

		if let Some(Inner::Ready(state)) = &self.0 {
			Some(state.snapshot())
		} else {
			None
		}
	}
}

/// The inner state machine as we establish a new event stream.
///
/// Niri guarantees that we will get [`niri_ipc::Event::WindowsChanged`] and
/// [`niri_ipc::Event::WorkspacesChanged`] events at the start of the stream before getting any
/// update events, but not which order they'll come in, so we have to handle that as we build up
/// the window set.
enum Inner {
	WindowsOnly(Vec<NiriWindow>),
	WorkspacesOnly(Vec<Workspace>),
	Ready(NiriState),
}

struct NiriState {
	windows: BTreeMap<u64, NiriWindow>,
	workspaces: BTreeMap<u64, Workspace>,
}

impl NiriState {
	fn new(windows: Vec<NiriWindow>, workspaces: Vec<Workspace>) -> Self {
		let mut niri = NiriState {
			windows: BTreeMap::new(),
			workspaces: BTreeMap::new(),
		};

		niri.replace_workspaces(workspaces);
		niri.replace_windows(windows);

		niri
	}

	fn remove_window(&mut self, id: u64) {
		self.windows.remove(&id);
	}

	fn replace_windows(&mut self, windows: Vec<NiriWindow>) {
		self.windows = windows.into_iter().map(|window| (window.id, window)).collect();
	}

	fn replace_workspaces(&mut self, workspaces: Vec<Workspace>) {
		self.workspaces = workspaces.into_iter().map(|ws| (ws.id, ws)).collect();
	}

	fn set_focus(&mut self, id: Option<u64>) {
		// We have to manually patch up the window is_focused values.
		for window in self.windows.values_mut() {
			window.is_focused = Some(window.id) == id;
		}
	}

	fn update_window_layout(&mut self, window_id: u64, layout: WindowLayout) {
		if let Some(window) = self.windows.get_mut(&window_id) {
			window.layout = layout;
		} else {
			// tracing::warn!(window_id, ?layout, "got window layout for unknown window");
		}
	}

	fn upsert_window(&mut self, window: NiriWindow) {
		println!("Upserting window id {}", window.id);
		// Ensure that we update other windows if the new window is focused.
		if window.is_focused {
			self.windows.values_mut().for_each(|window| {
				window.is_focused = false;
			})
		}

		self.windows.insert(window.id, window);
	}

	fn snapshot(&self) -> Vec<Window> {
		self.windows
			.values()
			.filter_map(|window| {
				if let Some(ws_id) = window.workspace_id
					&& let Some(workspace) = self.workspaces.get(&ws_id)
				{
					return Some(Window {
						window: window.clone(),
						workspace: workspace.clone(),
					});
				}

				None
			})
			.collect()
	}
}

pub struct Window {
	window: NiriWindow,
	workspace: Workspace,
}

impl Window {
	pub fn output(&self) -> Option<&str> {
		self.workspace.output.as_deref()
	}

	pub fn workspace_idx(&self) -> u8 {
		self.workspace.idx
	}

	pub fn workspace_id(&self) -> u64 {
		self.workspace.id
	}
}

impl Deref for Window {
	type Target = NiriWindow;

	fn deref(&self) -> &Self::Target {
		&self.window
	}
}
