//! A focused path field's in-window dropdown. Its layout never resizes the file pane.
use iced::{
	Element, Event, Length, Point, Rectangle, Size, Vector,
	advanced::{
		Layout, Renderer, Shell, Widget, layout, mouse, overlay, renderer,
		widget::{Id, Operation, Tree, operation::Focusable},
	},
};

use crate::Message;

pub(crate) const ROW_HEIGHT: f32 = 42.0;
pub(crate) const VISIBLE_ROWS: usize = 7;

pub(crate) fn menu(
	suggestions: &[crate::filesystem::locations::FolderIdentity], selected: Option<usize>,
	colors: neo_widgets::style::Colors,
) -> Element<'_, Message> {
	use crate::{browser::Message as BrowserMessage, ui::style};
	use iced::{
		Alignment,
		widget::{column, container, row, scrollable, svg, text},
	};

	let mut rows = column![];
	for (index, folder) in suggestions.iter().enumerate() {
		let label = column![
			text(&folder.label).size(12).wrapping(text::Wrapping::None),
			text(folder.path.to_string_lossy().into_owned())
				.size(10)
				.color(colors.secondary)
				.wrapping(text::Wrapping::None),
		]
		.width(Length::Fill);
		let content = row![
			svg(folder.icon.clone())
				.width(18)
				.height(18)
				.style(move |_, _| svg::Style {
					color: folder.symbolic.then_some(colors.text)
				}),
			label,
		]
		.spacing(8)
		.align_y(Alignment::Center);
		let button = style::control(content, selected == Some(index), colors)
			.padding([4, 8])
			.width(Length::Fill)
			.height(ROW_HEIGHT)
			.focusable(false)
			.focus_on_click(false)
			.on_press(BrowserMessage::FillPath(Some(folder.path.clone())).into());
		rows = rows.push(button);
	}

	container(
		scrollable(rows)
			.id("path-suggestions")
			.height(ROW_HEIGHT * suggestions.len().min(VISIBLE_ROWS) as f32),
	)
	.padding(2)
	.style(move |_| crate::ui::icons::framed(colors.surface, 1.0, colors))
	.width(Length::Fill)
	.into()
}

pub(crate) fn dropdown<'a, R: Renderer + 'a>(
	input: Element<'a, Message, iced::Theme, R>, menu: Element<'a, Message, iced::Theme, R>,
	visible: bool,
) -> Element<'a, Message, iced::Theme, R> {
	Element::new(Dropdown {
		input,
		menu,
		visible,
	})
}

struct Dropdown<'a, R> {
	input: Element<'a, Message, iced::Theme, R>,
	menu: Element<'a, Message, iced::Theme, R>,
	visible: bool,
}

#[derive(Default)]
struct LocationFocus(bool);

impl Operation for LocationFocus {
	fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
		operate(self);
	}

	fn focusable(&mut self, id: Option<&Id>, _: Rectangle, state: &mut dyn Focusable) {
		if id == Some(&Id::new("location")) {
			self.0 = state.is_focused();
		}
	}
}

impl<R: Renderer> Widget<Message, iced::Theme, R> for Dropdown<'_, R> {
	fn size(&self) -> Size<Length> {
		self.input.as_widget().size()
	}

	fn diff(&mut self, tree: &mut Tree) {
		tree.diff_children(&mut [self.input.as_widget_mut(), self.menu.as_widget_mut()]);
	}

	fn layout(&mut self, tree: &mut Tree, renderer: &R, limits: &layout::Limits) -> layout::Node {
		self.input
			.as_widget_mut()
			.layout(&mut tree.children[0], renderer, limits)
	}

	fn operate(
		&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &R, operation: &mut dyn Operation,
	) {
		self.input
			.as_widget_mut()
			.operate(&mut tree.children[0], layout, renderer, operation);
	}

	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &R, shell: &mut Shell<'_, Message>, viewport: &Rectangle,
	) {
		if shell.is_event_captured() {
			return;
		}

		self.input.as_widget_mut().update(
			&mut tree.children[0],
			event,
			layout,
			cursor,
			renderer,
			shell,
			viewport,
		);
	}

	fn draw(
		&self, tree: &Tree, renderer: &mut R, theme: &iced::Theme, style: &renderer::Style,
		layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		self.input.as_widget().draw(
			&tree.children[0],
			renderer,
			theme,
			style,
			layout,
			cursor,
			viewport,
		);
	}

	fn mouse_interaction(
		&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
		renderer: &R,
	) -> mouse::Interaction {
		self.input.as_widget().mouse_interaction(
			&tree.children[0],
			layout,
			cursor,
			viewport,
			renderer,
		)
	}

	fn overlay<'b>(
		&'b mut self, tree: &'b mut Tree, layout: Layout<'b>, renderer: &R, viewport: &Rectangle,
		translation: Vector,
	) -> Vec<overlay::Element<'b, Message, iced::Theme, R>> {
		let mut focus = LocationFocus::default();
		self.input
			.as_widget_mut()
			.operate(&mut tree.children[0], layout, renderer, &mut focus);

		if !self.visible || !focus.0 {
			return self.input.as_widget_mut().overlay(
				&mut tree.children[0],
				layout,
				renderer,
				viewport,
				translation,
			);
		}

		vec![overlay::Element::new(Box::new(Menu {
			content: &mut self.menu,
			tree: &mut tree.children[1],
			anchor: layout.bounds() + translation,
			viewport: *viewport,
		}))]
	}
}

struct Menu<'a, 'b, R> {
	content: &'a mut Element<'b, Message, iced::Theme, R>,
	tree: &'a mut Tree,
	anchor: Rectangle,
	viewport: Rectangle,
}

impl<R: Renderer> overlay::Overlay<Message, iced::Theme, R> for Menu<'_, '_, R> {
	fn layout(&mut self, renderer: &R, bounds: Size) -> layout::Node {
		let viewport = self
			.viewport
			.intersection(&Rectangle::with_size(bounds))
			.unwrap_or_default();
		let available = menu_bounds(self.anchor, viewport);
		let limits = layout::Limits::new(Size::new(available.width, 0.0), available.size());
		let node = self
			.content
			.as_widget_mut()
			.layout(self.tree, renderer, &limits);
		let y = if available.y < self.anchor.y {
			self.anchor.y - node.size().height - 2.0
		} else {
			available.y
		};

		node.move_to(Point::new(available.x, y.max(viewport.y)))
	}

	fn draw(
		&self, renderer: &mut R, theme: &iced::Theme, style: &renderer::Style, layout: Layout<'_>,
		cursor: mouse::Cursor,
	) {
		self.content.as_widget().draw(
			self.tree,
			renderer,
			theme,
			style,
			layout,
			cursor,
			&self.viewport,
		);
	}

	fn operate(&mut self, layout: Layout<'_>, renderer: &R, operation: &mut dyn Operation) {
		self.content
			.as_widget_mut()
			.operate(self.tree, layout, renderer, operation);
	}

	fn update(
		&mut self, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor, renderer: &R,
		shell: &mut Shell<'_, Message>,
	) {
		// The path field keeps keyboard focus; suggestion rows handle pointer input only.
		if matches!(
			event,
			Event::Keyboard(_) | Event::InputMethod(_) | Event::Clipboard(_)
		) {
			return;
		}

		self.content.as_widget_mut().update(
			self.tree,
			event,
			layout,
			cursor,
			renderer,
			shell,
			&self.viewport,
		);

		if cursor.is_over(layout.bounds()) && matches!(event, Event::Mouse(_) | Event::Touch(_)) {
			shell.capture_event();
		}
	}

	fn mouse_interaction(
		&self, layout: Layout<'_>, cursor: mouse::Cursor, renderer: &R,
	) -> mouse::Interaction {
		self.content.as_widget().mouse_interaction(
			self.tree,
			layout,
			cursor,
			&self.viewport,
			renderer,
		)
	}
}

fn menu_bounds(anchor: Rectangle, viewport: Rectangle) -> Rectangle {
	let below = (viewport.y + viewport.height - anchor.y - anchor.height - 2.0).max(0.0);
	let above = (anchor.y - viewport.y - 2.0).max(0.0);
	let flip = below < ROW_HEIGHT * VISIBLE_ROWS as f32 && above > below;
	let width = anchor.width.min(viewport.width);
	let height = if flip { above } else { below }.min(ROW_HEIGHT * VISIBLE_ROWS as f32 + 4.0);
	let x = anchor
		.x
		.clamp(viewport.x, viewport.x + viewport.width - width);
	let y = if flip {
		anchor.y - height - 2.0
	} else {
		anchor.y + anchor.height + 2.0
	};

	Rectangle {
		x,
		y,
		width,
		height,
	}
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use iced::{
		advanced::{
			shell::{Bus, Waker},
			widget::operation,
		},
		keyboard,
		widget::{Space, button, text_input},
		window,
	};

	#[test]
	fn dropdown_keeps_input_layout_and_only_opens_for_actual_field_focus() {
		let input: Element<'_, Message, iced::Theme, ()> = text_input("Path", "/work")
			.id("location")
			.on_input(|value| crate::browser::Message::PathChanged(value).into())
			.width(250)
			.into();
		let menu: Element<'_, Message, iced::Theme, ()> =
			Space::new().width(250).height(250).into();
		let mut widget = dropdown(input, menu, true);
		let mut tree = Tree::new(widget.as_widget());
		widget.as_widget_mut().diff(&mut tree);
		let node = widget.as_widget_mut().layout(
			&mut tree,
			&(),
			&layout::Limits::new(Size::ZERO, Size::new(500.0, 400.0)),
		);
		let viewport = Rectangle::with_size(Size::new(500.0, 400.0));

		assert!(node.size().height < 250.0);
		assert!(
			widget
				.as_widget_mut()
				.overlay(&mut tree, Layout::new(&node), &(), &viewport, Vector::ZERO)
				.is_empty()
		);

		widget.as_widget_mut().operate(
			&mut tree,
			Layout::new(&node),
			&(),
			&mut operation::focusable::focus::<()>(Id::new("location")),
		);
		let overlays = widget.as_widget_mut().overlay(
			&mut tree,
			Layout::new(&node),
			&(),
			&viewport,
			Vector::ZERO,
		);

		assert_eq!(overlays.len(), 1);
	}

	#[test]
	fn suggestion_click_publishes_fill_and_does_not_unfocus_the_field() {
		let input: Element<'_, Message, iced::Theme, ()> = text_input("Path", "/work")
			.id("location")
			.on_input(|value| crate::browser::Message::PathChanged(value).into())
			.width(250)
			.into();
		let target = std::path::PathBuf::from("/work/Child");
		let menu: Element<'_, Message, iced::Theme, ()> =
			button(Space::new().width(250).height(42))
				.on_press(crate::browser::Message::FillPath(Some(target.clone())).into())
				.into();
		let mut widget = dropdown(input, menu, true);
		let mut tree = Tree::new(widget.as_widget());
		widget.as_widget_mut().diff(&mut tree);
		let viewport = Rectangle::with_size(Size::new(500.0, 400.0));
		let node = widget.as_widget_mut().layout(
			&mut tree,
			&(),
			&layout::Limits::new(Size::ZERO, viewport.size()),
		);
		widget.as_widget_mut().operate(
			&mut tree,
			Layout::new(&node),
			&(),
			&mut operation::focusable::focus::<()>(Id::new("location")),
		);
		let mut bus = Bus::new();

		{
			let mut overlays = widget.as_widget_mut().overlay(
				&mut tree,
				Layout::new(&node),
				&(),
				&viewport,
				Vector::ZERO,
			);
			let overlay = overlays.first_mut().unwrap();
			let bounds = overlay.as_overlay_mut().layout(&(), viewport.size());
			let cursor = mouse::Cursor::Available(bounds.bounds().center());
			for event in [
				mouse::Event::ButtonPressed(mouse::Button::Left),
				mouse::Event::ButtonReleased(mouse::Button::Left),
			] {
				let mut shell = Shell::new(&window::Headless, Waker::noop(), &mut bus);
				overlay.as_overlay_mut().update(
					&Event::Mouse(event),
					Layout::new(&bounds),
					cursor,
					&(),
					&mut shell,
				);
				assert!(shell.is_event_captured());
			}

			let key = crate::browser::typeahead::tests::key(
				keyboard::Key::Named(keyboard::key::Named::Enter),
				None,
				keyboard::Modifiers::empty(),
			);
			let mut shell = Shell::new(&window::Headless, Waker::noop(), &mut bus);
			overlay.as_overlay_mut().update(
				&Event::Keyboard(key),
				Layout::new(&bounds),
				cursor,
				&(),
				&mut shell,
			);
			assert!(!shell.is_event_captured());
		}

		assert!(bus.into_iter().any(|message| matches!(message, Message::Browser(crate::browser::Message::FillPath(Some(path))) if path == target)));

		let mut focus = LocationFocus::default();
		widget
			.as_widget_mut()
			.operate(&mut tree, Layout::new(&node), &(), &mut focus);

		assert!(focus.0);
	}

	#[test]
	fn menu_fits_below_or_flips_above_without_leaving_the_viewport() {
		let viewport = Rectangle::with_size(Size::new(500.0, 400.0));

		for anchor in [
			Rectangle {
				x: 20.0,
				y: 10.0,
				width: 300.0,
				height: 30.0,
			},
			Rectangle {
				x: 350.0,
				y: 350.0,
				width: 300.0,
				height: 30.0,
			},
		] {
			let bounds = menu_bounds(anchor, viewport);

			assert!(bounds.x >= viewport.x && bounds.x + bounds.width <= viewport.width);
			assert!(bounds.y >= viewport.y && bounds.y + bounds.height <= viewport.height);
			assert!(bounds.height <= ROW_HEIGHT * VISIBLE_ROWS as f32 + 4.0);
		}
	}
}
