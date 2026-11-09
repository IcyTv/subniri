use crate::{Message, browser::Message as BrowserMessage};
use iced::{Event, event, keyboard, window};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
	Append(String),
	Backspace,
	Clear,
}

#[derive(Default)]
pub struct TypeAhead {
	text: String,
}

impl TypeAhead {
	pub fn text(&self) -> &str {
		&self.text
	}
	pub fn is_active(&self) -> bool {
		!self.text.is_empty()
	}
	pub fn clear(&mut self) {
		self.text.clear();
	}
	pub fn apply(&mut self, edit: Edit) {
		match edit {
			Edit::Append(text) => self.text.extend(text.chars().filter(|c| !c.is_control())),
			Edit::Backspace => {
				self.text.pop();
			}
			Edit::Clear => self.clear(),
		}
	}
}

/// Called before button widgets see the event: spaces within a prefix are text,
/// not "activate the focused file". Editors and open overlays keep their keys.
pub fn key_edit(event: &keyboard::Event, active: bool, editing: bool) -> Option<Edit> {
	if editing {
		return None;
	}
	let keyboard::Event::KeyPressed {
		key,
		text,
		modifiers,
		..
	} = event
	else {
		return None;
	};
	if modifiers.control() || modifiers.alt() || modifiers.logo() {
		return None;
	}
	match key.as_ref() {
		keyboard::Key::Named(keyboard::key::Named::Escape) if active => return Some(Edit::Clear),
		keyboard::Key::Named(keyboard::key::Named::Backspace) if active => {
			return Some(Edit::Backspace);
		}
		_ => {}
	}
	let text = text.as_deref().or_else(|| match key.as_ref() {
		keyboard::Key::Character(text) => Some(text),
		keyboard::Key::Named(keyboard::key::Named::Space) => Some(" "),
		_ => None,
	})?;
	if text.is_empty()
		|| text.chars().any(char::is_control)
		|| (!active && text.chars().all(char::is_whitespace))
	{
		return None;
	}
	Some(Edit::Append(text.to_owned()))
}

/// Observe captured pointer presses too, without swallowing the clicked control's
/// normal action. Losing window focus covers clicks outside Fileberg itself.
pub fn dismiss_event(event: Event, _status: event::Status, _window: window::Id) -> Option<Message> {
	match event {
		Event::Mouse(iced::mouse::Event::ButtonPressed(_))
		| Event::Touch(iced::touch::Event::FingerPressed { .. })
		| Event::Window(window::Event::Unfocused) => Some(BrowserMessage::TypeAhead(Edit::Clear).into()),
		_ => None,
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	pub fn key(
		key: keyboard::Key, text: Option<&str>, modifiers: keyboard::Modifiers,
	) -> keyboard::Event {
		keyboard::Event::KeyPressed {
			modified_key: key.clone(),
			key,
			physical_key: keyboard::key::Physical::Unidentified(
				keyboard::key::NativeCode::Unidentified,
			),
			location: keyboard::Location::Standard,
			modifiers,
			text: text.map(Into::into),
			repeat: false,
		}
	}
	#[test]
	fn editors_and_shortcuts_keep_their_keys_but_prefix_spaces_are_text() {
		let f = key(
			keyboard::Key::Character("f".into()),
			Some("f"),
			keyboard::Modifiers::default(),
		);
		assert_eq!(key_edit(&f, false, false), Some(Edit::Append("f".into())));
		assert_eq!(key_edit(&f, false, true), None);
		let ctrl_f = key(
			keyboard::Key::Character("f".into()),
			Some("f"),
			keyboard::Modifiers::CTRL,
		);
		assert_eq!(key_edit(&ctrl_f, false, false), None);
		let space = key(
			keyboard::Key::Named(keyboard::key::Named::Space),
			Some(" "),
			keyboard::Modifiers::default(),
		);
		assert_eq!(
			key_edit(&space, true, false),
			Some(Edit::Append(" ".into()))
		);
		assert_eq!(key_edit(&space, false, false), None);
	}
	#[test]
	fn buffer_edits_are_unicode_safe_and_escape_only_intercepts_an_active_prefix() {
		let mut prefix = TypeAhead::default();
		prefix.apply(Edit::Append("éclair".into()));
		prefix.apply(Edit::Backspace);
		assert_eq!(prefix.text(), "éclai");
		let escape = key(
			keyboard::Key::Named(keyboard::key::Named::Escape),
			None,
			keyboard::Modifiers::default(),
		);
		assert_eq!(key_edit(&escape, true, false), Some(Edit::Clear));
		assert_eq!(key_edit(&escape, false, false), None);
		prefix.apply(Edit::Clear);
		assert!(!prefix.is_active());
	}
}
