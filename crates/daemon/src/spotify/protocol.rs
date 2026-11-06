//! The private protocol seam. Unknown fields are fine; missing semantics are not.
use std::{
	collections::{HashSet, VecDeque},
	io::Read,
};

use base64::Engine;
use serde::Deserialize;
use serde_json::Value;

const MAX_PAYLOAD: usize = 1024 * 1024;
const COLLECTION_PREFIX: &str = "hm://collection/collection/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackChange {
	pub id: String,
	pub saved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
	Connected,
	Collection {
		changes: Vec<TrackChange>,
		update_id: Option<String>,
	},
	Reconcile,
	Ignored,
}

#[derive(Deserialize)]
struct Collection {
	items: Vec<Value>,
}

#[derive(Deserialize)]
struct TrackItem {
	identifier: String,
	removed: bool,
}

#[must_use]
pub fn valid_track_id(id: &str) -> bool {
	id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// Decode a Dealer envelope without assuming protobuf field meanings.
#[must_use]
pub fn decode(envelope: &Value, username: &str) -> Event {
	let Some(uri) = envelope.get("uri").and_then(Value::as_str) else {
		return Event::Ignored;
	};
	if uri.starts_with("hm://pusher/v1/connections/") {
		return Event::Connected;
	}
	let Some(suffix) = uri.strip_prefix(COLLECTION_PREFIX) else {
		return Event::Ignored;
	};
	if suffix != format!("{username}/json") {
		return Event::Ignored;
	}
	let Some(payload) = payload(envelope) else {
		return Event::Reconcile;
	};
	let Ok(collection) = serde_json::from_value::<Collection>(payload) else {
		return Event::Reconcile;
	};
	let mut changes = Vec::new();
	for item in collection.items {
		match item.get("type").and_then(Value::as_str) {
			Some("track") => {}
			Some(_) => continue,
			None => return Event::Reconcile,
		}
		let Ok(item) = serde_json::from_value::<TrackItem>(item) else {
			return Event::Reconcile;
		};
		if !valid_track_id(&item.identifier) {
			return Event::Reconcile;
		}
		changes.push(TrackChange {
			id: item.identifier,
			saved: !item.removed,
		});
	}
	let update_id = envelope
		.get("headers")
		.and_then(|v| v.get("Collection-Update-Id"))
		.and_then(Value::as_str)
		.filter(|id| id.len() <= 256)
		.map(str::to_owned);
	Event::Collection { changes, update_id }
}

fn payload(envelope: &Value) -> Option<Value> {
	let payloads = envelope.get("payloads")?.as_array()?;
	if payloads.len() != 1 {
		return None;
	}
	let payload = payloads.first()?;
	if payload.is_object() {
		return Some(payload.clone());
	}
	let bytes = if let Some(text) = payload.as_str() {
		if text.len() > MAX_PAYLOAD {
			return None;
		}
		if let Ok(value) = serde_json::from_str::<Value>(text) {
			return Some(value);
		}
		base64::prelude::BASE64_STANDARD.decode(text).ok()?
	} else {
		payload
			.as_array()?
			.iter()
			.map(|n| n.as_u64().and_then(|n| u8::try_from(n).ok()))
			.collect::<Option<Vec<_>>>()?
	};
	if bytes.len() > MAX_PAYLOAD {
		return None;
	}
	let encoding = envelope
		.get("headers")
		.and_then(|v| v.get("Transfer-Encoding"))
		.and_then(Value::as_str);
	let bytes = if encoding == Some("gzip") {
		let mut decoded = Vec::new();
		flate2::read::GzDecoder::new(bytes.as_slice())
			.take((MAX_PAYLOAD + 1) as u64)
			.read_to_end(&mut decoded)
			.ok()?;
		if decoded.len() > MAX_PAYLOAD {
			return None;
		}
		decoded
	} else {
		bytes
	};
	serde_json::from_slice(&bytes).ok()
}

#[derive(Default)]
pub struct Deduplicator {
	seen: HashSet<String>,
	order: VecDeque<String>,
}

impl Deduplicator {
	pub fn accept(&mut self, id: Option<&str>) -> bool {
		let Some(id) = id else {
			return true;
		};
		if !self.seen.insert(id.to_owned()) {
			return false;
		}
		self.order.push_back(id.to_owned());
		if self.order.len() > 256
			&& let Some(old) = self.order.pop_front()
		{
			self.seen.remove(&old);
		}
		true
	}
}

#[derive(Default)]
pub struct RevisionTracker {
	last: Option<String>,
}

impl RevisionTracker {
	/// Revisions are opaque links, never numbers. A missing link asks for one
	/// coalesced reconciliation; the new chain head remains usable afterwards.
	pub fn gap(&mut self, envelope: &Value) -> bool {
		let headers = envelope.get("headers");
		let source = headers
			.and_then(|value| value.get("Collection-Source-Revision"))
			.and_then(Value::as_str);
		let next = headers
			.and_then(|value| value.get("MC-ETag"))
			.and_then(Value::as_str);
		let gap = match (self.last.as_deref(), source) {
			(Some(last), Some(source)) => last != source,
			_ => false,
		};
		self.last = next.filter(|value| value.len() <= 256).map(str::to_owned);
		gap
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;
	fn envelope(item: Value) -> Value {
		json!({"uri":"hm://collection/collection/user/json", "payloads":[{"items":[item]}]})
	}
	#[test]
	fn save_unsave_extra_fields_and_unrelated_items() {
		for removed in [false, true] {
			let event = decode(
				&envelope(
					json!({"type":"track","identifier":"3uADmRFuyQBoCwUn19B6lV","removed":removed,"future":42}),
				),
				"user",
			);
			assert_eq!(
				event,
				Event::Collection {
					changes: vec![TrackChange {
						id: "3uADmRFuyQBoCwUn19B6lV".into(),
						saved: !removed
					}],
					update_id: None
				}
			);
		}
		assert_eq!(
			decode(&envelope(json!({"type":"album"})), "user"),
			Event::Collection {
				changes: vec![],
				update_id: None
			}
		);
	}
	#[test]
	fn missing_or_wrong_fields_never_mean_unsaved() {
		for item in [
			json!({"type":"track"}),
			json!({"type":"track","identifier":"3uADmRFuyQBoCwUn19B6lV","removed":"false"}),
		] {
			assert_eq!(decode(&envelope(item), "user"), Event::Reconcile);
		}
		assert_eq!(decode(&envelope(json!({})), "other-user"), Event::Ignored);
	}
	#[test]
	fn replay_live_capture_without_network_or_account_identifiers() {
		let mut states = Vec::new();
		for line in include_str!("fixtures/collection.jsonl").lines() {
			let envelope: Value = serde_json::from_str(line).unwrap();
			if let Event::Collection { changes, .. } = decode(&envelope, "fixture-user") {
				states.extend(changes.into_iter().map(|c| c.saved));
			}
		}
		assert_eq!(states, [true, true, false, false]);
	}
	#[test]
	fn opaque_ids_are_deduplicated_with_bounded_memory() {
		let mut seen = Deduplicator::default();
		assert!(seen.accept(Some("999")));
		assert!(!seen.accept(Some("999")));
		assert!(seen.accept(Some("1"))); // Not a monotonically increasing revision.
		for n in 0..1000 {
			seen.accept(Some(&format!("update-{n}")));
		}
		assert_eq!(seen.seen.len(), 256);
	}
	#[test]
	fn revision_chain_detects_gaps_without_numeric_order_assumptions() {
		let mut revisions = RevisionTracker::default();
		assert!(
			!revisions.gap(&json!({"headers":{"Collection-Source-Revision":"999","MC-ETag":"10"}}))
		);
		assert!(
			!revisions
				.gap(&json!({"headers":{"Collection-Source-Revision":"10","MC-ETag":"opaque"}}))
		);
		assert!(
			revisions
				.gap(&json!({"headers":{"Collection-Source-Revision":"missing","MC-ETag":"next"}}))
		);
		assert!(
			!revisions.gap(
				&json!({"headers":{"Collection-Source-Revision":"next","MC-ETag":"following"}})
			)
		);
	}
}
