//! Human-readable display context, never executable input or verified app identity.

use std::{collections::HashMap, fmt, time::Duration};

use zbus::{Connection, Proxy};

/// Literal wire shape from polkit 127's Authority XML.
pub type ActionDescription = (
	String,
	String,
	String,
	String,
	String,
	String,
	u32,
	u32,
	u32,
	HashMap<String, String>,
);

#[derive(Clone, Default)]
pub struct Presentation {
	pub description: Option<String>,
	pub icon_name: String,
	pub target_user: Option<String>,
	command: Option<String>,
}

impl Presentation {
	pub fn from_details(icon_name: String, details: &HashMap<String, String>) -> Self {
		Self {
			icon_name,
			command: nonempty(details.get("command_line"))
				.or_else(|| nonempty(details.get("program"))),
			target_user: nonempty(details.get("user"))
				.or_else(|| nonempty(details.get("user.display"))),
			..Default::default()
		}
	}

	pub fn operation<'a>(&'a self, message: &'a str) -> &'a str {
		// Real authority messages can include the requested program even when
		// BeginAuthentication exposes only PID details. The catalog is generic.
		self.command
			.as_deref()
			.or_else(|| (!message.trim().is_empty()).then_some(message))
			.or(self.description.as_deref())
			.unwrap_or("Authentication required")
	}

	pub async fn enrich(&mut self, connection: &Connection, owner: &str, action: &str) {
		// Optional presentation data must not delay a prompt indefinitely. The
		// caller's cancellation select also covers this bounded lookup.
		let lookup = async {
			let authority = Proxy::new(
				connection,
				owner,
				"/org/freedesktop/PolicyKit1/Authority",
				"org.freedesktop.PolicyKit1.Authority",
			)
			.await?;
			authority
				.call::<_, _, Vec<ActionDescription>>("EnumerateActions", &(locale(),))
				.await
		};

		if let Ok(Ok(actions)) = tokio::time::timeout(Duration::from_millis(500), lookup).await {
			self.apply_catalog(action, actions);
		}
	}

	fn apply_catalog(&mut self, action: &str, actions: Vec<ActionDescription>) {
		if let Some((_, description, _, _, _, icon, _, _, _, _)) =
			actions.into_iter().find(|entry| entry.0 == action)
		{
			self.description = nonempty(Some(&description));
			if self.icon_name.trim().is_empty() {
				self.icon_name = icon;
			}
		}
	}
}

impl fmt::Debug for Presentation {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter
			.debug_struct("Presentation")
			.field("has_description", &self.description.is_some())
			.field("has_command", &self.command.is_some())
			.field("has_target", &self.target_user.is_some())
			.finish_non_exhaustive()
	}
}

pub fn locale() -> String {
	["LC_ALL", "LC_MESSAGES", "LANG"]
		.into_iter()
		.find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
		.unwrap_or_else(|| "C".into())
}

fn nonempty(value: Option<&String>) -> Option<String> {
	value.filter(|value| !value.trim().is_empty()).cloned()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn catalog(description: &str, icon: &str) -> Vec<ActionDescription> {
		vec![(
			"test.action".into(),
			description.into(),
			"Message".into(),
			"Vendor".into(),
			String::new(),
			icon.into(),
			0,
			0,
			0,
			HashMap::new(),
		)]
	}

	#[test]
	fn operation_prefers_full_command_then_program_then_message_then_description() {
		let mut details = HashMap::from([
			("command_line".into(), "/bin/tool --check".into()),
			("program".into(), "/bin/tool".into()),
		]);
		let presentation = Presentation::from_details(String::new(), &details);

		assert_eq!(presentation.operation("Fallback"), "/bin/tool --check");

		details.remove("command_line");
		assert_eq!(
			Presentation::from_details(String::new(), &details).operation("Fallback"),
			"/bin/tool"
		);

		let mut presentation = Presentation::default();
		assert_eq!(presentation.operation("Fallback"), "Fallback");

		presentation.apply_catalog("test.action", catalog("Friendly action", "action-icon"));

		assert_eq!(presentation.operation("Fallback"), "Fallback");
		assert_eq!(presentation.operation(""), "Friendly action");
		assert_eq!(presentation.operation("  "), "Friendly action");
	}

	#[test]
	fn target_is_explicit_including_root_and_is_never_inferred() {
		let details = HashMap::from([
			("user".into(), "root".into()),
			("user.display".into(), "Administrator".into()),
		]);

		assert_eq!(
			Presentation::from_details(String::new(), &details)
				.target_user
				.as_deref(),
			Some("root")
		);
		assert!(Presentation::default().target_user.is_none());
	}

	#[test]
	fn catalog_icon_does_not_replace_icon_from_begin() {
		let mut presentation = Presentation::from_details("provided-icon".into(), &HashMap::new());

		presentation.apply_catalog("test.action", catalog("Friendly action", "catalog-icon"));

		assert_eq!(presentation.icon_name, "provided-icon");
		assert_eq!(presentation.description.as_deref(), Some("Friendly action"));
	}

	#[test]
	fn absent_catalog_entry_and_blank_details_have_safe_fallbacks() {
		let mut presentation = Presentation::from_details(
			String::new(),
			&HashMap::from([("command_line".into(), "  ".into())]),
		);

		presentation.apply_catalog("another.action", catalog("Unrelated", "unrelated-icon"));

		assert_eq!(
			presentation.operation("Authority message"),
			"Authority message"
		);
		assert!(presentation.icon_name.is_empty());
	}

	#[test]
	fn debug_does_not_disclose_command_arguments() {
		let presentation = Presentation::from_details(
			String::new(),
			&HashMap::from([("command_line".into(), "tool --token=private".into())]),
		);

		assert!(!format!("{presentation:?}").contains("private"));
	}

	#[test]
	fn pkexec_pid_only_agent_details_keep_the_request_specific_operation() {
		// Polkit 127 forwards localized_details (PID context), not pkexec's
		// original CheckAuthorization command_line/program/user detail map.
		let details = HashMap::from([
			("polkit.caller-pid".into(), "1234".into()),
			("polkit.subject-pid".into(), "1234".into()),
		]);
		let mut presentation = Presentation::from_details(String::new(), &details);
		presentation.apply_catalog("test.action", catalog("Run a program as another user", ""));
		let message =
			"Authentication is needed to run `/run/current-system/sw/bin/true' as the super user";

		let operation = presentation.operation(message);

		assert_eq!(operation, message);
		assert!(operation.contains("/run/current-system/sw/bin/true"));
	}
}
