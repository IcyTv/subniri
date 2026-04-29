use gtk4::gio;
use gtk4::prelude::{AppInfoExt, Cast, FileExt};

// use astal_notifd::Notification;
// use astal_notifd::prelude::NotificationExt;

pub fn register_bundled_icons() {
	let res_bytes = include_bytes!(concat!(env!("OUT_DIR"), "/lucide.gresource"));
	let resource = gio::Resource::from_data(&res_bytes.into()).unwrap();
	gio::resources_register(&resource);

	let display = gtk4::gdk::Display::default().unwrap();
	let theme = gtk4::IconTheme::for_display(&display);

	theme.add_resource_path("/de/icytv/subniri/icons");
}

pub fn resolve_app_icon_from_app_id(app_id: &str) -> Option<gio::Icon> {
	let icon_theme = gtk4::gdk::Display::default().map(|display| gtk4::IconTheme::for_display(&display));

	for candidate in app_id_candidates(app_id) {
		if let Some(icon) = resolve_desktop_entry_icon(candidate.as_str()) {
			return Some(icon);
		}

		if let Some(icon) = resolve_icon_candidate(candidate.as_str(), icon_theme.as_ref()) {
			return Some(icon);
		}
	}

	resolve_app_info_icon_by_fallback_match(app_id)
}

pub fn resolve_icon_candidate(candidate: &str, icon_theme: Option<&gtk4::IconTheme>) -> Option<gio::Icon> {
	if candidate.is_empty() {
		return None;
	}

	if candidate.contains('/') {
		let file = gio::File::for_path(candidate);
		if file.query_exists(gio::Cancellable::NONE) {
			return Some(gio::FileIcon::new(&file).upcast());
		}
		return None;
	}

	if icon_theme.is_some_and(|theme| theme.has_icon(candidate)) {
		return gio::Icon::for_string(candidate).ok();
	}

	None
}

pub fn resolve_desktop_entry_icon(desktop_entry: &str) -> Option<gio::Icon> {
	let desktop_file = if desktop_entry.ends_with(".desktop") {
		desktop_entry.to_owned()
	} else {
		format!("{desktop_entry}.desktop")
	};

	gio::DesktopAppInfo::new(&desktop_file).and_then(|app_info| app_info.icon())
}

pub fn app_id_candidates(app_id: &str) -> Vec<String> {
	let mut candidates = Vec::new();
	let mut push = |value: String| {
		if !value.is_empty() && !candidates.contains(&value) {
			candidates.push(value);
		}
	};

	let lower = app_id.to_ascii_lowercase();
	push(app_id.to_string());
	push(lower.clone());
	push(lower.replace(' ', "-"));
	push(lower.replace('_', "-"));

	if let Some(last_segment) = lower.rsplit('.').next()
		&& last_segment != lower
	{
		push(last_segment.to_string());
	}

	candidates
}

pub fn resolve_app_info_icon_by_fallback_match(app_id: &str) -> Option<gio::Icon> {
	let needle = app_id.to_ascii_lowercase();

	for app_info in gio::AppInfo::all() {
		// 1. Check if the ID exactly matches (ignoring case)
		let id_match = app_info
			.id()
			.map(|id| id.to_ascii_lowercase() == needle)
			.unwrap_or(false);

		// 2. Check if the executable name exactly matches the app_id
		let executable_match = app_info
			.executable()
			.file_name()
			.map(|f| f.to_string_lossy().to_ascii_lowercase() == needle)
			.unwrap_or(false);

		// 3. Check StartupWMClass (Crucial for XWayland apps)
		let startup_wm_class_match = app_info
			.clone()
			.downcast::<gio::DesktopAppInfo>()
			.ok()
			.and_then(|desktop_info| desktop_info.startup_wm_class())
			.map(|wm_class| wm_class.to_ascii_lowercase() == needle)
			.unwrap_or(false);

		if (id_match || executable_match || startup_wm_class_match)
			&& let Some(icon) = app_info.icon()
		{
			return Some(icon);
		}
	}

	None
}

include!(concat!(env!("OUT_DIR"), "/icons.rs"));
