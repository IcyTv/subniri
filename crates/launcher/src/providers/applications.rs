use std::{process::Stdio, sync::Arc};

use async_channel::{Receiver, Sender};
use freedesktop_desktop_entry::{DesktopEntry, desktop_entries, get_languages_from_env};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use tokio::{process::Command, sync::RwLock};

use crate::providers::{
	Activation, ActivationKey, Candidate, CandidateId, CandidateKind, MatchKind, Provider,
	ProviderContext, ProviderEvent, ProviderId, ProviderStatus, Query, SectionHint, SessionHandle,
};

const APP_PROVIDER_ID: ProviderId = ProviderId("apps");

pub struct ApplicationProvider {
	sender: Sender<ProviderEvent>,
	receiver: Receiver<ProviderEvent>,
	languages: Vec<String>,
	entries: Arc<RwLock<Vec<DesktopEntry>>>,
	matcher: Arc<SkimMatcherV2>,
}

impl ApplicationProvider {
	pub fn new() -> Self {
		let (sender, receiver) = async_channel::unbounded();
		let languages = get_languages_from_env();
		Self {
			sender,
			receiver,
			languages,
			entries: Arc::new(RwLock::new(vec![])),
			matcher: Arc::new(SkimMatcherV2::default()),
		}
	}

	fn entry_to_candidate(
		locales: &[String], entry: &DesktopEntry, score: i64, match_kind: MatchKind,
	) -> Candidate {
		let title = entry
			.full_name(locales)
			.or(entry.generic_name(locales))
			.map_or_else(|| Arc::from("Unknown"), |s| Arc::from(&*s));

		let subtitle = entry
			.comment(locales)
			.or(entry.generic_name(locales))
			.map(|s| Arc::from(&*s));

		Candidate {
			provider: APP_PROVIDER_ID,
			id: CandidateId(Arc::from(format!("app_{}", entry.appid))),
			activation: ActivationKey(Arc::from(entry.appid.as_str())),
			title,
			subtitle,
			right_text: None,
			icon: None,
			kind: CandidateKind::App,
			section_hint: Some(SectionHint::Apps),
			match_kind,
			provider_score: score as f32,
		}
	}
}

#[async_trait::async_trait]
impl Provider for ApplicationProvider {
	fn id(&self) -> ProviderId {
		APP_PROVIDER_ID
	}

	fn name(&self) -> &'static str {
		"Applications"
	}

	async fn init(&self, _ctx: Arc<dyn ProviderContext>) -> eyre::Result<Receiver<ProviderEvent>> {
		let langs = self.languages.clone();
		let entries = tokio::task::spawn_blocking(move || desktop_entries(&langs)).await?;
		let mut lock = self.entries.write().await;
		*lock = entries;

		Ok(self.receiver.clone())
	}

	async fn update_query(
		&self, _session: SessionHandle, query: Query, _ctx: Arc<dyn ProviderContext>,
	) -> eyre::Result<()> {
		let search = query.raw.clone();
		let entries = self.entries.clone();
		let langs = self.languages.clone();
		let matcher = self.matcher.clone();

		self.sender.send(ProviderEvent::Reset).await?;
		self.sender
			.send(ProviderEvent::Status(ProviderStatus::Loading))
			.await?;

		let sender = self.sender.clone();

		tokio::task::spawn_blocking(move || {
			let entries = entries.blocking_read();

			log::trace!("Checking {} entries", entries.len());

			for entry in entries.iter() {
				if !entry.hidden() {
					let app_match = get_match(&search, &matcher, entry, &langs);

					if let Some((score, kind)) = app_match {
						let cand = Self::entry_to_candidate(&langs, entry, score, kind);
						let _ = sender.send_blocking(ProviderEvent::CandidateUpsert(cand));
					}
				}
			}
		})
		.await?;

		self.sender.send(ProviderEvent::Done).await?;

		Ok(())
	}

	async fn activate(
		&self, _session: SessionHandle, _candidate_id: &CandidateId, activation: &ActivationKey,
	) -> eyre::Result<Activation> {
		let entries = self.entries.read().await;
		let entry = entries.iter().find(|e| *e.id() == *activation.0);

		if let Some(entry) = entry {
			if let Ok(exec) = entry.parse_exec() {
				let Some(cmd) = exec.first() else {
					log::error!("No command to launch");
					return Err(eyre::eyre!("No command to launch"));
				};

				let args = exec.get(1..).unwrap_or(&[]);

				unsafe {
					Command::new(cmd)
						.args(args)
						.stdin(Stdio::null())
						.stdout(Stdio::null())
						.stderr(Stdio::null())
						.pre_exec(|| {
							libc::setsid();
							Ok(())
						})
						.spawn()?;
				}
			}

			Ok(Activation::CloseLauncher)
		} else {
			Ok(Activation::KeepOpen)
		}
	}
}

const WEIGHT_NAME: i64 = 100;
const WEIGHT_GENERIC: i64 = 80;
const WEIGHT_ACTION: i64 = 70;
const WEIGHT_KEYWORD: i64 = 60;
const WEIGHT_CATEGORY: i64 = 30;

fn get_match(
	input: &str, matcher: &SkimMatcherV2, entry: &DesktopEntry, locales: &[String],
) -> Option<(i64, MatchKind)> {
	let mut best_score = -1;
	let mut best_kind = MatchKind::Unknown;

	if let Some(name) = entry.name(locales)
		&& let Some((score, kind)) = analyze_match(&name, input, matcher)
	{
		best_score = score * WEIGHT_NAME;
		best_kind = kind;
	}

	if let Some(generic_name) = entry.generic_name(locales)
		&& let Some((score, kind)) = analyze_match(&generic_name, input, matcher)
	{
		let score = score * WEIGHT_GENERIC;
		if score > best_score {
			best_score = score;
			best_kind = kind;
		}
	}

	for kw in entry.keywords(locales).iter().flatten() {
		if let Some((score, kind)) = analyze_match(kw, input, matcher) {
			let score = score * WEIGHT_KEYWORD;
			if score > best_score {
				best_score = score;
				best_kind = kind;
			}
		}
	}

	for cat in entry.categories().iter().flatten() {
		if let Some((score, kind)) = analyze_match(cat, input, matcher) {
			let score = score * WEIGHT_CATEGORY;
			if score > best_score {
				best_score = score;
				best_kind = kind;
			}
		}
	}

	if best_score > 0 {
		Some((best_score, best_kind))
	} else {
		None
	}
}

fn analyze_match(text: &str, pattern: &str, matcher: &SkimMatcherV2) -> Option<(i64, MatchKind)> {
	let (score, indices) = matcher.fuzzy_indices(text, pattern)?;

	#[allow(clippy::indexing_slicing)]
	let is_contiguous = indices.windows(2).all(|w| w[0] + 1 == w[1]);

	let match_kind = if indices.len() == text.len() {
		MatchKind::Exact
	} else if is_contiguous && indices.first().is_some_and(|i| *i == 0) {
		MatchKind::Prefix
	} else if is_contiguous {
		MatchKind::Substring
	} else {
		MatchKind::Fuzzy
	};

	Some((score, match_kind))
}
