//! One request may contain multiple prompts and fresh helper attempts.

use std::path::Path;

use crate::{
	helper::{Record, Session},
	model::{Command, Event, Identity, Request, Secret},
};

enum Outcome {
	Success,
	Failed,
	Identity(Identity),
	Cancelled,
}

pub async fn run(
	request: Request, cookie: Secret, commands: async_channel::Receiver<Command>,
	command_sender: async_channel::Sender<Command>, events: async_channel::Sender<Event>,
	helper_socket: &Path,
) -> bool {
	let Some(mut identity) = request
		.identities
		.iter()
		.find(|identity| identity.uid == nix::unistd::getuid().as_raw())
		.or_else(|| request.identities.first())
		.cloned()
	else {
		return false;
	};

	let id = request.id;
	if events
		.send(Event::Started(request.clone(), command_sender))
		.await
		.is_err()
	{
		return false;
	}

	let mut generation = 0;

	loop {
		let _ = events
			.send(Event::Reset {
				id,
				uid: identity.uid,
			})
			.await;
		let outcome = attempt(
			&request,
			&cookie,
			&identity,
			&commands,
			&events,
			&mut generation,
			helper_socket,
		)
		.await;

		match outcome {
			Outcome::Success => return true,
			Outcome::Cancelled => return false,
			Outcome::Identity(selected) => identity = selected,
			Outcome::Failed => {
				let _ = events.send(Event::Retry { id }).await;

				loop {
					match commands.recv().await {
						Ok(Command::Retry) => break,
						Ok(Command::SelectIdentity(uid)) => {
							if let Some(selected) = request
								.identities
								.iter()
								.find(|identity| identity.uid == uid)
							{
								identity = selected.clone();
								break;
							}
						}
						Ok(Command::Cancel) | Err(_) => return false,
						Ok(Command::Respond { .. }) => {}
					}
				}
			}
		}
	}
}

async fn attempt(
	request: &Request, cookie: &Secret, identity: &Identity,
	commands: &async_channel::Receiver<Command>, events: &async_channel::Sender<Event>,
	generation: &mut u64, helper_socket: &Path,
) -> Outcome {
	let connecting = Session::connect(helper_socket, &identity.username, cookie.as_str());
	tokio::pin!(connecting);

	let mut session = loop {
		tokio::select! {
			connection = &mut connecting => match connection {
				Ok(session) => break session,
				Err(_) => {
					let _ = events.send(Event::Info {
						id: request.id,
						text: concat!(
							"Cannot connect to the polkit authentication helper. ",
							"Check that system polkit and its helper socket are enabled.",
						).into(),
						error: true,
					}).await;
					return Outcome::Failed;
				}
			},
			command = commands.recv() => {
				if let Some(outcome) = interrupt(command.ok(), request) {
					return outcome;
				}
			}
		}
	};

	let mut pending_prompt = None;

	loop {
		tokio::select! {
			record = session.next() => match record {
				Ok(Record::Prompt { text, secret }) => {
					*generation += 1;
					pending_prompt = Some(*generation);
					let _ = events.send(Event::Prompt {
						id: request.id, generation: *generation, text, secret,
					}).await;
				}
				Ok(Record::Info { text, error }) => {
					let _ = events.send(Event::Info { id: request.id, text, error }).await;
				}
				Ok(Record::Success) => return Outcome::Success,
				Ok(Record::Failure) => return Outcome::Failed,
				Err(_) => {
					let _ = events.send(Event::Info {
						id: request.id,
						text: "Authentication helper disconnected or sent an invalid response.".into(),
						error: true,
					}).await;
					return Outcome::Failed;
				}
			},
			command = commands.recv() => {
				let command = command.ok();

				if let Some(Command::Respond { generation, response }) = &command
					&& pending_prompt == Some(*generation)
				{
					pending_prompt = None;
					let writing = session.respond(response.as_str());
					tokio::pin!(writing);

					loop {
						tokio::select! {
							result = &mut writing => {
								if result.is_err() {
									return Outcome::Failed;
								}
								break;
							}
							command = commands.recv() => {
								if let Some(outcome) = interrupt(command.ok(), request) {
									return outcome;
								}
							}
						}
					}
				} else if let Some(outcome) = interrupt(command, request) {
					return outcome;
				}
			}
		}
	}
}

fn interrupt(command: Option<Command>, request: &Request) -> Option<Outcome> {
	match command {
		None | Some(Command::Cancel) => Some(Outcome::Cancelled),
		Some(Command::SelectIdentity(uid)) => request
			.identities
			.iter()
			.find(|identity| identity.uid == uid)
			.cloned()
			.map(Outcome::Identity),
		Some(Command::Respond { .. } | Command::Retry) => None,
	}
}
