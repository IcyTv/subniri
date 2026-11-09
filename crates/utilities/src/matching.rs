//! Shared launcher/menu matching. Indices are character positions, never bytes.
use fuzzy_matcher::FuzzyMatcher;
pub use fuzzy_matcher::skim::SkimMatcherV2;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MatchKind {
	Exact,
	Prefix,
	Fuzzy,
	Substring,
	Typo,
	#[default]
	Unknown,
}

impl MatchKind {
	pub fn priority(self) -> i32 {
		match self {
			Self::Exact => 4,
			Self::Prefix => 3,
			Self::Substring => 2,
			Self::Typo => 1,
			Self::Fuzzy => 0,
			Self::Unknown => -1,
		}
	}
}

#[derive(Clone, Copy, Debug)]
pub struct TypoOptions {
	pub min_chars: u32,
	pub short_query_chars: u32,
	pub short_max_distance: u32,
	pub medium_query_chars: u32,
	pub medium_max_distance: u32,
	pub long_max_distance: u32,
}

impl Default for TypoOptions {
	fn default() -> Self {
		Self {
			min_chars: 3,
			short_query_chars: 4,
			short_max_distance: 1,
			medium_query_chars: 7,
			medium_max_distance: 2,
			long_max_distance: 3,
		}
	}
}

#[derive(Debug, Clone)]
pub struct Match {
	pub score: i64,
	pub kind: MatchKind,
	pub indices: Arc<[usize]>,
}

pub fn analyze(
	text: &str, pattern: &str, matcher: &SkimMatcherV2, typo: Option<&TypoOptions>,
) -> Option<Match> {
	let fuzzy = fuzzy(text, pattern, matcher);
	if fuzzy.as_ref().is_some_and(|m| {
		matches!(
			m.kind,
			MatchKind::Exact | MatchKind::Prefix | MatchKind::Substring
		)
	}) {
		return fuzzy;
	}
	typo.and_then(|options| typo_match(text, pattern, options))
		.or(fuzzy)
}

fn fuzzy(text: &str, pattern: &str, matcher: &SkimMatcherV2) -> Option<Match> {
	// Skim's case-insensitive comparison is ASCII-only. Preserve its normal
	// scoring (including word/case boundaries), falling back for Unicode case.
	let (score, indices) = matcher.fuzzy_indices(text, pattern).or_else(|| {
		if text.is_ascii() && pattern.is_ascii() {
			return None;
		}
		let mut folded = String::new();
		let mut positions = Vec::new();
		for (index, ch) in text.chars().enumerate() {
			for lower in ch.to_lowercase() {
				folded.push(lower);
				positions.push(index);
			}
		}
		let (score, indices) = matcher.fuzzy_indices(&folded, &pattern.to_lowercase())?;
		let mut indices = indices
			.into_iter()
			.filter_map(|i| positions.get(i).copied())
			.collect::<Vec<_>>();
		indices.dedup();
		Some((score, indices))
	})?;
	let contiguous = indices
		.windows(2)
		.all(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a + 1 == *b));
	let kind = if indices.len() == text.chars().count() {
		MatchKind::Exact
	} else if contiguous && indices.first() == Some(&0) {
		MatchKind::Prefix
	} else if contiguous {
		MatchKind::Substring
	} else {
		MatchKind::Fuzzy
	};
	Some(Match {
		score,
		kind,
		indices: indices.into(),
	})
}

pub fn typo_match(text: &str, pattern: &str, options: &TypoOptions) -> Option<Match> {
	let len = pattern.chars().count();
	if len < options.min_chars as usize || len == 0 {
		return None;
	}
	let max = if len <= options.short_query_chars as usize {
		options.short_max_distance
	} else if len <= options.medium_query_chars as usize {
		options.medium_max_distance
	} else {
		options.long_max_distance
	} as usize;
	let pattern = pattern.to_lowercase();
	let mut best: Option<Match> = None;
	let chars: Vec<char> = text.chars().collect();
	let mut offset = 0;
	for token in chars.split(|c| !c.is_alphanumeric()) {
		if token.len() >= len {
			let prefix = token.iter().take(len).collect::<String>().to_lowercase();
			let distance = strsim::damerau_levenshtein(&pattern, &prefix);
			let score = 48 - distance as i64 * 12 - offset.min(20) as i64;
			if distance > 0
				&& distance <= max
				&& score > 0 && best.as_ref().is_none_or(|m| score > m.score)
			{
				best = Some(Match {
					score,
					kind: MatchKind::Typo,
					indices: (offset..offset + len).collect::<Vec<_>>().into(),
				});
			}
		}
		offset += token.len() + 1;
	}
	best
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	#[test]
	fn exact_prefix_substring_typo_and_fuzzy_share_priority() {
		let matcher = SkimMatcherV2::default().ignore_case();
		let typo = TypoOptions::default();
		for (text, query, kind) in [
			("Open", "open", MatchKind::Exact),
			("Open With", "open", MatchKind::Prefix),
			("Reopen", "open", MatchKind::Substring),
			("Firefox", "frie", MatchKind::Typo),
			("Open With", "ow", MatchKind::Fuzzy),
		] {
			assert_eq!(
				analyze(text, query, &matcher, Some(&typo)).unwrap().kind,
				kind
			);
		}
		assert!(MatchKind::Substring.priority() > MatchKind::Typo.priority());
		assert!(MatchKind::Typo.priority() > MatchKind::Fuzzy.priority());
	}
	#[test]
	fn unicode_exact_matches_and_highlighting_use_characters() {
		let m = analyze(
			"Éditer",
			"éditer",
			&SkimMatcherV2::default().ignore_case(),
			None,
		)
		.unwrap();
		assert_eq!(m.kind, MatchKind::Exact);
		assert_eq!(m.indices.as_ref(), &[0, 1, 2, 3, 4, 5]);
		assert_eq!(
			typo_match("工具 Firefox", "frie", &TypoOptions::default())
				.unwrap()
				.indices
				.as_ref(),
			&[3, 4, 5, 6]
		);
	}
	#[test]
	fn typo_limits_are_configurable_and_empty_queries_are_safe() {
		assert!(typo_match("Firefox", "fo", &TypoOptions::default()).is_none());
		assert!(
			typo_match(
				"Firefox",
				"fo",
				&TypoOptions {
					min_chars: 2,
					..Default::default()
				}
			)
			.is_some()
		);
		assert!(
			typo_match(
				"",
				"",
				&TypoOptions {
					min_chars: 0,
					..Default::default()
				}
			)
			.is_none()
		);
		assert!(analyze("", "zzz", &SkimMatcherV2::default(), None).is_none());
	}
}
