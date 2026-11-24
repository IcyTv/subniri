use zeroize::{Zeroize, Zeroizing};

// Linux-PAM's maximum response size includes the terminating NUL.
const CAPACITY: usize = 511;

pub(crate) struct Secret {
	bytes: Zeroizing<Vec<u8>>,
	len: usize,
	// Always a UTF-8 scalar boundary in bytes[..len]. No plaintext editor cache.
	cursor: usize,
}

impl Default for Secret {
	fn default() -> Self {
		Self {
			bytes: Zeroizing::new(vec![0; CAPACITY]),
			len: 0,
			cursor: 0,
		}
	}
}

impl Secret {
	pub(crate) fn append(&mut self, text: &str) -> bool {
		if text.chars().any(char::is_control) {
			return false;
		}
		let Some(end) = self
			.len
			.checked_add(text.len())
			.filter(|end| *end <= CAPACITY)
		else {
			return false;
		};

		self.bytes
			.copy_within(self.cursor..self.len, self.cursor + text.len());
		self.bytes[self.cursor..self.cursor + text.len()].copy_from_slice(text.as_bytes());
		self.cursor += text.len();
		self.len = end;
		true
	}

	pub(crate) fn as_bytes(&self) -> &[u8] {
		&self.bytes[..self.len]
	}

	fn text(&self) -> &str {
		// Only valid UTF-8 is inserted; movement and deletion preserve scalar boundaries.
		std::str::from_utf8(self.as_bytes()).expect("secret UTF-8 invariant")
	}

	pub(crate) fn left(&mut self) {
		self.cursor = self.text()[..self.cursor]
			.char_indices()
			.next_back()
			.map_or(0, |(i, _)| i);
	}

	pub(crate) fn right(&mut self) {
		if let Some(ch) = self.text()[self.cursor..].chars().next() {
			self.cursor += ch.len_utf8();
		}
	}

	pub(crate) fn home(&mut self) {
		self.cursor = 0;
	}

	pub(crate) fn end(&mut self) {
		self.cursor = self.len;
	}

	fn erase(&mut self, start: usize, end: usize) {
		let new_len = self.len - (end - start);
		self.bytes.copy_within(end..self.len, start);
		self.bytes[new_len..self.len].zeroize();
		self.len = new_len;
		self.cursor = start;
	}

	pub(crate) fn backspace(&mut self) {
		let end = self.cursor;
		self.left();
		self.erase(self.cursor, end);
	}

	pub(crate) fn delete(&mut self) {
		let start = self.cursor;
		self.right();
		self.erase(start, self.cursor);
	}

	pub(crate) fn clear(&mut self) {
		self.bytes.as_mut_slice().zeroize();
		self.len = 0;
		self.cursor = 0;
	}

	pub(crate) fn mask(&self) -> (String, String) {
		let prefix = self.text()[..self.cursor].chars().count();
		let suffix = self.text()[self.cursor..].chars().count();
		("•".repeat(prefix), "•".repeat(suffix))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn edits_preserve_utf8_and_erase_removed_bytes() {
		let mut secret = Secret::default();
		assert!(secret.append("aé"));
		assert_eq!(secret.mask(), ("••".into(), String::new()));

		secret.backspace();

		assert_eq!(secret.as_bytes(), b"a");
		assert!(secret.bytes.iter().skip(1).all(|byte| *byte == 0));

		secret.clear();

		assert!(secret.as_bytes().is_empty());
		assert!(secret.bytes.iter().all(|byte| *byte == 0));
	}

	#[test]
	fn unicode_cursor_insertion_and_deletion_preserve_boundaries() {
		let mut secret = Secret::default();
		assert!(secret.append("aé🧊z"));
		secret.left();
		secret.left();
		assert_eq!(secret.cursor, 3);
		assert!(secret.append("界"));
		assert_eq!(secret.as_bytes(), "aé界🧊z".as_bytes());
		assert_eq!(secret.mask(), ("•••".into(), "••".into()));

		secret.delete();
		assert_eq!(secret.as_bytes(), "aé界z".as_bytes());
		secret.backspace();
		assert_eq!(secret.as_bytes(), "aéz".as_bytes());
		assert_eq!(secret.cursor, 3);
		assert!(secret.bytes[secret.len..].iter().all(|byte| *byte == 0));

		secret.home();
		secret.left();
		secret.backspace();
		assert_eq!(secret.cursor, 0);
		secret.delete();
		assert_eq!(secret.as_bytes(), "éz".as_bytes());
		secret.right();
		assert_eq!(secret.cursor, 2);
		secret.end();
		secret.right();
		secret.delete();
		assert_eq!(secret.cursor, secret.len);
		assert_eq!(secret.as_bytes(), "éz".as_bytes());
	}

	#[test]
	fn full_capacity_and_clear_or_submit_reset_cursor() {
		let mut secret = Secret::default();
		secret.left();
		secret.right();
		secret.delete();
		secret.backspace();
		assert_eq!(secret.mask(), (String::new(), String::new()));

		assert!(secret.append(&"x".repeat(CAPACITY)));
		secret.home();
		assert!(!secret.append("é"));
		assert_eq!(secret.len, CAPACITY);
		assert_eq!(secret.cursor, 0);
		secret.delete();
		assert!(secret.append("a"));
		assert_eq!(secret.len, CAPACITY);

		let submitted = std::mem::take(&mut secret);
		assert_eq!(submitted.len, CAPACITY);
		assert_eq!(secret.cursor, 0);
		assert!(secret.as_bytes().is_empty());
		assert!(secret.append("failure"));
		secret.clear();
		assert_eq!(secret.cursor, 0);
		assert!(secret.bytes.iter().all(|byte| *byte == 0));
	}

	#[test]
	fn near_capacity_ascii_editing_keeps_cursor_and_masks_aligned() {
		let mut secret = Secret::default();
		assert!(secret.append(&"x".repeat(CAPACITY)));
		secret.home();
		assert_eq!(secret.cursor, 0);
		assert_eq!(secret.mask().1.chars().count(), CAPACITY);
		secret.end();
		assert_eq!(secret.cursor, CAPACITY);

		for _ in 0..255 {
			secret.left();
		}
		secret.delete();
		assert!(secret.append("y"));
		assert_eq!(secret.len, CAPACITY);
		assert_eq!(secret.cursor, 257);
		let (prefix, suffix) = secret.mask();
		assert_eq!(prefix.chars().count(), 257);
		assert_eq!(suffix.chars().count(), 254);
		assert!(prefix.chars().chain(suffix.chars()).all(|ch| ch == '•'));

		secret.backspace();
		assert_eq!(secret.len, CAPACITY - 1);
		assert!(secret.bytes[secret.len..].iter().all(|byte| *byte == 0));
	}

	#[test]
	fn near_capacity_multibyte_editing_preserves_boundaries_and_erases_tail() {
		let mut secret = Secret::default();
		assert!(secret.append(&"é".repeat(254)));
		assert!(secret.append("界"));
		assert_eq!(secret.len, CAPACITY);
		secret.home();
		secret.right();
		secret.delete();
		assert!(secret.append("ø"));
		assert_eq!(secret.cursor, 4);
		assert_eq!(secret.len, CAPACITY);
		assert!(secret.text().is_char_boundary(secret.cursor));
		let (prefix, suffix) = secret.mask();
		assert_eq!(prefix.chars().count(), 2);
		assert_eq!(suffix.chars().count(), 253);
		assert!(prefix.chars().chain(suffix.chars()).all(|ch| ch == '•'));

		secret.end();
		secret.backspace();
		assert_eq!(secret.len, 508);
		assert_eq!(secret.cursor, 508);
		assert!(secret.bytes[secret.len..].iter().all(|byte| *byte == 0));
	}

	#[test]
	fn rejects_nul_control_characters_and_overflow_without_truncation() {
		let mut secret = Secret::default();
		assert!(secret.append("a"));

		for invalid in ["\0", "\n", "\t", &"x".repeat(CAPACITY)] {
			assert!(!secret.append(invalid));
			assert_eq!(secret.as_bytes(), b"a");
		}
	}
}
