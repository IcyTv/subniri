//! Fileberg executable entry point.
mod app;
mod browser;
mod context_menu;
mod dialogs;
mod drives;
mod filesystem;
mod tags;
#[cfg(test)]
mod test_support;
mod trash;
mod ui;

#[cfg(test)]
use app::Args;
pub(crate) use app::{FileManager, Message, ViewMode, default_path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
	app::run()
}
