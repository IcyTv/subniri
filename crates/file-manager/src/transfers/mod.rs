//! One owner for filesystem mutations, persistent recovery, and inverse operations.
mod app;
mod central;
mod durability;
mod fs;
mod journal;
mod model;
pub(crate) mod native;
mod portal;
mod restore;
mod retention;
#[cfg(test)]
mod tests;
mod trash_delete;
mod view;
mod worker;

pub(crate) use app::{Message, State};
pub(crate) use journal::Store;
pub(crate) use model::{
	Choice, Conflict, Durability, Event, Operation, Outcome, Phase, Request, Status,
};
pub(crate) use trash_delete::{
	PendingDeletion, delete_trash, read_pending_trash_deletions, retry_trash_deletion,
};
pub(crate) use worker::restore_trash;
pub(crate) use worker::{execute, undo};
