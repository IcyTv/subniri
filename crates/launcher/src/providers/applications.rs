use std::{
	collections::HashSet,
	fs,
	io::{self, Write},
	os::{fd::AsRawFd, unix::net::UnixStream},
	process::Stdio,
	sync::Arc,
	time::Duration,
};

use async_channel::{Receiver, Sender};
use config::LauncherTypoSearch;
use freedesktop_desktop_entry::{DesktopEntry, desktop_entries, get_languages_from_env};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use iced::widget::{span, text::Span};
use neo_widgets::{icons::resolve_icon, style::COLORS};
use strsim::damerau_levenshtein;
use tokio::{process::Command, sync::RwLock};
use zbus::zvariant::{OwnedObjectPath, Value};

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
	typo_search: LauncherTypoSearch,
}

impl ApplicationProvider {
	pub fn new(typo_search: LauncherTypoSearch) -> Self {
		let (sender, receiver) = async_channel::unbounded();
		let languages = get_languages_from_env();
		Self {
			sender,
			receiver,
			languages,
			entries: Arc::new(RwLock::new(vec![])),
			matcher: Arc::new(SkimMatcherV2::default()),
			typo_search,
		}
	}

	fn entry_to_candidate(
		session_handle: SessionHandle, locales: &[String], entry: &DesktopEntry,
		app_match: AppMatch,
	) -> Candidate {
		let title = entry_title(entry, locales);
		let subtitle_text = entry
			.comment(locales)
			.or(entry.generic_name(locales))
			.map(|s| Arc::<str>::from(&*s));

		let title_spans = if app_match.field == MatchField::Title {
			Some(highlight_spans(&title, &app_match.indices))
		} else {
			None
		};

		let subtitle = match app_match.field {
			MatchField::Title => subtitle_text.as_deref().map(plain_spans),
			MatchField::GenericName | MatchField::Comment => subtitle_text.as_deref().map(|text| {
				if text == app_match.text.as_ref() {
					highlight_spans(text, &app_match.indices)
				} else {
					plain_spans(text)
				}
			}),
			MatchField::Keyword | MatchField::Category => {
				Some(highlight_spans(&app_match.text, &app_match.indices))
			}
		};

		Candidate {
			session_handle,
			provider: APP_PROVIDER_ID,
			id: CandidateId(Arc::from(format!("app_{}", entry.appid))),
			activation: ActivationKey(Arc::from(entry.appid.as_str())),
			title,
			title_spans,
			subtitle,
			right_text: None,
			icon: Some(resolve_icon(&entry.appid, 32, 2)),
			kind: CandidateKind::App,
			section_hint: Some(SectionHint::Apps),
			match_kind: app_match.kind,
			provider_score: app_match.score as f32,
		}
	}
}

fn highlight_spans(text: &str, indices: &[usize]) -> Arc<[Span<'static, ()>]> {
	// TODO: This might be better with a linear search on an array, since indices should generally
	// be quite small
	let matched: HashSet<usize> = indices.iter().copied().collect();
	let mut spans = Vec::new();
	let mut buf = String::new();
	let mut current_highlight = None;

	for (idx, ch) in text.chars().enumerate() {
		let highlight = matched.contains(&idx);

		if current_highlight == Some(highlight) {
			buf.push(ch);
			continue;
		}

		if !buf.is_empty() {
			spans.push(span_for(
				std::mem::take(&mut buf),
				current_highlight.unwrap_or_default(),
			));
		}

		current_highlight = Some(highlight);
		buf.push(ch);
	}

	if !buf.is_empty() {
		spans.push(span_for(buf, current_highlight.unwrap_or_default()));
	}

	Arc::from(spans)
}

fn plain_spans(text: &str) -> Arc<[Span<'static, ()>]> {
	Arc::from([span(text.to_owned())])
}

fn span_for(text: String, highlight: bool) -> Span<'static, ()> {
	let mut span = span(text);
	if highlight {
		span = span.color(COLORS.decorative.purple);
	}

	span
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
		&self, session: SessionHandle, query: Query, _ctx: Arc<dyn ProviderContext>,
	) -> eyre::Result<()> {
		let search = query.raw.clone();
		let entries = self.entries.clone();
		let langs = self.languages.clone();
		let matcher = self.matcher.clone();
		let typo_search = self.typo_search.clone();

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
					let app_match = get_match(&search, &matcher, &typo_search, entry, &langs);

					if let Some(m) = app_match {
						let cand = Self::entry_to_candidate(session, &langs, entry, m);
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

				// Spawn the process, but keep it blocked in `pre_exec` until we have had a
				// chance to move its PID out of avalaunch.service. Without the pause, a fast
				// app could exec or fork before systemd sees the transient scope assignment.
				let paused_child = spawn_paused_detached(cmd, args)?;

				if let Some(pid) = paused_child.id()
					&& running_under_systemd_service()
				{
					match tokio::time::timeout(
						Duration::from_secs(2),
						move_process_to_systemd_scope(pid),
					)
					.await
					{
						Ok(Ok(())) => (),
						Ok(Err(error)) => {
							log::warn!("Failed to move launched app into systemd scope: {error}");
						}
						Err(_) => {
							log::warn!("Timed out moving launched app into systemd scope");
						}
					}
				}

				// Always release the child, even if scope creation failed. Scope handoff is a
				// robustness improvement, not a reason to prevent the requested app launch.
				paused_child.release()?;
			}

			Ok(Activation::CloseLauncher)
		} else {
			Ok(Activation::KeepOpen)
		}
	}
}

struct PausedChild {
	child: tokio::process::Child,
	release: UnixStream,
}

impl PausedChild {
	fn id(&self) -> Option<u32> {
		self.child.id()
	}

	fn release(mut self) -> io::Result<()> {
		self.release.write_all(&[1])
	}
}

fn spawn_paused_detached(cmd: &str, args: &[String]) -> eyre::Result<PausedChild> {
	// This socket pair is an exec gate. The child blocks on `wait` in `pre_exec`,
	// and the parent writes to `release` after the optional systemd scope handoff.
	let (release, wait) = UnixStream::pair()?;
	let wait_fd = wait.as_raw_fd();
	let max_fd = open_fd_limit();

	let mut command = Command::new(cmd);
	command
		.args(args)
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null());

	// SAFETY: `pre_exec` runs in the child after `fork` and before `exec`, so the closure only
	// calls async-signal-safe libc functions (`close`, `setsid`, `read`) and avoids allocation,
	// locking, or touching shared Rust state. The raw fds come from `UnixStream::pair`, remain open
	// in the parent until `spawn` returns, and are inherited by the child. All inherited fds except
	// stdio and the wait socket are closed before `exec`, so launched apps do not receive avalaunch's
	// DBus/socket handles.
	let child = unsafe {
		command
			.pre_exec(move || {
				// The parent moves this PID to a systemd scope before allowing exec. First,
				// close inherited avalaunch fds so the launched app cannot keep DBus sockets,
				// layer-shell/display handles, or other internal resources alive or usable.
				close_inherited_fds(max_fd, wait_fd);

				if libc::setsid() == -1 {
					let error = io::Error::last_os_error();
					libc::close(wait_fd);
					return Err(error);
				}

				let mut byte = 0_u8;
				loop {
					// Block here until the parent either completes the systemd handoff or gives
					// up. EOF also releases the child, so this remains a best-effort handoff if
					// avalaunch exits or drops the release socket unexpectedly. This is a blocking
					// socket read, not polling; the loop only retries if a signal interrupts it.
					let read = libc::read(wait_fd, (&raw mut byte).cast(), 1);
					if read == 1 || read == 0 {
						break;
					}

					let error = io::Error::last_os_error();
					if read == -1 && error.raw_os_error() == Some(libc::EINTR) {
						continue;
					}

					libc::close(wait_fd);
					return Err(error);
				}

				if libc::close(wait_fd) == -1 {
					return Err(io::Error::last_os_error());
				}

				Ok(())
			})
			.spawn()
	};

	match child {
		Ok(child) => Ok(PausedChild { child, release }),
		Err(error) => Err(error.into()),
	}
}

fn open_fd_limit() -> i32 {
	// Capture the limit in the parent before fork. Reading `/proc/self/fd` would be
	// more precise, but directory iteration allocates and is not suitable for the
	// `pre_exec` child path.
	let mut limit = libc::rlimit {
		rlim_cur: 0,
		rlim_max: 0,
	};

	// SAFETY: `getrlimit` writes to the valid `limit` pointer for `RLIMIT_NOFILE` and does not retain
	// the pointer after returning.
	if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) } == 0
		&& limit.rlim_cur != libc::RLIM_INFINITY
	{
		return i32::try_from(limit.rlim_cur).unwrap_or(i32::MAX);
	}

	1024
}

fn close_inherited_fds(max_fd: i32, keep_fd: i32) {
	// Stdio is already redirected to `/dev/null` by `Command`; keep only the gate
	// socket so the child can wait for the parent. Everything else belongs to
	// avalaunch and should not survive into the launched application.
	for fd in 3..max_fd {
		if fd != keep_fd {
			// SAFETY: Closing arbitrary inherited file descriptors in the forked child is safe here.
			// Invalid or already-closed descriptors simply return `EBADF`, which is intentionally ignored.
			unsafe {
				libc::close(fd);
			}
		}
	}
}

async fn move_process_to_systemd_scope(pid: u32) -> zbus::Result<()> {
	let connection = zbus::Connection::session().await?;
	let proxy = zbus::Proxy::new(
		&connection,
		"org.freedesktop.systemd1",
		"/org/freedesktop/systemd1",
		"org.freedesktop.systemd1.Manager",
	)
	.await?;

	let unit = format!("avalaunch-app-{pid}.scope");
	let description = format!("Application launched by avalaunch ({pid})");
	let properties = [
		("Description", Value::new(description)),
		("PIDs", Value::new(vec![pid])),
	];
	let auxiliary_units: [(&str, Vec<(&str, Value<'_>)>); 0] = [];

	let _: OwnedObjectPath = proxy
		.call(
			"StartTransientUnit",
			&(
				unit.as_str(),
				"fail",
				properties.as_slice(),
				auxiliary_units.as_slice(),
			),
		)
		.await?;

	Ok(())
}

fn running_under_systemd_service() -> bool {
	fs::read_to_string("/proc/self/cgroup").is_ok_and(|cgroup| {
		cgroup.lines().any(|line| {
			let path = line.rsplit_once(':').map_or(line, |(_, path)| path);
			path.split('/').any(|part| part.ends_with(".service"))
		})
	})
}

const WEIGHT_NAME: i64 = 100;
const WEIGHT_GENERIC: i64 = 80;
const WEIGHT_ACTION: i64 = 70;
const WEIGHT_KEYWORD: i64 = 60;
const WEIGHT_CATEGORY: i64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum MatchField {
	Category,
	Keyword,
	Comment,
	GenericName,
	Title,
}

struct AppMatch {
	score: i64,
	kind: MatchKind,
	field: MatchField,
	text: Arc<str>,
	indices: Arc<[usize]>,
}

impl Default for AppMatch {
	fn default() -> Self {
		Self {
			score: -1,
			kind: MatchKind::Unknown,
			field: MatchField::Category,
			text: Arc::from(""),
			indices: Arc::new([]),
		}
	}
}

impl AppMatch {
	fn weight(mut self, weight: i64) -> Self {
		self.score *= weight;
		self
	}

	fn max(&mut self, other: Self) {
		if other.rank() > self.rank() {
			*self = other;
		}
	}

	fn rank(&self) -> (MatchField, i32, i64) {
		(self.field, self.kind.priority(), self.score)
	}
}

fn entry_title(entry: &DesktopEntry, locales: &[String]) -> Arc<str> {
	entry
		.full_name(locales)
		.or(entry.name(locales))
		.or(entry.generic_name(locales))
		.map_or_else(|| Arc::from("Unknown"), |s| Arc::from(&*s))
}

fn get_match(
	input: &str, matcher: &SkimMatcherV2, typo_search: &LauncherTypoSearch, entry: &DesktopEntry,
	locales: &[String],
) -> Option<AppMatch> {
	let mut best = AppMatch::default();
	let title = entry_title(entry, locales);

	if let Some(m) = analyze_match(&title, input, matcher, Some(typo_search)) {
		best = m.with_text(title, MatchField::Title).weight(WEIGHT_NAME);
	}

	if let Some(generic_name) = entry.generic_name(locales)
		&& let Some(m) = analyze_match(&generic_name, input, matcher, Some(typo_search))
	{
		best.max(
			m.with_text(generic_name, MatchField::GenericName)
				.weight(WEIGHT_GENERIC),
		);
	}

	if let Some(comment) = entry.comment(locales)
		&& let Some(m) = analyze_match(&comment, input, matcher, None)
	{
		best.max(
			m.with_text(comment, MatchField::Comment)
				.weight(WEIGHT_ACTION),
		);
	}

	for kw in entry.keywords(locales).iter().flatten() {
		if let Some(m) = analyze_match(kw, input, matcher, None) {
			best.max(
				m.with_text(kw.as_ref(), MatchField::Keyword)
					.weight(WEIGHT_KEYWORD),
			);
		}
	}

	for cat in entry.categories().iter().flatten() {
		if let Some(m) = analyze_match(cat, input, matcher, None) {
			best.max(
				m.with_text(cat.as_ref(), MatchField::Category)
					.weight(WEIGHT_CATEGORY),
			)
		}
	}

	if best.score > 0 { Some(best) } else { None }
}

struct AnalyzeMatch {
	score: i64,
	kind: MatchKind,
	indices: Arc<[usize]>,
}

impl AnalyzeMatch {
	fn with_text(self, text: impl Into<Arc<str>>, field: MatchField) -> AppMatch {
		AppMatch {
			score: self.score,
			kind: self.kind,
			field,
			text: text.into(),
			indices: self.indices,
		}
	}
}

fn analyze_match(
	text: &str, pattern: &str, matcher: &SkimMatcherV2, typo_search: Option<&LauncherTypoSearch>,
) -> Option<AnalyzeMatch> {
	let fuzzy = analyze_fuzzy_match(text, pattern, matcher);

	if fuzzy.as_ref().is_some_and(|m| {
		matches!(
			m.kind,
			MatchKind::Exact | MatchKind::Prefix | MatchKind::Substring
		)
	}) {
		return fuzzy;
	}

	if let Some(typo_search) = typo_search {
		analyze_typo_match(text, pattern, typo_search).or(fuzzy)
	} else {
		fuzzy
	}
}

fn analyze_fuzzy_match(text: &str, pattern: &str, matcher: &SkimMatcherV2) -> Option<AnalyzeMatch> {
	let (score, indices) = matcher.fuzzy_indices(text, pattern)?;

	#[allow(clippy::indexing_slicing)]
	let is_contiguous = indices.windows(2).all(|w| w[0] + 1 == w[1]);

	let kind = if indices.len() == text.len() {
		MatchKind::Exact
	} else if is_contiguous && indices.first().is_some_and(|i| *i == 0) {
		MatchKind::Prefix
	} else if is_contiguous {
		MatchKind::Substring
	} else {
		MatchKind::Fuzzy
	};

	Some(AnalyzeMatch {
		score,
		kind,
		indices: Arc::from(indices),
	})
}

fn analyze_typo_match(
	text: &str, pattern: &str, typo_search: &LauncherTypoSearch,
) -> Option<AnalyzeMatch> {
	let pattern_len = pattern.chars().count();
	let max_distance = typo_distance_limit(pattern_len, typo_search)?;
	let pattern = pattern.to_lowercase();
	let mut best: Option<AnalyzeMatch> = None;

	for token in typo_tokens(text) {
		if token.indices.len() < pattern_len {
			continue;
		}

		let token_prefix = token
			.chars
			.iter()
			.take(pattern_len)
			.collect::<String>()
			.to_lowercase();
		let distance = damerau_levenshtein(&pattern, &token_prefix);

		if distance == 0 || distance > max_distance {
			continue;
		}

		let start = token.indices[0];
		let score = 48 - (distance as i64 * 12) - (start.min(20) as i64);
		if score <= 0 {
			continue;
		}

		let candidate = AnalyzeMatch {
			score,
			kind: MatchKind::Typo,
			indices: Arc::from(&token.indices[..pattern_len]),
		};

		if best
			.as_ref()
			.is_none_or(|best| candidate.score > best.score)
		{
			best = Some(candidate);
		}
	}

	best
}

fn typo_distance_limit(pattern_len: usize, typo_search: &LauncherTypoSearch) -> Option<usize> {
	if pattern_len < typo_search.min_chars as usize {
		None
	} else if pattern_len <= typo_search.short_query_chars as usize {
		Some(typo_search.short_max_distance as usize)
	} else if pattern_len <= typo_search.medium_query_chars as usize {
		Some(typo_search.medium_max_distance as usize)
	} else {
		Some(typo_search.long_max_distance as usize)
	}
}

struct TypoToken {
	chars: Vec<char>,
	indices: Vec<usize>,
}

fn typo_tokens(text: &str) -> impl Iterator<Item = TypoToken> + '_ {
	let mut tokens = Vec::new();
	let mut chars = Vec::new();
	let mut indices = Vec::new();

	for (idx, ch) in text.chars().enumerate() {
		if ch.is_alphanumeric() {
			chars.push(ch);
			indices.push(idx);
		} else if !chars.is_empty() {
			tokens.push(TypoToken { chars, indices });
			chars = Vec::new();
			indices = Vec::new();
		}
	}

	if !chars.is_empty() {
		tokens.push(TypoToken { chars, indices });
	}

	tokens.into_iter()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn typo_match_accepts_one_substitution_in_title_prefix() {
		let m = analyze_typo_match("Firefox", "fore", &LauncherTypoSearch::default())
			.expect("fore should typo-match fire");

		assert_eq!(m.kind, MatchKind::Typo);
		assert_eq!(m.indices.as_ref(), &[0, 1, 2, 3]);
	}

	#[test]
	fn typo_match_accepts_one_transposition_in_title_prefix() {
		let m = analyze_typo_match("Firefox", "frie", &LauncherTypoSearch::default())
			.expect("frie should typo-match fire");

		assert_eq!(m.kind, MatchKind::Typo);
		assert_eq!(m.indices.as_ref(), &[0, 1, 2, 3]);
	}

	#[test]
	fn typo_match_rejects_short_patterns() {
		assert!(analyze_typo_match("Firefox", "fo", &LauncherTypoSearch::default()).is_none());
	}

	#[test]
	fn typo_match_uses_configured_min_chars() {
		let typo_search = LauncherTypoSearch {
			min_chars: 2,
			..Default::default()
		};

		let m = analyze_typo_match("Firefox", "fo", &typo_search)
			.expect("configured min_chars should allow two-character typo matching");

		assert_eq!(m.indices.as_ref(), &[0, 1]);
	}
}
