use iced::{
	Color, Element, Length, Shadow, Vector,
	widget::{PickList, overlay::menu, pick_list},
};

use crate::style::{COLORS, Colors};

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
	colors: Option<Colors>,
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
			colors: None,
		}
	}

	pub fn with_colors(mut self, colors: Colors) -> Self {
		self.pick_list = self
			.pick_list
			.style(move |_theme, _status| closed_style_with_colors(colors))
			.menu_style(move |_theme| menu_style_with_colors(colors));
		self.colors = Some(colors);
		self
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
		let field = NeoField::new(pick_list.pick_list, pick_list.width, pick_list.enabled);

		Element::new(match pick_list.colors {
			Some(colors) => field.with_colors(colors),
			None => field,
		})
	}
}

fn closed_style(_theme: &iced::Theme, _status: pick_list::Status) -> pick_list::Style {
	closed_style_with_colors(COLORS)
}

fn closed_style_with_colors(colors: Colors) -> pick_list::Style {
	pick_list::Style {
		text_color: colors.text,
		placeholder_color: colors.secondary,
		handle_color: colors.text,
		background: Color::TRANSPARENT.into(),
		border: iced::Border {
			color: colors.border,
			width: 0.0,
			radius: 0.0.into(),
		},
	}
}

fn menu_style(_theme: &iced::Theme) -> menu::Style {
	menu_style_with_colors(Colors {
		surface: COLORS.white,
		selection: COLORS.decorative.blue90,
		..COLORS
	})
}

fn menu_style_with_colors(colors: Colors) -> menu::Style {
	menu::Style {
		background: colors.surface.into(),
		border: iced::Border {
			color: colors.border,
			width: 2.0,
			radius: 3.0.into(),
		},
		text_color: colors.text,
		selected_text_color: colors.text,
		selected_background: colors.selection.into(),
		shadow: Shadow {
			color: colors.border,
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
	fn explicit_dark_colors_style_closed_picker_and_menu() {
		let colors = crate::style::DARK;
		let options = ["One", "Two"];
		let picker =
			neo_pick_list::<_, ()>(Some(&options[0]), &options, |value| (*value).to_owned())
				.with_colors(colors);
		let closed = closed_style_with_colors(colors);
		let menu = menu_style_with_colors(colors);

		assert_eq!(picker.colors, Some(colors));
		assert_eq!(closed.text_color, colors.text);
		assert_eq!(closed.placeholder_color, colors.secondary);
		assert_eq!(closed.handle_color, colors.text);
		assert_eq!(closed.border.color, colors.border);
		assert_eq!(closed.background, Color::TRANSPARENT.into());
		assert_eq!(menu.background, colors.surface.into());
		assert_eq!(menu.text_color, colors.text);
		assert_eq!(menu.selected_text_color, colors.text);
		assert_eq!(menu.selected_background, colors.selection.into());
		assert_eq!(menu.border.color, colors.border);
		assert_eq!(menu.shadow.color, colors.border);
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
