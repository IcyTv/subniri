use std::collections::HashMap;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use zbus::{
	Connection, Result as ZbusResult, proxy,
	zvariant::{OwnedObjectPath, OwnedValue, Type, Value},
};

const DEFAULT_COLLECTION_ALIAS: &str = "default";
const PLAIN_SESSION_ALGORITHM: &str = "plain";
const TEXT_CONTENT_TYPE: &str = "text/plain; charset=utf8";

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("D-Bus error: {0}")]
	Dbus(#[from] zbus::Error),

	#[error("invalid D-Bus object path: {0}")]
	InvalidObjectPath(#[from] zbus::zvariant::Error),

	#[error("secret prompt was dismissed")]
	PromptDismissed,

	#[error("default secret collection does not exist")]
	MissingDefaultCollection,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Default)]
pub struct SecretAttributes(HashMap<String, String>);

impl SecretAttributes {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn insert(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
		self.0.insert(key.into(), value.into());
		self
	}

	pub fn as_map(&self) -> &HashMap<String, String> {
		&self.0
	}
}

impl From<HashMap<String, String>> for SecretAttributes {
	fn from(attributes: HashMap<String, String>) -> Self {
		Self(attributes)
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, Type)]
struct Secret {
	session: OwnedObjectPath,
	parameters: Vec<u8>,
	value: Vec<u8>,
	content_type: String,
}

pub struct SecretStore {
	conn: Connection,
	session: OwnedObjectPath,
}

impl SecretStore {
	pub async fn connect() -> Result<Self> {
		let conn = Connection::session().await?;
		let service = SecretServiceProxy::new(&conn).await?;
		let (_, session) = service
			.open_session(PLAIN_SESSION_ALGORITHM, Value::new(""))
			.await?;

		Ok(Self { conn, session })
	}

	pub async fn from_connection(conn: Connection) -> Result<Self> {
		let service = SecretServiceProxy::new(&conn).await?;
		let (_, session) = service
			.open_session(PLAIN_SESSION_ALGORITHM, Value::new(""))
			.await?;
		Ok(Self { conn, session })
	}

	pub async fn get(&self, attributes: &SecretAttributes) -> Result<Option<String>> {
		let service = SecretServiceProxy::new(&self.conn).await?;
		let (mut unlocked, locked) = service.search_items(attributes.as_map()).await?;

		if !locked.is_empty() {
			let (mut newly_unlocked, prompt) = service.unlock(&locked).await?;
			if let Some(result) = self.complete_prompt(prompt).await? {
				newly_unlocked.extend(Vec::<OwnedObjectPath>::try_from(result)?);
			}
			unlocked.extend(newly_unlocked);
		}

		let Some(item) = unlocked.into_iter().next() else {
			return Ok(None);
		};

		let item = SecretItemProxy::builder(&self.conn)
			.path(item)?
			.build()
			.await?;
		let secret = item.get_secret(&self.session).await?;
		let token = String::from_utf8(secret.value).ok();

		Ok(token)
	}

	pub async fn store(
		&self, label: impl Into<String>, attributes: &SecretAttributes, secret: impl AsRef<str>,
	) -> Result<()> {
		let collection = self.default_collection().await?;
		let collection = SecretCollectionProxy::builder(&self.conn)
			.path(collection)?
			.build()
			.await?;

		let mut properties = HashMap::new();
		properties.insert(
			"org.freedesktop.Secret.Item.Label".to_string(),
			Value::new(label.into()),
		);
		properties.insert(
			"org.freedesktop.Secret.Item.Attributes".to_string(),
			Value::new(attributes.as_map().clone()),
		);

		let secret = Secret {
			session: self.session.clone(),
			parameters: Vec::new(),
			value: secret.as_ref().as_bytes().to_vec(),
			content_type: TEXT_CONTENT_TYPE.to_string(),
		};

		let (_, prompt) = collection.create_item(&properties, &secret, true).await?;
		self.complete_prompt(prompt).await.map(|_| ())
	}

	pub async fn delete(&self, attributes: &SecretAttributes) -> Result<()> {
		let service = SecretServiceProxy::new(&self.conn).await?;
		let (unlocked, locked) = service.search_items(attributes.as_map()).await?;

		if !locked.is_empty() {
			let (_, prompt) = service.unlock(&locked).await?;
			self.complete_prompt(prompt).await?;
		}

		for item in unlocked.into_iter().chain(locked.into_iter()) {
			let item = SecretItemProxy::builder(&self.conn)
				.path(item)?
				.build()
				.await?;
			let prompt = item.delete().await?;
			self.complete_prompt(prompt).await?;
		}

		Ok(())
	}

	async fn default_collection(&self) -> Result<OwnedObjectPath> {
		let service = SecretServiceProxy::new(&self.conn).await?;
		let collection = service.read_alias(DEFAULT_COLLECTION_ALIAS).await?;

		if is_empty_path(&collection) {
			Err(Error::MissingDefaultCollection)
		} else {
			Ok(collection)
		}
	}

	async fn complete_prompt(&self, prompt: OwnedObjectPath) -> Result<Option<OwnedValue>> {
		if is_empty_path(&prompt) {
			return Ok(None);
		}

		let prompt = SecretPromptProxy::builder(&self.conn)
			.path(prompt)?
			.build()
			.await?;
		let mut completed = prompt.receive_completed().await?;
		prompt.prompt("").await?;

		let Some(signal) = completed.next().await else {
			return Err(Error::PromptDismissed);
		};

		let args = signal.args()?;
		if *args.dismissed() {
			Err(Error::PromptDismissed)
		} else {
			Ok(Some(args.result().try_clone()?))
		}
	}
}

fn is_empty_path(path: &OwnedObjectPath) -> bool {
	path.to_string() == "/"
}

#[proxy(
	interface = "org.freedesktop.Secret.Service",
	default_service = "org.freedesktop.secrets",
	default_path = "/org/freedesktop/secrets"
)]
trait SecretService {
	#[zbus(name = "OpenSession")]
	fn open_session(
		&self, algorithm: &str, input: Value<'_>,
	) -> ZbusResult<(OwnedValue, OwnedObjectPath)>;

	#[zbus(name = "SearchItems")]
	fn search_items(
		&self, attributes: &HashMap<String, String>,
	) -> ZbusResult<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)>;

	#[zbus(name = "Unlock")]
	fn unlock(
		&self, objects: &[OwnedObjectPath],
	) -> ZbusResult<(Vec<OwnedObjectPath>, OwnedObjectPath)>;

	#[zbus(name = "ReadAlias")]
	fn read_alias(&self, name: &str) -> ZbusResult<OwnedObjectPath>;
}

#[proxy(
	interface = "org.freedesktop.Secret.Collection",
	default_service = "org.freedesktop.secrets"
)]
trait SecretCollection {
	#[zbus(name = "CreateItem")]
	fn create_item(
		&self, properties: &HashMap<String, Value<'_>>, secret: &Secret, replace: bool,
	) -> ZbusResult<(OwnedObjectPath, OwnedObjectPath)>;
}

#[proxy(
	interface = "org.freedesktop.Secret.Item",
	default_service = "org.freedesktop.secrets"
)]
trait SecretItem {
	#[zbus(name = "GetSecret")]
	fn get_secret(&self, session: &OwnedObjectPath) -> ZbusResult<Secret>;

	#[zbus(name = "Delete")]
	fn delete(&self) -> ZbusResult<OwnedObjectPath>;
}

#[proxy(
	interface = "org.freedesktop.Secret.Prompt",
	default_service = "org.freedesktop.secrets"
)]
trait SecretPrompt {
	#[zbus(name = "Prompt")]
	fn prompt(&self, window_id: &str) -> ZbusResult<()>;

	#[zbus(signal, name = "Completed")]
	fn completed(&self, dismissed: bool, result: OwnedValue) -> ZbusResult<()>;
}
