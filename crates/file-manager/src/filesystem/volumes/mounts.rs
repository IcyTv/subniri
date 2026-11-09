use std::{
	ffi::OsString,
	os::unix::ffi::{OsStrExt, OsStringExt},
	path::{Path, PathBuf},
};

#[derive(Debug)]
pub(super) struct Mount {
	pub id: u64,
	pub root: PathBuf,
	pub target: PathBuf,
	source: PathBuf,
	number: String,
}

impl Mount {
	pub(super) fn matches(&self, device: &Path, number: u64) -> bool {
		(number != 0
			&& self.number == format!("{}:{}", nix::libc::major(number), nix::libc::minor(number)))
			|| self.source == device
			|| std::fs::canonicalize(&self.source)
				.ok()
				.zip(std::fs::canonicalize(device).ok())
				.is_some_and(|(a, b)| a == b)
	}

	pub(super) fn system_configured(&self, mounts: &[Self], configured: &[&PathBuf]) -> bool {
		let mut current = self;

		loop {
			if configured.contains(&&current.target) {
				return true;
			}

			let Some(parent) = mounts
				.iter()
				.filter(|m| m.target != current.target && current.target.starts_with(&m.target))
				.max_by_key(|m| m.target.components().count())
			else {
				return false;
			};
			let Ok(relative) = current.target.strip_prefix(&parent.target) else {
				return false;
			};

			// Only inherit through a re-exposure of the very same directory.
			// A different subvolume or intervening filesystem breaks the chain.
			if current.number != parent.number
				|| current.root != parent.root.join(relative)
				|| mounts.iter().filter(|m| m.target == parent.target).count() != 1
			{
				return false;
			}

			current = parent;
		}
	}
}

pub(super) fn parse_mounts(bytes: &[u8]) -> Vec<Mount> {
	bytes
		.split(|b| *b == b'\n')
		.filter_map(|line| {
			let fields: Vec<_> = line
				.split(|b| b.is_ascii_whitespace())
				.filter(|s| !s.is_empty())
				.collect();
			let separator = fields.iter().position(|s| *s == b"-")?;
			Some(Mount {
				id: std::str::from_utf8(fields.first()?).ok()?.parse().ok()?,
				root: decode(fields.get(3)?),
				target: decode(fields.get(4)?),
				source: decode(fields.get(separator + 2)?),
				number: String::from_utf8_lossy(fields.get(2)?).into_owned(),
			})
		})
		.collect()
}

pub(super) struct ConfiguredMount {
	pub target: PathBuf,
	source: PathBuf,
}

impl ConfiguredMount {
	pub(super) fn matches(&self, device: &Path, uuid: &str, label: &str) -> bool {
		let source = self.source.as_os_str().as_bytes();
		if let Some(value) = source.strip_prefix(b"UUID=") {
			return !uuid.is_empty() && unquote(value) == uuid.as_bytes();
		}
		if let Some(value) = source.strip_prefix(b"LABEL=") {
			return !label.is_empty() && unquote(value) == label.as_bytes();
		}

		self.source == device
			|| std::fs::canonicalize(&self.source)
				.ok()
				.zip(std::fs::canonicalize(device).ok())
				.is_some_and(|(a, b)| a == b)
	}
}

fn unquote(bytes: &[u8]) -> &[u8] {
	bytes
		.strip_prefix(b"\"")
		.and_then(|v| v.strip_suffix(b"\""))
		.unwrap_or(bytes)
}

pub(super) fn parse_fstab(bytes: &[u8]) -> Vec<ConfiguredMount> {
	bytes
		.split(|b| *b == b'\n')
		.filter_map(|line| {
			let line = line.split(|b| *b == b'#').next()?;
			let mut fields = line
				.split(|b| b.is_ascii_whitespace())
				.filter(|s| !s.is_empty());
			let source = decode(fields.next()?);
			let target = decode(fields.next()?);
			target
				.is_absolute()
				.then_some(ConfiguredMount { source, target })
		})
		.collect()
}

fn decode(bytes: &[u8]) -> PathBuf {
	let mut decoded = Vec::new();
	let mut offset = 0;

	while let Some(&byte) = bytes.get(offset) {
		if byte == b'\\'
			&& let Some(octal) = bytes.get(offset + 1..offset + 4)
			&& octal.iter().all(|b| (b'0'..=b'7').contains(b))
		{
			let value = octal
				.iter()
				.fold(0u16, |value, b| value * 8 + u16::from(b - b'0'));
			if let Ok(value) = u8::try_from(value) {
				decoded.push(value);
				offset += 4;
				continue;
			}
		}

		decoded.push(byte);
		offset += 1;
	}

	OsString::from_vec(decoded).into()
}
