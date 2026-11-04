//! Saved-track decisions only: no widgets, clocks, tasks, or connections.
use zbus::zvariant::OwnedObjectPath;

#[derive(Clone, Debug, Default)]
pub(super) struct SavedTrack {
	id: Option<String>,
	path: Option<OwnedObjectPath>,
	saved: Option<bool>,
	pending: bool,
	generation: u64,
	mutation: u64,
	pending_mutation: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Source {
	Initial,
	Signal,
	Mutation(u64),
}

pub(super) struct Observation {
	pub track_id: String,
	pub generation: u64,
	pub path: Option<OwnedObjectPath>,
	pub source: Source,
	pub result: Result<bool, String>,
}

pub(super) struct Mutation {
	pub track_id: String,
	pub path: OwnedObjectPath,
	pub generation: u64,
	pub sequence: u64,
	pub saved: bool,
}

impl SavedTrack {
	pub fn set_track(&mut self, id: Option<String>) {
		if self.id == id {
			return;
		}
		self.generation = self.generation.wrapping_add(1);
		self.pending = id.is_some();
		self.pending_mutation = None;
		self.id = id;
		self.path = None;
		self.saved = None;
	}

	pub fn observation(&self) -> Option<(String, u64)> {
		self.id.as_ref().map(|id| (id.clone(), self.generation))
	}

	pub fn saved(&self) -> Option<bool> {
		self.saved
	}
	pub fn pending(&self) -> bool {
		self.pending
	}

	pub fn toggle(&mut self) -> Option<Mutation> {
		if self.pending {
			return None;
		}
		let (track_id, path, saved) = (self.id.clone()?, self.path.clone()?, self.saved?);
		self.pending = true;
		self.mutation = self.mutation.wrapping_add(1);
		self.pending_mutation = Some(self.mutation);
		Some(Mutation {
			track_id,
			path,
			generation: self.generation,
			sequence: self.mutation,
			saved: !saved,
		})
	}

	pub fn apply(&mut self, observation: Observation) -> Result<(), String> {
		let Observation {
			track_id,
			generation,
			path,
			source,
			result,
		} = observation;
		if generation != self.generation || self.id.as_deref() != Some(&track_id) {
			return Ok(());
		}
		match (&source, &result) {
			(Source::Initial, Err(_)) if self.pending_mutation.is_some() => return Ok(()),
			(Source::Mutation(mutation), _) if self.pending_mutation != Some(*mutation) => {
				return Ok(());
			}
			_ => {}
		}
		if matches!(source, Source::Mutation(_))
			|| matches!(source, Source::Initial) && self.pending_mutation.is_none()
		{
			self.pending = false;
			self.pending_mutation = None;
		}
		match result {
			Ok(saved) => {
				if let Some(path) = path {
					self.path = Some(path);
				}
				self.saved = Some(saved);
				Ok(())
			}
			Err(error) => {
				if matches!(source, Source::Initial) {
					self.path = None;
					self.saved = None;
				}
				Err(error)
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn observation_and_mutation_ordering_is_independent_of_ui()
	-> Result<(), Box<dyn std::error::Error>> {
		let mut track = SavedTrack::default();
		track.set_track(Some("A".into()));
		let first = track.observation().ok_or("missing observation")?;
		track.set_track(Some("A".into()));
		assert_eq!(track.observation(), Some(first.clone()));
		let path = OwnedObjectPath::try_from("/track/A")?;
		let observation = |generation, source, result| Observation {
			track_id: "A".into(),
			generation,
			path: Some(path.clone()),
			source,
			result,
		};
		track.apply(observation(first.1, Source::Initial, Ok(false)))?;
		let mutation = track.toggle().ok_or("toggle should be allowed")?;
		assert!(mutation.saved);
		assert!(track.toggle().is_none());
		track.apply(observation(first.1, Source::Signal, Ok(true)))?;
		assert!(track.pending());
		track.apply(observation(
			first.1,
			Source::Mutation(mutation.sequence + 1),
			Ok(false),
		))?;
		assert_eq!(track.saved(), Some(true));
		assert!(
			track
				.apply(observation(
					first.1,
					Source::Mutation(mutation.sequence),
					Err("rejected".into())
				))
				.is_err()
		);
		assert!(!track.pending());
		assert_eq!(track.saved(), Some(true));
		track.set_track(Some("B".into()));
		track.set_track(Some("A".into()));
		track.apply(observation(first.1, Source::Initial, Ok(true)))?;
		assert_eq!(track.saved(), None);
		assert!(track.pending());
		Ok(())
	}

	#[test]
	fn unknown_and_detached_tracks_cannot_mutate() {
		let mut track = SavedTrack::default();
		assert!(track.toggle().is_none());
		track.set_track(Some("A".into()));
		assert!(track.pending());
		assert!(track.toggle().is_none());
		track.set_track(None);
		assert!(track.observation().is_none());
		assert!(!track.pending());
		assert!(track.toggle().is_none());
	}
}
