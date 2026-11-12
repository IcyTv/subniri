//! Offscreen layout proof using the same renderer and widgets as the bar.
use iced::{
	Element, Rectangle, Size,
	advanced::{Layout, layout, renderer, widget},
};
use std::collections::BTreeMap;

pub(super) type Error = Box<dyn std::error::Error>;

pub(super) async fn renderer() -> Result<iced::Renderer, Error> {
	<iced::Renderer as renderer::Headless>::new(renderer::Settings::default(), None)
		.await
		.ok_or_else(|| "No headless renderer available".into())
}

fn collect(layout: Layout<'_>, nodes: &mut Vec<Rectangle>) {
	nodes.push(layout.bounds());
	for child in layout.children() {
		collect(child, nodes);
	}
}

#[derive(Default)]
pub(super) struct Snapshot {
	pub texts: BTreeMap<String, Rectangle>,
	pub nodes: Vec<Rectangle>,
}

impl widget::Operation for Snapshot {
	fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn widget::Operation)) {
		operate(self);
	}

	fn text(&mut self, _: Option<&widget::Id>, bounds: Rectangle, text: &str) {
		self.texts.insert(text.to_owned(), bounds);
	}
}

pub(super) fn capture<Message>(
	mut content: Element<'_, Message>, renderer: &mut iced::Renderer,
	colors: neo_widgets::style::Colors, name: &str,
) -> Result<Snapshot, Error> {
	use renderer::Renderer;

	let mut tree = widget::Tree::new(content.as_widget());
	content.as_widget_mut().diff(&mut tree);
	let node = content.as_widget_mut().layout(
		&mut tree,
		renderer,
		&layout::Limits::new(Size::ZERO, Size::new(1280.0, 720.0)),
	);
	let bounds = Rectangle::with_size(node.size());
	let mut snapshot = Snapshot::default();
	content
		.as_widget_mut()
		.operate(&mut tree, Layout::new(&node), renderer, &mut snapshot);

	collect(Layout::new(&node), &mut snapshot.nodes);

	if let Ok(directory) = std::env::var("SUBNIRI_UI_ARTIFACTS") {
		let size = Size::new(bounds.width.ceil() as u32, bounds.height.ceil() as u32);
		renderer.reset(bounds);
		content.as_widget().draw(
			&tree,
			renderer,
			&colors.iced_theme(),
			&renderer::Style {
				text_color: colors.text,
			},
			Layout::new(&node),
			iced::advanced::mouse::Cursor::Unavailable,
			&bounds,
		);
		let pixels = renderer::Headless::screenshot(renderer, size, 1.0, colors.body);
		let directory = std::path::Path::new(&directory);
		std::fs::create_dir_all(directory)?;
		image::save_buffer(
			directory.join(format!("{name}.png")),
			&pixels,
			size.width,
			size.height,
			image::ColorType::Rgba8,
		)?;
	}

	Ok(snapshot)
}
