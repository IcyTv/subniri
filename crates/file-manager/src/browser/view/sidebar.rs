//! Give the background context target explicit fill constraints. A shrink-sized
//! button around proportional scroll regions otherwise collapses their layout.
use crate::{Message, drives, ui::style};
use iced::{Element, Length, Theme, advanced::Renderer, widget::container};
use neo_widgets::widgets::{NeoButton, NeoButtonStyle, NeoContentSurfaceStyle, NeoSurfaceStyle};

pub(super) fn background<'a, R: Renderer + 'a>(
	content: Element<'a, Message, Theme, R>,
) -> Element<'a, Message, Theme, R> {
	let surface = NeoSurfaceStyle {
		background: style::SIDEBAR,
		border: style::INK,
		radius: 0.0,
		border_width: 0.0,
		shadow_width: 0.0,
	};

	NeoButton::new(container(content).width(Length::Fill).height(Length::Fill))
		.style(NeoButtonStyle {
			surface: NeoContentSurfaceStyle {
				surface,
				padding: 0.into(),
			},
			hovered: surface,
			focused: NeoContentSurfaceStyle {
				surface,
				padding: 0.into(),
			},
			disabled_background: style::SIDEBAR,
		})
		.width(Length::Fill)
		.height(Length::Fill)
		.focusable(false)
		.on_context_menu_with_mouse_position(|point| drives::Message::MenuAt(None, point).into())
		.into()
}

#[cfg(test)]
mod tests {
	use super::*;
	use iced::{
		Size,
		advanced::{layout, widget::Tree},
		widget::{Space, column, scrollable},
	};

	#[test]
	fn background_context_wrapper_preserves_sidebar_fill_regions() {
		let content: Element<'_, Message, Theme, ()> = column![
			scrollable(Space::new().height(200)).height(Length::FillPortion(3)),
			scrollable(Space::new().height(200)).height(Length::FillPortion(2)),
		]
		.height(Length::Fill)
		.into();
		let mut view = background(content);
		let mut tree = Tree::new(view.as_widget());
		view.as_widget_mut().diff(&mut tree);

		let node = view.as_widget_mut().layout(
			&mut tree,
			&(),
			&layout::Limits::new(Size::ZERO, Size::new(200.0, 700.0)),
		);

		assert_eq!(node.size(), Size::new(200.0, 700.0));
		assert!(
			node.children()
				.first()
				.is_some_and(|child| child.size().height == 700.0)
		);
	}
}
