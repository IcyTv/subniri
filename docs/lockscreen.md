# Manual session locking

`frostguard` is a standalone Wayland session-lock client for a compositor
supporting `ext-session-lock-v1`, such as niri. Run it manually from the desktop
session as that session's ordinary user:

```sh
frostguard
```

It is not a daemon and has no arguments or configuration file. Installing it does
not add a keybinding, idle action, suspend hook, or systemd service. The existing
logout menu and CLI are unchanged.

## System login defaults and optional PAM override

Both the Cargo development binary and the Nix package use the system's `login`
PAM service by default, without installing or activating a dedicated policy.
The host must already provide a working `login` authentication and account stack
usable by the desktop user. This is an operational default, not a guarantee that
every host's login policy supports a graphical password conversation.

Before authentication, Frostguard selects `frostguard` instead if a dedicated
policy exists. Selection happens once at startup, never after a rejection. The
account comes from the process's real UID, not `$USER`, arguments, or the
environment. Root and mismatched real/effective UIDs are rejected. Never install
the GUI setuid or run it through sudo.

The package ships this default policy in `share/pam.d/frostguard`:

```pam
auth include login
account include login
```

Both authentication and account rules include the system's `login` policy. This
asset is an optional administrator override template; `share/pam.d` is not
PAM-discovered. Shipping the file does not activate a dedicated system service.
The default invokes `login` directly: `PAM_SERVICE` is `login`, unlike an include
from `frostguard`. Modules with service-conditional behavior can distinguish them.

On NixOS, importing Subniri's NixOS module declares the native PAM service,
independently of package installation and `services.subniri.resumeRestart`:

```nix
{ inputs, pkgs, ... }: {
  imports = [ inputs.subniri.nixosModules.subniri ];
  environment.systemPackages = [ inputs.subniri.packages.${pkgs.stdenv.hostPlatform.system}.frostguard ];
}
```

The module defaults `security.pam.services.frostguard.enable` to `true`. NixOS
then generates its standard service rules, as for a native empty service declaration;
it does not force the portable `include login` template into `.text`. Normal native
service options and administrator `.text` overrides remain available. Importing the
module alone does not install the binary. Install it separately through
`environment.systemPackages` or Home Manager.

Set `security.pam.services.frostguard.enable = false;` to opt out of the dedicated
NixOS policy. This does **not** disable the locker: when no dedicated policy exists
in any checked directory, the runtime selects `login` instead.

Home Manager remains install-only:

```nix
services.subniri = {
  enable = true;
  frostguard.enable = true;
};
```

Home Manager cannot provision `/etc/pam.d`, but its installation works with the
same system `login` default. For different rules, have the administrator install
or adapt the dedicated policy through the system configuration.

Override detection checks `/etc/pam.d/frostguard`, then
`/usr/lib/pam.d/frostguard`, then the compile-time vendor directory if supplied.
The Nix package supplies `${pkgs.pam}/etc/pam.d`, matching the pinned Linux-PAM
1.7.2 derivation's vendor directory. Only the lockscreen derivation receives this
compile-time setting; it is not a runtime service selector. Plain Cargo builds
check the two standard directories unless compiled with
`FROSTGUARD_PAM_VENDOR_DIR` set to the linked PAM library's vendor directory.

Detection uses symlink metadata to distinguish absence from errors. Existing
entries must resolve to readable regular files. Broken symlinks, unreadable files,
invalid entries, and permission/I/O errors stop startup rather than downgrade to
`login`. A selected policy's later PAM failure never causes a different-service
retry. PAM still owns policy parsing and module resolution; concurrent system
policy changes remain administrator-controlled.

Linux-PAM exposes no public service-discovery API. Detection does not parse legacy
`/etc/pam.conf` service entries or discover arbitrary distribution-specific vendor
directories. Such layouts need a dedicated `/etc/pam.d/frostguard` policy or the
correct compile-time vendor directory. The NixOS directory layout is authoritative
for the packaged build.

The GUI remains unprivileged. Use a system-administrator-reviewed PAM stack if
local account policy requires different rules. Authentication and account checks
must both succeed. Expired passwords, password-change requests, missing modules,
unsupported conversations, and worker failures do not authorize unlock.

### Current authentication-method limits

Frostguard currently supports only the current account name and **one hidden
password prompt**. It rejects additional hidden prompts, including multi-prompt
OTP/password-change flows, and successful stacks that never request the password.
Unsupported prompts and passwordless success emit fixed, explicit diagnostics
without logging raw prompts or credential material.

The NixOS module leaves all inherited authentication policy untouched. It does
not inspect providers, disable methods, or emit evaluation-time capability warnings.
Non-password and multi-step conversations are not implemented; policies requiring
them may fail to unlock. Custom PAM stacks and the fallback `login` stack cannot
be reliably classified from their configuration, so a successful configuration
evaluation is not proof of client compatibility. Runtime unsupported-flow diagnostics
describe the client's conversation limit rather than identifying a provider.

Fingerprint, YubiKey/security-key/smart-card, and Howdy support are future work,
not implemented features or promised compatibility. Security-key and smart-card
flows vary: some require touch, PIN, OTP, or additional prompts. These conversations
remain unvalidated rather than inherently impossible in every mode. Review the
actual host policy and arrange recovery before runtime testing.

## Interaction and lifecycle

Every output shows an opaque palette canvas, accent clock, and date. Only the
known session-lock surface with actual keyboard focus shows the colored neobrutal
password/status/Unlock panel. Without keyboard focus, no output shows that panel;
all lock surfaces remain opaque and open. Focus transfers preserve the shared
password, insertion position, and authentication state. Pointer position and
output creation order do not select an input monitor.

Type at the blinking masked caret. The single-line mask viewport follows the
measured caret position, including for long passwords; masks never wrap.
Left/Right move by one Unicode scalar value;
Home/End move to the beginning/end. Backspace deletes before the caret; Delete
deletes after it. Enter or **Unlock** submits. Escape clears the field; it does not
exit. The caret hides while authentication blocks editing. Clipboard paste, text
selection, and word navigation are not supported. Input is limited to 511 UTF-8
bytes, excluding control characters, to fit the Linux-PAM response limit. Overlong
input clears the field.

PAM runs on a blocking worker, not the GUI thread. Only one authentication attempt
can run at a time. A failed attempt clears the field and imposes a two-second retry
delay, in addition to any PAM-enforced delay. The only application-requested normal
backend exit follows a successful password and account check for the current
attempt. Input before compositor lock confirmation cannot authenticate.

Startup logs the selected fixed PAM service name to stderr. Failures log only
fixed stage names and numeric PAM status codes (or policy-detection errno), never
passwords, raw prompts, or secret-bearing error objects. Credential rejection,
account-policy failure, infrastructure failure, and worker failure have distinct
status messages. A rejection does not prove that the supplied password was wrong.
During the retry delay the field remains editable and Unlock remains disabled.

The window-aware backend daemon starts in background mode without an initial
layer surface. Its boot task sends one private global Lock action; background
settings alone do not acquire a lock. No other application message converts to a
shell action, and none can request unlock or surface removal. Per-window views
use standard keyboard focus events; the default redraw policy updates all outputs.
`ShellType::SessionLock` identifies registered lock surfaces, not startup settings.

The existing backend creates lock surfaces for connected and newly added outputs.
Removing one surface preserves input while another surface remains. Closing the
final surface clears input and invalidates any running authentication attempt
without starting a second worker. A confirmed lock stays alive without outputs so
a newly connected monitor can receive a lock surface. The startup watchdog fails
after ten seconds only if the compositor has not confirmed the lock. Fatal startup
or lifecycle paths disconnect instead of using normal backend exit, because normal
backend exit releases the lock.

A lock request can be refused, for example when another locker already owns the
session. A refusal is not a successful lock. A compositor may also end the lock
through its own secure policy; the pinned backend acknowledges that event before
notifying this application. Compositor policy remains authoritative.

## Security limits and validation

The application keeps password buffers in fixed-size, zeroizing allocations, clears
submitted and rejected input, and redacts password-bearing message debug output.
The field renders only mask characters; no plaintext is passed to text-editor
caches, undo history, or the clipboard. PAM owns response allocations after a
successful conversation callback and is responsible for their disposal. The input
backend and PAM modules can retain their own copies; this is not a guarantee
against process-memory inspection, core dumps, swap, or a compromised desktop.

Session-lock safety depends on compositor enforcement and the existing backend,
not merely on compilation. If the locker dies after acquisition, the protocol
requires the compositor to keep the session locked; recovery is compositor policy
and may require ending the session from another login. Arrange recovery before
testing. Do not test failures on a desktop containing unsaved work.

Validate in a private headless Weston host with nested niri, not the user's running
desktop. Cover correct and incorrect passwords, PAM errors, startup refusal, two
monitors, output removal/re-addition, no outputs, and locker crashes. A build or
unit-test pass alone is not a security assessment.
