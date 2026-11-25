mod auth;
mod daemonize;
mod masked_field;
mod secret;
mod state;

use std::{
	collections::HashSet,
	fmt,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	time::{Duration, Instant},
};

use clap::Parser;
use futures::stream::{self, StreamExt};
use iced::{
	Alignment, Element, Length, Subscription, Task, keyboard,
	widget::{column, container, text},
	window,
};
use iced_exwlshell::{
	actions::{ExwlShellCustomAction, ExwlShellCustomActionWithId},
	daemon,
	settings::{LayerShellSettings, Settings, StartMode},
};
use iced_wayland_subscriber::shell::{self, ShellEvent, ShellReceiver, ShellType};

use neo_widgets::{
	appearance::{Appearance, Event as AppearanceEvent},
	widgets::{neo_button, neo_card},
};

use auth::{Account, AuthResult};
use secret::Secret;
use state::{LockState, Origin};

// How often a background fingerprint/Howdy probe retries while the screen
// sits locked and idle. Cheap even when nobody's enrolled: the PAM-side gate
// (icy.internal.pamSecurity.lockscreenGraceMinutes in pam-security.nix,
// defaulting to an hour) rejects a stale probe in a single fast pam_exec
// check, well before it would ever reach the fingerprint reader or camera.
const PROBE_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Parser)]
struct Args {
	/// Detach from the controlling terminal once the screen is locked.
	///
	/// For swayidle's before-sleep/lock hooks, which must return promptly:
	/// mirrors `swaylock --daemonize`.
	#[clap(long)]
	daemonize: bool,
}

fn main() {
	let args = Args::parse();

	// Must happen before anything else: forking after Wayland, iced, or
	// tokio have set up connections or threads would hand the child a
	// half-initialized copy of them.
	if args.daemonize {
		daemonize::daemonize();
	}

	let account = match Account::current() {
		Ok(account) => Arc::new(account),
		Err(message) => fail_closed(message),
	};
	let (shell_broadcast, shell_events) = shell::channel();
	let authorized = Arc::new(AtomicBool::new(false));
	let exit_authorized = authorized.clone();

	let result = daemon(
		move || {
			(
				Lockscreen::new(account.clone(), shell_events.clone(), authorized.clone()),
				Task::done(Message::RequestLock),
			)
		},
		"frostguard",
		Lockscreen::update,
		Lockscreen::view,
	)
	.settings(Settings {
		layer_settings: LayerShellSettings {
			start_mode: StartMode::Background,
			..LayerShellSettings::default()
		},
		shell_broadcast,
		..Settings::default()
	})
	.theme(|app: &Lockscreen, _window| app.appearance.colors().iced_theme())
	.subscription(Lockscreen::subscription)
	.run();

	if result.is_err() || !exit_authorized.load(Ordering::Acquire) {
		fail_closed("Lockscreen backend stopped unexpectedly; the session may remain locked.");
	}
}

fn fail_closed(message: &'static str) -> ! {
	eprintln!("frostguard: {message}");
	daemonize::signal_ready(false);
	// Normal backend teardown releases the session lock. A fatal path must
	// disconnect instead, leaving an acquired lock under compositor control.
	std::process::exit(1)
}

struct Lockscreen {
	appearance: Appearance,
	account: Arc<Account>,
	shell_events: ShellReceiver,
	authorized: Arc<AtomicBool>,
	state: LockState,
	password: Secret,
	focus: Focus,
	caret_visible: bool,
	caret_reset: Instant,
	surfaces: HashSet<iced::window::Id>,
	status: &'static str,
	// Live PAM conversation text (e.g. "Place your finger on the reader")
	// for whichever attempt, probe or submitted, is currently in flight.
	prompt: Option<String>,
	next_probe: Instant,
	clock: String,
	date: String,
}

#[derive(Clone)]
enum Message {
	RequestLock,
	Input(window::Id, Arc<Secret>),
	Edit(window::Id, Edit),
	Submit(window::Id),
	Focus(window::Id, bool),
	Prompt(u64, String),
	Authenticated {
		attempt: u64,
		origin: Origin,
		result: AuthResult,
	},
	Appearance(AppearanceEvent),
	Shell(ShellEvent),
	Tick,
	Noop,
}

impl fmt::Debug for Message {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		// Iced may format messages. Never include key text, cursor, or password bytes.
		formatter.write_str(match self {
			Self::RequestLock => "RequestLock",
			Self::Input(..) => "Input(<redacted>)",
			Self::Edit(..) => "Edit(<redacted>)",
			Self::Submit(_) => "Submit",
			Self::Focus(..) => "Focus",
			Self::Prompt(..) => "Prompt",
			Self::Authenticated { .. } => "Authenticated",
			Self::Appearance(_) => "Appearance",
			Self::Shell(_) => "Shell",
			Self::Tick => "Tick",
			Self::Noop => "Noop",
		})
	}
}

// Only boot may request acquisition. No message can request unlock or surface removal.
impl TryInto<ExwlShellCustomActionWithId> for Message {
	type Error = Self;

	fn try_into(self) -> Result<ExwlShellCustomActionWithId, Self> {
		match self {
			Self::RequestLock => Ok(ExwlShellCustomActionWithId::new(
				None,
				ExwlShellCustomAction::Lock,
			)),
			other => Err(other),
		}
	}
}

#[derive(Clone, Copy)]
enum Edit {
	Backspace,
	Delete,
	Left,
	Right,
	Home,
	End,
	Clear,
}

#[derive(Default)]
struct Focus(Vec<window::Id>);

impl Focus {
	fn update(&mut self, window: window::Id, focused: bool) {
		// Backend delivery groups events by window, preserving only per-window order.
		// Removing one window's indication must not erase another's surviving focus.
		self.0.retain(|candidate| *candidate != window);
		if focused {
			self.0.push(window);
		}
	}

	fn panel(&self, window: window::Id, surfaces: &HashSet<window::Id>) -> bool {
		self.0.last() == Some(&window) && surfaces.contains(&window)
	}
}

impl Lockscreen {
	fn new(
		account: Arc<Account>, shell_events: ShellReceiver, authorized: Arc<AtomicBool>,
	) -> Self {
		let now = Instant::now();
		let mut app = Self {
			appearance: Appearance::default(),
			account,
			shell_events,
			authorized,
			state: LockState::new(now),
			password: Secret::default(),
			focus: Focus::default(),
			caret_visible: true,
			caret_reset: now,
			surfaces: HashSet::new(),
			status: "Locking…",
			prompt: None,
			// Fires on the very first Tick once the lock is confirmed: can_edit()
			// stays false until then, so nothing probes before a surface exists.
			next_probe: now,
			clock: String::new(),
			date: String::new(),
		};
		app.update_clock();
		app
	}

	fn subscription(&self) -> Subscription<Message> {
		Subscription::batch([
			self.appearance.subscription().map(Message::Appearance),
			self.shell_events.listen().map(Message::Shell),
			iced::event::listen_with(|event, _status, window| match event {
				iced::Event::Keyboard(event) => Some(key_message(event, window)),
				iced::Event::Window(window::Event::Focused) => Some(Message::Focus(window, true)),
				iced::Event::Window(window::Event::Unfocused | window::Event::Closed) => {
					Some(Message::Focus(window, false))
				}
				_ => None,
			}),
			iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick),
		])
	}

	fn update(&mut self, message: Message) -> Task<Message> {
		let now = Instant::now();
		self.state.refresh(now);

		match message {
			Message::Appearance(event) => self.appearance.update(event),
			Message::Focus(window, focused) => {
				self.focus.update(window, focused);
				if focused {
					self.caret_visible = true;
					self.caret_reset = now;
				}
			}
			Message::Input(window, input)
				if self.focus.panel(window, &self.surfaces) && self.can_edit() =>
			{
				self.caret_visible = true;
				self.caret_reset = now;
				if let Ok(value) = std::str::from_utf8(input.as_bytes()) {
					if self.password.append(value) {
						self.status = "Enter your password to unlock";
					} else {
						self.password.clear();
						self.status = "Password input is too long or unsupported";
					}
				}
			}
			Message::Edit(window, edit)
				if self.focus.panel(window, &self.surfaces) && self.can_edit() =>
			{
				match edit {
					Edit::Backspace => self.password.backspace(),
					Edit::Delete => self.password.delete(),
					Edit::Left => self.password.left(),
					Edit::Right => self.password.right(),
					Edit::Home => self.password.home(),
					Edit::End => self.password.end(),
					Edit::Clear => self.password.clear(),
				}
				self.caret_visible = true;
				self.caret_reset = now;
			}
			Message::Submit(window)
				if self.focus.panel(window, &self.surfaces) && self.can_submit() =>
			{
				if self.password.as_bytes().is_empty() {
					return Task::none();
				}
				let Some(attempt) = self.state.begin(now) else {
					return Task::none();
				};

				let password = std::mem::take(&mut self.password);
				self.prompt = None;
				self.status = "Authenticating…";

				return run_attempt(self.account.clone(), Some(password), attempt, Origin::Submit);
			}
			Message::Prompt(attempt, text) => {
				if self.state.current_attempt() == Some(attempt) {
					self.prompt = Some(text);
				}
			}
			Message::Authenticated {
				attempt,
				origin,
				result,
			} => {
				self.prompt = None;
				// A probe never touched the typed password; clearing it here
				// wiped out whatever the user was still mid-typing every time
				// a background probe completed (every few seconds).
				if origin == Origin::Submit {
					self.password.clear();
				}
				if self
					.state
					.complete(attempt, result.accepted() && !self.surfaces.is_empty(), now)
				{
					self.authorized.store(true, Ordering::Release);
					self.account.refresh_unlock_gate();
					// This is the only normal-exit path, hence the only application
					// request that can release an acquired session lock.
					return iced::exit();
				}
				// A failed probe found no finger/face, which is routine, not an
				// error -- only a submitted attempt's rejection is worth saying.
				if origin == Origin::Submit {
					self.status = result.status();
				}
			}
			Message::Shell(ShellEvent::Locked) => {
				self.state.locked();
				self.status = "Enter your password to unlock";
				daemonize::signal_ready(true);
			}
			Message::Shell(ShellEvent::NewShell(info)) if info.shell == ShellType::SessionLock => {
				if self.surfaces.is_empty() && self.state.can_edit() {
					self.status = "Enter your password to unlock";
				}
				self.surfaces.insert(info.window);
			}
			Message::Shell(ShellEvent::Closed(window)) => {
				self.focus.update(window, false);
				if remove_surface(&mut self.surfaces, window, &mut self.password) {
					self.state.cancel_attempt();
					self.status = "Waiting for an output…";
				}
			}
			Message::Shell(ShellEvent::LockDenied | ShellEvent::LockedFinished) => {
				self.state.fail();
				self.password.clear();
				fail_closed("Compositor refused or ended the lock; no unlock was authorized.");
			}
			Message::Tick => {
				if now.duration_since(self.caret_reset) >= Duration::from_millis(500) {
					self.caret_visible = !self.caret_visible;
					self.caret_reset = now;
				}
				self.update_clock();
				if self.state.startup_timed_out(now) {
					self.state.fail();
					self.password.clear();
					fail_closed("Lock startup did not complete; the session may remain locked.");
				}
				if self.can_edit() && now >= self.next_probe {
					self.next_probe = now + PROBE_INTERVAL;
					if let Some(attempt) = self.state.begin_probe(now) {
						self.prompt = None;
						return run_attempt(self.account.clone(), None, attempt, Origin::Probe);
					}
				}
			}
			_ => {}
		}

		Task::none()
	}

	fn can_edit(&self) -> bool {
		!self.surfaces.is_empty() && self.state.can_edit()
	}

	fn can_submit(&self) -> bool {
		!self.surfaces.is_empty() && self.state.can_submit()
	}

	fn update_clock(&mut self) {
		let now = jiff::Zoned::now();
		self.clock = now.strftime("%H:%M").to_string();
		self.date = now.strftime("%A, %B %d").to_string();
	}

	fn view(&self, window: window::Id) -> Element<'_, Message> {
		let colors = self.appearance.colors();
		let accent = colors.decorative.blue;
		let mut content = column![
			text(&self.clock).size(64).color(accent),
			text(&self.date).size(22).color(colors.secondary),
		]
		.spacing(18)
		.align_x(Alignment::Center);

		if self.focus.panel(window, &self.surfaces) {
			let (prefix, suffix) = self.password.mask();
			let field_content = masked_field::MaskedField::new(
				prefix.chars().count(),
				suffix.chars().count(),
				colors,
				self.can_edit() && self.caret_visible,
			);
			let field = neo_card(field_content)
				.with_colors(neo_widgets::style::Colors {
					border: accent,
					..colors
				})
				.padding(14)
				.width(Length::Fill);

			let enabled = self.can_submit();
			let button_text = if enabled {
				colors.text_on(accent)
			} else {
				colors.secondary
			};
			let mut unlock = neo_button(text("Unlock").color(button_text))
				.with_colors(colors)
				.background(accent)
				.enabled(enabled)
				.padding([10, 24]);
			if enabled {
				unlock = unlock.on_press(Message::Submit(window));
			}

			let status = if self.state.authenticating() {
				self.prompt.as_deref().unwrap_or("Authenticating…")
			} else {
				self.status
			};
			let header_color = colors.decorative.purple;
			let header = neo_card(
				text("Unlock session")
					.size(22)
					.color(colors.text_on(header_color)),
			)
			.with_colors(colors)
			.background(header_color)
			.padding([10, 14])
			.width(Length::Fill);
			let panel = column![
				header,
				field,
				text(status).size(16).color(if self.state.cooling_down() {
					colors.feedback.danger
				} else {
					colors.secondary
				}),
				unlock,
			]
			.spacing(18)
			.align_x(Alignment::Center);

			content = content.push(neo_card(panel).with_colors(colors).padding(24).width(408));
		}
		let canvas = iced::Color {
			a: 1.0,
			..colors.canvas
		};

		container(content)
			.padding(24)
			.center_x(Length::Fill)
			.center_y(Length::Fill)
			.style(move |_| container::Style {
				background: Some(canvas.into()),
				text_color: Some(colors.text),
				..container::Style::default()
			})
			.into()
	}
}

enum Progress {
	Prompt(String),
	Done(AuthResult),
}

/// Runs one PAM attempt (a submitted password, or a passwordless probe) on
/// the blocking worker pool, streaming any live conversation text back as
/// `Message::Prompt` before the final `Message::Authenticated`.
fn run_attempt(
	account: Arc<Account>, password: Option<Secret>, attempt: u64, origin: Origin,
) -> Task<Message> {
	let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
	let handle = tokio::task::spawn_blocking(move || account.authenticate(password, sender));

	let prompts = stream::poll_fn(move |cx| receiver.poll_recv(cx)).map(Progress::Prompt);
	let done = stream::once(async move {
		Progress::Done(handle.await.unwrap_or_else(|_| {
			eprintln!("frostguard: authentication worker failed");
			AuthResult::WorkerFailure
		}))
	});

	Task::run(prompts.chain(done), move |progress| match progress {
		Progress::Prompt(text) => Message::Prompt(attempt, text),
		Progress::Done(result) => Message::Authenticated {
			attempt,
			origin,
			result,
		},
	})
}

// Return whether the last lock surface was removed.
fn remove_surface(
	surfaces: &mut HashSet<iced::window::Id>, window: iced::window::Id, password: &mut Secret,
) -> bool {
	if surfaces.remove(&window) && surfaces.is_empty() {
		password.clear();
		true
	} else {
		false
	}
}

fn key_message(event: keyboard::Event, window: window::Id) -> Message {
	let keyboard::Event::KeyPressed {
		key,
		modifiers,
		text,
		..
	} = event
	else {
		return Message::Noop;
	};

	// No clipboard, selection, or word-navigation commands.
	if modifiers.control() || modifiers.alt() || modifiers.logo() {
		return Message::Noop;
	}
	let edit = match key {
		keyboard::Key::Named(keyboard::key::Named::Enter) => return Message::Submit(window),
		keyboard::Key::Named(keyboard::key::Named::Escape) => Some(Edit::Clear),
		keyboard::Key::Named(keyboard::key::Named::Backspace) => Some(Edit::Backspace),
		keyboard::Key::Named(keyboard::key::Named::Delete) => Some(Edit::Delete),
		keyboard::Key::Named(keyboard::key::Named::ArrowLeft) => Some(Edit::Left),
		keyboard::Key::Named(keyboard::key::Named::ArrowRight) => Some(Edit::Right),
		keyboard::Key::Named(keyboard::key::Named::Home) => Some(Edit::Home),
		keyboard::Key::Named(keyboard::key::Named::End) => Some(Edit::End),
		_ => None,
	};
	if let Some(edit) = edit {
		return Message::Edit(window, edit);
	}

	let mut input = Secret::default();
	if let Some(text) = text {
		if input.append(&text) {
			return Message::Input(window, Arc::new(input));
		}
	}
	Message::Noop
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn removing_one_surface_preserves_shared_password() {
		let first = iced::window::Id::unique();
		let second = iced::window::Id::unique();
		let mut surfaces = HashSet::from([first, second]);
		let mut password = Secret::default();
		assert!(password.append("shared input"));

		assert!(!remove_surface(&mut surfaces, first, &mut password));

		assert_eq!(surfaces, HashSet::from([second]));
		assert_eq!(password.as_bytes(), b"shared input");
	}

	#[test]
	fn removing_last_surface_clears_shared_password() {
		let window = iced::window::Id::unique();
		let mut surfaces = HashSet::from([window]);
		let mut password = Secret::default();
		assert!(password.append("shared input"));

		assert!(remove_surface(&mut surfaces, window, &mut password));

		assert!(surfaces.is_empty());
		assert!(password.as_bytes().is_empty());
	}

	#[test]
	fn keyboard_focus_alone_selects_one_known_panel() {
		let first = window::Id::unique();
		let second = window::Id::unique();
		let mut surfaces = HashSet::from([first, second]);
		let mut focus = Focus::default();
		assert!(!focus.panel(first, &surfaces));
		assert!(!focus.panel(second, &surfaces));

		focus.update(first, true);
		assert!(focus.panel(first, &surfaces));
		focus.update(second, true);
		focus.update(first, false);
		assert!(!focus.panel(first, &surfaces));
		assert!(focus.panel(second, &surfaces));
		assert_eq!(surfaces.len(), 2);

		// An unfocused closure must not clear newer focus.
		focus.update(first, false);
		surfaces.remove(&first);
		assert!(focus.panel(second, &surfaces));
		focus.update(second, false);
		assert!(!focus.panel(second, &surfaces));
		assert_eq!(surfaces, HashSet::from([second]));
	}

	#[test]
	fn grouped_round_trip_focus_keeps_final_window_in_either_group_order() {
		let a = window::Id::unique();
		let b = window::Id::unique();
		let surfaces = HashSet::from([a, b]);
		let a_group = [(a, false), (a, true)];
		let b_group = [(b, true), (b, false)];

		for groups in [[a_group, b_group], [b_group, a_group]] {
			let mut focus = Focus::default();
			focus.update(a, true);
			for (window, focused) in groups.into_iter().flatten() {
				focus.update(window, focused);
				assert!(
					surfaces
						.iter()
						.filter(|id| focus.panel(**id, &surfaces))
						.count() <= 1
				);
			}

			assert!(focus.panel(a, &surfaces));
			assert!(!focus.panel(b, &surfaces));
			assert_eq!(focus.0, vec![a]);
			assert_eq!(surfaces.len(), 2);
		}
	}

	#[test]
	fn reordered_simple_transfer_and_selected_closure_remove_only_their_window() {
		let a = window::Id::unique();
		let b = window::Id::unique();
		let surfaces = HashSet::from([a, b]);

		for events in [[(a, false), (b, true)], [(b, true), (a, false)]] {
			let mut focus = Focus::default();
			focus.update(a, true);
			for (window, focused) in events {
				focus.update(window, focused);
			}
			assert!(focus.panel(b, &surfaces));
			focus.update(b, true);
			assert_eq!(focus.0, vec![b]);

			focus.update(b, false);
			assert!(focus.0.is_empty());
			assert!(!focus.panel(a, &surfaces));
			assert!(!focus.panel(b, &surfaces));
		}
	}

	#[test]
	fn focus_before_registration_waits_for_known_lock_surface() {
		let window = window::Id::unique();
		let mut focus = Focus::default();
		let mut surfaces = HashSet::new();

		focus.update(window, true);
		assert!(!focus.panel(window, &surfaces));
		surfaces.insert(window);
		assert!(focus.panel(window, &surfaces));
		focus.update(window, false);
		assert!(!focus.panel(window, &surfaces));
	}

	#[test]
	fn only_boot_message_converts_to_a_lock_action() {
		let action: ExwlShellCustomActionWithId = Message::RequestLock.try_into().unwrap();
		assert!(action.0.is_none());
		assert!(matches!(action.1, ExwlShellCustomAction::Lock));

		let window = window::Id::unique();
		let messages = [
			Message::Noop,
			Message::Tick,
			Message::Submit(window),
			Message::Edit(window, Edit::Delete),
			Message::Input(window, Arc::new(Secret::default())),
			Message::Focus(window, true),
			Message::Authenticated {
				attempt: 1,
				origin: Origin::Submit,
				result: AuthResult::WorkerFailure,
			},
			Message::Prompt(1, "Place your finger on the reader".to_string()),
			Message::Shell(ShellEvent::Locked),
			Message::Shell(ShellEvent::LockDenied),
		];
		for message in messages {
			let action: Result<ExwlShellCustomActionWithId, Message> = message.try_into();
			assert!(action.is_err());
		}
	}

	#[test]
	fn message_debug_never_contains_password_text() {
		let mut password = Secret::default();
		assert!(password.append("do not log this"));

		assert_eq!(
			format!(
				"{:?}",
				Message::Input(window::Id::unique(), Arc::new(password))
			),
			"Input(<redacted>)"
		);
	}
}
