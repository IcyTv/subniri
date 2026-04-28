use std::sync::{Arc, RwLock};

use async_channel::{Receiver, Sender};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use gio::{
	glib::object::Cast as _,
	prelude::{AppInfoExt, IconExt},
};
use launcher_common::{
	Activation, ActivationKey, Candidate, CandidateId, CandidateKind, IconRef, MatchKind, Provider, ProviderContext,
	ProviderEvent, ProviderId, ProviderStatus, Query, RuntimeHandle, SectionHint, SessionHandle,
};
use rayon::prelude::*;

const APP_PROVIDER_ID: ProviderId = ProviderId("apps");

pub struct AppProvider {
	apps: Arc<RwLock<Vec<App>>>,
	matcher: Arc<SkimMatcherV2>,
	sender: Sender<ProviderEvent>,
	receiver: Receiver<ProviderEvent>,
}

impl AppProvider {
	pub fn new() -> Self {
		let (sender, receiver) = async_channel::unbounded();
		Self {
			apps: Arc::new(RwLock::new(vec![])),
			matcher: Arc::new(SkimMatcherV2::default()),
			sender,
			receiver,
		}
	}
}

const WEIGHT_NAME: i64 = 100;
const WEIGHT_GENERIC: i64 = 80;
const WEIGHT_ACTION: i64 = 70;
const WEIGHT_KEYWORD: i64 = 60;
const WEIGHT_CATEGORY: i64 = 30;

fn analyze_match(text: &str, pattern: &str, matcher: &SkimMatcherV2) -> Option<(i64, MatchKind)> {
	if let Some((score, indices)) = matcher.fuzzy_indices(text, pattern) {
		let is_contiguous = indices.windows(2).all(|w| w[0] + 1 == w[1]);

		let match_kind = if indices.len() == text.len() {
			MatchKind::Exact
		} else if is_contiguous && indices.len() > 0 && indices[0] == 0 {
			MatchKind::Prefix
		} else if is_contiguous {
			MatchKind::Substring
		} else {
			MatchKind::Fuzzy
		};

		Some((score, match_kind))
	} else {
		None
	}
}

#[derive(Clone)]
struct App {
	id: Arc<str>,
	display_name: Arc<str>,
	icon: Option<Arc<str>>,
	actions: Arc<[Arc<str>]>,
	categories: Arc<[Arc<str>]>,
	keywords: Arc<[Arc<str>]>,
	generic_name: Option<Arc<str>>,
	subtitle: Option<Arc<str>>,
}

impl App {
	#[inline]
	fn to_cand(&self, score: i64, match_kind: MatchKind) -> Candidate {
		Candidate {
			provider: APP_PROVIDER_ID,
			id: CandidateId(self.id.clone()),
			activation: ActivationKey(self.id.clone()),
			title: self.display_name.clone(),
			subtitle: self.subtitle.clone().or_else(|| self.generic_name.clone()),
			right_text: None,
			icon: self.icon.clone().map(IconRef::SerializedIcon),
			kind: CandidateKind::App,
			section_hint: Some(SectionHint::Apps),
			match_kind,
			provider_score: score as f32,
		}
	}

	#[inline]
	fn action_candidate(&self, action_name: Arc<str>, score: i64, match_kind: MatchKind) -> Candidate {
		let id = Arc::<str>::from(format!("{}::{}", self.id, action_name));
		Candidate {
			id: CandidateId(id.clone()),
			activation: ActivationKey(id.clone()),
			title: action_name,
			subtitle: Some(self.display_name.clone()),
			kind: CandidateKind::Action,
			section_hint: Some(SectionHint::Actions),
			..self.to_cand(score, match_kind)
		}
	}
}

fn get_match(input: &str, matcher: &SkimMatcherV2, entry: &App) -> Option<(i64, MatchKind)> {
	let mut best_score = -1;
	let mut best_kind = MatchKind::Unknown;

	if let Some((score, kind)) = analyze_match(entry.display_name.as_ref(), input.as_ref(), &matcher) {
		best_score = score * WEIGHT_NAME;
		best_kind = kind;
	}

	if let Some(generic_name) = entry.generic_name.as_ref() {
		if let Some((score, kind)) = analyze_match(generic_name, input.as_ref(), &matcher) {
			let score = score * WEIGHT_GENERIC;
			if score > best_score {
				best_score = score;
				best_kind = kind;
			}
		}
	}

	for kw in entry.keywords.iter() {
		if let Some((score, kind)) = analyze_match(kw, input.as_ref(), &matcher) {
			let score = score * WEIGHT_KEYWORD;
			if score > best_score {
				best_score = score;
				best_kind = kind;
			}
		}
	}

	for cat in entry.categories.iter() {
		if let Some((score, kind)) = analyze_match(cat, input.as_ref(), &matcher) {
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

#[async_trait::async_trait]
impl Provider for AppProvider {
	fn id(&self) -> ProviderId {
		APP_PROVIDER_ID
	}
	fn name(&self) -> &'static str {
		"Applications"
	}

	async fn init(
		&self, _ctx: Arc<dyn ProviderContext>, rt: RuntimeHandle,
	) -> anyhow::Result<async_channel::Receiver<ProviderEvent>> {
		let app_list = rt
			.spawn_blocking(|| {
				gio::AppInfo::all()
					.into_iter()
					.filter(|info| info.should_show())
					.filter_map(|info| info.downcast::<gio::DesktopAppInfo>().ok())
					.map(|app| App {
						id: Arc::from(app.id().unwrap_or_else(|| app.name()).as_str()),
						display_name: Arc::from(app.display_name().as_str()),
						icon: app
							.icon()
							.and_then(|icon| icon.to_string())
							.map(|s| Arc::from(s.as_str())),
						actions: Arc::from(
							app.list_actions()
								.into_iter()
								.map(|action| app.action_name(&action))
								.map(|action| Arc::from(action.as_str()))
								.collect::<Vec<_>>(),
						),
						categories: Arc::from(
							app.categories()
								.map(|c| c.split(';').map(|c| Arc::from(c)).collect::<Vec<_>>())
								.into_iter()
								.flatten()
								.collect::<Vec<_>>(),
						),
						keywords: Arc::from(
							app.keywords()
								.into_iter()
								.map(|kw| Arc::from(kw.as_str()))
								.collect::<Vec<_>>(),
						),
						generic_name: app.generic_name().map(|n| Arc::from(n.as_str())),
						subtitle: app.string("Comment").map(|n| Arc::from(n.as_str())),
					})
					.collect::<Vec<_>>()
			})
			.await?;

		{
			let mut apps = self.apps.write().unwrap();
			*apps = app_list;
		}

		Ok(self.receiver.clone())
	}

	async fn update_query(
		&self, _session: SessionHandle, query: Query, _ctx: Arc<dyn ProviderContext>, rt: RuntimeHandle,
	) -> anyhow::Result<()> {
		let input = query.raw;

		// TODO: Figure out when/if to send a Reset
		self.sender.send(ProviderEvent::Reset).await?;
		if input.is_empty() {
			self.sender.send(ProviderEvent::Done).await?;
		}

		self.sender.send(ProviderEvent::Status(ProviderStatus::Loading)).await?;

		let apps = self.apps.clone();
		let matcher = self.matcher.clone();
		let sender = self.sender.clone();
		rt.spawn_blocking(move || {
			let apps = apps.read().unwrap();
			apps.par_iter().for_each_with(sender, |s, entry| {
				let app_match = get_match(&input, &matcher, entry);

				if let Some((score, kind)) = app_match {
					s.send_blocking(ProviderEvent::CandidateUpsert(entry.to_cand(score, kind)))
						.unwrap();
					for action in entry.actions.iter() {
						s.send_blocking(ProviderEvent::CandidateUpsert(entry.action_candidate(
							action.clone(),
							score - 10,
							kind,
						)))
						.unwrap();
					}
				} else {
					for action in entry.actions.iter() {
						if let Some((score, kind)) =
							analyze_match(&format!("{} {}", entry.display_name, action), &input, &matcher)
						{
							let score = score * WEIGHT_ACTION;
							if score > 0 {
								s.send_blocking(ProviderEvent::CandidateUpsert(entry.action_candidate(
									action.clone(),
									score,
									kind,
								)))
								.unwrap();
							}
						}
					}
				}
			});
		})
		.await?;

		self.sender.send(ProviderEvent::Done).await?;

		Ok(())
	}

	async fn activate(
		&self, _session: SessionHandle, _candidate_id: &CandidateId, _activation: &ActivationKey, _rt: RuntimeHandle,
	) -> anyhow::Result<Activation> {
		Ok(Activation::CloseLauncher)
	}
}
