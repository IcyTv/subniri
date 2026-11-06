//! HTTP failures never include response bodies, request URLs or credentials.
use std::time::Duration;

use tokio::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
	AuthenticationRequired,
	RateLimited(Duration),
	SessionThrottled,
	Unavailable,
	Forbidden,
	InvalidResponse,
}

impl std::fmt::Display for Failure {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::AuthenticationRequired => f.write_str("Spotify requires pairing"),
			Self::RateLimited(delay) => write!(
				f,
				"Spotify cooldown: retry in {} seconds",
				delay.as_secs().saturating_add(1)
			),
			Self::SessionThrottled => f.write_str(
				"Spotify session was throttled without exposing retry timing; automatic reconnect is paused",
			),
			Self::Unavailable => f.write_str("Spotify is temporarily unavailable"),
			Self::Forbidden => f.write_str("Spotify denied access to this operation"),
			Self::InvalidResponse => f.write_str("Spotify returned an unsupported response"),
		}
	}
}

impl std::error::Error for Failure {}

/// Only fixed stage/origin labels and numeric metadata belong here. Never pass
/// dependency errors, request URLs, headers, response bodies or credentials.
pub(super) fn diagnose(
	stage: &'static str, status: Option<u16>, origin: &'static str, retry: Option<Duration>,
	failure: Failure,
) -> Failure {
	log::warn!(
		"{}",
		diagnostic_message(stage, status, origin, retry, &failure)
	);
	failure
}

fn diagnostic_message(
	stage: &'static str, status: Option<u16>, origin: &'static str, retry: Option<Duration>,
	failure: &Failure,
) -> String {
	format!(
		"Spotify diagnostic: stage={stage} http_status={} cooldown_origin={origin} retry_seconds={} failure={failure}",
		status.map_or_else(|| "none".into(), |value| value.to_string()),
		retry.map_or_else(
			|| "none".into(),
			|value| value
				.as_secs()
				.saturating_add(u64::from(value.subsec_nanos() > 0))
				.to_string()
		),
	)
}

pub(super) fn cooldown_origin(retry: Option<Duration>) -> &'static str {
	if retry.is_some_and(|value| !value.is_zero()) {
		"server_retry_after"
	} else {
		"local_backoff"
	}
}

#[derive(Default)]
pub struct Cooldown {
	until: Option<Instant>,
	failures: u32,
	indefinite: bool,
}

impl Cooldown {
	pub fn check(&self) -> Result<(), Failure> {
		if self.indefinite {
			return Err(Failure::RateLimited(Duration::MAX));
		}
		if let Some(until) = self.until {
			let remaining = until.saturating_duration_since(Instant::now());
			if !remaining.is_zero() {
				return Err(Failure::RateLimited(remaining));
			}
		}
		Ok(())
	}

	pub fn success(&mut self) {
		self.failures = 0;
	}

	pub fn defer(&mut self, retry_after: Option<Duration>) -> Duration {
		self.failures = self.failures.saturating_add(1);
		let fallback =
			Duration::from_secs(30_u64.saturating_mul(1 << self.failures.min(6)).min(3600));
		// Never cap the server's requested wait. A missing/zero header gets backoff.
		let delay = retry_after
			.filter(|delay| !delay.is_zero())
			.unwrap_or(fallback)
			.saturating_add(Duration::from_millis(
				u64::from(rand::random::<u16>()) % 1000,
			));
		let Some(deadline) = Instant::now().checked_add(delay) else {
			self.indefinite = true;
			return delay;
		};
		self.until = Some(
			self.until
				.map_or(deadline, |previous| previous.max(deadline)),
		);
		delay
	}
}

pub fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
	let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
	if let Ok(seconds) = value.parse::<u64>() {
		return Some(Duration::from_secs(seconds));
	}
	let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
	(date.with_timezone(&chrono::Utc) - chrono::Utc::now())
		.to_std()
		.ok()
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn diagnostics_distinguish_server_errors_and_local_suppression() {
		let retry = Some(Duration::from_secs(120));
		let message = diagnostic_message(
			"token_refresh",
			Some(503),
			cooldown_origin(retry),
			retry,
			&Failure::RateLimited(retry.unwrap()),
		);
		assert!(message.contains(
			"stage=token_refresh http_status=503 cooldown_origin=server_retry_after retry_seconds=120"
		));
		let message = diagnostic_message(
			"library_read",
			None,
			"existing_local_cooldown",
			retry,
			&Failure::RateLimited(retry.unwrap()),
		);
		assert!(message.contains("http_status=none cooldown_origin=existing_local_cooldown"));
		assert_eq!(cooldown_origin(None), "local_backoff");
		assert_eq!(cooldown_origin(Some(Duration::ZERO)), "local_backoff");
	}
	#[tokio::test(start_paused = true)]
	async fn cooldown_blocks_all_following_requests_and_honors_long_server_waits() {
		let mut cooldown = Cooldown::default();
		let wait = cooldown.defer(Some(Duration::from_secs(7200)));
		assert!(wait >= Duration::from_secs(7200));
		assert!(matches!(cooldown.check(), Err(Failure::RateLimited(_))));
		tokio::time::advance(wait).await;
		assert_eq!(cooldown.check(), Ok(()));
	}
	#[tokio::test(start_paused = true)]
	async fn malformed_or_missing_headers_use_increasing_backoff() {
		let mut cooldown = Cooldown::default();
		let first = cooldown.defer(None);
		let second = cooldown.defer(Some(Duration::ZERO));
		assert!(first >= Duration::from_secs(60));
		assert!(second > first);
	}
	#[test]
	fn parses_seconds_and_http_dates_without_ignoring_invalid_headers() {
		let mut headers = reqwest::header::HeaderMap::new();
		headers.insert("retry-after", "125".parse().unwrap());
		assert_eq!(retry_after(&headers), Some(Duration::from_secs(125)));
		headers.insert("retry-after", "not-a-time".parse().unwrap());
		assert_eq!(retry_after(&headers), None);
		let date = (chrono::Utc::now() + chrono::Duration::minutes(5))
			.format("%a, %d %b %Y %H:%M:%S GMT")
			.to_string();
		headers.insert("retry-after", date.parse().unwrap());
		assert!(retry_after(&headers).unwrap() > Duration::from_secs(290));
	}
}
