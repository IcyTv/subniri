//! Notification-provided paths never enter the shared theme resolver.
use neo_widgets::icons::{ResolvedIcon, background};

pub fn local_source(source: &str) -> bool {
	source.starts_with('/') || source.starts_with("file:")
}

pub fn load_local(source: &str) -> Option<ResolvedIcon> {
	let bytes = super::body::local_bytes(source, 256 * 1024)?;
	if let Some(handle) = validated_svg(&bytes) {
		return Some(ResolvedIcon::Svg(handle));
	}
	// Raster decoding uses the same allocation and dimension limits as body images.
	super::body::decode_image(bytes, 48, 48).map(ResolvedIcon::Image)
}

fn validated_svg(bytes: &[u8]) -> Option<iced::widget::svg::Handle> {
	// Parse XML directly: no SVGZ decompression or DTD/entity expansion.
	let document = usvg::roxmltree::Document::parse_with_options(
		std::str::from_utf8(bytes).ok()?,
		usvg::roxmltree::ParsingOptions {
			allow_dtd: false,
			nodes_limit: 4096,
		},
	)
	.ok()?;
	// Bound nesting before conversion, including non-rendering resource groups.
	if document
		.descendants()
		.any(|node| node.ancestors().take(65).count() > 64)
	{
		return None;
	}
	let options = usvg::Options {
		image_href_resolver: usvg::ImageHrefResolver {
			resolve_data: Box::new(|_, _, _| None),
			resolve_string: Box::new(|_, _| None),
		},
		..Default::default()
	};
	let tree = usvg::Tree::from_xmltree(&document, &options).ok()?;
	let size = tree.size();
	if size.width() > 2048.0 || size.height() > 2048.0 {
		return None;
	}
	// Filters allocate intermediate surfaces. Do not admit them for tiny icons.
	if !tree.filters().is_empty() || tree.root().children().is_empty() {
		return None;
	}
	// Serialize the parsed tree, not the untrusted source: disabled hrefs cannot
	// be reopened later by Iced's renderer, whose resolver uses default options.
	let sanitized = tree.to_string(&usvg::WriteOptions::default());
	if sanitized.len() > 1024 * 1024 {
		return None;
	}
	Some(iced::widget::svg::Handle::from_memory(
		sanitized.into_bytes(),
	))
}

pub fn resolve(
	n: &daemon_common::Notification, local: Option<&Option<ResolvedIcon>>,
) -> ResolvedIcon {
	let explicit = if local_source(&n.app_icon) {
		local.cloned().flatten()
	} else if !n.app_icon.is_empty() && n.app_icon.len() < 256 && !n.app_icon.contains('/') {
		background::try_resolve_icon_name(&n.app_icon, 24, 1)
	} else {
		None
	};
	if let Some(icon) = explicit {
		return icon;
	}

	let missing = neo_widgets::icons::default_icon();
	if let Some(id) = n
		.hints
		.get("desktop-entry")
		.and_then(|v| <&str>::try_from(v).ok())
	{
		if id.len() < 256 && !id.contains('/') {
			let icon = background::resolve_icon(id, 24, 1);
			if icon != missing {
				return icon;
			}
		}
	}
	if n.app_name.len() < 256 && !n.app_name.contains('/') {
		let icon = background::resolve_icon(&n.app_name, 24, 1);
		if icon != missing {
			return icon;
		}
	}
	ResolvedIcon::Svg(neo_widgets::phosphor_icon!("app-window", "bold"))
}
