//! Synthetic UI preview: never registers with polkit or opens its helper socket.
//! Pass --smoke to close automatically after a few seconds.

use std::{
	collections::HashMap,
	io::Write,
	sync::{Arc, atomic::AtomicU8},
};

#[path = "../src/app.rs"]
mod app;
#[path = "../src/dialog.rs"]
mod dialog;
#[path = "../src/model.rs"]
// This synthetic backend intentionally exercises only a subset of shared messages.
#[allow(dead_code)]
mod model;

#[path = "../src/metadata.rs"]
#[allow(dead_code)]
mod metadata;

fn main() -> iced::Result {
	let smoke = std::env::args().any(|argument| argument == "--smoke");
	let multiple = std::env::args().any(|argument| argument == "--multiple-identities");
	let long = std::env::args().any(|argument| argument == "--long-operation");
	let structured = std::env::args().any(|argument| argument == "--structured-details");
	let uid = nix::unistd::getuid().as_raw();
	let mut identities = vec![model::Identity {
		uid,
		username: "preview-user".into(),
	}];

	if multiple {
		identities.push(model::Identity {
			uid: if uid == 0 { 1001 } else { 0 },
			username: "administrator".into(),
		});
	}

	let command = if long {
		format!(
			"/run/current-system/sw/bin/example --document '{}' --check",
			"very-long-document-name/".repeat(30)
		)
	} else {
		"/run/current-system/sw/bin/true".into()
	};

	let message =
		format!("Authentication is needed to run `{command}' as the super user. (Preview only.)");
	let details = if structured {
		HashMap::from([
			("command_line".into(), command),
			("user".into(), "root".into()),
		])
	} else {
		// Model the agent-visible payload, not pkexec's CheckAuthorization input.
		HashMap::from([
			("polkit.caller-pid".into(), "1234".into()),
			("polkit.subject-pid".into(), "1234".into()),
		])
	};
	let mut presentation = metadata::Presentation::from_details("system-run".into(), &details);
	presentation.description = Some("Run a program as another user (preview)".into());

	let (events, receiver) = async_channel::unbounded();
	let (commands, responses) = async_channel::unbounded();
	let request = model::Request {
		id: 1,
		message,
		action: "org.freedesktop.policykit.exec".into(),
		identities,
		presentation,
	};

	events
		.try_send(model::Event::Started(request, commands))
		.unwrap();
	events
		.try_send(model::Event::Prompt {
			id: 1,
			generation: 1,
			text: "Password: ".into(),
			secret: true,
		})
		.unwrap();

	std::thread::spawn(move || {
		let mut generation = 1;
		let mut trace = std::env::var_os("THAWTH_PREVIEW_EVENTS")
			.and_then(|path| std::fs::File::create(path).ok());

		if smoke {
			std::thread::sleep(std::time::Duration::from_secs(5));
		} else {
			while let Ok(command) = responses.recv_blocking() {
				match command {
					model::Command::Cancel => {
						if let Some(trace) = &mut trace {
							let _ = writeln!(trace, "cancel");
						}
						break;
					}
					model::Command::Respond { .. } => {
						if let Some(trace) = &mut trace {
							let _ = writeln!(trace, "responded");
						}
						let _ = events.send_blocking(model::Event::Info {
							id: 1,
							text: "Preview only: no credentials were checked.".into(),
							error: true,
						});
						let _ = events.send_blocking(model::Event::Retry { id: 1 });
					}
					model::Command::Retry => {
						generation += 1;
						if let Some(trace) = &mut trace {
							let _ = writeln!(trace, "retry");
						}
						let _ = events.send_blocking(model::Event::Prompt {
							id: 1,
							generation,
							text: "Password: ".into(),
							secret: true,
						});
					}
					model::Command::SelectIdentity(uid) => {
						generation += 1;
						if let Some(trace) = &mut trace {
							let _ = writeln!(trace, "identity:{uid}");
						}
						let _ = events.send_blocking(model::Event::Reset { id: 1, uid });
						let _ = events.send_blocking(model::Event::Prompt {
							id: 1,
							generation,
							text: "Password: ".into(),
							secret: true,
						});
					}
				}
			}
		}

		let _ = events.send_blocking(model::Event::Finished { id: 1 });
		let _ = events.send_blocking(model::Event::Fatal {
			code: 0,
			message: "Preview finished".into(),
		});
	});

	app::App::run(receiver, Arc::new(AtomicU8::new(0)))
}
