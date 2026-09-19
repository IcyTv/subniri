use std::{
	fmt,
	hash::{Hash, Hasher},
	ops::Deref,
	sync::Arc,
};

/// A value with hash identity based on a stable allocation address.
///
/// Clones share both the value and its identity. Wrapping the same value more
/// than once creates distinct identities.
pub struct Hashable<T: ?Sized> {
	inner: Arc<T>,
}

impl<T> Hashable<T> {
	#[must_use]
	pub fn new(value: T) -> Self {
		Self {
			inner: Arc::new(value),
		}
	}
}

impl<T: ?Sized> Hashable<T> {
	#[must_use]
	pub fn from_arc(value: Arc<T>) -> Self {
		Self { inner: value }
	}

	#[must_use]
	pub fn as_ptr(&self) -> *const T {
		Arc::as_ptr(&self.inner)
	}

	#[must_use]
	pub fn into_arc(self) -> Arc<T> {
		self.inner
	}
}

impl<T: ?Sized> Clone for Hashable<T> {
	fn clone(&self) -> Self {
		Self {
			inner: Arc::clone(&self.inner),
		}
	}
}

impl<T: ?Sized> Deref for Hashable<T> {
	type Target = T;

	fn deref(&self) -> &Self::Target {
		&self.inner
	}
}

impl<T: ?Sized> Hash for Hashable<T> {
	fn hash<H: Hasher>(&self, state: &mut H) {
		self.as_ptr().hash(state);
	}
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for Hashable<T> {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		self.inner.fmt(formatter)
	}
}

#[cfg(test)]
mod tests {
	use std::{
		collections::hash_map::DefaultHasher,
		hash::{Hash, Hasher},
		ptr,
	};

	use super::Hashable;

	#[test]
	fn dereferences_to_wrapped_value() {
		let value = Hashable::new(String::from("value"));

		assert_eq!(value.as_str(), "value");
	}

	#[test]
	fn clones_keep_the_same_identity() {
		let value = Hashable::new(String::from("value"));
		let clone = value.clone();

		assert!(ptr::eq(value.as_ptr(), clone.as_ptr()));
		assert_eq!(hash(&value), hash(&clone));
	}

	#[test]
	fn separate_wrappers_have_distinct_identities() {
		let first = Hashable::new(String::from("value"));
		let second = Hashable::new(String::from("value"));

		assert!(!ptr::eq(first.as_ptr(), second.as_ptr()));
	}

	fn hash<T: Hash>(value: &T) -> u64 {
		let mut hasher = DefaultHasher::new();
		value.hash(&mut hasher);
		hasher.finish()
	}
}
