//! Context menus: action ownership, target snapshots, popup orchestration, and views.
mod actions;
pub(crate) mod favorites;
mod message;
mod model;
mod popup;
mod rows;
mod update;
mod view;

pub use actions::{Action, Command};
pub use message::Message;
pub use model::{Context, Drag, Menu, Page, Target};
