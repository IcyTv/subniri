//! Select the login session polkit will independently associate with this agent.

use zbus::{Connection, Proxy};
use zvariant::OwnedObjectPath;

type SessionEntry = (String, u32, String, String, OwnedObjectPath);

pub async fn discover(connection: &Connection) -> Result<String, String> {
	let manager = Proxy::new(
		connection,
		"org.freedesktop.login1",
		"/org/freedesktop/login1",
		"org.freedesktop.login1.Manager",
	)
	.await
	.map_err(|error| format!("Cannot contact login1: {error}"))?;
	let direct: zbus::Result<OwnedObjectPath> = manager
		.call("GetSessionByPID", &(std::process::id(),))
		.await;

	match direct {
		Ok(path) => {
			let session = session_proxy(connection, &path).await?;
			if !is_graphical(&session).await? {
				return Err("thawth must run in a graphical login session".into());
			}
			return session
				.get_property("Id")
				.await
				.map_err(|error| error.to_string());
		}
		Err(zbus::Error::MethodError(name, _, _))
			if name.as_str() == "org.freedesktop.login1.NoSessionForPID" => {}
		Err(error) => return Err(format!("Cannot resolve agent login session: {error}")),
	}

	// User services have no direct session. Polkit uses this user's Display;
	// refuse to guess when multiple graphical sessions could receive the prompt.
	let uid = nix::unistd::getuid().as_raw();
	let user_path: OwnedObjectPath = manager
		.call("GetUser", &(uid,))
		.await
		.map_err(|error| error.to_string())?;
	let user = Proxy::new(
		connection,
		"org.freedesktop.login1",
		user_path,
		"org.freedesktop.login1.User",
	)
	.await
	.map_err(|error| error.to_string())?;
	let (display_id, display_path): (String, OwnedObjectPath) = user
		.get_property("Display")
		.await
		.map_err(|error| error.to_string())?;
	let sessions: Vec<SessionEntry> = manager
		.call("ListSessions", &())
		.await
		.map_err(|error| error.to_string())?;
	let mut graphical = Vec::new();

	for (id, owner, _, _, path) in sessions {
		if owner == uid && is_graphical(&session_proxy(connection, &path).await?).await? {
			graphical.push((id, path));
		}
	}

	select_display(&display_id, &display_path, &graphical)
}

async fn session_proxy<'a>(
	connection: &'a Connection, path: &OwnedObjectPath,
) -> Result<Proxy<'a>, String> {
	Proxy::new(
		connection,
		"org.freedesktop.login1",
		path.clone(),
		"org.freedesktop.login1.Session",
	)
	.await
	.map_err(|error| error.to_string())
}

async fn is_graphical(session: &Proxy<'_>) -> Result<bool, String> {
	let kind: String = session
		.get_property("Type")
		.await
		.map_err(|error| error.to_string())?;
	let state: String = session
		.get_property("State")
		.await
		.map_err(|error| error.to_string())?;
	let class: String = session
		.get_property("Class")
		.await
		.map_err(|error| error.to_string())?;

	Ok(matches!(kind.as_str(), "wayland" | "x11")
		&& state != "closing"
		&& matches!(class.as_str(), "user" | "user-early"))
}

fn select_display(
	display_id: &str, display_path: &OwnedObjectPath, graphical: &[(String, OwnedObjectPath)],
) -> Result<String, String> {
	match graphical {
		[(id, path)] if id == display_id && path == display_path => Ok(id.clone()),
		[] => Err("No graphical login session available for thawth".into()),
		[_] => Err("login1 Display does not match the graphical session; refusing an unrelated session".into()),
		_ => Err("Multiple graphical login sessions found; launch thawth directly in the intended session".into()),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn fallback_requires_unique_graphical_session_matching_display() {
		let path = OwnedObjectPath::try_from("/session/one").unwrap();
		let candidates = vec![("one".into(), path.clone())];

		assert_eq!(select_display("one", &path, &candidates).unwrap(), "one");
		assert!(select_display("other", &path, &candidates).is_err());
		assert!(select_display("one", &path, &[]).is_err());

		let ambiguous = vec![
			candidates[0].clone(),
			(
				"two".into(),
				OwnedObjectPath::try_from("/session/two").unwrap(),
			),
		];
		assert!(select_display("one", &path, &ambiguous).is_err());
	}
}
