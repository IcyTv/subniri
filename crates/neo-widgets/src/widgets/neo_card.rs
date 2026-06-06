use iced::{
	Color, Element, Event, Length, Padding, Rectangle,
	advanced::{
		Layout, Shell, Widget, layout, mouse, renderer,
		widget::{self, Tree},
	},
};

use super::neo_surface::{self, NeoContentSurfaceStyle};

pub type NeoCardStyle = NeoContentSurfaceStyle;

pub fn neo_card<'a, Message, Theme, Renderer>(
	content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> NeoCard<'a, Message, Theme, Renderer> {
	NeoCard::new(content)
}

#[must_use]
pub struct NeoCard<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
	content: Element<'a, Message, Theme, Renderer>,
	style: NeoCardStyle,
	width: Length,
	height: Length,
}

impl<'a, Message, Theme, Renderer> NeoCard<'a, Message, Theme, Renderer> {
	pub fn new(content: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
		Self {
			content: content.into(),
			style: NeoCardStyle::default(),
			width: Length::Shrink,
			height: Length::Shrink,
		}
	}

	pub fn style(mut self, style: NeoCardStyle) -> Self {
		self.style = style;
		self
	}

	pub fn width(mut self, width: impl Into<Length>) -> Self {
		self.width = width.into();
		self
	}

	pub fn height(mut self, height: impl Into<Length>) -> Self {
		self.height = height.into();
		self
	}

	pub fn background(mut self, color: Color) -> Self {
		self.style.surface.background = color;
		self
	}

	pub fn radius(mut self, radius: f32) -> Self {
		self.style.surface.radius = radius;
		self
	}

	pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
		self.style.padding = padding.into();
		self
	}
}

impl<'a, Message: 'a, Theme: 'a, Renderer: iced::advanced::Renderer + 'a>
	From<NeoCard<'a, Message, Theme, Renderer>> for Element<'a, Message, Theme, Renderer>
{
	fn from(card: NeoCard<'a, Message, Theme, Renderer>) -> Self {
		Self::new(card)
	}
}

impl<'a, Message, Theme: 'a, Renderer: 'a> Widget<Message, Theme, Renderer>
	for NeoCard<'a, Message, Theme, Renderer>
where
	Renderer: renderer::Renderer,
{
	fn size(&self) -> iced::Size<Length> {
		neo_surface::size(self.width, self.height)
	}

	fn children(&self) -> Vec<Tree> {
		vec![Tree::new(&self.content)]
	}

	fn diff(&self, tree: &mut Tree) {
		tree.diff_children(std::slice::from_ref(&self.content));
	}

	fn layout(
		&mut self, tree: &mut widget::Tree, renderer: &Renderer, limits: &layout::Limits,
	) -> layout::Node {
		neo_surface::layout(
			&mut self.content,
			tree,
			renderer,
			limits,
			self.width,
			self.height,
			self.style,
		)
	}

	fn update(
		&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor,
		renderer: &Renderer, shell: &mut Shell<'_, Message>, viewport: &Rectangle,
	) {
		self.content.as_widget_mut().update(
			&mut tree.children[0],
			event,
			layout.child(0),
			cursor,
			renderer,
			shell,
			viewport,
		);
	}

	fn mouse_interaction(
		&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
		renderer: &Renderer,
	) -> mouse::Interaction {
		self.content.as_widget().mouse_interaction(
			&tree.children[0],
			layout.child(0),
			cursor,
			viewport,
			renderer,
		)
	}

	fn draw(
		&self, tree: &iced::advanced::widget::Tree, renderer: &mut Renderer, theme: &Theme,
		style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
	) {
		let bounds = layout.bounds();
		let child_layout = layout.child(0);

		neo_surface::draw(renderer, bounds, self.style.surface, 0.0, |renderer| {
			self.content.as_widget().draw(
				&tree.children[0],
				renderer,
				theme,
				style,
				child_layout,
				cursor,
				viewport,
			);
		});
	}
}
