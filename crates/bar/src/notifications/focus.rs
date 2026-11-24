//! Niri's event stream reports focused workspace and output without polling the desktop.
use futures::Stream;
use std::collections::HashMap;
use tokio::{
	io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
	net::UnixStream,
};

pub fn stream() -> impl Stream<Item = Option<String>> {
	async_stream::stream! {
		loop {
			let connection = async {
				let path = std::env::var_os(niri_ipc::socket::SOCKET_PATH_ENV).ok_or("NIRI_SOCKET missing")?;
				let mut socket = BufReader::new(UnixStream::connect(path).await.map_err(|_| "Niri unavailable")?);
				let mut request = serde_json::to_string(&niri_ipc::Request::EventStream).map_err(|_| "Invalid request")?;
				request.push('\n');
				socket.write_all(request.as_bytes()).await.map_err(|_| "Niri write failed")?;
				let mut line = String::new();
				socket.read_line(&mut line).await.map_err(|_| "Niri reply failed")?;
				let reply: niri_ipc::Reply = serde_json::from_str(&line).map_err(|_| "Invalid reply")?;
				if !matches!(reply, Ok(niri_ipc::Response::Handled)) { return Err("Niri rejected event stream"); }
				Ok(socket)
			}.await;
			if let Ok(mut socket) = connection {
				let mut workspaces = HashMap::new();
				let mut line = String::new();
				loop {
					line.clear();
					if !matches!(socket.read_line(&mut line).await, Ok(n) if n > 0) { break; }
					match serde_json::from_str::<niri_ipc::Event>(&line) {
						Ok(niri_ipc::Event::WorkspacesChanged { workspaces: updated }) => {
							let focus = updated.iter().find(|w| w.is_focused).and_then(|w| w.output.clone());
							workspaces = updated.into_iter().map(|w| (w.id, w.output)).collect();
							yield focus;
						}
						Ok(niri_ipc::Event::WorkspaceActivated { id, focused: true }) => yield workspaces.get(&id).cloned().flatten(),
						_ => {}
					}
				}
			}
			yield None;
			tokio::time::sleep(std::time::Duration::from_secs(2)).await;
		}
	}
}
