use std::{
	hash::{DefaultHasher, Hasher as _},
	sync::Arc,
};

use async_channel::{Receiver, Sender};
use iced::widget::text::Span;
use indexer_common::{NixIndexerProxy, Package};
use zbus::Connection;

use crate::providers::{
	Activation, ActivationKey, Candidate, CandidateId, CandidateKind, MatchKind, Provider,
	ProviderContext, ProviderEvent, ProviderId, ProviderStatus, Query, SectionHint, SessionHandle,
};

const NIX_SHELL_PROVIDER_ID: ProviderId = ProviderId("nix_shell");

enum Event {
	Search(SessionHandle, Arc<str>),
}

pub struct NixShellProvider {
	sender: Sender<ProviderEvent>,
	receiver: Receiver<ProviderEvent>,

	event_sender: Sender<Event>,
	event_receiver: Receiver<Event>,
}

impl NixShellProvider {
	pub fn new() -> Self {
		let (sender, receiver) = async_channel::unbounded();
		let (event_sender, event_receiver) = async_channel::unbounded();
		Self {
			sender,
			receiver,
			event_sender,
			event_receiver,
		}
	}
}

#[async_trait::async_trait]
impl Provider for NixShellProvider {
	fn id(&self) -> ProviderId {
		NIX_SHELL_PROVIDER_ID
	}

	fn name(&self) -> &'static str {
		"Nix Shell"
	}

	async fn init(&self, _ctx: Arc<dyn ProviderContext>) -> eyre::Result<Receiver<ProviderEvent>> {
		let sender = self.sender.clone();
		let event_receiver = self.event_receiver.clone();

		tokio::task::spawn(async move {
			task(sender, event_receiver).await.unwrap();
		});

		Ok(self.receiver.clone())
	}

	async fn update_query(
		&self, session: SessionHandle, query: Query, _ctx: Arc<dyn ProviderContext>,
	) -> eyre::Result<()> {
		self.sender
			.send(ProviderEvent::Status(ProviderStatus::Loading))
			.await?;
		self.sender.send(ProviderEvent::Reset).await?;

		self.event_sender
			.send(Event::Search(session, query.raw))
			.await?;

		Ok(())
	}

	async fn activate(
		&self, _session: SessionHandle, _candidate_id: &CandidateId, _activation: &ActivationKey,
	) -> eyre::Result<Activation> {
		Ok(Activation::Noop)
	}
}

async fn task(sender: Sender<ProviderEvent>, event_receiver: Receiver<Event>) -> eyre::Result<()> {
	let connection = Connection::session().await?;
	let proxy = NixIndexerProxy::new(&connection).await?;

	while let Ok(event) = event_receiver.recv().await {
		match event {
			Event::Search(session, query) => {
				log::info!("Searching nix for '{}'", query);
				let results = proxy
					.search(&query, 10)
					.await
					.inspect_err(|e| log::warn!("{e}"))
					.unwrap_or_default();
				let candidates: Vec<_> = results
					.into_iter()
					.map(|item| item_to_cand(session, item))
					.collect();

				for cand in candidates {
					sender
						.send(ProviderEvent::CandidateUpsert(cand))
						.await
						.unwrap_or_else(|e| {
							log::error!("Failed to send candidate: {e}");
						});
				}
			}
		}

		let _ = sender.send(ProviderEvent::Done).await;
	}

	Ok(())
}
fn item_to_cand(session: SessionHandle, item: Package) -> Candidate {
	Candidate {
		session_handle: session,
		provider: NIX_SHELL_PROVIDER_ID,
		id: id_for_path(&item.attr_path),
		activation: ActivationKey(item.attr_path.clone().into()),
		title: item.attr_name.into(),
		title_spans: None,
		subtitle: Some(Arc::new([
			Span::new(
				item.attr_path
					.trim_start_matches("legacyPackages.x86_64-linux.")
					.to_string(),
			),
			Span::new("\n\n"),
			Span::new(item.description.clone()),
		])),
		right_text: None,
		icon: None,
		kind: CandidateKind::File,
		section_hint: Some(SectionHint::Files),
		match_kind: MatchKind::Fuzzy,
		provider_score: 3.0,
	}
}

fn id_for_path(path: &str) -> CandidateId {
	let mut hasher = DefaultHasher::new();
	hasher.write(path.as_bytes());
	let hash = hasher.finish();

	CandidateId(Arc::from(format!("{hash:x}")))
}
