use iced::widget::image;
use neo_widgets::icons::{
	ResolvedIcon, default_icon, try_resolve_icon_name, try_resolve_icon_name_in_path,
};
use zbus::Proxy;

use super::ICON_SIZE;

type Pixmap = (i32, i32, Vec<u8>);

#[derive(Debug, Clone, Default)]
pub struct TrayIcon {
	pixmap: Option<image::Handle>,
	name: Option<String>,
	theme_path: Option<String>,
}

impl TrayIcon {
	pub fn from_parts(name: String, pixmaps: Vec<Pixmap>, theme_path: Option<String>) -> Self {
		Self {
			pixmap: decode_pixmaps(pixmaps),
			name: (!name.is_empty()).then_some(name),
			theme_path,
		}
	}

	pub async fn read(
		proxy: &Proxy<'_>, name_property: &str, pixmap_property: &str, theme_path: Option<String>,
	) -> Self {
		let name = proxy
			.get_property::<String>(name_property)
			.await
			.ok()
			.filter(|name| !name.is_empty());
		let pixmap = proxy
			.get_property::<Vec<Pixmap>>(pixmap_property)
			.await
			.ok()
			.and_then(decode_pixmaps);

		Self {
			pixmap,
			name,
			theme_path,
		}
	}

	pub fn resolve(&self, size: u32) -> ResolvedIcon {
		let named = self.name.as_ref().and_then(|name| {
			self.theme_path.as_ref().map_or_else(
				|| try_resolve_icon_name(name, size, 1),
				|path| try_resolve_icon_name_in_path(name, path, size, 1),
			)
		});

		named
			.or_else(|| self.pixmap.clone().map(ResolvedIcon::Image))
			.unwrap_or_else(default_icon)
	}
}

pub fn decode_pixmaps(pixmaps: Vec<Pixmap>) -> Option<image::Handle> {
	let (width, height, pixels) = pixmaps
		.into_iter()
		.filter(|(width, height, pixels)| {
			*width > 0 && *height > 0 && pixels.len() >= (*width as usize) * (*height as usize) * 4
		})
		.min_by_key(|(width, height, _)| (width.abs_diff(ICON_SIZE), height.abs_diff(ICON_SIZE)))?;

	let pixel_count = (width as usize) * (height as usize);
	let mut rgba = Vec::with_capacity(pixel_count * 4);
	for pixel in pixels.chunks_exact(4).take(pixel_count) {
		// StatusNotifierItem pixmaps contain ARGB32 pixels in network byte order.
		rgba.extend_from_slice(&[pixel[1], pixel[2], pixel[3], pixel[0]]);
	}

	Some(image::Handle::from_rgba(width as u32, height as u32, rgba))
}
