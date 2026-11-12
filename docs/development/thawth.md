# thawth authentication agent

`thawth` is Subniri's unprivileged polkit authentication agent. The Cargo package
is `polkit-agent`; the Nix package and executable are `thawth`.

## Session integration

Home Manager enables the component by default with Subniri. Its optional user
service runs under `subniri.target`. To disable it explicitly:

```nix
services.subniri.thawth.enable = false;
```

The host must already provide polkit, its PAM configuration, and the privileged
helper socket at `/run/polkit/agent-helper.socket`. On NixOS these are supplied by
`security.polkit.enable`. The Home Manager component does not change system-wide
security configuration or disable another agent.

If an agent is already registered for the login session, thawth exits with status
77. The generated service uses `RestartPreventExitStatus=77`: the exit remains a
failure, but does not cause a restart loop. Other failures use the ordinary
restart-on-failure policy. Restart thawth manually to retry after removing a
competing agent; it does not wait in the background to take over.

Session registration normally uses the process's login1 session. For user
services with no direct session, thawth requires exactly one eligible graphical
session matching login1's user Display. It refuses ambiguous fallback selection;
in that case launch the agent directly within the intended graphical session.

## Prototype interaction

The agent remains running without a window between requests. Authentication uses
a fixed-size ordinary Wayland window with Subniri styling. Long content scrolls.
There are no compositor rules, layer-shell overlays, temporary resize tricks, or
claims of a secure desktop. Placement and initial focus remain compositor policy.

The cream window is the outer surface. A blue monospace inset highlights the
command/executable when supplied, otherwise the request-specific authority
message, with the generic localized action description as a last fallback.
Polkit's pkexec challenge usually supplies the program inside that message rather
than forwarding its original command details to the agent. `as <user>` names the
explicit execution target, including root; it is separate from the identity used
to authenticate. The action ID and
full explanatory message are available under Details rather than being presented
as an application name. The supplied action icon is used when available, with a
lock fallback. No requester-identity guesses are made from executable names.

One authentication identity is displayed as a static row. Multiple identities use
the shared neo selector, with an opaque hard shadow and button-like press motion.
The shared neo input preserves native Iced editing and focus; its placeholder
shows `Password` or the meaningful custom authentication prompt. Cancel is red
and left-aligned; Authenticate/retry is green and right-aligned.

Cancel, Escape, closing the prompt, and cancellation by polkit stop the active
conversation. Focus loss does not. Multiple offered user identities appear in a
selector; changing identity discards the previous response and helper connection.
Authentication failures permit an explicit retry using a fresh helper connection.
Concurrent authorization requests are rejected while a prompt is active.

## Implementation boundaries

- `agent.rs`: system-bus endpoint, sender validation, registration and lifecycle.
- `session.rs`: login1 session selection.
- `conversation.rs`: request, identity, prompt, retry and cancellation orchestration.
- `helper.rs`: bounded, asynchronous polkit 127 socket protocol and text decoding.
- `model.rs`: shared events, commands and redacted, zeroizing secret ownership.
- `metadata.rs`: optional localized action lookup and display-context fallbacks.
- `app.rs`: persistent Iced daemon and ordinary-window lifecycle.
- `dialog.rs`: struct-based authentication widget and its local interaction state.

Action descriptions come from the current authority's `EnumerateActions` API,
with a bounded best-effort lookup that can be cancelled with the request. Missing
metadata does not prevent authentication. Command text is display context only;
it is never interpreted, executed, or logged by thawth.

thawth uses zbus/Tokio rather than GLib bindings. Password verification and PAM
policy belong to the existing privileged system helper, not this application.
That system helper and polkitd may themselves use GLib. Secrets are excluded from
application debug formatting and logs; Iced/OS internals are not claimed to offer
complete credential-memory zeroization.

## Development checks

Run inside the project's flake environment:

```sh
cargo check -p polkit-agent --all-targets
cargo test -p polkit-agent
cargo fmt --all -- --check
```

A synthetic UI preview (no polkit registration, no credential verification):

```sh
cargo run -p polkit-agent --example prompt-preview
cargo run -p polkit-agent --example prompt-preview -- --multiple-identities
cargo run -p polkit-agent --example prompt-preview -- --long-operation
cargo run -p polkit-agent --example prompt-preview -- --structured-details
```

Use dummy responses only in the preview; it cannot authorize anything.
The default preview models polkit's PID-only agent details. `--structured-details`
exercises the alternate presentation when explicit command and target fields are
provided. Neither localized messages nor process arguments are parsed to guess an
execution target.

Tests use mock helper sockets and a private D-Bus broker, never the host polkit
authority. End-to-end PAM and compositor tests require a separately isolated
graphical/login session. A nested compositor alone does not isolate system polkit
registration; do not run an experimental agent against the user's live session.

Future configuration seams are documented by TODOs for ordinary-window versus
UAC-style presentation and reject/queue/concurrent request behavior. Neither is a
configuration option in this prototype.
