//! Build and parse the SPA pods exchanged with libpipewire for sink volume.
//!
//! Values are built as [`Value`] trees and serialized into an 8-byte aligned
//! [`PodBuffer`]. Decoding walks object properties one at a time, so a single
//! unsupported or mistyped property is skipped instead of failing the pod.
use pipewire::spa::{
	pod::{
		Object, Pod, Property, Value, ValueArray, deserialize::PodDeserializer,
		serialize::PodSerializer,
	},
	sys as spa_sys,
	utils::{Id, SpaTypes},
};

use super::{PwRoute, PwVolumeUpdate};

const MAX_VOLUME_PARAM: &str = "channelmix.max-volume";

/// An owned pod whose storage is aligned for [`Pod::from_bytes`].
///
/// `Pod::from_bytes` dereferences the start of the slice as a `spa_pod`
/// header, so the bytes must be at least 4-byte aligned; SPA itself keeps
/// pods 8-byte aligned. Backing the bytes with `u64` words guarantees both
/// and also zero-pads the tail up to the next 8-byte boundary.
pub(super) struct PodBuffer {
	words: Vec<u64>,
}

impl PodBuffer {
	fn from_bytes(bytes: &[u8]) -> Self {
		let words = bytes
			.chunks(8)
			.map(|chunk| {
				let mut word = [0u8; 8];
				word.iter_mut()
					.zip(chunk)
					.for_each(|(dst, src)| *dst = *src);
				u64::from_ne_bytes(word)
			})
			.collect();

		Self { words }
	}

	pub(super) fn bytes(&self) -> &[u8] {
		// SAFETY: the pointer and length cover exactly the initialized `u64`
		// allocation, and `u8` has no alignment or validity requirements.
		unsafe {
			std::slice::from_raw_parts(
				self.words.as_ptr().cast::<u8>(),
				std::mem::size_of_val(self.words.as_slice()),
			)
		}
	}

	pub(super) fn pod(&self) -> Result<&Pod, String> {
		Pod::from_bytes(self.bytes()).ok_or_else(|| "truncated SPA pod".to_owned())
	}
}

/// Props object carrying only the given channel volumes and mute state.
pub(super) fn props_value(channels: Option<&[f32]>, mute: Option<bool>) -> Value {
	let mut properties = Vec::new();

	if let Some(channels) = channels {
		properties.push(Property::new(
			spa_sys::SPA_PROP_channelVolumes,
			Value::ValueArray(ValueArray::Float(channels.to_vec())),
		));
	}
	if let Some(mute) = mute {
		properties.push(Property::new(spa_sys::SPA_PROP_mute, Value::Bool(mute)));
	}

	Value::Object(Object {
		type_: spa_sys::SPA_TYPE_OBJECT_Props,
		id: spa_sys::SPA_PARAM_Props,
		properties,
	})
}

/// Route object selecting `route` and applying `props` to it.
///
/// The route is intentionally not marked with the `save` key.
pub(super) fn route_value(route: &PwRoute, props: Value) -> Value {
	Value::Object(Object {
		type_: spa_sys::SPA_TYPE_OBJECT_ParamRoute,
		id: spa_sys::SPA_PARAM_Route,
		properties: vec![
			Property::new(spa_sys::SPA_PARAM_ROUTE_index, Value::Int(route.index)),
			Property::new(spa_sys::SPA_PARAM_ROUTE_device, Value::Int(route.device)),
			Property::new(
				spa_sys::SPA_PARAM_ROUTE_direction,
				Value::Id(Id(route.direction)),
			),
			Property::new(spa_sys::SPA_PARAM_ROUTE_props, props),
		],
	})
}

pub(super) fn serialize(value: &Value) -> Result<PodBuffer, String> {
	let (cursor, _) = PodSerializer::serialize(std::io::Cursor::new(Vec::new()), value)
		.map_err(|error| format!("failed to serialize SPA pod: {error:?}"))?;

	Ok(PodBuffer::from_bytes(&cursor.into_inner()))
}

pub(super) fn decode_props(pod: &Pod) -> Result<PwVolumeUpdate, String> {
	let object = pod
		.as_object()
		.map_err(|error| format!("Props pod is not an object: {error}"))?;
	let mut out = PwVolumeUpdate::default();

	for prop in object.props() {
		let key = prop.key().0;
		let value = match pod_value(prop.value()) {
			Ok(value) => value,
			Err(error) => {
				log::debug!("skipping undecodable Props key {key:#x}: {error}");
				continue;
			}
		};
		log::trace!("prop {key:#x} -> {value:?}");

		match (key, value) {
			(spa_sys::SPA_PROP_volume, Value::Float(v)) => out.volume = Some(v),
			(spa_sys::SPA_PROP_channelVolumes, Value::ValueArray(ValueArray::Float(v))) => {
				out.channel_volums = Some(v);
			}
			(spa_sys::SPA_PROP_volumeBase, Value::Float(v)) => out.volume_base = Some(v),
			(spa_sys::SPA_PROP_volumeStep, Value::Float(v)) => out.step = Some(v),
			(spa_sys::SPA_PROP_mute, Value::Bool(v)) => out.mute = Some(v),
			(spa_sys::SPA_PROP_softVolumes, Value::ValueArray(ValueArray::Float(v))) => {
				out.soft_volumes = Some(v);
			}
			(spa_sys::SPA_PROP_softMute, Value::Bool(v)) => out.soft_mute = Some(v),
			(spa_sys::SPA_PROP_params, Value::Struct(fields)) => {
				out.max_volume = channelmix_max_volume(&fields);
			}
			(
				spa_sys::SPA_PROP_volume
				| spa_sys::SPA_PROP_channelVolumes
				| spa_sys::SPA_PROP_volumeBase
				| spa_sys::SPA_PROP_volumeStep
				| spa_sys::SPA_PROP_mute
				| spa_sys::SPA_PROP_softVolumes
				| spa_sys::SPA_PROP_softMute
				| spa_sys::SPA_PROP_params,
				value,
			) => log::debug!("skipping Props key {key:#x} with unexpected value {value:?}"),
			_ => (),
		}
	}

	Ok(out)
}

/// Decodes an output route; input routes and incomplete routes yield `None`.
pub(super) fn decode_route(pod: &Pod) -> Result<Option<PwRoute>, String> {
	let object = pod
		.as_object()
		.map_err(|error| format!("Route pod is not an object: {error}"))?;
	if object.type_() != SpaTypes::ObjectParamRoute {
		return Ok(None);
	}

	let mut index = None;
	let mut device = None;
	let mut direction = None;

	for prop in object.props() {
		let key = prop.key().0;
		if !matches!(
			key,
			spa_sys::SPA_PARAM_ROUTE_index
				| spa_sys::SPA_PARAM_ROUTE_device
				| spa_sys::SPA_PARAM_ROUTE_direction
		) {
			continue;
		}

		match (key, pod_value(prop.value())) {
			(spa_sys::SPA_PARAM_ROUTE_index, Ok(Value::Int(v))) => index = Some(v),
			(spa_sys::SPA_PARAM_ROUTE_device, Ok(Value::Int(v))) => device = Some(v),
			(spa_sys::SPA_PARAM_ROUTE_direction, Ok(Value::Id(Id(v)))) => direction = Some(v),
			(_, value) => log::debug!("skipping Route key {key} with value {value:?}"),
		}
	}

	Ok(match (index, device, direction) {
		(Some(index), Some(device), Some(direction))
			if direction == spa_sys::SPA_DIRECTION_OUTPUT =>
		{
			Some(PwRoute {
				index,
				device,
				direction,
			})
		}
		_ => None,
	})
}

/// Deserializes one pod into a [`Value`].
///
/// The bytes are copied into a padded buffer because the deserializer expects
/// trailing padding that a property value's own size does not cover.
fn pod_value(pod: &Pod) -> Result<Value, String> {
	let buffer = PodBuffer::from_bytes(pod.as_bytes());

	PodDeserializer::deserialize_any_from(buffer.bytes())
		.map(|(_, value)| value)
		.map_err(|error| format!("{error:?}"))
}

/// Finds `channelmix.max-volume` in a Props `params` struct of key/value pairs.
fn channelmix_max_volume(fields: &[Value]) -> Option<f32> {
	fields.chunks_exact(2).find_map(|pair| match pair {
		[Value::String(key), value] if key == MAX_VOLUME_PARAM => numeric(value),
		_ => None,
	})
}

#[allow(clippy::cast_possible_truncation)]
fn numeric(value: &Value) -> Option<f32> {
	match value {
		Value::Float(v) => Some(*v),
		Value::Double(v) => Some(*v as f32),
		Value::Int(v) => Some(*v as f32),
		Value::Long(v) => Some(*v as f32),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn round_trip_props(value: &Value) -> Result<PwVolumeUpdate, String> {
		let buffer = serialize(value)?;
		decode_props(buffer.pod()?)
	}

	fn props_with(properties: Vec<Property>) -> Value {
		Value::Object(Object {
			type_: spa_sys::SPA_TYPE_OBJECT_Props,
			id: spa_sys::SPA_PARAM_Props,
			properties,
		})
	}

	fn params(value: Value) -> Property {
		Property::new(
			spa_sys::SPA_PROP_params,
			Value::Struct(vec![
				Value::String("monitor.channel-volumes".to_owned()),
				Value::Bool(false),
				Value::String(MAX_VOLUME_PARAM.to_owned()),
				value,
			]),
		)
	}

	#[test]
	fn volume_properties_round_trip_and_mute_omits_channels() -> Result<(), String> {
		let channels = [0.125, 0.421875];

		let decoded = round_trip_props(&props_value(Some(&channels), Some(false)))?;

		assert_eq!(decoded.channel_volums.as_deref(), Some(channels.as_slice()));
		assert_eq!(decoded.mute, Some(false));

		let mute = round_trip_props(&props_value(None, Some(true)))?;

		assert_eq!(mute.mute, Some(true));
		assert!(mute.channel_volums.is_none());
		Ok(())
	}

	#[test]
	fn all_volume_properties_decode() -> Result<(), String> {
		let value = props_with(vec![
			Property::new(spa_sys::SPA_PROP_volume, Value::Float(0.5)),
			Property::new(spa_sys::SPA_PROP_volumeBase, Value::Float(1.0)),
			Property::new(spa_sys::SPA_PROP_volumeStep, Value::Float(0.01)),
			Property::new(spa_sys::SPA_PROP_softMute, Value::Bool(true)),
			Property::new(
				spa_sys::SPA_PROP_softVolumes,
				Value::ValueArray(ValueArray::Float(vec![0.25, 0.75])),
			),
		]);

		let decoded = round_trip_props(&value)?;

		assert_eq!(decoded.volume, Some(0.5));
		assert_eq!(decoded.volume_base, Some(1.0));
		assert_eq!(decoded.step, Some(0.01));
		assert_eq!(decoded.soft_mute, Some(true));
		assert_eq!(
			decoded.soft_volumes.as_deref(),
			Some([0.25, 0.75].as_slice())
		);
		Ok(())
	}

	#[test]
	fn route_round_trips_through_route_value() -> Result<(), String> {
		let route = PwRoute {
			index: 4,
			device: 7,
			direction: spa_sys::SPA_DIRECTION_OUTPUT,
		};

		let buffer = serialize(&route_value(&route, props_value(Some(&[0.5]), None)))?;
		let decoded = decode_route(buffer.pod()?)?;

		let decoded = decoded.ok_or("output route must decode")?;
		assert_eq!(decoded.index, 4);
		assert_eq!(decoded.device, 7);
		assert_eq!(decoded.direction, spa_sys::SPA_DIRECTION_OUTPUT);
		Ok(())
	}

	#[test]
	fn input_route_decodes_to_none() -> Result<(), String> {
		let route = PwRoute {
			index: 1,
			device: 2,
			direction: spa_sys::SPA_DIRECTION_INPUT,
		};

		let buffer = serialize(&route_value(&route, props_value(None, Some(true))))?;

		assert!(decode_route(buffer.pod()?)?.is_none());
		Ok(())
	}

	#[test]
	fn max_volume_parses_numeric_variants() -> Result<(), String> {
		for (value, expected) in [
			(Value::Float(1.5), 1.5),
			(Value::Double(2.0), 2.0),
			(Value::Int(3), 3.0),
			(Value::Long(4), 4.0),
		] {
			let decoded = round_trip_props(&props_with(vec![params(value)]))?;

			assert_eq!(decoded.max_volume, Some(expected));
		}

		let decoded = round_trip_props(&props_with(vec![params(Value::Bool(true))]))?;

		assert!(decoded.max_volume.is_none());
		Ok(())
	}

	#[test]
	fn unknown_and_mistyped_keys_are_skipped() -> Result<(), String> {
		let value = props_with(vec![
			Property::new(0x0123_4567, Value::String("unknown".to_owned())),
			Property::new(
				spa_sys::SPA_PROP_volume,
				Value::String("mistyped".to_owned()),
			),
			Property::new(spa_sys::SPA_PROP_mute, Value::Bool(true)),
		]);

		let decoded = round_trip_props(&value)?;

		assert!(decoded.volume.is_none());
		assert_eq!(decoded.mute, Some(true));
		Ok(())
	}

	#[test]
	fn spa_keys_match_protocol_values() {
		assert_eq!(spa_sys::SPA_PARAM_ROUTE_index, 1);
		assert_eq!(spa_sys::SPA_PARAM_ROUTE_direction, 2);
		assert_eq!(spa_sys::SPA_PARAM_ROUTE_device, 3);
		assert_eq!(spa_sys::SPA_PARAM_ROUTE_props, 10);
		assert_eq!(spa_sys::SPA_PARAM_ROUTE_save, 13);

		assert_eq!(spa_sys::SPA_PROP_volume, 0x10003);
		assert_eq!(spa_sys::SPA_PROP_mute, 0x10004);
		assert_eq!(spa_sys::SPA_PROP_channelVolumes, 0x10008);
		assert_eq!(spa_sys::SPA_PROP_volumeBase, 0x10009);
		assert_eq!(spa_sys::SPA_PROP_volumeStep, 0x1000a);
		assert_eq!(spa_sys::SPA_PROP_softMute, 0x1000f);
		assert_eq!(spa_sys::SPA_PROP_softVolumes, 0x10010);
		assert_eq!(spa_sys::SPA_PROP_params, 0x80001);

		assert_eq!(spa_sys::SPA_DIRECTION_OUTPUT, 1);
	}
}
