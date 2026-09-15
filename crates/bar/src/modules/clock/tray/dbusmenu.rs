use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use zbus::zvariant::{self, OwnedValue, Type, Value};

pub type PropertyMap = HashMap<String, OwnedValue>;
pub type UpdatedProperties = Vec<(i32, PropertyMap)>;
pub type RemovedProperties = Vec<(i32, Vec<String>)>;

#[zbus::proxy(
	interface = "com.canonical.dbusmenu",
	default_service = "com.canonical.dbusmenu",
	default_path = "/com/canonical/dbusmenu"
)]
pub trait DbusMenu {
	#[zbus(name = "GetLayout")]
	#[doc(hidden)]
	fn __get_layout_inner(
		&self, parent_id: i32, recursion_depth: i32, property_names: Vec<&str>,
	) -> zbus::Result<(u32, LayoutWire)>;

	#[zbus(name = "AboutToShow")]
	fn about_to_show(&self, id: i32) -> zbus::Result<bool>;

	#[zbus(name = "Event")]
	fn event(&self, id: i32, event_id: &str, data: &Value<'_>, timestamp: u32) -> zbus::Result<()>;

	#[zbus(signal, name = "LayoutUpdated")]
	fn layout_updated(&self, revision: u32, parent: i32) -> zbus::Result<()>;

	#[zbus(signal, name = "ItemsPropertiesUpdated")]
	fn items_properties_updated(
		&self, updated_props: UpdatedProperties, removed_props: RemovedProperties,
	) -> zbus::Result<()>;
}

impl<'a> DbusMenuProxy<'a> {
	pub async fn get_layout(
		&self, parent_id: i32, recursion_depth: i32, property_names: Vec<&str>,
	) -> zbus::Result<(u32, Layout)> {
		let (revision, layout_wire) = self
			.__get_layout_inner(parent_id, recursion_depth, property_names)
			.await?;
		let layout = layout_wire.try_into()?;
		Ok((revision, layout))
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, zvariant::OwnedValue)]
pub struct LayoutWire {
	id: i32,
	properties: HashMap<String, OwnedValue>,
	children: Vec<OwnedValue>,
}

#[derive(Debug, Clone)]
pub struct Layout {
	pub id: i32,
	pub properties: LayoutProperties,
	pub children: Vec<Layout>,
}

impl Layout {
	pub fn find(&self, id: i32) -> Option<&Self> {
		if self.id == id {
			return Some(self);
		}

		self.children.iter().find_map(|child| child.find(id))
	}

	pub fn find_mut(&mut self, id: i32) -> Option<&mut Self> {
		if self.id == id {
			return Some(self);
		}

		self.children
			.iter_mut()
			.find_map(|child| child.find_mut(id))
	}

	pub fn replace_subtree(&mut self, replacement: Self) -> Option<Self> {
		let target = self.find_mut(replacement.id)?;
		Some(std::mem::replace(target, replacement))
	}

	pub fn apply_property_updates(
		&mut self, updated: &UpdatedProperties, removed: &RemovedProperties,
	) {
		for (id, properties) in updated {
			if let Some(item) = self.find_mut(*id) {
				item.properties.apply_updates(properties);
			}
		}

		for (id, properties) in removed {
			if let Some(item) = self.find_mut(*id) {
				item.properties.remove(properties);
			}
		}
	}
}

impl TryFrom<LayoutWire> for Layout {
	type Error = zvariant::Error;

	fn try_from(wire: LayoutWire) -> Result<Self, Self::Error> {
		let children = wire
			.children
			.into_iter()
			.map(|value| {
				let child = LayoutWire::try_from(value)?;
				Layout::try_from(child)
			})
			.collect::<Result<Vec<_>, _>>()?;

		let mut properties = LayoutProperties::default();

		properties.apply_updates(&wire.properties);

		Ok(Self {
			id: wire.id,
			properties,
			children,
		})
	}
}

impl LayoutProperties {
	pub fn apply_updates(&mut self, updates: &PropertyMap) {
		for (key, value) in updates {
			match key.as_str() {
				"type" => {
					if let Ok(ty) = value.downcast_ref::<&str>() {
						self.ty = match ty {
							"standard" => LayoutItemType::Standard,
							"separator" => LayoutItemType::Separator,
							_ => LayoutItemType::Unknown,
						}
					}
				}
				"label" => {
					if let Ok(label) = value.downcast_ref::<&str>() {
						self.label = label.to_string();
					}
				}
				"enabled" => {
					if let Ok(enabled) = value.downcast_ref::<bool>() {
						self.enabled = enabled;
					}
				}
				"visible" => {
					if let Ok(visible) = value.downcast_ref::<bool>() {
						self.visible = visible;
					}
				}
				"icon-name" => {
					if let Ok(icon_name) = value.downcast_ref::<&str>() {
						self.icon_name = icon_name.to_string();
					}
				}
				"icon-data" => {
					if let Ok(icon_data) = Vec::<u8>::try_from(value.clone()) {
						self.icon_data = icon_data;
					}
				}
				"shortcut" => {
					if let Ok(shortcut) = Vec::<Vec<String>>::try_from(value.clone()) {
						self.shortcut = shortcut;
					}
				}
				"toggle-type" => {
					if let Ok(toggle_type) = value.downcast_ref::<&str>() {
						self.toggle_type = match toggle_type {
							"checkmark" => ToggleType::Checkmark,
							"radio" => ToggleType::Radio,
							_ => ToggleType::Empty,
						};
					}
				}
				"toggle-state" => {
					if let Ok(toggle_state) = value.downcast_ref::<i32>() {
						self.toggle_state = match toggle_state {
							0 => ToggleState::Off,
							1 => ToggleState::On,
							_ => ToggleState::Indeterminate,
						};
					}
				}
				"children-display" => {
					if let Ok(display) = value.downcast_ref::<&str>() {
						self.children_display = match display {
							"none" => ChildrenDisplay::None,
							"submenu" => ChildrenDisplay::Submenu,
							_ => ChildrenDisplay::Unknown,
						};
					}
				}
				"disposition" => {
					if let Ok(disposition) = value.downcast_ref::<&str>() {
						self.disposition = match disposition {
							"informative" => Disposition::Informative,
							"warning" => Disposition::Warning,
							"alert" => Disposition::Alert,
							_ => Disposition::Unknown,
						};
					}
				}
				_ => {
					self.unknown.insert(key.clone(), value.clone());
				}
			}
		}
	}

	pub fn remove(&mut self, names: &[String]) {
		let defaults = Self::default();
		for name in names {
			match name.as_str() {
				"type" => self.ty = defaults.ty,
				"label" => self.label.clear(),
				"enabled" => self.enabled = defaults.enabled,
				"visible" => self.visible = defaults.visible,
				"icon-name" => self.icon_name.clear(),
				"icon-data" => self.icon_data.clear(),
				"shortcut" => self.shortcut.clear(),
				"toggle-type" => self.toggle_type = defaults.toggle_type,
				"toggle-state" => self.toggle_state = defaults.toggle_state,
				"children-display" => self.children_display = defaults.children_display,
				"disposition" => self.disposition = defaults.disposition,
				_ => {
					self.unknown.remove(name);
				}
			}
		}
	}
}

#[derive(Debug, Clone)]
pub struct LayoutProperties {
	pub ty: LayoutItemType,
	pub label: String,
	pub enabled: bool,
	pub visible: bool,
	pub icon_name: String,
	pub icon_data: Vec<u8>,
	pub shortcut: Vec<Vec<String>>,
	pub toggle_type: ToggleType,
	pub toggle_state: ToggleState,
	pub children_display: ChildrenDisplay,
	pub disposition: Disposition,

	pub unknown: HashMap<String, OwnedValue>,
}

impl Default for LayoutProperties {
	fn default() -> Self {
		Self {
			ty: Default::default(),
			label: Default::default(),
			enabled: true,
			visible: true,
			icon_name: Default::default(),
			icon_data: Default::default(),
			shortcut: Default::default(),
			toggle_type: Default::default(),
			toggle_state: Default::default(),
			children_display: Default::default(),
			disposition: Default::default(),
			unknown: Default::default(),
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutItemType {
	#[default]
	Standard,
	Separator,
	Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToggleType {
	#[default]
	Empty,
	Checkmark,
	Radio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToggleState {
	Off,
	On,
	#[default]
	Indeterminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChildrenDisplay {
	#[default]
	None,
	Submenu,
	Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Disposition {
	#[default]
	Normal,
	Informative,
	Warning,
	Alert,
	Unknown,
}

#[cfg(test)]
mod tests {
	use super::*;

	fn layout(id: i32, label: &str, children: Vec<Layout>) -> Layout {
		Layout {
			id,
			properties: LayoutProperties {
				label: label.to_string(),
				..LayoutProperties::default()
			},
			children,
		}
	}

	#[test]
	fn layout_wire_conversion_is_recursive_and_uses_protocol_defaults()
	-> Result<(), zvariant::Error> {
		let mut grandchild_properties = PropertyMap::new();
		grandchild_properties.insert(
			"label".to_string(),
			OwnedValue::try_from(Value::from("_Open"))?,
		);
		let grandchild = LayoutWire {
			id: 2,
			properties: grandchild_properties,
			children: Vec::new(),
		};

		let mut child_properties = PropertyMap::new();
		child_properties.insert(
			"type".to_string(),
			OwnedValue::try_from(Value::from("separator"))?,
		);
		let child = LayoutWire {
			id: 1,
			properties: child_properties,
			children: vec![OwnedValue::try_from(grandchild)?],
		};
		let root = Layout::try_from(LayoutWire {
			id: 0,
			properties: PropertyMap::new(),
			children: vec![OwnedValue::try_from(child)?],
		})?;

		assert_eq!(root.id, 0);
		assert!(root.properties.enabled);
		assert!(root.properties.visible);
		assert_eq!(root.children[0].properties.ty, LayoutItemType::Separator);
		assert_eq!(root.children[0].children[0].properties.label, "_Open");
		Ok(())
	}

	#[test]
	fn lookup_and_subtree_replacement_use_ids_not_mnemonics() {
		let mut root = layout(
			0,
			"_Root",
			vec![layout(1, "_File", vec![layout(2, "_Open", Vec::new())])],
		);

		assert_eq!(
			root.find(2).map(|item| item.properties.label.as_str()),
			Some("_Open")
		);
		assert!(root.find(99).is_none());

		let old = root.replace_subtree(layout(1, "Fi_le", vec![layout(3, "_Save", Vec::new())]));
		assert_eq!(
			old.map(|item| item.properties.label),
			Some("_File".to_string())
		);
		assert!(root.find(2).is_none());
		assert_eq!(
			root.find(3).map(|item| item.properties.label.as_str()),
			Some("_Save")
		);
	}

	#[test]
	fn property_updates_parse_typed_arrays_and_removals() -> Result<(), zvariant::Error> {
		let mut updates = PropertyMap::new();
		updates.insert("enabled".to_string(), OwnedValue::from(false));
		updates.insert(
			"icon-data".to_string(),
			OwnedValue::try_from(Value::from(vec![1_u8, 2, 3]))?,
		);
		updates.insert(
			"shortcut".to_string(),
			OwnedValue::try_from(Value::from(vec![
				vec!["Control".to_string(), "Q".to_string()],
				vec!["Alt".to_string(), "F4".to_string()],
			]))?,
		);
		updates.insert("vendor-property".to_string(), OwnedValue::from(7_i32));

		let mut properties = LayoutProperties::default();
		properties.apply_updates(&updates);

		assert!(!properties.enabled);
		assert_eq!(properties.icon_data, vec![1, 2, 3]);
		assert_eq!(
			properties.shortcut,
			vec![vec!["Control", "Q"], vec!["Alt", "F4"]]
		);
		assert!(properties.unknown.contains_key("vendor-property"));

		properties.remove(&[
			"enabled".to_string(),
			"icon-data".to_string(),
			"shortcut".to_string(),
			"vendor-property".to_string(),
		]);
		assert!(properties.enabled);
		assert!(properties.icon_data.is_empty());
		assert!(properties.shortcut.is_empty());
		assert!(!properties.unknown.contains_key("vendor-property"));
		Ok(())
	}
}
