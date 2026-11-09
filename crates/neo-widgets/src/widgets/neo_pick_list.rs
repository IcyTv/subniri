use iced::{
	Color, Element, Length, Shadow, Vector,
	widget::{PickList, overlay::menu, pick_list},
};

use crate::style::COLORS;

use super::neo_input::NeoField;

pub fn neo_pick_list<'a, T: Clone + PartialEq + 'a, Message: Clone + 'a>(
	selected: Option<&'a T>, options: &'a [T], to_string: impl Fn(&T) -> String + 'a,
) -> NeoPickList<'a, T, Message> {
	NeoPickList::new(selected, options, to_string)
}

/// Native menu state, placement, and selection with a hard-shadow neo opener.
#[must_use]
pub struct NeoPickList<'a, T: Clone + PartialEq + 'a, Message> {
	pick_list: PickList<'a, T, &'a [T], &'a T, Message>,
	width: Length,
	enabled: bool,
}

impl<'a, T: Clone + PartialEq + 'a, Message: Clone + 'a> NeoPickList<'a, T, Message> {
	pub fn new(
		selected: Option<&'a T>, options: &'a [T], to_string: impl Fn(&T) -> String + 'a,
	) -> Self {
		Self {
			pick_list: PickList::new(selected, options, to_string)
				.style(closed_style)
				.menu_style(menu_style),
			width: Length::Shrink,
			enabled: false,
		}
	}

	pub fn on_select(mut self, on_select: impl Fn(T) -> Message + 'a) -> Self {
		self.pick_list = self.pick_list.on_select(on_select);
		self.enabled = true;
		self
	}

	pub fn width(mut self, width: impl Into<Length>) -> Self {
		self.width = width.into();
		self.pick_list = self.pick_list.width(self.width);
		self
	}
}

impl<'a, T: Clone + PartialEq + 'a, Message: Clone + 'a> From<NeoPickList<'a, T, Message>>
	for Element<'a, Message>
{
	fn from(pick_list: NeoPickList<'a, T, Message>) -> Self {
		Element::new(NeoField::new(
			pick_list.pick_list,
			pick_list.width,
			pick_list.enabled,
		))
	}
}

fn closed_style(_theme: &iced::Theme, _status: pick_list::Status) -> pick_list::Style {
	pick_list::Style {
		text_color: COLORS.black,
		placeholder_color: COLORS.secondary,
		handle_color: COLORS.black,
		background: Color::TRANSPARENT.into(),
		border: iced::Border {
			color: COLORS.black,
			width: 0.0,
			radius: 0.0.into(),
		},
	}
}

fn menu_style(_theme: &iced::Theme) -> menu::Style {
	menu::Style {
		background: COLORS.white.into(),
		border: iced::Border {
			color: COLORS.black,
			width: 2.0,
			radius: 3.0.into(),
		},
		text_color: COLORS.black,
		selected_text_color: COLORS.black,
		selected_background: COLORS.decorative.blue90.into(),
		shadow: Shadow {
			color: COLORS.black,
			offset: Vector::new(4.0, 4.0),
			blur_radius: 0.0,
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn closed_pick_list_has_no_native_chrome() {
		let style = closed_style(&iced::Theme::Light, pick_list::Status::Active);

		assert_eq!(style.background, Color::TRANSPARENT.into());
		assert_eq!(style.border.width, 0.0);
		assert_eq!(style.text_color, COLORS.black);
		assert_eq!(style.handle_color, COLORS.black);
	}

	#[test]
	fn menu_uses_opaque_hard_shadow_and_blue_selection() {
		let style = menu_style(&iced::Theme::Dark);

		assert_eq!(style.background, COLORS.white.into());
		assert_eq!(style.border.color, COLORS.black);
		assert_eq!(style.border.width, 2.0);
		assert_eq!(style.selected_background, COLORS.decorative.blue90.into());
		assert_eq!(style.selected_text_color, COLORS.black);
		assert_eq!(style.shadow.color, COLORS.black);
		assert_eq!(style.shadow.offset, Vector::new(4.0, 4.0));
		assert_eq!(style.shadow.blur_radius, 0.0);
	}
}
