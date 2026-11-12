//! A normal authentication dialog; presentation policy stays out of the backend.

use iced::{
	Element, Font, Length,
	widget::{column, container, image, row, scrollable, space, svg, text},
};
use neo_widgets::{
	icons::{ResolvedIcon, background},
	phosphor_icon,
	style::Colors,
	widgets::{neo_button, neo_card, neo_pick_list, neo_text_input},
};

use crate::model::{Command, Event, Identity, Request, Secret};

pub const INPUT_ID: &str = "authentication-response";

#[derive(Debug, Clone)]
pub enum Message {
	Input(u64, Secret),
	Submit(u64),
	Identity(Identity),
	Retry,
	Cancel,
	ToggleDetails,
}

pub struct Dialog {
	pub request: Request,
	pub commands: async_channel::Sender<Command>,
	selected: Identity,
	response: Secret,
	prompt: Option<(u64, String, bool)>,
	status: String,
	error: bool,
	retry: bool,
	details_open: bool,
	status_loading: bool,
}

impl Dialog {
	pub fn new(request: Request, commands: async_channel::Sender<Command>) -> Option<Self> {
		let selected = request
			.identities
			.iter()
			.find(|identity| identity.uid == nix::unistd::getuid().as_raw())
			.or_else(|| request.identities.first())?
			.clone();

		Some(Self {
			request,
			commands,
			selected,
			response: Secret::new(String::new()),
			prompt: None,
			status: "Starting authentication…".into(),
			error: false,
			retry: false,
			details_open: false,
			status_loading: true,
		})
	}

	pub fn update(&mut self, message: Message) -> Option<Command> {
		match message {
			Message::Input(generation, response) => {
				if self
					.prompt
					.as_ref()
					.is_some_and(|prompt| prompt.0 == generation)
				{
					self.response = response;
				}
				None
			}
			Message::Submit(generation) => {
				if !self
					.prompt
					.as_ref()
					.is_some_and(|prompt| prompt.0 == generation)
				{
					return None;
				}

				self.prompt = None;
				self.status = "Authenticating…".into();
				self.status_loading = true;
				let response = std::mem::replace(&mut self.response, Secret::new(String::new()));

				Some(Command::Respond {
					generation,
					response,
				})
			}
			Message::Identity(identity) => {
				if identity == self.selected || !self.request.identities.contains(&identity) {
					return None;
				}

				self.selected = identity;
				self.clear_conversation();

				Some(Command::SelectIdentity(self.selected.uid))
			}
			Message::Retry if self.retry => {
				self.clear_conversation();
				Some(Command::Retry)
			}
			Message::Cancel => {
				self.clear_conversation();
				Some(Command::Cancel)
			}
			Message::Retry => None,
			Message::ToggleDetails => {
				self.details_open = !self.details_open;
				None
			}
		}
	}

	pub fn apply(&mut self, event: Event) -> bool {
		match event {
			Event::Prompt {
				id,
				generation,
				text,
				secret,
			} if id == self.request.id => {
				self.response = Secret::new(String::new());
				self.prompt = Some((generation, text, secret));
				self.retry = false;
				if self.status_loading {
					self.status.clear();
					self.status_loading = false;
				}
				true
			}
			Event::Info { id, text, error } if id == self.request.id => {
				self.status = text;
				self.error = error;
				self.status_loading = false;
				false
			}
			Event::Retry { id } if id == self.request.id => {
				self.prompt = None;
				self.response = Secret::new(String::new());
				if !self.error {
					self.status = "Authentication failed. You can try again.".into();
				}
				self.error = true;
				self.retry = true;
				self.status_loading = false;
				false
			}
			Event::Reset { id, uid } if id == self.request.id => {
				if let Some(identity) = self
					.request
					.identities
					.iter()
					.find(|identity| identity.uid == uid)
				{
					self.selected = identity.clone();
				}
				self.clear_conversation();
				false
			}
			_ => false,
		}
	}

	pub fn view(&self, colors: Colors) -> Element<'_, Message> {
		let mut content = column![
			self.header(colors),
			self.operation(colors),
			self.identity_view(colors)
		]
		.spacing(8);

		if let Some((generation, prompt, secret)) = &self.prompt {
			let generation = *generation;
			content = content.push(
				neo_text_input(prompt_placeholder(prompt), self.response.as_str())
					.with_colors(colors)
					.id(INPUT_ID)
					.secure(*secret)
					.size(16)
					.on_input(move |value| Message::Input(generation, Secret::new(value)))
					.on_submit(Message::Submit(generation))
					.padding(8),
			);
		}

		if !self.status.is_empty() {
			content = content.push(
				scrollable(text(&self.status).size(13).color(if self.error {
					colors.feedback.danger
				} else {
					colors.text
				}))
				.height(Length::Fit.max(36))
				.width(Length::Fill)
				.spacing(8),
			);
		}

		let mut buttons = row![
			neo_button(text("Cancel").size(15))
				.with_colors(colors)
				.padding([6, 10])
				.background(colors.feedback.danger90)
				.focus_color(colors.feedback.danger90)
				.on_press(Message::Cancel),
			space::horizontal(),
		]
		.spacing(12);
		if let Some((generation, _, _)) = self.prompt {
			buttons = buttons.push(
				neo_button(text("Authenticate").size(15))
					.with_colors(colors)
					.padding([6, 10])
					.background(colors.decorative.green90)
					.focus_color(colors.decorative.green90)
					.on_press(Message::Submit(generation)),
			);
		} else if self.retry {
			buttons = buttons.push(
				neo_button(text("Try again").size(15))
					.with_colors(colors)
					.padding([6, 10])
					.background(colors.decorative.green90)
					.focus_color(colors.decorative.green90)
					.on_press(Message::Retry),
			);
		}

		container(content.push(buttons))
			.padding(14)
			.width(Length::Fill)
			.height(Length::Fill)
			.into()
	}

	fn header(&self, colors: Colors) -> Element<'_, Message> {
		let mut title = column![
			text("Authentication Required")
				.color(colors.text)
				.size(16)
				.weight(iced::font::Weight::Bold)
		]
		.spacing(2)
		.width(Length::Fill);

		if let Some(description) = &self.request.presentation.description
			&& description != self.request.presentation.operation(&self.request.message)
		{
			title = title.push(
				text(description)
					.size(13)
					.width(Length::Fill)
					.wrapping(text::Wrapping::None)
					.ellipsis(text::Ellipsis::End),
			);
		}

		row![self.action_icon(colors), title]
			.spacing(10)
			.align_y(iced::Alignment::Center)
			.into()
	}

	fn operation(&self, colors: Colors) -> Element<'_, Message> {
		let operation = self.request.presentation.operation(&self.request.message);
		let mut content = column![
			neo_card(
				text(operation)
					.font(Font::MONOSPACE)
					.size(16)
					.wrapping(text::Wrapping::WordOrGlyph)
					.width(Length::Fill)
			)
			.with_colors(colors)
			.background(colors.decorative.blue90)
			.padding(8)
			.width(Length::Fill),
			self.context_row(colors),
		]
		.spacing(10)
		.width(Length::Fill);

		if self.details_open {
			content = content
				.push(text("Action ID").size(12).weight(iced::font::Weight::Bold))
				.push(
					text(&self.request.action)
						.font(Font::MONOSPACE)
						.size(12)
						.wrapping(text::Wrapping::WordOrGlyph),
				);
			if let Some(description) = &self.request.presentation.description {
				content = content.push(text(description).size(13));
			}
			if self.request.message != operation {
				content = content.push(text(&self.request.message).size(13));
			}
		}

		scrollable(content)
			.height(Length::Fill)
			.width(Length::Fill)
			.spacing(8)
			.into()
	}

	fn context_row(&self, colors: Colors) -> Element<'_, Message> {
		let target = self
			.request
			.presentation
			.target_user
			.as_ref()
			.map(|user| format!("as {user}"))
			.unwrap_or_default();

		row![
			text(target).size(14),
			space::horizontal(),
			neo_button(
				text(if self.details_open {
					"Hide details"
				} else {
					"Details"
				})
				.size(12)
			)
			.with_colors(colors)
			.padding([3, 6])
			.shadow_width(2.0)
			.background(colors.body)
			.focus_color(colors.decorative.blue90)
			.on_press(Message::ToggleDetails),
		]
		.align_y(iced::Alignment::Center)
		.into()
	}

	fn identity_view(&self, colors: Colors) -> Element<'_, Message> {
		let identity: Element<'_, Message> = if self.has_identity_choice() {
			neo_pick_list(
				Some(&self.selected),
				self.request.identities.as_slice(),
				|identity| identity.username.clone(),
			)
			.with_colors(colors)
			.on_select(Message::Identity)
			.width(Length::Fill)
			.into()
		} else {
			text(&self.selected.username)
				.size(14)
				.weight(iced::font::Weight::Bold)
				.into()
		};

		row![text("Authenticate as").size(13), identity]
			.spacing(10)
			.align_y(iced::Alignment::Center)
			.into()
	}

	fn has_identity_choice(&self) -> bool {
		self.request.identities.len() > 1
	}

	fn action_icon(&self, colors: Colors) -> Element<'_, Message> {
		let name = &self.request.presentation.icon_name;
		let icon = if name.trim().is_empty() {
			None
		} else {
			background::try_resolve_icon_name(name, 28, 1)
		};
		let Some(icon) = icon else {
			return svg(phosphor_icon!("lock"))
				.style(move |_, _| svg::Style {
					color: Some(colors.text),
				})
				.width(28)
				.height(28)
				.into();
		};

		match icon {
			ResolvedIcon::Svg(handle) => svg(handle).width(28).height(28).into(),
			ResolvedIcon::Image(handle) => image(handle).width(28).height(28).into(),
		}
	}

	fn clear_conversation(&mut self) {
		self.response = Secret::new(String::new());
		self.prompt = None;
		self.retry = false;
		self.status = "Starting authentication…".into();
		self.error = false;
		self.status_loading = true;
	}
}

fn prompt_placeholder(prompt: &str) -> String {
	let prompt = prompt.trim().trim_end_matches(':').trim_end();

	if prompt.eq_ignore_ascii_case("password") {
		"Password".into()
	} else if prompt.is_empty() {
		"Response".into()
	} else {
		prompt.to_owned()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn dialog() -> Dialog {
		let (commands, _) = async_channel::unbounded();
		let request = Request {
			id: 1,
			message: "Authenticate".into(),
			action: "test.action".into(),
			identities: vec![Identity {
				uid: 0,
				username: "root".into(),
			}],
			presentation: Default::default(),
		};

		Dialog::new(request, commands).unwrap()
	}

	#[test]
	fn submitting_clears_response_and_disables_duplicate_submission() {
		let mut dialog = dialog();
		dialog.apply(Event::Prompt {
			id: 1,
			generation: 2,
			text: "Password:".into(),
			secret: true,
		});
		dialog.update(Message::Input(2, Secret::new("secret".into())));

		let command = dialog.update(Message::Submit(2));

		assert!(matches!(
			command,
			Some(Command::Respond { generation: 2, .. })
		));
		assert!(dialog.response.as_str().is_empty());
		assert!(dialog.update(Message::Submit(2)).is_none());
	}

	#[test]
	fn stale_requests_and_input_cannot_replace_current_prompt() {
		let mut dialog = dialog();
		dialog.apply(Event::Prompt {
			id: 1,
			generation: 3,
			text: "Password:".into(),
			secret: true,
		});

		assert!(!dialog.apply(Event::Retry { id: 9 }));
		dialog.update(Message::Input(2, Secret::new("old".into())));

		assert!(dialog.response.as_str().is_empty());
		assert!(dialog.update(Message::Submit(2)).is_none());
		assert_eq!(dialog.prompt.as_ref().unwrap().0, 3);
	}

	#[test]
	fn failure_allows_explicit_retry_and_cancellation_clears_input() {
		let mut dialog = dialog();
		dialog.apply(Event::Retry { id: 1 });

		assert!(matches!(
			dialog.update(Message::Retry),
			Some(Command::Retry)
		));
		assert!(!dialog.retry);

		dialog.response = Secret::new("secret".into());
		assert!(matches!(
			dialog.update(Message::Cancel),
			Some(Command::Cancel)
		));
		assert!(dialog.response.as_str().is_empty());
	}

	#[test]
	fn retry_preserves_actionable_helper_errors() {
		let mut dialog = dialog();
		dialog.apply(Event::Info {
			id: 1,
			text: "Enable the system helper socket".into(),
			error: true,
		});

		dialog.apply(Event::Retry { id: 1 });

		assert_eq!(dialog.status, "Enable the system helper socket");
		assert!(dialog.retry);
	}

	#[test]
	fn changing_identity_discards_response_and_waits_for_a_new_prompt() {
		let mut dialog = dialog();
		let other = Identity {
			uid: 1234,
			username: "another-user".into(),
		};
		dialog.request.identities.push(other.clone());
		dialog.apply(Event::Prompt {
			id: 1,
			generation: 1,
			text: "Password:".into(),
			secret: true,
		});
		dialog.update(Message::Input(1, Secret::new("old-response".into())));

		let command = dialog.update(Message::Identity(other));

		assert!(matches!(command, Some(Command::SelectIdentity(1234))));
		assert!(dialog.response.as_str().is_empty());
		assert!(dialog.prompt.is_none());
		assert!(dialog.update(Message::Submit(1)).is_none());
	}

	#[test]
	fn each_new_prompt_resets_input_and_respects_echo_mode() {
		let mut dialog = dialog();
		dialog.apply(Event::Prompt {
			id: 1,
			generation: 1,
			text: "Password:".into(),
			secret: true,
		});
		dialog.update(Message::Input(1, Secret::new("old-response".into())));

		dialog.apply(Event::Prompt {
			id: 1,
			generation: 2,
			text: "Verification code:".into(),
			secret: false,
		});

		assert!(dialog.response.as_str().is_empty());
		assert_eq!(dialog.prompt, Some((2, "Verification code:".into(), false)));
	}

	#[test]
	fn placeholder_normalizes_password_but_preserves_custom_prompt_meaning() {
		assert_eq!(prompt_placeholder("Password: "), "Password");
		assert_eq!(prompt_placeholder("password:"), "Password");
		assert_eq!(
			prompt_placeholder("Verification code: "),
			"Verification code"
		);
		assert_eq!(prompt_placeholder("PIN:"), "PIN");
		assert_eq!(prompt_placeholder("Mot de passe : "), "Mot de passe");
		assert_eq!(
			prompt_placeholder("Touch your security key"),
			"Touch your security key"
		);
		assert_eq!(prompt_placeholder(""), "Response");
	}

	#[test]
	fn single_identity_is_static_and_multiple_identities_offer_choice() {
		let mut dialog = dialog();
		assert!(!dialog.has_identity_choice());

		dialog.request.identities.push(Identity {
			uid: 1234,
			username: "other-user".into(),
		});

		assert!(dialog.has_identity_choice());
	}

	#[test]
	fn details_toggle_is_local_and_never_submits_credentials() {
		let mut dialog = dialog();

		assert!(dialog.update(Message::ToggleDetails).is_none());
		assert!(dialog.details_open);
		assert!(dialog.update(Message::ToggleDetails).is_none());
		assert!(!dialog.details_open);
	}

	#[test]
	fn prompt_clears_loading_text_but_preserves_pam_information() {
		let mut dialog = dialog();
		dialog.apply(Event::Prompt {
			id: 1,
			generation: 1,
			text: "Password:".into(),
			secret: true,
		});
		assert!(dialog.status.is_empty());

		dialog.apply(Event::Info {
			id: 1,
			text: "Use your security key".into(),
			error: false,
		});
		dialog.apply(Event::Prompt {
			id: 1,
			generation: 2,
			text: "PIN:".into(),
			secret: true,
		});

		assert_eq!(dialog.status, "Use your security key");
	}
}
