use std::{
	collections::{HashMap, HashSet},
	future::Future,
	hash::Hash,
};

use futures::StreamExt;
use tokio::sync::{oneshot, watch};
use zbus::{
	Connection,
	names::{BusName, OwnedUniqueName, UniqueName},
};

#[derive(Debug)]
pub struct Observed<K> {
	max_clients: usize,
	max_objects_per_client: usize,
	max_objects: usize,
	clients: HashMap<OwnedUniqueName, HashSet<K>>,
	key_clients: HashMap<K, usize>,
}

pub async fn watch_disconnections<F, Fut>(
	connection: &Connection, mut shutdown: watch::Receiver<bool>, ready: oneshot::Sender<()>,
	mut disconnected: F,
) -> zbus::Result<()>
where
	F: FnMut(OwnedUniqueName) -> Fut,
	Fut: Future<Output = ()>,
{
	let proxy = zbus::fdo::DBusProxy::new(connection).await?;
	let mut owner_changes = proxy.receive_name_owner_changed().await?;
	let _ = ready.send(());
	loop {
		tokio::select! {
			shutdown_event = shutdown.changed() => {
				if shutdown_event.is_err() || *shutdown.borrow() {
					return Ok(());
				}
			}
			change = owner_changes.next() => {
				let Some(change) = change else {
					return Ok(());
				};
				let args = change.args()?;
				if args.new_owner().is_none()
					&& let BusName::Unique(client) = args.name()
				{
					disconnected(OwnedUniqueName::from(client.clone())).await;
				}
			}
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddResult {
	pub added: bool,
	pub newly_observed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationLimitReached {
	Clients,
	ObjectsPerClient,
	Objects,
}

impl<K> Observed<K>
where
	K: Eq + Hash + Clone,
{
	#[must_use]
	pub fn with_limits(
		max_clients: usize, max_objects_per_client: usize, max_objects: usize,
	) -> Self {
		Self {
			max_clients,
			max_objects_per_client,
			max_objects,
			clients: HashMap::new(),
			key_clients: HashMap::new(),
		}
	}

	pub fn add(
		&mut self, client: &UniqueName<'_>, key: K,
	) -> Result<AddResult, ObservationLimitReached> {
		if self
			.clients
			.get(client.as_str())
			.is_some_and(|keys| keys.contains(&key))
		{
			return Ok(AddResult {
				added: false,
				newly_observed: false,
			});
		}
		if !self.clients.contains_key(client.as_str()) && self.clients.len() >= self.max_clients {
			return Err(ObservationLimitReached::Clients);
		}
		if self.clients.get(client.as_str()).map_or(0, HashSet::len) >= self.max_objects_per_client
		{
			return Err(ObservationLimitReached::ObjectsPerClient);
		}
		if !self.key_clients.contains_key(&key) && self.key_clients.len() >= self.max_objects {
			return Err(ObservationLimitReached::Objects);
		}

		let newly_observed = !self.key_clients.contains_key(&key);
		*self.key_clients.entry(key.clone()).or_insert(0) += 1;
		self.clients
			.entry(OwnedUniqueName::from(client.clone()))
			.or_default()
			.insert(key);

		Ok(AddResult {
			added: true,
			newly_observed,
		})
	}

	#[must_use]
	pub fn is_observed(&self, key: &K) -> bool {
		self.key_clients.contains_key(key)
	}

	pub fn remove(&mut self, client: &UniqueName<'_>, key: &K) -> bool {
		let Some(keys) = self.clients.get_mut(client.as_str()) else {
			return false;
		};
		if !keys.remove(key) {
			return false;
		}
		if keys.is_empty() {
			self.clients.remove(client.as_str());
		}
		self.remove_key_client(key.clone()).is_some()
	}

	pub fn release(&mut self, client: &UniqueName<'_>) -> HashSet<K> {
		let Some(keys) = self.clients.remove(client.as_str()) else {
			return HashSet::new();
		};
		keys.into_iter()
			.filter_map(|key| self.remove_key_client(key))
			.collect()
	}

	pub fn remove_key(&mut self, key: &K) -> bool {
		if self.key_clients.remove(key).is_none() {
			return false;
		}
		self.clients.retain(|_, keys| {
			keys.remove(key);
			!keys.is_empty()
		});
		true
	}

	#[must_use]
	pub fn observed_keys(&self) -> HashSet<K> {
		self.key_clients.keys().cloned().collect()
	}

	#[must_use]
	#[cfg(test)]
	pub fn client_count(&self) -> usize {
		self.clients.len()
	}

	#[must_use]
	#[cfg(test)]
	pub fn objects_for_client(&self, client: &UniqueName<'_>) -> HashSet<K> {
		self.clients
			.get(client.as_str())
			.cloned()
			.unwrap_or_default()
	}

	fn remove_key_client(&mut self, key: K) -> Option<K> {
		let clients = self.key_clients.get_mut(&key)?;
		*clients -= 1;
		if *clients == 0 {
			self.key_clients.remove(&key);
			Some(key)
		} else {
			None
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const CLIENT_1: UniqueName<'static> = UniqueName::from_static_str_checked(":1.1");
	const CLIENT_2: UniqueName<'static> = UniqueName::from_static_str_checked(":1.2");
	const CLIENT_3: UniqueName<'static> = UniqueName::from_static_str_checked(":1.3");

	#[test]
	fn enforces_client_limit() {
		let mut observed = Observed::with_limits(2, 2, 3);

		assert!(observed.add(&CLIENT_1, "one").is_ok());
		assert!(observed.add(&CLIENT_2, "two").is_ok());
		assert_eq!(
			observed.add(&CLIENT_3, "three"),
			Err(ObservationLimitReached::Clients)
		);
		assert_eq!(observed.client_count(), 2);
	}

	#[test]
	fn shares_keys_between_clients() {
		let mut observed = Observed::with_limits(2, 2, 2);

		assert_eq!(
			observed.add(&CLIENT_1, "shared"),
			Ok(AddResult {
				added: true,
				newly_observed: true,
			})
		);
		assert_eq!(
			observed.add(&CLIENT_2, "shared"),
			Ok(AddResult {
				added: true,
				newly_observed: false,
			})
		);
		assert_eq!(observed.observed_keys(), HashSet::from(["shared"]));
	}

	#[test]
	fn supports_multiple_objects_per_client() {
		let mut observed = Observed::with_limits(2, 2, 3);
		assert!(observed.add(&CLIENT_1, "one").is_ok());
		assert!(observed.add(&CLIENT_1, "two").is_ok());
		assert_eq!(
			observed.add(&CLIENT_1, "three"),
			Err(ObservationLimitReached::ObjectsPerClient)
		);
		assert_eq!(
			observed.objects_for_client(&CLIENT_1),
			HashSet::from(["one", "two"])
		);
	}

	#[test]
	fn remove_and_release_report_orphaned_keys() {
		let mut observed = Observed::with_limits(3, 3, 3);
		assert!(observed.add(&CLIENT_1, "shared").is_ok());
		assert!(observed.add(&CLIENT_1, "unique").is_ok());
		assert!(observed.add(&CLIENT_2, "shared").is_ok());

		assert!(!observed.remove(&CLIENT_1, &"shared"));
		assert_eq!(observed.release(&CLIENT_1), HashSet::from(["unique"]));
		assert_eq!(observed.release(&CLIENT_2), HashSet::from(["shared"]));
		assert!(observed.release(&CLIENT_2).is_empty());
		assert_eq!(observed.client_count(), 0);
		assert!(observed.observed_keys().is_empty());
	}

	#[test]
	fn enforces_global_object_limit_but_allows_sharing() {
		let mut observed = Observed::with_limits(3, 3, 2);
		assert!(observed.add(&CLIENT_1, "one").is_ok());
		assert!(observed.add(&CLIENT_1, "two").is_ok());
		assert_eq!(
			observed.add(&CLIENT_2, "three"),
			Err(ObservationLimitReached::Objects)
		);
		assert!(observed.add(&CLIENT_2, "one").is_ok());
	}

	#[test]
	fn removing_a_key_releases_it_from_every_client() {
		let mut observed = Observed::with_limits(3, 3, 3);
		assert!(observed.add(&CLIENT_1, "shared").is_ok());
		assert!(observed.add(&CLIENT_1, "other").is_ok());
		assert!(observed.add(&CLIENT_2, "shared").is_ok());

		assert!(observed.remove_key(&"shared"));
		assert!(!observed.remove_key(&"shared"));
		assert_eq!(
			observed.objects_for_client(&CLIENT_1),
			HashSet::from(["other"])
		);
		assert!(observed.objects_for_client(&CLIENT_2).is_empty());
		assert_eq!(observed.observed_keys(), HashSet::from(["other"]));
	}
}
