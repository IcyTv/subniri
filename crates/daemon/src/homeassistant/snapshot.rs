//! Decode Home Assistant state and registry metadata into entity snapshots.

use daemon_common::HomeassistantEntityInfo;
use hass_rs::{HassEntity, HassRegistryEntity};

use super::capabilities::entity_capabilities;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EntitySnapshot {
	info: HomeassistantEntityInfo,
	pub(super) attributes: serde_json::Value,
	registry_name: String,
	registry_icon: String,
}

impl std::ops::Deref for EntitySnapshot {
	type Target = HomeassistantEntityInfo;

	fn deref(&self) -> &Self::Target {
		&self.info
	}
}

impl std::ops::DerefMut for EntitySnapshot {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.info
	}
}

impl EntitySnapshot {
	pub(super) fn new(state: HassEntity, registry: Option<&HassRegistryEntity>) -> Self {
		let registry_name = registry
			.and_then(|entity| entity.name.clone().or_else(|| entity.original_name.clone()))
			.unwrap_or_default();
		let registry_icon = registry
			.and_then(|entity| entity.icon.clone())
			.unwrap_or_default();

		let mut snapshot = Self {
			info: HomeassistantEntityInfo {
				entity_id: state.entity_id.clone(),
				domain: state
					.entity_id
					.split_once('.')
					.map_or_else(String::new, |(domain, _)| domain.to_string()),
				device_id: registry
					.and_then(|entity| entity.device_id.clone())
					.unwrap_or_default(),
				area_id: registry
					.and_then(|entity| entity.area_id.clone())
					.unwrap_or_default(),
				platform: registry.map_or_else(String::new, |entity| entity.platform.clone()),
				entity_category: registry
					.and_then(|entity| entity.entity_category.clone())
					.into(),
				..Default::default()
			},
			registry_name,
			registry_icon,
			attributes: serde_json::Value::Null,
		};

		snapshot.apply_state(state);
		snapshot
	}

	pub(super) fn with_state(&self, state: HassEntity) -> Self {
		let mut snapshot = self.clone();
		snapshot.apply_state(state);
		snapshot
	}

	fn apply_state(&mut self, state: HassEntity) {
		let capabilities = entity_capabilities(&self.domain, &state.attributes);
		// Missing attributes while unavailable must not remove otherwise supported controls.
		if (state.state != "unavailable" && state.state != "unknown")
			|| state.attributes.get("supported_color_modes").is_some()
			|| self.capabilities.is_empty()
		{
			self.capabilities = capabilities;
		}
		self.entity_id = state.entity_id;
		self.state = state.state;
		self.available = self.state != "unavailable";
		self.friendly_name = attribute_string(&state.attributes, "friendly_name")
			.unwrap_or_else(|| self.registry_name.clone());
		self.icon = attribute_string(&state.attributes, "icon")
			.unwrap_or_else(|| self.registry_icon.clone());
		self.device_class = attribute_string(&state.attributes, "device_class").unwrap_or_default();
		self.unit_of_measurement =
			attribute_string(&state.attributes, "unit_of_measurement").unwrap_or_default();
		self.attributes_json =
			serde_json::to_string(&state.attributes).unwrap_or_else(|_| "{}".to_string());
		self.last_changed = state.last_changed;
		self.last_updated = state.last_updated;
		self.attributes = state.attributes;
	}
}

fn attribute_string(attributes: &serde_json::Value, name: &str) -> Option<String> {
	attributes.get(name)?.as_str().map(ToString::to_string)
}

#[cfg(test)]
mod tests {
	use super::*;
	use daemon_common::HomeassistantCapability;

	#[test]
	fn retains_light_capabilities_while_unavailable_and_reconciles_on_recovery() {
		let mut state = HassEntity {
			entity_id: "light.office".to_string(),
			state: "off".to_string(),
			attributes: serde_json::json!({"supported_color_modes": ["rgb", "color_temp"]}),
			last_changed: String::new(),
			last_updated: String::new(),
			context: None,
		};
		let mut snapshot = EntitySnapshot::new(state.clone(), None);
		let capabilities = snapshot.capabilities.clone();
		state.state = "unavailable".to_string();
		state.attributes = serde_json::json!({});
		snapshot.apply_state(state.clone());
		assert!(!snapshot.available);
		assert_eq!(snapshot.capabilities, capabilities);
		state.state = "on".to_string();
		state.attributes = serde_json::json!({"supported_color_modes": ["onoff"]});
		snapshot.apply_state(state);
		assert_eq!(
			snapshot.capabilities,
			vec![HomeassistantCapability::Togglable]
		);
	}

	#[test]
	fn derives_entity_properties_from_state_attributes() {
		let snapshot = EntitySnapshot::new(
			HassEntity {
				entity_id: "sensor.office_temperature".to_string(),
				last_changed: "2026-01-01T00:00:00Z".to_string(),
				state: "21.5".to_string(),
				attributes: serde_json::json!({
					"friendly_name": "Office temperature",
					"device_class": "temperature",
					"unit_of_measurement": "C"
				}),
				last_updated: "2026-01-01T00:00:01Z".to_string(),
				context: None,
			},
			None,
		);

		assert_eq!(snapshot.domain, "sensor");
		assert_eq!(snapshot.state, "21.5");
		assert!(snapshot.available);
		assert_eq!(snapshot.friendly_name, "Office temperature");
		assert_eq!(snapshot.device_class, "temperature");
		assert_eq!(snapshot.unit_of_measurement, "C");
	}
}
