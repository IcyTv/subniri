//! Capability detection, D-Bus interaction interfaces, and entity commands.

use std::{sync::atomic::Ordering, time::Duration};

use daemon_common::{
	HomeassistantBrightness, HomeassistantBrightnessServer, HomeassistantCallable,
	HomeassistantCallableServer, HomeassistantCapability, HomeassistantColor,
	HomeassistantColorServer, HomeassistantColorTemperature, HomeassistantColorTemperatureServer,
	HomeassistantStatus, HomeassistantTogglable, HomeassistantTogglableServer,
};

use super::entity::{HomeassistantEntityService, entity_path};

const COMMAND_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn entity_capabilities(
	domain: &str, attributes: &serde_json::Value,
) -> Vec<HomeassistantCapability> {
	use HomeassistantCapability::*;
	match domain {
		"script" | "scene" | "button" | "input_button" => vec![Callable],
		"switch" | "input_boolean" => vec![Togglable],
		"light" => {
			let modes = attributes
				.get("supported_color_modes")
				.and_then(serde_json::Value::as_array);
			let supports = |names: &[&str]| {
				modes.is_some_and(|modes| {
					modes
						.iter()
						.any(|mode| mode.as_str().is_some_and(|mode| names.contains(&mode)))
				})
			};
			let mut capabilities = vec![Togglable];
			if supports(&[
				"brightness",
				"color_temp",
				"hs",
				"xy",
				"rgb",
				"rgbw",
				"rgbww",
				"white",
			]) {
				capabilities.push(Brightness);
			}
			if supports(&["color_temp"]) {
				capabilities.push(ColorTemperature);
			}
			if supports(&["hs", "xy", "rgb", "rgbw", "rgbww"]) {
				capabilities.push(Color);
			}
			capabilities
		}
		_ => Vec::new(),
	}
}

fn unknown_reading() -> zbus::fdo::Error {
	zbus::fdo::Error::Failed("Home Assistant reading is unknown or unavailable".to_string())
}

impl HomeassistantEntityService {
	pub(super) async fn sync_interfaces(&self) -> Result<(), String> {
		let capabilities = self.data.read().await.capabilities.clone();
		self.set_interfaces(&capabilities).await
	}

	pub(super) async fn remove_interfaces(&self) -> Result<(), String> {
		self.set_interfaces(&[]).await
	}

	async fn set_interfaces(&self, desired: &[HomeassistantCapability]) -> Result<(), String> {
		let service = self.service().map_err(|error| error.to_string())?;
		let path =
			entity_path(&self.data.read().await.entity_id).map_err(|error| error.to_string())?;
		let server = service.inner.connection.object_server();
		let mut current = self.interfaces.lock().await;
		macro_rules! reconcile {
			($capability:ident, $server:ident) => {{
				let capability = HomeassistantCapability::$capability;
				if desired.contains(&capability) && !current.contains(&capability) {
					if !server
						.at(path.clone(), $server(self.clone()))
						.await
						.map_err(|e| e.to_string())?
					{
						return Err(format!(
							"Capability {capability:?} is already registered at {path}"
						));
					}
					current.push(capability);
				} else if !desired.contains(&capability) && current.contains(&capability) {
					server
						.remove::<$server<Self>, _>(path.clone())
						.await
						.map_err(|e| e.to_string())?;
					current.retain(|value| *value != capability);
				}
			}};
		}
		reconcile!(Callable, HomeassistantCallableServer);
		reconcile!(Togglable, HomeassistantTogglableServer);
		reconcile!(Brightness, HomeassistantBrightnessServer);
		reconcile!(ColorTemperature, HomeassistantColorTemperatureServer);
		reconcile!(Color, HomeassistantColorServer);
		Ok(())
	}

	async fn command(
		&self, capability: HomeassistantCapability, action: &str, mut payload: serde_json::Value,
	) -> zbus::fdo::Result<()> {
		let service = self.service()?;
		// Select the entity and client from the same catalog generation, but do
		// not block incoming state events while waiting for the network command.
		let reconcile = service.inner.reconcile.lock().await;
		let (entity_id, domain) = {
			let data = self.data.read().await;
			if self.retiring.load(Ordering::Acquire) || !data.available {
				return Err(zbus::fdo::Error::Failed(
					"Home Assistant entity is unavailable".to_string(),
				));
			}
			if !data.capabilities.contains(&capability) {
				return Err(zbus::fdo::Error::NotSupported(
					"Entity does not support this interaction".to_string(),
				));
			}
			(data.entity_id.clone(), data.domain.clone())
		};
		let (client, generation) = {
			let state = service.inner.state.read().await;
			if !state.authenticated || state.status != HomeassistantStatus::Ready {
				return Err(zbus::fdo::Error::Failed(
					"Home Assistant is not connected".to_string(),
				));
			}
			(
				state.client.clone().ok_or_else(unknown_reading)?,
				state.generation,
			)
		};
		payload["entity_id"] = entity_id.into();
		drop(reconcile);
		tokio::time::timeout(
			COMMAND_REQUEST_TIMEOUT,
			client.call_service(domain, action.to_string(), Some(payload)),
		)
		.await
		.map_err(|_| zbus::fdo::Error::Timeout("Home Assistant command timed out".to_string()))?
		.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
		if !service.is_current_client(&client, generation).await {
			return Err(zbus::fdo::Error::Failed(
				"Home Assistant connection changed during command".to_string(),
			));
		}
		Ok(())
	}

	async fn numeric_attribute(&self, name: &str) -> zbus::fdo::Result<u32> {
		let data = self.data.read().await;
		if !data.available || data.state == "unknown" {
			return Err(unknown_reading());
		}
		data.attributes
			.get(name)
			.and_then(serde_json::Value::as_u64)
			.and_then(|value| u32::try_from(value).ok())
			.ok_or_else(unknown_reading)
	}
}

impl HomeassistantCallable for HomeassistantEntityService {
	async fn call(&self) -> zbus::fdo::Result<()> {
		let action = match self.data.read().await.domain.as_str() {
			"button" | "input_button" => "press",
			_ => "turn_on",
		};
		self.command(
			HomeassistantCapability::Callable,
			action,
			serde_json::json!({}),
		)
		.await
	}
}

impl HomeassistantTogglable for HomeassistantEntityService {
	async fn enabled(&self) -> zbus::fdo::Result<bool> {
		match self.data.read().await.state.as_str() {
			"on" => Ok(true),
			"off" => Ok(false),
			_ => Err(unknown_reading()),
		}
	}
	async fn set_enabled(&self, enabled: bool) -> zbus::fdo::Result<()> {
		self.command(
			HomeassistantCapability::Togglable,
			if enabled { "turn_on" } else { "turn_off" },
			serde_json::json!({}),
		)
		.await
	}
	async fn toggle(&self) -> zbus::fdo::Result<()> {
		self.command(
			HomeassistantCapability::Togglable,
			"toggle",
			serde_json::json!({}),
		)
		.await
	}
}

impl HomeassistantBrightness for HomeassistantEntityService {
	async fn brightness(&self) -> zbus::fdo::Result<u8> {
		u8::try_from(self.numeric_attribute("brightness").await?).map_err(|_| unknown_reading())
	}
	async fn set_brightness(&self, brightness: u8) -> zbus::fdo::Result<()> {
		self.command(
			HomeassistantCapability::Brightness,
			"turn_on",
			serde_json::json!({"brightness": brightness}),
		)
		.await
	}
}

impl HomeassistantColorTemperature for HomeassistantEntityService {
	async fn color_temperature(&self) -> zbus::fdo::Result<u32> {
		self.numeric_attribute("color_temp_kelvin").await
	}
	async fn min_kelvin(&self) -> zbus::fdo::Result<u32> {
		self.numeric_attribute("min_color_temp_kelvin").await
	}
	async fn max_kelvin(&self) -> zbus::fdo::Result<u32> {
		self.numeric_attribute("max_color_temp_kelvin").await
	}
	async fn set_color_temperature(&self, kelvin: u32) -> zbus::fdo::Result<()> {
		let min = self.min_kelvin().await?;
		let max = self.max_kelvin().await?;
		if !(min..=max).contains(&kelvin) {
			return Err(zbus::fdo::Error::InvalidArgs(format!(
				"Color temperature must be within {min}–{max} K"
			)));
		}
		self.command(
			HomeassistantCapability::ColorTemperature,
			"turn_on",
			serde_json::json!({"color_temp_kelvin": kelvin}),
		)
		.await
	}
}

impl HomeassistantColor for HomeassistantEntityService {
	async fn rgb(&self) -> zbus::fdo::Result<(u8, u8, u8)> {
		let data = self.data.read().await;
		if !data.available || data.state == "unknown" {
			return Err(unknown_reading());
		}
		let rgb: [u8; 3] = serde_json::from_value(
			data.attributes
				.get("rgb_color")
				.cloned()
				.ok_or_else(unknown_reading)?,
		)
		.map_err(|_| unknown_reading())?;
		let [red, green, blue] = rgb;
		Ok((red, green, blue))
	}
	async fn set_rgb(&self, red: u8, green: u8, blue: u8) -> zbus::fdo::Result<()> {
		self.command(
			HomeassistantCapability::Color,
			"turn_on",
			serde_json::json!({"rgb_color": [red, green, blue]}),
		)
		.await
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn detects_interactions_from_supported_modes_not_current_readings() {
		use HomeassistantCapability::*;
		for (modes, expected) in [
			(vec!["onoff"], vec![Togglable]),
			(vec!["brightness"], vec![Togglable, Brightness]),
			(
				vec!["color_temp"],
				vec![Togglable, Brightness, ColorTemperature],
			),
			(
				vec!["hs", "color_temp"],
				vec![Togglable, Brightness, ColorTemperature, Color],
			),
			(vec!["xy"], vec![Togglable, Brightness, Color]),
			(vec!["rgbw"], vec![Togglable, Brightness, Color]),
			(vec!["rgbww"], vec![Togglable, Brightness, Color]),
			(vec!["future_mode"], vec![Togglable]),
		] {
			assert_eq!(
				entity_capabilities(
					"light",
					&serde_json::json!({
						"supported_color_modes": modes, "color_mode": null, "brightness": null,
					})
				),
				expected
			);
		}
		for domain in ["script", "scene", "button", "input_button"] {
			assert_eq!(
				entity_capabilities(domain, &serde_json::json!({})),
				vec![Callable]
			);
		}
		for domain in ["switch", "input_boolean"] {
			assert_eq!(
				entity_capabilities(domain, &serde_json::json!({})),
				vec![Togglable]
			);
		}
		assert!(entity_capabilities("sensor", &serde_json::json!({})).is_empty());
	}
}
