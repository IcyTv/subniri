use super::*;
use hass_rs::{HassEntity, HassError, HassRegistryEntity};
use serde_json::json;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn state(value: &str) -> HassEntity {
	HassEntity {
		entity_id: "light.office".into(),
		state: value.into(),
		attributes: json!({"friendly_name": "Office", "supported_color_modes": ["onoff"]}),
		last_changed: String::new(),
		last_updated: String::new(),
		context: None,
	}
}

fn registry_payload() -> serde_json::Value {
	json!([{
		"entity_id": "light.office", "platform": "test", "unique_id": "office",
		"id": "registry-office", "categories": {}, "labels": [], "options": {},
		"created_at": 0.0, "modified_at": 0.0, "device_id": "device-office",
		"area_id": "office", "name": "Registry office", "icon": "mdi:lightbulb"
	}])
}

fn registry_decode_error() -> TestResult<HassError> {
	let error = serde_json::from_value::<Vec<HassRegistryEntity>>(registry_payload())
		.err()
		.ok_or("current hass-rs should require has_entity_name")?;
	assert!(
		error
			.to_string()
			.contains("missing field `has_entity_name`")
	);
	Ok(error.into())
}

fn valid_registry() -> TestResult<Vec<HassRegistryEntity>> {
	let mut payload = registry_payload();
	let entry = payload
		.pointer_mut("/0")
		.and_then(serde_json::Value::as_object_mut)
		.ok_or("missing registry fixture entry")?;
	entry.insert("has_entity_name".into(), json!(true));
	Ok(serde_json::from_value(payload)?)
}

fn office(snapshots: &HashMap<String, EntitySnapshot>) -> TestResult<&EntitySnapshot> {
	Ok(snapshots
		.get("light.office")
		.ok_or("missing office snapshot")?)
}

#[test]
fn registry_decode_failure_still_discovers_and_updates_states() -> TestResult {
	let mut metadata = RegistryMetadata::default();
	for value in ["off", "on"] {
		let snapshots = metadata.snapshots(vec![state(value)], Err(registry_decode_error()?))?;
		let snapshot = office(&snapshots)?;
		assert_eq!(snapshot.state, value);
		assert_eq!(snapshot.friendly_name, "Office");
		assert_eq!(snapshot.device_id, "");
		assert_eq!(
			snapshot.capabilities,
			vec![daemon_common::HomeassistantCapability::Togglable]
		);
		assert!(metadata.decode_failed);
	}
	let snapshots = metadata.snapshots(vec![state("on")], Ok(valid_registry()?))?;
	assert_eq!(office(&snapshots)?.device_id, "device-office");
	assert!(!metadata.decode_failed);
	Ok(())
}

#[test]
fn registry_decode_failure_preserves_metadata_and_recovers() -> TestResult {
	let mut metadata = RegistryMetadata::default();
	metadata.snapshots(vec![state("off")], Ok(valid_registry()?))?;
	let mut live_state = state("on");
	live_state.attributes = json!({});
	let snapshots = metadata.snapshots(vec![live_state], Err(registry_decode_error()?))?;
	let snapshot = office(&snapshots)?;
	assert_eq!(snapshot.device_id, "device-office");
	assert_eq!(snapshot.area_id, "office");
	assert_eq!(snapshot.friendly_name, "Registry office");
	assert_eq!(snapshot.icon, "mdi:lightbulb");
	assert_eq!(snapshot.state, "on");
	assert!(metadata.decode_failed);

	let mut updated = valid_registry()?;
	updated
		.first_mut()
		.ok_or("missing registry fixture entry")?
		.area_id = Some("new-office".into());
	let snapshots = metadata.snapshots(vec![state("off")], Ok(updated))?;
	assert_eq!(office(&snapshots)?.area_id, "new-office");
	assert!(!metadata.decode_failed);
	let snapshots = metadata.snapshots(vec![state("off")], Ok(vec![]))?;
	assert_eq!(office(&snapshots)?.device_id, "");
	Ok(())
}

#[test]
fn cached_registry_entries_do_not_keep_removed_states_alive() -> TestResult {
	let mut metadata = RegistryMetadata::default();
	metadata.snapshots(vec![state("off")], Ok(valid_registry()?))?;
	let snapshots = metadata.snapshots(vec![], Err(registry_decode_error()?))?;
	assert!(snapshots.is_empty());
	Ok(())
}

#[test]
fn registry_connection_and_authentication_errors_are_fatal() {
	for error in [
		HassError::ConnectionClosed,
		HassError::AuthenticationFailed("denied".into()),
	] {
		assert!(
			RegistryMetadata::default()
				.snapshots(vec![state("off")], Err(error))
				.is_err()
		);
	}
}
