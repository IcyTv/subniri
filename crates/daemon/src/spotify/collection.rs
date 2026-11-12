//! Native collection-v2 wire contract for saved-track membership and mutations.
//! No endpoint discovery, authentication, I/O, or retry policy lives here.
use protobuf::{CodedInputStream, CodedOutputStream, rt::WireType};

use super::{protocol::valid_track_id, rate_limit::Failure};

pub const MEDIA_TYPE: &str = "application/vnd.collection-v2.spotify.proto";
pub const CONTAINS: &str = "/collection/v2/contains";
pub const WRITE: &str = "/collection/v2/write";
pub const MAX_BODY: usize = 64 * 1024;
pub const MAX_ITEMS: usize = 40;

pub fn contains(username: &str, ids: &[String]) -> Result<Vec<u8>, Failure> {
	encode(username, ids, None)
}

pub fn write(username: &str, id: &str, saved: bool, now: i64) -> Result<Vec<u8>, Failure> {
	// Upstream's extracted schema has int32 timestamps, unlike the Go subset.
	let added_at = i32::try_from(now).map_err(|_| Failure::InvalidResponse)?;
	if added_at < 0 {
		return Err(Failure::InvalidResponse);
	}
	encode(username, &[id.to_owned()], Some((saved, added_at)))
}

fn encode(
	username: &str, ids: &[String], mutation: Option<(bool, i32)>,
) -> Result<Vec<u8>, Failure> {
	if username.is_empty()
		|| username.len() > 1024
		|| ids.is_empty()
		|| ids.len() > MAX_ITEMS
		|| ids.iter().any(|id| !valid_track_id(id))
	{
		return Err(Failure::InvalidResponse);
	}
	let mut request = Vec::new();
	let mut out = CodedOutputStream::vec(&mut request);
	out.write_string(1, username)
		.map_err(|_| Failure::InvalidResponse)?;
	out.write_string(2, "collection")
		.map_err(|_| Failure::InvalidResponse)?;
	for id in ids {
		let mut item = Vec::new();
		{
			let mut field = CodedOutputStream::vec(&mut item);
			field
				.write_string(1, &format!("spotify:track:{id}"))
				.map_err(|_| Failure::InvalidResponse)?;
			if let Some((saved, added_at)) = mutation {
				if saved {
					field
						.write_int32(2, added_at)
						.map_err(|_| Failure::InvalidResponse)?;
				}
				field
					.write_bool(3, !saved)
					.map_err(|_| Failure::InvalidResponse)?;
			}
			field.flush().map_err(|_| Failure::InvalidResponse)?;
		}
		out.write_bytes(3, &item)
			.map_err(|_| Failure::InvalidResponse)?;
	}
	out.flush().map_err(|_| Failure::InvalidResponse)?;
	drop(out);
	Ok(request)
}

pub fn decode_contains(body: &[u8], expected: usize) -> Result<Vec<bool>, Failure> {
	if body.len() > MAX_BODY || expected == 0 || expected > MAX_ITEMS {
		return Err(Failure::InvalidResponse);
	}
	let mut input = CodedInputStream::from_bytes(body);
	let mut found = Vec::with_capacity(expected);
	while let Some(tag) = input
		.read_raw_tag_or_eof()
		.map_err(|_| Failure::InvalidResponse)?
	{
		if tag >> 3 == 0 {
			return Err(Failure::InvalidResponse);
		}
		match (tag >> 3, tag & 7) {
			(1, 0) => found.push(input.read_bool().map_err(|_| Failure::InvalidResponse)?),
			(1, 2) => {
				// Enforce the declared boundary: the convenience packed reader can
				// accept a truncated final field as a complete answer.
				let bytes = input.read_bytes().map_err(|_| Failure::InvalidResponse)?;
				let mut packed = CodedInputStream::from_bytes(&bytes);
				while !packed.eof().map_err(|_| Failure::InvalidResponse)? {
					found.push(packed.read_bool().map_err(|_| Failure::InvalidResponse)?);
					if found.len() > expected {
						return Err(Failure::InvalidResponse);
					}
				}
			}
			(1, _) => return Err(Failure::InvalidResponse),
			(_, wire) => {
				let wire = match wire {
					0 => WireType::Varint,
					1 => WireType::Fixed64,
					2 => WireType::LengthDelimited,
					5 => WireType::Fixed32,
					_ => return Err(Failure::InvalidResponse),
				};
				input
					.skip_field(wire)
					.map_err(|_| Failure::InvalidResponse)?;
			}
		}
		if found.len() > expected {
			return Err(Failure::InvalidResponse);
		}
	}
	if found.len() != expected {
		return Err(Failure::InvalidResponse);
	}
	Ok(found)
}

#[cfg(test)]
mod tests {
	use super::*;
	const ID: &str = "3uADmRFuyQBoCwUn19B6lV";

	fn expected(tail: &[u8]) -> Vec<u8> {
		let mut bytes = b"\x0a\x0cfixture-user\x12\x0acollection\x1a".to_vec();
		bytes.push(u8::try_from(38 + tail.len()).unwrap());
		bytes.extend_from_slice(b"\x0a\x24spotify:track:");
		bytes.extend_from_slice(ID.as_bytes());
		bytes.extend_from_slice(tail);
		bytes
	}

	#[test]
	fn request_fields_are_independent_of_the_decoder() {
		assert_eq!(
			contains("fixture-user", &[ID.into()]).unwrap(),
			expected(&[])
		);
		assert_eq!(
			write("fixture-user", ID, true, 123).unwrap(),
			expected(&[16, 123, 24, 0])
		);
		assert_eq!(
			write("fixture-user", ID, false, 123).unwrap(),
			expected(&[24, 1])
		);
		assert!(write("fixture-user", ID, true, i64::MAX).is_err());
		assert!(contains("fixture-user", &[]).is_err());
		assert!(contains("fixture-user", &["invalid".into()]).is_err());
		assert!(contains("fixture-user", &vec![ID.into(); MAX_ITEMS + 1]).is_err());
	}
	#[test]
	fn packed_unpacked_additive_and_false_values_preserve_order() {
		assert_eq!(decode_contains(&[10, 2, 1, 0], 2).unwrap(), [true, false]);
		assert_eq!(
			decode_contains(&[8, 0, 8, 1, 18, 1, 42], 2).unwrap(),
			[false, true]
		);
		assert_eq!(decode_contains(&[10, 1, 0], 1).unwrap(), [false]);
	}
	#[test]
	fn missing_truncated_wrong_type_and_extra_values_are_never_unsaved() {
		for bytes in [
			&[][..],
			&[10, 0],
			&[10, 9, 1],
			&[10, 2, 1, 0],
			&[13, 0, 0, 0, 0],
			&[0],
		] {
			assert_eq!(decode_contains(bytes, 1), Err(Failure::InvalidResponse));
		}
		assert!(decode_contains(&vec![0; MAX_BODY + 1], 1).is_err());
	}
}
