//! Shared Home Assistant transport and entity reconstruction, independent of the
//! visual menu. Entity observation retries remain scoped to each observation.
use std::{collections::HashMap, time::Duration};

use iced::{Subscription, Task};
use utilities::Hashable;
use zbus::Connection;

use super::{HomeassistantEntity, Message as EntityMessage};
use crate::modules::connection::initialize;

#[derive(Debug, Clone)]
pub struct Integration {
	ids: Vec<String>,
	entities: HashMap<String, HomeassistantEntity>,
	connection: Option<Hashable<Connection>>,
	generation: u64,
	attempt: u32,
}

#[derive(Debug, Clone)]
pub enum Message {
	Connected(u64, Result<Hashable<Connection>, String>),
	Disconnected(u64),
	Observed(u64, String, EntityMessage),
	Entity(String, EntityMessage),
}

impl Integration {
	pub fn new(mut ids: Vec<String>) -> Self {
		ids.sort();
		ids.dedup();
		Self {
			ids,
			entities: HashMap::new(),
			connection: None,
			generation: 0,
			attempt: 0,
		}
	}

	pub fn init(&self) -> Task<Message> {
		self.connect(Duration::ZERO, None)
	}

	pub fn reconfigure(&mut self, mut ids: Vec<String>) -> Task<Message> {
		ids.sort();
		ids.dedup();
		if self.ids == ids {
			return Task::none();
		}
		self.ids = ids;
		self.generation = self.generation.wrapping_add(1);
		self.attempt = 0;
		self.entities.clear();
		let old = self.connection.take();
		// A fresh sender identity prevents a cancelled old observation's
		// unobserve from unregistering its replacement for the same entity.
		self.connect(Duration::ZERO, old)
	}

	fn connect(&self, delay: Duration, previous: Option<Hashable<Connection>>) -> Task<Message> {
		let generation = self.generation;
		Task::future(async move {
			if let Some(previous) = previous {
				let _ = (*previous).clone().close().await;
			}
			let result = initialize(delay, async {
				Connection::session()
					.await
					.map(Hashable::new)
					.map_err(|error| error.to_string())
			})
			.await;
			Message::Connected(generation, result)
		})
	}

	pub fn get(&self, id: &str) -> Option<&HomeassistantEntity> {
		self.entities.get(id)
	}

	pub fn subscription(&self) -> Subscription<Message> {
		let mut subscriptions = Vec::new();
		let generation = self.generation;
		if let Some(connection) = &self.connection {
			subscriptions.push(Subscription::run_with(
				(generation, connection.clone()),
				|data| {
					let (generation, connection) = data.clone();
					async_stream::stream! {
						connection.closed().await;
						yield Message::Disconnected(generation);
						std::future::pending::<()>().await;
					}
				},
			));
		}
		for (id, entity) in &self.entities {
			subscriptions.push(
				entity
					.subscription()
					.with(id.clone())
					.with(generation)
					.map(|(generation, (id, message))| Message::Observed(generation, id, message)),
			);
		}
		Subscription::batch(subscriptions)
	}

	pub fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Connected(generation, result)
				if generation == self.generation && self.connection.is_none() =>
			{
				match result {
					Ok(connection) => {
						self.attempt = 0;
						self.entities = self
							.ids
							.iter()
							.map(|id| {
								(
									id.clone(),
									HomeassistantEntity::new((*connection).clone(), id.clone()),
								)
							})
							.collect();
						self.connection = Some(connection);
					}
					Err(error) => {
						log::warn!("Home Assistant session bus unavailable: {error}");
						let delay = Duration::from_secs((1_u64 << self.attempt.min(5)).min(30));
						self.attempt = self.attempt.saturating_add(1);
						self.generation = self.generation.wrapping_add(1);
						return self.connect(delay, None);
					}
				}
			}
			Message::Disconnected(generation)
				if generation == self.generation && self.connection.is_some() =>
			{
				self.connection = None;
				self.entities.clear();
				self.generation = self.generation.wrapping_add(1);
				return self.connect(Duration::from_secs(1), None);
			}
			Message::Observed(generation, id, message)
				if generation == self.generation && self.connection.is_some() =>
			{
				return self.update(Message::Entity(id, message));
			}
			Message::Entity(id, message) => {
				if let Some(entity) = self.entities.get_mut(&id) {
					let generation = self.generation;
					return entity
						.update(message)
						.map(move |message| Message::Observed(generation, id.clone(), message));
				}
				log::warn!("Tried to address non-existent homeassistant entity: '{id}'");
			}
			_ => {}
		}
		Task::none()
	}

	#[cfg(test)]
	pub fn session(&self) -> Option<(u64, Hashable<Connection>)> {
		self.connection
			.clone()
			.map(|connection| (self.generation, connection))
	}
	#[cfg(test)]
	pub fn len(&self) -> usize {
		self.entities.len()
	}
}
