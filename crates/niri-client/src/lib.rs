use std::collections::{BTreeMap, HashMap};
use std::ops::Deref;

use async_channel::{Receiver, Sender};
use futures::Stream;
pub use niri_ipc::Event;
use niri_ipc::socket::Socket;
use niri_ipc::{Action, Output, Request, Response, Window as NiriWindow, WindowLayout, Workspace};

pub use niri_ipc::{
	Output as NiriOutput,
	Window as NiriWindowRaw,
	WindowLayout as NiriWindowLayout,
	Workspace as NiriWorkspace,
};

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

	pub fn screenshot(&self, show_pointer: bool, path: Option<String>) -> bool {
		let mut socket = match Socket::connect() {
			Ok(socket) => socket,
			Err(err) => {
				eprintln!("Failed to connect to niri IPC socket: {err}");
				return false;
			}
		};

		let reply = socket.send(Request::Action(Action::Screenshot { show_pointer, path }));
		matches!(reply, Ok(Ok(Response::Handled)))
	}

	pub fn outputs(&self) -> HashMap<String, Output> {
		fetch_outputs()
	}

	pub fn workspaces(&self) -> Vec<Workspace> {
		fetch_workspaces()
	}

	pub fn window_stream(&self) -> WindowStream {
		WindowStream::new()
	}

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
					Ok(Event::WorkspacesChanged { workspaces }) => yield workspaces,
					Ok(_) => {}
					Err(e) => eprintln!("Niri IPC error reading from event stream: {e}"),
				}
			}
		}
	}
}

pub fn fetch_outputs() -> HashMap<String, Output> {
	let mut socket = match Socket::connect() {
		Ok(socket) => socket,
		Err(_) => return HashMap::new(),
	};

	match socket.send(Request::Outputs) {
		Ok(Ok(Response::Outputs(outputs))) => outputs,
		_ => HashMap::new(),
	}
}

pub fn fetch_workspaces() -> Vec<Workspace> {
	let mut socket = match Socket::connect() {
		Ok(socket) => socket,
		Err(_) => return Vec::new(),
	};

	match socket.send(Request::Workspaces) {
		Ok(Ok(Response::Workspaces(workspaces))) => workspaces,
		_ => Vec::new(),
	}
}

pub fn event_stream() -> impl Stream<Item = Event> + use<> {
	let (tx, rx) = async_channel::unbounded();

	std::thread::spawn(move || {
		loop {
			let mut socket = match Socket::connect() {
				Ok(socket) => socket,
				Err(err) => {
					eprintln!("Failed to connect to niri IPC socket: {err}");
					std::thread::sleep(std::time::Duration::from_secs(1));
					continue;
				}
			};

			match socket.send(Request::EventStream) {
				Ok(Ok(Response::Handled)) => {}
				Ok(reply) => {
					eprintln!("Failed to request event stream: {reply:?}");
					std::thread::sleep(std::time::Duration::from_secs(1));
					continue;
				}
				Err(err) => {
					eprintln!("Failed to request event stream: {err}");
					std::thread::sleep(std::time::Duration::from_secs(1));
					continue;
				}
			}

			let mut next_event = socket.read_events();
			while let Ok(event) = next_event() {
				if tx.send_blocking(event).is_err() {
					return;
				}
			}

			eprintln!("Niri IPC event stream disconnected, reconnecting...");
			std::thread::sleep(std::time::Duration::from_secs(1));
		}
	});

	async_stream::stream! {
		while let Ok(event) = rx.recv().await {
			yield event;
		}
	}
}

pub fn focus_first_window_matching_app_ids(app_ids: &[String]) -> bool {
	if app_ids.is_empty() {
		return false;
	}

	let mut socket = match Socket::connect() {
		Ok(socket) => socket,
		Err(_) => return false,
	};

	let windows = match socket.send(Request::Windows) {
		Ok(Ok(Response::Windows(windows))) => windows,
		_ => return false,
	};

	for window in windows {
		if let Some(app_id) = window.app_id.as_deref()
			&& app_ids.iter().any(|candidate| candidate == app_id)
		{
			let reply = socket.send(Request::Action(Action::FocusWindow { id: window.id }));
			return matches!(reply, Ok(Ok(Response::Handled)));
		}
	}

	false
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
				}
			}
			Event::WindowOpenedOrChanged { window } => {
				if let Some(Inner::Ready(state)) = &mut self.0 {
					state.upsert_window(window);
				}
			}
			Event::WindowFocusChanged { id } => {
				if let Some(Inner::Ready(state)) = &mut self.0 {
					state.set_focus(id);
				}
			}
			Event::WindowLayoutsChanged { changes } => {
				if let Some(Inner::Ready(state)) = &mut self.0 {
					for (window_id, layout) in changes {
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
		for window in self.windows.values_mut() {
			window.is_focused = Some(window.id) == id;
		}
	}

	fn update_window_layout(&mut self, window_id: u64, layout: WindowLayout) {
		if let Some(window) = self.windows.get_mut(&window_id) {
			window.layout = layout;
		}
	}

	fn upsert_window(&mut self, window: NiriWindow) {
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
