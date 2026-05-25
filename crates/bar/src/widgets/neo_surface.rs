use iced::{
	Alignment, Color, Element, Length, Padding, Rectangle, Size, Vector,
	advanced::{layout, renderer, widget},
};

use crate::style::COLORS;

#[derive(Debug, Clone, Copy)]
pub struct NeoSurfaceStyle {
	pub background: Color,
	pub disabled_background: Color,
	pub border: Color,
	pub text: Color,
	pub disabled_text_alpha: f32,
	pub radius: f32,
	pub border_width: f32,
	pub shadow_width: f32,
	pub padding: Padding,
}

impl Default for NeoSurfaceStyle {
	fn default() -> Self {
		Self {
			background: COLORS.white,
			disabled_background: COLORS.disabled_background,
			border: COLORS.border,
			text: COLORS.text,
			disabled_text_alpha: 0.5,
			radius: 3.0,
			border_width: 2.0,
			shadow_width: 4.0,
			padding: 8.0.into(),
		}
	}
}

pub fn layout<'a, Message, Theme, Renderer>(
	content: &mut Element<'a, Message, Theme, Renderer>, tree: &mut widget::Tree,
	renderer: &Renderer, limits: &layout::Limits, width: Length, height: Length,
	style: NeoSurfaceStyle,
) -> layout::Node
where
	Renderer: renderer::Renderer,
{
	let padding = style.padding;
	let shadow = style.shadow_width;

	layout::positioned(
		limits,
		width,
		height,
		Padding {
			top: padding.top,
			right: padding.right + shadow,
			bottom: padding.bottom + shadow,
			left: padding.left,
		},
		|limits| {
			content
				.as_widget_mut()
				.layout(&mut tree.children[0], renderer, &limits.loose())
		},
		|content, size| content.align(Alignment::Center, Alignment::Center, size),
	)
}

pub fn draw<Renderer>(
	renderer: &mut Renderer, bounds: Rectangle, style: NeoSurfaceStyle, offset: f32,
	draw_content: impl FnOnce(&mut Renderer),
) where
	Renderer: renderer::Renderer,
{
	let shadow_width = style.shadow_width;

	let shadow = Rectangle {
		x: bounds.x + shadow_width,
		y: bounds.y + shadow_width,
		width: bounds.width - shadow_width,
		height: bounds.height - shadow_width,
	};

	let surface = Rectangle {
		x: bounds.x,
		y: bounds.y,
		width: bounds.width - shadow_width,
		height: bounds.height - shadow_width,
	};

	renderer.fill_quad(
		renderer::Quad {
			bounds: shadow,
			border: iced::Border {
				radius: style.radius.into(),
				..Default::default()
			},
			snap: true,
			..Default::default()
		},
		style.border,
	);

	renderer.with_translation(Vector::new(offset, offset), |renderer| {
		renderer.fill_quad(
			renderer::Quad {
				bounds: surface,
				border: iced::Border {
					color: style.border,
					width: style.border_width,
					radius: style.radius.into(),
				},
				snap: true,
				..Default::default()
			},
			style.background,
		);

		draw_content(renderer);
	});
}

pub fn size(width: Length, height: Length) -> Size<Length> {
	Size::new(width, height)
}
