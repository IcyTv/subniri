use glib::types::StaticType;
use glib::value::{FromValue, ToValue};

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
	#[default]
	PlayerIcon,
	SwitcherIcon,
}

impl glib::enums::EnumerationValue<ButtonState> for ButtonState {
	type GlibType = glib::GString;

	const ZERO: Self = Self::PlayerIcon;
}

impl glib::property::Property for ButtonState {
	type Value = glib::GString;
}

impl ToValue for ButtonState {
	fn to_value(&self) -> glib::Value {
		let s = match self {
			ButtonState::PlayerIcon => "player-icon",
			ButtonState::SwitcherIcon => "switcher-icon",
		};
		s.to_value()
	}

	fn value_type(&self) -> glib::Type {
		glib::GString::static_type()
	}
}

impl StaticType for ButtonState {
	fn static_type() -> glib::Type {
		glib::GString::static_type()
	}
}

unsafe impl<'a> FromValue<'a> for ButtonState {
	type Checker = glib::value::GenericValueTypeChecker<Self>;

	unsafe fn from_value(value: &'a glib::Value) -> Self {
		let s = value
			.get::<glib::GString>()
			.expect("ButtonState should be convertible from GString");

		match s.as_str() {
			"player-icon" => ButtonState::PlayerIcon,
			"switcher-icon" => ButtonState::SwitcherIcon,
			_ => ButtonState::PlayerIcon,
		}
	}
}

impl std::borrow::Borrow<str> for ButtonState {
	fn borrow(&self) -> &str {
		match self {
			ButtonState::PlayerIcon => "player-icon",
			ButtonState::SwitcherIcon => "switcher-icon",
		}
	}
}
