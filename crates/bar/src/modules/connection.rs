//! Shared mechanics only: each integration still owns its readiness and recovery.
use std::{future::Future, time::Duration};

#[derive(Debug)]
pub(crate) enum State<T> {
	Connecting,
	Ready(T),
	Unavailable { error: String, attempt: u32 },
}

impl<T> State<T> {
	pub(super) fn failed(&mut self, error: String) -> Duration {
		let attempt = match self {
			Self::Unavailable { attempt, .. } => attempt.saturating_add(1),
			_ => 0,
		};
		*self = Self::Unavailable { error, attempt };
		Duration::from_secs((1_u64 << attempt.min(5)).min(30))
	}
}

pub(super) async fn initialize<T>(
	delay: Duration, connect: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
	tokio::time::sleep(delay).await;
	tokio::time::timeout(Duration::from_secs(10), connect)
		.await
		.map_err(|_| "Service initialization timed out".to_string())?
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test(start_paused = true)]
	async fn cancellation_drops_connection_work_during_backoff_and_initialization() {
		for delay in [Duration::ZERO, Duration::from_secs(30)] {
			let (sender, mut receiver) = tokio::sync::oneshot::channel::<()>();
			let connect = async move {
				let _sender = sender;
				std::future::pending::<Result<(), String>>().await
			};
			let mut attempt = Box::pin(initialize(delay, connect));
			assert!(futures::poll!(attempt.as_mut()).is_pending());
			drop(attempt);
			assert!(matches!(
				receiver.try_recv(),
				Err(tokio::sync::oneshot::error::TryRecvError::Closed)
			));
		}
	}
}
