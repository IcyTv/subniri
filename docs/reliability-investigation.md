# Reliability investigation

Initial snapshot: `ed53c89`, investigated 2026-11-02. The findings below describe
that baseline; implementation progress is recorded separately here.

## Implementation progress: first batch

Approved implementation work has started. Completed changes:

- Bar Network/Bluetooth startup now has a ten-second application deadline,
  explicit unavailable state, and retry delays capped at thirty seconds. Tests
  cover both failure handlers, deadline expiry, delayed attempts and backoff.
  This does not yet fix the separate system-menu monitoring implementations or
  mid-session state drift.
- Calendar subscription setup/requests return errors instead of panicking,
  retry with fresh connections, subscribe before the initial snapshot, and
  periodically refresh after daemon owner changes. A private-bus test verifies
  that the same subscription recovers when the daemon appears later.
- Calendar snapshots replace old data; replayed event IDs upsert. Civil date
  conversion and bounded grid work replace unchecked timezone constructors and
  unbounded per-event date iteration. Navigation/view tests cover date limits.
- Nightlight's service wait now observes controller command-channel closure,
  including worker error or panic, so the existing supervisor gets a failure to
  restart. A regression test failed before the change and passes afterward.
  Completed reconciler handles are consumed before cleanup, avoiding a second
  await of a completed JoinHandle; this has its own regression test.
  This does not establish gamma rejection rate limits, output recovery, applied
  state acknowledgement, or replay of a custom preset across service restart.
- MPRIS bar subscription independently monitors the underlying zbus connection,
  rather than relying on mprizzle's event queue to report broker death. Old
  players/connections are dropped before retry. A subprocess test failed before
  the change and passes after terminating its own private broker. The fork itself
  is unchanged, and its original direct recv() probe is still a dependency issue.
- `git ls-remote` verified the custom mprizzle fork's HEAD/main is `456d3374`, the
  same revision pinned here. There was no newer remote fix to adopt at this check.

Additional lifecycle work, the unresolved panic sites, PipeWire write recovery,
system-menu radio synchronization, measured MPRIS traffic, and GPU recovery are
still outstanding. No dependency or system configuration was changed. The only
manifest change enables Tokio's test clock for bar dev-dependencies.

Tests for explicit private-bus/subprocess scenarios are ignored in the default
suite to avoid using the user's bus; run them separately:

```sh
dbus-run-session -- cargo test --locked --offline -p bar \
  modules::clock::calendar::tests:: -- --ignored --test-threads=1
cargo test --locked --offline -p bar broker_loss_reaches_bar \
  -- --ignored --test-threads=1
```

First-batch verification: 44 default-suite unit tests passed across bar, daemon,
config, config-traits and utilities; both explicit private-bus calendar tests
passed; the private-broker MPRIS subprocess regression passed. Formatting of the
changed Rust files and `git diff --check` passed. The four ignored entries in the
default suite are the two calendar cases, MPRIS parent case, and its child helper.

Clippy was also run for bar/daemon all-targets. It is not clean: pre-existing
denied indexing/unwrap sites remain in the tray, media focus, popup management,
system-menu construction and HA capabilities. These remain audit work, not a
reason to weaken the workspace lint policy.

## Implementation progress: second batch

Private BlueZ fixtures reproduced two further failures before the changes:
the first stable powered state was never published by the system-menu monitor,
and a rejected property write followed by a failed read invented the requested
power state. Both are now regression tests.

- System-menu Bluetooth and Wi-Fi subscriptions now own initial reads and
  reconnects. There is no separate initialization task that can overwrite a
  newer monitoring snapshot. Initial confirmed state is published, failed
  initialization/reads are explicit unavailable states, operations have a
  five-second deadline, and reconnect delay is capped at thirty seconds.
- Toggle failures no longer create a default/assumed successful radio state.
  Overlapping toggles are blocked; observation revisions prevent old command
  snapshots from replacing newer confirmed state. Delayed results from a lost
  session cannot complete a new session's pending request.
- Wi-Fi now retains nmrs's two MonitorHandles. In the pinned nmrs version,
  awaiting setup returns a handle; it does not wait for monitor termination.
  The old implementation dropped those handles immediately. Notifications now
  use a capacity-one refresh queue and periodic reconciliation rather than an
  unbounded callback backlog; monitor handles are dropped before reconnect.
- Private NetworkManager fixtures reproduced silent monitor setup failure and
  a scan that never replies. Bar monitor failures/termination, owner changes,
  and transport loss now report failure to the module. Scans, radio writes and
  snapshot refreshes have a five-second deadline. A failure replaces the old
  connection through the existing capped startup retry path.
- Network subscription/task results carry their source connection identity.
  Duplicate terminal events cannot start multiple retries, and stale failures
  or snapshots cannot overwrite a replacement connection. The old transport
  is explicitly closed during failure cleanup.
- Additional private tests cover real service-owner loss, successful module
  replacement, duplicate/stale failures, and radio recovery after initially
  absent services. Radio recovery tests also perform actual property writes
  against their fake services; they do not just check an indicator or transport.

The new `crates/bar/src/modules/test_bus.rs` helper launches a subprocess with
both bus addresses pointing to its own broker. It never mutates the test
runner's environment or touches the live BlueZ/NetworkManager. Helper tests are
ignored and require a private-child marker, so invoke the parent cases:

```sh
dbus-run-session -- cargo test --locked --offline -p bar \
  -- --ignored --skip _child --test-threads=1
```

This includes the previous calendar/MPRIS cases plus the new radio/network
cases. Second-batch verification: 49 default-suite unit tests passed across the
same five packages, and all 12 explicitly run private-bus parent cases passed.
Formatting and `git diff --check` also passed. Full physical hotplug/suspend behaviour, the separate bar Bluetooth
adapter/device event implementation, PipeWire write recovery, and GPU surface
recovery remain outstanding. The fixes target reproduced mechanisms rather
than claiming to identify every past intermittent incident.

See [the panic ledger](panic-audit.md) for the explicit panic inventory and its
justifications. This report distinguishes reproduced defects from static risks;
it does not claim that every intermittent production symptom has been reproduced.

## Implementation progress: third batch

- A real private PipeWire daemon with a null sink reproduced the read-only
  symptom: external writes reached the bar, while UI writes returned without
  touching the server because the node had no Device route. Volume and mute now
  use Node Props when no Device route is cached; existing routes remain preferred.
  Absolute/relative volume and mute/unmute tests require observed server updates,
  not optimistic state. Mute-only writes omit channel volumes.
- Default sink/source names are retained until matching nodes arrive and
  re-resolved on removal/reappearance. A metadata-before-node test failed before
  this change. Default metadata clearing and reconnect cleanup clear saved names.
  Reducer replay covers default removal/reappearance; it is not hardware hotplug
  evidence. Non-core request errors are logged, and non-finite UI targets ignored.
- A leap-second configuration string reproduced a Chrono-to-Jiff conversion
  unwrap panic. Unsupported conversion now returns `ConfigError`; ordinary
  timestamp round trips and the leap-second regression pass.
- An absent-session-bus subprocess reproduced a `SystemMenu::new` panic, even
  without HA entities. Construction now does no bus connection work. Shared HA
  connection initialization has a ten-second deadline and retry capped at thirty
  seconds. Transport closure clears entities and reconnects; generation-tagged
  observations, command completions, and disconnects reject stale results.
  Private-bus tests cover unavailable startup, closure, replacement, entity
  reconstruction and stale-disconnect rejection.
- Logout's shortcut parser reproduced a panic on `1x`: the guard validated only
  the first character before parsing the whole string. Exact single ASCII digit
  matching replaces parsing; malformed/multi-character payloads are ignored.

The PipeWire fixture uses its own socket/runtime directory, no hardware discovery
or session manager, and never modifies the user's server. Run its parent tests:

```sh
env TMPDIR=/tmp/opencode cargo test --locked --offline -p bar \
  modules::volume::tests:: -- --ignored --skip _child --test-threads=1
```

Physical-device route/profile association, asynchronous write-rejection
recovery, stale PipeWire connection events, runtime nightlight numeric validation,
detached launcher provider failures, and GPU surface-local recovery remain open.
The virtual-sink fix is not proof of recovery from incorrect cached hardware
routes. No GPU/device recreation or dependency revision change was made.

Third-batch default-suite verification: 53 unit tests passed across bar, daemon,
config, config-traits, utilities and logout (30 private fixture/helper tests
intentionally ignored). All 16 explicitly run private-bus/PipeWire parent cases
also passed. Changed Rust files pass formatting checks and `git diff --check`.

## Scope and evidence

- All first-party Rust crates: explicit panics, unchecked unwraps, relevant
  indexing, fatal error propagation, subscription termination, and task ownership.
- Focused dependency inspection: pinned mprizzle `456d337`, Iced `18560b6`,
  exwlshelleventloop `be4e00e`, wgpu/wgpu-core `29.0.4`.
- Not an exhaustive audit of every transitive dependency or unsafe block.
- No restarts of the user's bus, compositor, NetworkManager, BlueZ, PipeWire,
  or daemon; no access to stored credentials or Home Assistant endpoints.
- Local read-only GPU query: GTX 1080 Ti, NVIDIA `580.178.04`. This is the current
  environment, not proof of the exact software running at the logged crash.

### Existing baseline: green

```sh
cargo test --locked --offline -p daemon -p bar -p config -p config-traits -p utilities
```

32 tests passed: bar 10, daemon 15, configuration 4, utilities 3. Config-traits
currently has no unit tests. Warnings: unused tray fields and a future Rust
compatibility warning for proc-macro-error2. This is not the entire workspace
test suite. These tests do not cover the reported service-loss scenarios.

### Investigation probes: red

Probes are deliberately outside the repository, at
`/tmp/opencode/subniri-reliability-probes/`. They include the real application
source or call the real pinned dependency; no production source was copied and
modified to force a failure. The temporary manifest was seeded with this
repository's Cargo.lock. Keep these clearly marked investigation artifacts until
the relevant tests are promoted into the project.

```sh
dbus-run-session -- cargo test --locked --offline \
  --manifest-path /tmp/opencode/subniri-reliability-probes/Cargo.toml \
  --target-dir /home/michael/projects/subniri-iced/target \
  --bin subniri-reliability-probes investigation:: \
  -- --test-threads=1
```

Repeated result: **0 passed, 4 failed**, test execution under 0.03 s:

| Probe | Actual failure | Limits of the evidence |
| --- | --- | --- |
| Network startup failure | Injected `NetworkInitialized(Err(...))` returns no recovery task; state remains the loading variant | Exercises the real failure handler, not an actual NetworkManager disconnect |
| Bluetooth startup failure | Injected `BluetoothInitialized(Err(...))` returns no recovery task | Does not reproduce intermittent power-state drift |
| Calendar daemon unavailable | Polling the real subscription on a private empty bus panics at calendar.rs:146 with `ServiceUnknown` | Does not test a mid-call disconnect or full GUI panic propagation |
| Calendar out-of-range event | A valid Chrono date in year 10000 panics in Jiff's `civil::date` constructor | Drives the real event-update handler with a DTO, not a remote D-Bus sender |

The broker-loss probe starts its own dbus-daemon, connects mprizzle, waits for
setup, terminates only that broker, and waits one second for `recv()` to report
an error:

```sh
cargo build --locked --offline \
  --manifest-path /tmp/opencode/subniri-reliability-probes/Cargo.toml \
  --target-dir /home/michael/projects/subniri-iced/target \
  --bin mpris_bus_loss
python /tmp/opencode/subniri-reliability-probes/mpris_bus_loss.py
```

Repeated result: exit 101, `MPRIS recv never reports loss of its D-Bus
connection`. The observed assertion is specifically a one-second deadline,
not a measurement of infinite elapsed time. Source inspection shows the missing
stream-end handling described below. All private brokers were stopped.

## Findings

### 1. D-Bus recovery is inconsistent across component lifetimes

**Reproduced:** an absent daemon panics the calendar subscription. Startup errors
leave Network and Bluetooth in their loading variants without recovery work.

**Static observations:**

- `crates/bar/src/modules/mod.rs:106-120,152-175,237-250,275-290`: startup tasks
  are one-shot; errors are only logged. An uninitialized module has no service
  subscription. The same pattern also exists for taskbar initialization. Volume
  initialization returns `Err`, rendered as loading without a health subscription.
- `crates/bar/src/modules/system_menu/mod.rs:117-119`: system-menu construction
  synchronously opens the session bus and unwraps the result, even with no
  configured HA entities. This is an additional startup crash boundary.
- `crates/daemon/src/main.rs:20-34`: one D-Bus connection is established at
  startup. Supervisors reuse clones of it; no outer bus-replacement loop is
  present. Restarting an inner service is not equivalent to replacing a dead
  transport and re-exporting all interfaces/names.
- `crates/indexer/src/main.rs:43-50`: exports interfaces once, then waits for
  Ctrl-C. There is no connection-loss/re-export loop.
- `crates/launcher/src/launcher.rs:286-329`: name/connection health checks and
  reconnects already exist. First connection failure exits the launcher, while
  subsequent failures are retried via health checks. In-flight health results
  deserve generation/staleness tests.
- `crates/launcher/src/providers/{files,nix_shell}.rs:53-60`: detached provider
  startup unwraps its result. Initial bus/proxy failure can kill the worker
  without the provider reporting startup failure. Request errors later become
  empty results, obscuring service failure as a successful empty search.
- `crates/bar/src/modules/clock/tray/watcher.rs:198-211,214-377`: watcher errors
  terminate the stream; closed signal streams are matched as `Some(...)` and
  therefore are not explicit disconnect events. Spawned item/menu watchers do
  not have an enclosing abort-on-drop guard, so cancellation/error cleanup needs
  a resource-lifetime test, not just assertions about visible tray items.
- `crates/daemon/src/observed.rs:23-54`: client disconnect watcher returns
  successfully on end-of-stream. Spotify/HA store the task handles but do not
  continuously monitor completion; client-observation cleanup can stop silently.
- Secret-store sessions are tied to a connection/session object. Replacing a
  broker or secret-service owner needs session reopening, not only proxy reuse.

**Missing tests:** actual private-bus disconnect/restart and owner replacement
for every component; absent service versus absent broker; method timeout;
subscription cancellation; re-exported names/interfaces; no accumulated workers.
Do not simulate these by restarting the user's session or system bus.

### 2. NetworkManager: startup loading and event-stream death are distinct

**Reproduced:** the startup-error handler has no recovery task.

`crates/bar/src/modules/network.rs:47-50,62-88` awaits initial manager/snapshot
creation without an application-level deadline, and exits event monitoring on
subscription setup failure or stream completion. Snapshot refresh errors only
log. Scanning completes only after `scan_networks()` returns; there is no explicit
timeout/error state in the module messages. The system-menu Wi-Fi implementation
is separate (`system_menu/wifi.rs:64-125`), with its own one-shot monitor setup.

**Acceptance tests:** unavailable at startup; available later; disappears after
initialization; nonreplying methods; scan failure; restored snapshot and control
after restart. Loading, unavailable, disabled, and scanning must be distinguishable.

### 3. Bluetooth: two state authorities, plus startup recovery

**Reproduced:** bar-module startup failure schedules no recovery.

**Not reproduced:** the user's intermittent on/off drift.

- `modules/bluetooth.rs:49-55,173-191` reads power at construction but emits
  only AdapterFound on adapter selection, without refreshing that adapter's
  initial power state. AdapterLost does not clear the cached device list/power.
- Session/adapter monitor setup failure and termination need explicit lifetime
  tests. Connected-device tracking handles adapter DeviceAdded/Removed, not a
  full per-device Connected-property lifecycle.
- `system_menu/bluetooth.rs:47-79` independently initializes UI state and polls
  every five seconds. Its first polling state is cached without being yielded.
  Updates are emitted only when that polling cache changes; ordering between
  initialization, toggles, and polling deserves a deterministic test.
- Toggle read/write failures can become a default/assumed state rather than an
  explicit unavailable/error result (`:96-112`).

**Acceptance tests:** initial read/subscription ordering; external radio toggle;
failed write; adapter replacement; BlueZ restart; concurrent menu/bar views.
These observations are not a verified explanation for the user's exact drift.

### 4. PipeWire: transport health does not establish write health

**Not reproduced:** the user's read-only volume-control state.

`modules/volume.rs:256-306` gets volume updates from Node Props. Writes at
`:576-733` require a device ID and cached Device Route; early exits log or return
without restoring write capability. A node without a physical Device route has
no fallback write path, despite `node_proxies` being stored. One route is stored
per Device ID (`:509-510`), while route events include a route index and route
device. Correct association with the selected sink is not demonstrated.

Transport health uses core sync acknowledgments (`:749-778`). It does not check
whether a volume write was accepted/applied. Core errors are forwarded only for
ID 0 (`:213-223`); per-object write failures need separate observation. The
default sink/source is resolved only when its metadata event arrives; node
arrival/removal ordering needs tests. Old and new connections share an event
queue without a connection generation in `PwEvent`.

**Acceptance tests:** default before node; node recreation; route/profile changes;
multiple routes per physical device; virtual sinks; asynchronous permission or
set-param error; reconnect with queued old events. Verify both externally
observed volume and the UI result. Do not blindly reconnect a healthy transport
to paper over a route-selection or permission problem.

### 5. MPRIS: demonstrated bus-loss hang, unmeasured flooding risk

**Reproduced:** pinned mprizzle does not report private-broker loss within the
one-second deadline. The bar reconnects only when mprizzle returns an error
(`media_controls.rs:620-667`). In the dependency's `mprizzle.rs:201-266`, the
signal branch matches `Some(signal)`; stream completion is not handled as a
disconnect. `Mpris` itself retains an event sender (`:100-104`), so loss of a
watcher is not by itself sufficient to close `recv()`.

**Static traffic/lifetime observations, not a reproduced broker overload:**

- mprizzle has an unbounded event queue and detached watcher tasks. Calling
  watch repeatedly is not visibly guarded against duplicates.
- Player polling uses a one-second interval and uncached property proxies:
  PlaybackStatus per player, plus Position when playing. Default interval burst
  behaviour after a stalled task deserves measurement.
- Each PropertiesChanged/Seeked event triggers a full snapshot in the bar, even
  for inactive players (`media_controls.rs:785-789`). With successful metadata,
  a snapshot invokes Metadata plus ten additional property getters
  (`:1164-1168,1205-1227`); this is source-level request accounting, not a measured
  sustained broker rate. No coalescing of pending refreshes is apparent.
- Snapshot work is awaited inside the same select loop that receives commands
  and watcher events. The three-second snapshot timeout limits one operation,
  not the unbounded backlog of operations.
- Owner transfer with both old and new owners nonempty is not handled by the
  dependency's attach/detach conditions.
- Dependency channel-send unwraps race consumer cancellation. Checking
  `sender.closed()` in select is not an atomic guarantee for a later send.
- Metadata read failure or snapshot timeout creates `PlayerSnapshot::default()`;
  updates replace the last snapshot. Player discovery exists, metadata absent,
  and transport failure are conflated in the displayed state.

**Missing reproduction:** mock-player signal bursts, stalls, owner replacement,
consumer drop, multiple bars/outputs, repeated watch/reconnect cycles. Measure
request counts, queue growth and watcher cleanup. No claim that the historic
broker overload remains, or is fixed, is justified yet.

### 6. Nightlight: handover improved, worker recovery still needs proof

`3e09eaa` added the gamma lock and better service cleanup; the lock-release test
passes. Current code also makes initial controller creation cancellable by the
shutdown signal (`nightlight.rs:43-50`). Do not regress this.

**Static observations, not a reproduced production failure:**

- Controller run errors are logged in a detached spawn (`:565-569`). The outer
  service selects shutdown or preset-reconciler completion (`:73-83`), not
  controller completion. A controller error is not directly returned to the
  supervisor. Subsequent commands can fail on the closed command channel.
- The preset reconciler skips unchanged presets (`:130-133`), so it is not a
  general liveness check or reapply mechanism after controller loss.
- `available` is initialized from configured enablement (`:307-313,432-433`),
  not tracked controller/output health. The intended contract needs clarification.
- SetState acknowledges queued desired state before debounce, gamma writes,
  and flush complete (`:788-791,819-833`). API success does not prove application.
- Output removal/addition is handled; receiving GammaSize schedules reapplication.
  GammaFailed immediately recreates the control and flushes without retry delay
  (`:915-930,950-956`). Persistent rejection needs a bounded-rate test.
- Shutdown sends a command but does not await worker termination/lock release.
  Lock serialization helps handover but is not a worker completion notification.
- Runtime D-Bus temperature/brightness setters do not enforce configuration
  validation ranges. Out-of-range temperatures reach a debug assertion; release
  math and invalid/nonfinite brightness require explicit validation tests.

**Missing reproduction:** a fake Wayland gamma-control peer to reject controls,
remove/re-add outputs, close the connection, and verify desired-state replay,
bounded retries and lock release. No safe full worker-loss loop exists yet in
these probes. A real suspend/output-change trace would complement that test.

### 7. Popup surfaces: do not equate unconfigured with device loss

The exact reported line `wgpu_core.rs:3934` matches pinned wgpu `29.0.4`:

- `src/backend/wgpu_core.rs:3921-3934`: a surface-acquisition error without a
  configured surface error sink takes `handle_error_fatal`. This is not just a
  normal returned SurfaceError being mishandled by the application.
- Successful backend configure assigns that sink (`:3873-3882`). This narrows the
  state to investigate, but does not prove which initialization/configuration
  event produced it on the user's machine.
- `wgpu-core/src/present.rs:158-162` distinguishes absence of presentation
  configuration from checking validity of the configured device.
- Iced's `wgpu/src/window/compositor.rs:301-335` skips configure for zero size;
  present acquires a texture before Iced can map Validation to Other (`:235-260`).
- exwlshelleventloop's `iced_exwlshell/src/multi_window.rs:715-735` caches size/
  scale before configure; unchanged geometry does not independently prove a
  valid configured surface. It skips zero-size and first-new-window presents.
- Its recovery policy already reconfigures Outdated and replaces only the
  affected surface on Lost. Other is only logged; replacement must also be
  tested against stale cached geometry. The renderer is shared across windows.
- A device error callback or broad catch_unwind alone is not a demonstrated
  recovery mechanism; this fatal branch is a particularly important boundary.

**Missing reproduction:** headless/fake-compositor failure injection at new
surface creation/configuration, and the real first-popup trace. Log surface IDs,
creation/configure outcomes, dimensions/scale, redraw requests, acquisition
outcomes and device-loss notifications. Distinguish a new surface from existing
surfaces in the same log. Do not infer a driver fault or destroy shared device
resources just from this message. Whole-device recreation should require
separate device-loss evidence.

Related but not identical upstream report:
<https://github.com/gfx-rs/wgpu/issues/7020> (wgpu 24, parent device lost).

### 8. Home Assistant: recovery exists, initial connection is different

`homeassistant/catalog.rs:87-139` reconnects catalog sessions with capped
exponential delay. It has generation checks, bounded forwarded event channels,
and abort-on-drop forwarders. The UI entity subscription reobserves periodically
and retries failures (`system_menu/homeassistant.rs:125-187`). These are useful
existing protections, not proof that all reconnect paths work.

Initial setup (`homeassistant/connection.rs:49-85,95-159`) returns after secret
store, URL validation, connection, or authentication failure before starting the
catalog reconnect loop. Reconfiguration with the same enabled/URL pair returns
early. Some initial network calls lack the timeout used by reconnect. Stored
UI/daemon D-Bus connections still require transport replacement after broker
loss. No authenticated runtime tests were performed.

**Acceptance tests:** unreachable at startup versus loss after Ready; secret
service restart; stale authentication/catalog completion after reconfigure;
disable during reconnect; closed event streams; observation release and replay;
queued requests that must not act on an obsolete client.

## Prioritized implementation plan (approved; partially implemented)

1. Promote the demonstrated failures into proper project regression tests:
   startup recovery and explicit unavailable state, calendar bus/data errors,
   and mprizzle disconnect propagation. Preserve the current red probes as a
   before/after check. Mprizzle changes may belong in its own fork.
2. Specify component lifecycle contracts: startup deadlines, service-owner
   changes versus broker replacement, cancellable capped retry, current-generation
   snapshots/results, re-export/reobserve after reconnect, and cleanup of old
   workers. Keep read health distinct from write health.
3. Test and repair bar/system-menu Network and Bluetooth paths together; then
   PipeWire sink/route association and asynchronous write outcomes. Do not invent
   a generic retry abstraction before the lifecycle requirements are understood.
4. Build nightlight fake-peer tests and explicitly supervise controller lifetime;
   establish what Available and successful SetState mean before changing them.
5. Measure MPRIS amplification and cancellation/churn. Apply coalescing, queue
   bounds or watcher changes only against measured failing acceptance tests.
6. Add first-popup surface diagnostics/failure injection in the dependency forks;
   establish surface-local recovery before any shared-device rebuild policy.
7. Close remaining unjustified panic sites from the ledger, including generated
   code and dependency boundaries. Retain locally proven invariants/tests rather
   than replacing every unwrap mechanically. Add a repeatable inventory/CI rule
   so new sites require a documented justification.
8. Add private-bus lifecycle tests for daemon/indexer/launcher/providers/settings,
   plus HA initial/reconnect cases; run broader workspace tests and a hardware
   soak involving popup churn, output changes and suspend/resume.

## What is not yet established

- The user's precise Bluetooth, PipeWire, empty-player, nightlight, and GPU
  incidents have not been reproduced end-to-end.
- No actual broker saturation, sustained traffic rate, or task/queue growth has
  been measured; MPRIS accounting above is static.
- The full transitive dependency panic surface and every implicit Rust panic
  mechanism have not been exhaustively proved safe.
- At the initial investigation no source fixes were applied; see implementation
  progress above for the subsequent first batch. No dependency updates,
  live-service disruption, or system configuration changes were made. Next evidence needed is fixture-based
  lifecycle tests and, for hardware-only failures, a redacted incident trace or
  approved targeted instrumentation.
