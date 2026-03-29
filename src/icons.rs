use gtk4::gio;
use gtk4::prelude::{AppInfoExt, Cast, FileExt};

use astal_notifd::prelude::NotificationExt;
use astal_notifd::Notification;

pub fn register_bundled_icons() {
	let res_bytes = include_bytes!(concat!(env!("OUT_DIR"), "/lucide.gresource"));
	let resource = gio::Resource::from_data(&res_bytes.into()).unwrap();
	gio::resources_register(&resource);

	let display = gtk4::gdk::Display::default().unwrap();
	let theme = gtk4::IconTheme::for_display(&display);

	theme.add_resource_path("/de/icytv/niribar/icons");
}

pub fn resolve_notification_icon(notification: &Notification) -> Option<gio::Icon> {
	let icon_theme = gtk4::gdk::Display::default().map(|display| gtk4::IconTheme::for_display(&display));

	for candidate in [notification.app_icon(), notification.image()] {
		if let Some(icon) = resolve_icon_candidate(candidate.as_str(), icon_theme.as_ref()) {
			return Some(icon);
		}
	}

	let desktop_entry = notification.desktop_entry();
	if !desktop_entry.is_empty() {
		if let Some(icon) = resolve_desktop_entry_icon(desktop_entry.as_str()) {
			return Some(icon);
		}

		if let Some(icon) = resolve_icon_candidate(desktop_entry.as_str(), icon_theme.as_ref()) {
			return Some(icon);
		}
	}

	let app_name = notification.app_name();
	if !app_name.is_empty() {
		let normalized = app_name.to_ascii_lowercase().replace(' ', "-");
		if let Some(icon) = resolve_icon_candidate(&normalized, icon_theme.as_ref()) {
			return Some(icon);
		}
	}

	None
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

	resolve_app_info_icon_by_fuzzy_match(app_id)
}

fn resolve_icon_candidate(candidate: &str, icon_theme: Option<&gtk4::IconTheme>) -> Option<gio::Icon> {
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

fn resolve_desktop_entry_icon(desktop_entry: &str) -> Option<gio::Icon> {
	let desktop_file = if desktop_entry.ends_with(".desktop") {
		desktop_entry.to_owned()
	} else {
		format!("{desktop_entry}.desktop")
	};

	gio::DesktopAppInfo::new(&desktop_file).and_then(|app_info| app_info.icon())
}

fn app_id_candidates(app_id: &str) -> Vec<String> {
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

fn resolve_app_info_icon_by_fuzzy_match(app_id: &str) -> Option<gio::Icon> {
	let needle = app_id.to_ascii_lowercase();

	for app_info in gio::AppInfo::all() {
		let id_match = app_info
			.id()
			.map(|id| id.to_ascii_lowercase().contains(&needle))
			.unwrap_or(false);
		let name_match = app_info.name().to_ascii_lowercase().contains(&needle);
		let display_name_match = app_info.display_name().to_ascii_lowercase().contains(&needle);
		let executable_match = app_info
			.executable()
			.to_string_lossy()
			.to_ascii_lowercase()
			.contains(&needle);
		let startup_wm_class_match = app_info
			.clone()
			.downcast::<gio::DesktopAppInfo>()
			.ok()
			.and_then(|desktop_info| desktop_info.startup_wm_class())
			.map(|wm_class| wm_class.to_ascii_lowercase().contains(&needle))
			.unwrap_or(false);

		if (id_match || name_match || display_name_match || executable_match || startup_wm_class_match)
			&& let Some(icon) = app_info.icon()
		{
			return Some(icon);
		}
	}

	None
}

include!(concat!(env!("OUT_DIR"), "/icons.rs"));
