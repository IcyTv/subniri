use zbus::{Connection, Proxy, names::OwnedBusName, zvariant::OwnedObjectPath};

use super::dbusmenu::{self, DbusMenuProxy};
use super::icon::TrayIcon;

const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";

#[derive(Debug, Clone)]
pub struct TrayItem {
	pub address: ItemAddress,
	pub id: String,
	pub title: String,
	pub icon: TrayIcon,
	pub tooltip: Option<TrayItemToolTip>,
	pub status: TrayItemStatus,
	pub window_id: Option<u32>,
	pub overlay_icon: TrayIcon,
	pub attention_icon: TrayIcon,
	pub attention_movie_name: Option<String>,
	pub advertised_item_is_menu: Option<bool>,
	pub activation: Activation,
	pub menu: Option<OwnedObjectPath>,
}

#[derive(Debug, Clone)]
pub struct TrayItemToolTip {
	pub icon: TrayIcon,
	pub title: String,
	pub description: String,
}

#[derive(Debug, Clone, Copy)]
pub enum TrayItemStatus {
	Passive,
	Active,
	NeedsAttention,
	Unknown,
}

#[derive(Debug, Clone, Copy)]
pub enum Activation {
	Activate,
	ContextMenu,
	DbusMenu,
	Unsupported,
}

impl TrayItem {
	pub async fn read(connection: &Connection, service: &str) -> zbus::Result<Self> {
		let address = ItemAddress::parse(service)?;
		let proxy = Proxy::new_owned(
			connection.clone(),
			address.bus_name.clone(),
			address.object_path.clone(),
			ITEM_INTERFACE.to_string(),
		)
		.await?;

		let id = proxy.get_property::<String>("Id").await.unwrap_or_default();
		let title = proxy
			.get_property::<String>("Title")
			.await
			.unwrap_or_default();
		let theme_path = optional_string(&proxy, "IconThemePath").await;
		let icon = TrayIcon::read(&proxy, "IconName", "IconPixmap", theme_path.clone()).await;
		let overlay_icon = TrayIcon::read(
			&proxy,
			"OverlayIconName",
			"OverlayIconPixmap",
			theme_path.clone(),
		)
		.await;
		let attention_icon = TrayIcon::read(
			&proxy,
			"AttentionIconName",
			"AttentionIconPixmap",
			theme_path.clone(),
		)
		.await;
		let advertised_item_is_menu = proxy.get_property::<bool>("ItemIsMenu").await.ok();
		let menu = proxy
			.get_property::<OwnedObjectPath>("Menu")
			.await
			.ok()
			.filter(|path| path.as_str() != "/");
		let activation = read_activation(&proxy, advertised_item_is_menu, menu.is_some())
			.await
			.unwrap_or(Activation::Activate);

		Ok(Self {
			address,
			id,
			title,
			icon,
			tooltip: read_tooltip(&proxy, theme_path).await,
			status: read_status(&proxy).await,
			window_id: proxy
				.get_property::<u32>("WindowId")
				.await
				.ok()
				.filter(|id| *id != 0),
			overlay_icon,
			attention_icon,
			attention_movie_name: optional_string(&proxy, "AttentionMovieName").await,
			advertised_item_is_menu,
			activation,
			menu,
		})
	}
}

pub async fn activate(connection: &Connection, service: &str, x: i32, y: i32) -> zbus::Result<()> {
	call_action(connection, service, "Activate", x, y).await
}

pub async fn context_menu(
	connection: &Connection, service: &str, x: i32, y: i32,
) -> zbus::Result<()> {
	call_action(connection, service, "ContextMenu", x, y).await
}

pub async fn dbusmenu(
	connection: &Connection, bus_name: OwnedBusName, path: OwnedObjectPath,
) -> zbus::Result<(u32, dbusmenu::Layout)> {
	let proxy = dbusmenu_proxy(connection, bus_name, path).await?;
	if let Err(error) = proxy.about_to_show(0).await {
		// Electron's DBusMenu implementation can fail this hint while GetLayout works.
		log::debug!("DBusMenu root AboutToShow failed, continuing with GetLayout: {error}");
	}
	proxy.get_layout(0, -1, vec![]).await
}

pub async fn dbusmenu_submenu(
	connection: &Connection, bus_name: OwnedBusName, path: OwnedObjectPath, item_id: i32,
) -> zbus::Result<(u32, dbusmenu::Layout)> {
	let proxy = dbusmenu_proxy(connection, bus_name, path).await?;
	proxy.about_to_show(item_id).await?;
	proxy
		.event(item_id, "opened", &zbus::zvariant::Value::from(0i32), 0)
		.await?;
	proxy.get_layout(item_id, -1, vec![]).await
}

pub async fn dbusmenu_clicked(
	connection: &Connection, bus_name: OwnedBusName, path: OwnedObjectPath, item_id: i32,
) -> zbus::Result<()> {
	let proxy = dbusmenu_proxy(connection, bus_name, path).await?;
	proxy
		.event(item_id, "clicked", &zbus::zvariant::Value::from(0i32), 0)
		.await
}

async fn dbusmenu_proxy<'a>(
	connection: &'a Connection, bus_name: OwnedBusName, path: OwnedObjectPath,
) -> zbus::Result<DbusMenuProxy<'a>> {
	DbusMenuProxy::builder(connection)
		.destination(bus_name)?
		.path(path)?
		.build()
		.await
}

async fn call_action(
	connection: &Connection, service: &str, method: &str, x: i32, y: i32,
) -> zbus::Result<()> {
	let address = ItemAddress::parse(service)?;
	let proxy = Proxy::new_owned(
		connection.clone(),
		address.bus_name,
		address.object_path,
		ITEM_INTERFACE.to_string(),
	)
	.await?;
	proxy.call_method(method, &(x, y)).await?;
	Ok(())
}

#[derive(Debug, Clone)]
pub struct ItemAddress {
	pub bus_name: OwnedBusName,
	pub object_path: OwnedObjectPath,
}

impl ItemAddress {
	fn parse(service: &str) -> zbus::Result<Self> {
		let (bus_name, object_path) = service.split_once('/').map_or_else(
			|| (service.to_string(), DEFAULT_ITEM_PATH.to_string()),
			|(bus_name, path)| (bus_name.to_string(), format!("/{path}")),
		);
		Ok(Self {
			bus_name: OwnedBusName::try_from(bus_name)?,
			object_path: OwnedObjectPath::try_from(object_path)?,
		})
	}
}

async fn optional_string(proxy: &Proxy<'_>, property: &str) -> Option<String> {
	proxy
		.get_property::<String>(property)
		.await
		.ok()
		.filter(|value| !value.is_empty())
}

async fn read_status(proxy: &Proxy<'_>) -> TrayItemStatus {
	match proxy.get_property::<String>("Status").await.as_deref() {
		Ok("Passive") => TrayItemStatus::Passive,
		Ok("Active") => TrayItemStatus::Active,
		Ok("NeedsAttention") => TrayItemStatus::NeedsAttention,
		_ => TrayItemStatus::Unknown,
	}
}

async fn read_tooltip(proxy: &Proxy<'_>, theme_path: Option<String>) -> Option<TrayItemToolTip> {
	let (name, pixmaps, title, description) = proxy
		.get_property::<(String, Vec<(i32, i32, Vec<u8>)>, String, String)>("ToolTip")
		.await
		.ok()?;
	let icon = TrayIcon::from_parts(name, pixmaps, theme_path);
	Some(TrayItemToolTip {
		icon,
		title,
		description,
	})
}

async fn read_activation(
	proxy: &Proxy<'_>, advertised_menu_only: Option<bool>, has_dbus_menu: bool,
) -> Option<Activation> {
	let xml = proxy.introspect().await.ok()?;
	let node = zbus_xml::Node::try_from(xml.as_str()).ok()?;
	let interface = node
		.interfaces()
		.iter()
		.find(|interface| interface.name().as_str() == ITEM_INTERFACE)?;
	let has_method = |name: &str| {
		interface
			.methods()
			.iter()
			.any(|method| method.name().as_str() == name)
	};
	let has_activate = has_method("Activate");
	let has_context_menu = has_method("ContextMenu");

	Some(if advertised_menu_only == Some(true) {
		if has_dbus_menu {
			Activation::DbusMenu
		} else if has_context_menu {
			Activation::ContextMenu
		} else {
			Activation::Unsupported
		}
	} else if has_activate {
		Activation::Activate
	} else if has_context_menu {
		Activation::ContextMenu
	} else if has_dbus_menu {
		Activation::DbusMenu
	} else {
		Activation::Unsupported
	})
}
