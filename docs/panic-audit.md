# Panic justification ledger

Snapshot: `ed53c89`, 2026-11-02. Line numbers refer to this snapshot. Scope:
every lexical explicit panic/unchecked-unwrap site found under first-party
`crates/**/*.rs`, including generated-code templates, tests and inactive code.
Focused dependency sites are listed separately. This is an inventory and
justification review, not a proof that Rust or all dependencies cannot panic.

## Resolution progress

The tables remain a baseline ledger with original line references. First-batch
implementation removed the calendar production unwraps and panicking date
construction paths listed below. Invalid/out-of-domain dates are ignored and
calendar navigation stays in the representable range; date work is bounded to
the visible grid. Tests cover out-of-range DTOs, snapshot replacement/replay,
navigation at both date limits, absent daemon, and later daemon availability.

Nightlight worker termination now reaches the service supervisor, and MPRIS bar
monitoring independently detects zbus transport loss. These lifecycle changes do
not justify the remaining dependency panics or establish GPU recovery. New test
assertions are deliberate test oracles; no new production unwrap/expect/panic
site was introduced. See reliability-investigation.md for remaining work.

Second-batch radio/network recovery changes likewise introduce no production
unwrap/expect/panic sites. Private-service test assertions and fixture failures
are deliberate test oracles. Silent stream termination and guessed radio-state
fallbacks were replaced with explicit failures, deadlines and reconnect handling;
these were reliability failures even though they did not contain panic macros.

Third-batch resolutions: the session-bus constructor unwrap was removed after an
absent-bus regression reproduced it. Connection establishment is now bounded,
asynchronous and retried; private tests also cover shared transport replacement
and stale disconnects. The timestamp conversion unwrap was removed after a
leap-second string reproduced incompatible Chrono/Jiff subsecond semantics;
conversion errors now become `ConfigError`. New PipeWire/private-bus and timestamp
test assertions are deliberate test oracles. Original line references below are
retained to identify the baseline sites.

Logout keyboard parsing was reproduced with the multi-character payload `1x`.
Only exact single ASCII digits `1` through `6` now select entries, without numeric
parsing or unwrap. Tests also reject overflowing, multi-digit, combining-character
and non-ASCII payloads. These new assertions are deliberate test oracles.

Inventory query:

```text
\b(unwrap|expect|unwrap_err|expect_err|unwrap_unchecked)\s*\(
\b(panic|todo|unimplemented|unreachable|assert|assert_eq|assert_ne|debug_assert|debug_assert_eq|debug_assert_ne)!
```

The search returned 205 matching lines, including commented-out matches. Grouped
rows below explicitly enumerate their sites; grouping is not an exemption.
Fallback operations such as unwrap_or/default/else are not panics but may still
hide unavailable state, and are discussed in the investigation report.

Verdicts:

- **J**: current control flow/literal establishes the invariant locally.
- **C**: conditional/caller invariant; record it and guard with tests.
- **R**: recoverable/external failure is not a justified panic.
- **U**: not established; needs a concrete test or stronger proof.
- **T**: deliberate test failure/fixture assertion, not a production path.
- **I**: inactive source/comment, not linked into the current application.

Any task panic below can still become process-fatal with an aborting panic
configuration; the current workspace does not select panic=abort. Detached task
panics are not acceptable recovery merely because the process survives.

## First-party production and generated runtime sites

Paths in this table are relative to `crates/`.

| File:line(s) | Operation / invariant | Exposure and consequence | Verdict / next verification |
| --- | --- | --- | --- |
| bar/src/modules/system_menu/mod.rs:119 | Session connection unwrap | External bus availability; synchronous startup panic | **Resolved**; removed, absent-bus constructor regression and bounded async recovery |
| bar/src/modules/clock/calendar.rs:101,102,114,115 | Chrono-to-Jiff conversions unwrap | Remote DTO date/time, local timezone gaps/ambiguities; UI update panic | **R**; year-10000 event already reproduces a panic inside the conversion constructor before these unwraps |
| bar/src/modules/clock/calendar.rs:135 | Session connection unwrap | Absent/dead broker; subscription panic/task loss | **R**; missing-service probe covers the downstream request, absent broker still needs its own case |
| bar/src/modules/clock/calendar.rs:136 | Proxy creation unwrap | Fallible external proxy setup; subscription panic | **R**; fail proxy setup explicitly |
| bar/src/modules/clock/calendar.rs:140 | Zoned midnight unwrap | Valid civil dates need not have an unambiguous local midnight | **R**; timezone-transition fixture |
| bar/src/modules/clock/calendar.rs:141 | Month addition unwrap | Selected date may reach library limits | **R**; calendar bounds test |
| bar/src/modules/clock/calendar.rs:143,144 | Jiff-to-Chrono conversion unwrap | Civil dates/times ordinarily representable in Chrono, but proof is only conditional on preceding valid state | **C**; encode range/conversion invariant and test boundaries |
| bar/src/modules/clock/calendar.rs:146 | get_events unwrap | Daemon absent/restarted, remote error, bus loss | **R**; reproduced ServiceUnknown on private bus |
| bar/src/modules/clock/calendar.rs:151 | Signal subscription unwrap | External subscription failure | **R**; disconnect/setup-error test |
| bar/src/modules/clock/calendar.rs:154 | Signal argument unwrap | Remote malformed/unexpected payload | **R**; malformed signal fixture |
| bar/src/modules/clock/calendar.rs:165 | View's zoned-date unwrap | Selected year can exceed Jiff's supported year; timezone midnight may be ambiguous/nonexistent | **R**; navigation bounds plus timezone fixture; `civil::date` can panic before unwrap |
| bar/src/modules/clock/calendar.rs:167 | first_of_month unwrap | Requires an in-range representable zoned first-of-month | **C**; current construction requests day 1 but timezone/boundaries still need proof |
| bar/src/modules/clock/calendar.rs:170 | Leading-day subtraction unwrap | Underflows at minimum representable date | **R**; lower-bound grid test |
| bar/src/modules/clock/calendar.rs:194,211 | Grid date addition unwrap | Overflows at maximum representable date | **R**; upper-bound grid test |
| bar/src/main.rs:395 | unreachable message variant | Enclosing match binds only OpenSettings/OpenPowerMenu; no mutation before inner match | **J**; UI main-thread panic only if that invariant is changed |
| bar/src/modules/media_controls.rs:734 | unreachable CyclePlayer | Enclosing command arm admits only PlayPause/SkipNext/SkipPrevious | **J**; no external input can enter this variant within this arm |
| bar/src/modules/taskbar.rs:303 | unreachable slice length | Only caller collects `.take(4)` before calling workspace_preview | **C**; caller cap at :224; test lengths 0..4 or make helper total |
| daemon/src/main.rs:18 | Rustls provider installation unwrap | Requires no provider installed by earlier code; currently before network setup, after log init | **C**; document single-install startup order; preinstalled-provider case is fallible, not a transport recovery issue |
| daemon/src/homeassistant/mod.rs:153 | unreachable Ok(true) | Immediately preceding immutable matches! rejects Ok(true) before entering block | **J**; registration failure cleanup test |
| daemon/src/nightlight.rs:161 | unreachable Custom schedule preset | current_preset yields Day/Night; set_preset rejects Custom; private helper callers need to preserve contract | **C**; test schedule/custom paths |
| daemon/src/nightlight.rs:1031 | Temperature debug_assert | Config validates ranges, but runtime D-Bus setter accepts arbitrary u32 | **R** in debug; release does not assert; validate at the runtime boundary and test invalid input |
| config-traits/src/lib.rs:199 | Debug-only panic in expected() | Caller must supply WrongType; private error conversion discipline, but method is public | **C**; error-annotation tests; release returns self rather than panicking |
| config-traits/src/lib.rs:423 | unsafe first().unwrap_unchecked() | Same immutable slice checked nonempty and length <=1 just above; no intervening mutation | **J** safety invariant; violation would be UB, not a panic; safe destructuring can remove unnecessary unsafe |
| config-traits/src/lib.rs:569 | Parsed Chrono timestamp -> Jiff unwrap | Dateparser success does not by itself establish identical range/leap-second semantics | **Resolved**; leap-second regression reproduced panic; conversion now returns ConfigError |
| config/src/lib.rs:619,624 | Generated default Time unwrap_unchecked | Literal 07:00 and 20:00 with zero seconds/nanos are valid | **J** safety invariant; replace with safe/constant construction when convenient, no demonstrated external trigger |
| config/src/lib.rs:682,683 | Default Time unwrap_unchecked | Same valid literal times | **J**, not justification for unsafe generally; default construction test |
| config-macros/src/config.rs:998,1023,1106,1124 | Generated child serializer last_mut unwrap | Immediately pushes to the same mutable KDL child vector before last_mut, no intervening callback after push | **J** runtime invariant; option/vector/scalar serialization tests |
| config-macros/src/config.rs:1148,1169,1247,1265 | Generated document serializer last_mut unwrap | Immediately pushes to the same mutable document vector | **J** runtime invariant; document serialization tests |
| launcher/src/providers/files.rs:59 | Detached task result unwrap | Session/proxy creation or worker error; task loss can leave provider loading | **R**; startup acknowledgement, provider error state, retry/cancellation test |
| launcher/src/providers/nix_shell.rs:58 | Detached task result unwrap | Same external failure boundary | **R**; same tests, not just launcher D-Bus registration health |
| logout/src/main.rs:197 | Character string -> u8 unwrap | Guard checks only first character in '1'..='6', not entire string or numeric range | **Resolved**; `1x` reproduced panic; exact single-byte shortcut matching replaces parsing |
| logout/src/main.rs:348 | unreachable action index | selected_action is called only after is_entry_enabled(selected_entry) succeeds in update | **C**; command/index mapping tests |
| logout/src/main.rs:362 | unreachable Lock | Earlier `if matches!(action, Lock)` returns without mutation | **J** |
| indexer/src/main.rs:80,100,133,148 | Timestamp elapsed unwrap | Fallible civil/timestamp span computation used for diagnostics; no domain proof supplied | **U**; use monotonic elapsed timing; do not claim normal backward clock motion necessarily errors |
| indexer/src/nix.rs:157,158 | Fast string-column Result and Option expect | Disk index may be old/mismatched/unreadable; opening an existing index does not establish current field columns | **R**; old-schema/corrupt-segment fixture, propagate/rebuild rather than panic |
| indexer/src/nix.rs:162,163 | pname fast string-column Result and Option expect | Same disk/schema boundary | **R**; test both missing column and reader error |
| indexer/src/nix.rs:167 | Numeric fast-field expect | Same disk/schema boundary | **R**; incompatible cache test |
| neo-widgets/src/widgets/neo_scrollable.rs:558 | unreachable non-Dragging | Match guard checks same unchanged state.interaction before destructuring | **J** |

## Compile-time macro implementation sites

| File:line(s) | Justification | Verdict |
| --- | --- | --- |
| config-macros/src/config.rs:160 | let-else has already excluded Struct on the same input.data | **J**, compiler/proc-macro process only |
| config-macros/src/config.rs:631,790,956 | is_option checks Option plus angle-bracketed arguments, but option_inner_ty additionally requires an actual type argument | **U**; the predicates are not equivalent on arbitrary syn AST. Malformed/generic AST fixture should yield a diagnostic rather than a proc-macro panic; not a long-running runtime path |

Proc-macro `abort!` calls are intentional compiler diagnostics for unsupported
input, not process-abort calls in the running shell. Generated runtime code is
listed in the production table rather than dismissed as compile-time code.

## Inactive/commented source

| File:line(s) | Classification and justification |
| --- | --- |
| bar/src/modules/volume_pa.rs:72 | **I/R**; detached thread runtime creation can fail. File is not declared by current modules/mod.rs |
| bar/src/modules/volume_pa.rs:78,79 | **I/U**; allocation/proplist operations assumed infallible; not linked |
| bar/src/modules/volume_pa.rs:82,86,88 | **I/R**; external PulseAudio context/connect/readiness failures are recoverable; not linked |
| bar/src/modules/volume_pa.rs:109 | **I/R**; subscription rejection is external, not a valid assertion; not linked |
| neo-widgets/src/widgets/neo_slider.rs:184,224 | **I**; commented unwrap/debug_assert, no executable code |

Inactive does not mean safe to reactivate. Preserve this classification until the
file is removed with appropriate authorization or receives its own recovery work.

## Test-only sites (explicitly accounted for)

Every listed assertion is an intended test oracle. Fixture unwrap/expect failures
should fail the test rather than fabricate state. Mutex unwraps in configuration
tests intentionally expose fixture poisoning; this is not a production recovery
strategy. Test branches are cfg(test), not linked into release binaries.

| File | Line(s) | Justification |
| --- | --- | --- |
| utilities/src/lib.rs | 84,92,93,101 | **T**, wrapper value/identity/hash assertions |
| daemon/src/homeassistant/capabilities.rs | 304,315,321,326 | **T**, capability extraction assertions |
| daemon/src/homeassistant/snapshot.rs | 124,125,129,153,154,155,156,157,158 | **T**, snapshot preservation/value assertions |
| daemon/src/homeassistant/entity.rs | 349,366 | **T**, stable entity paths and rejection oracles |
| daemon/src/observed.rs | 205,206,207,211,218,225,232,238,239,240,244,253,254,255,257,258,259,260,261,262,268,269,270,274,280,281,282,284,285,286,290,291 | **T**, observation/reference-count/limit oracles |
| daemon/src/spotify.rs | 1399,1409,1424 | **T**, path validation and refresh-token preservation |
| daemon/src/nightlight.rs | 1076 | **T**, competing lock acquisition must fail |
| bar/src/widget_config.rs | 76,77,81,83,84,86,87 | **T**, coalesced writer-state assertions |
| bar/src/main.rs | 1011 | **T**, context-menu coordinates |
| config/src/lib.rs | 926,965,967,968,971,973,974,975,976,980,981,982,987,989,992,993,995,998,999,1005,1006,1009,1011,1013,1015,1026 | **T**, isolated config fixtures/locks and validation/round-trip oracles |
| indexer/src/lib.rs | 215,235,248,254,262,263,265 | **T**, temporary index/random-word/filesystem fixture creation; test setup failure must be visible |
| launcher/src/providers/applications.rs | 683,685,686,692,694,695,700,711,713 | **T**, typo-match fixture presence and match-index assertions |
| bar/src/modules/system_menu/editor.rs | 491,510,530,540,541,547,548 | **T**, reordering, slot mapping and drag state |
| bar/src/modules/media_controls.rs | 1097,1101,1112,1118 | **T**, Spotify track URL parsing |
| bar/src/modules/clock/tray/dbusmenu.rs | 362,363,364,365,366,378,382,385,389,390,416,417,418,422,430,431,432,433 | **T**, layout tree/properties oracles; fixture children indexing intentionally fails malformed test expectations |
| bar/src/modules/clock/tray/watcher.rs | 609,611 | **T**, item signal refresh policy |

## Focused dependency panic boundaries

This is not an exhaustive dependency-wide inventory.

| Dependency/file:line(s) | Justification review |
| --- | --- |
| mprizzle/player.rs:137,146,160,174,210,216,245 | **R**, unbounded event-channel send unwraps. Consumer can disappear between readiness checks and send; worker panic is not orderly cancellation |
| mprizzle/mprizzle.rs:137,150,165,187,198,224,233,242,252,262 | **R**, same send/cancellation race in discovery and attach/detach/error paths |
| mprizzle/identity.rs:13,14; mprizzle/mprizzle.rs:88 | Documentation examples, not active library panic sites |
| iced/wgpu/src/window/compositor.rs:311 | **R/U**, create_surface expect. Surface/native-handle/driver creation is fallible; public compositor trait currently makes this awkward to propagate |
| exwlshelleventloop/iced_exwlshell/src/multi_window.rs:796 | OutOfMemory explicitly fatal. **U**, deliberate policy rather than a demonstrated unavoidable process termination; discuss graceful UI shutdown/fallback separately |
| exwlshelleventloop/iced_exwlshell/src/multi_window.rs:809 | **C**, recovering surface assumes matching shell unit still exists. Test close/recreate ordering; stale native window must not become panic |
| exwlshelleventloop/iced_exwlshell/src/multi_window/window_manager.rs:140 | **J**, map get immediately follows inserting same key |
| wgpu/src/backend/wgpu_core.rs:3934 | Fatal acquisition-error path with no configured error sink; matches reported line. **R** as an application recovery boundary; prevent invalid acquisition and establish recoverable configuration handling, not a proven device-loss event |
| wgpu/src/api/surface.rs:136 | **C**, successful texture acquisition assumes high-level configuration exists; not the line in the user's trace |

## Implicit panic and silent-failure review

- **Calendar constructors:** `jiff::civil::date/time` panic on invalid arguments
  even inside functions returning Option. Year 10000 is a reproduced example.
  Leap seconds, timezone transitions, narrowing Chrono years to i16, and unbounded
  navigation need separate range tests. Checked arithmetic followed by unwrap is
  still a panic boundary, not bounds handling.
- **Popup vectors:** main.rs:439,491,567 use positions found in the same unchanged
  vector; :726 indexes windows(2). Locally justified. Preserve no-mutation-between-
  lookup-and-use assumptions in future refactors.
- **Other position-index operations:** launcher.rs:419, taskbar.rs:152 and
  nightlight.rs:944 derive their index from the same vector immediately before
  access/removal. Locally justified under current single-owner control flow.
- **Media focus vectors:** media_controls.rs:1359,1378 follow a nonempty guard;
  locally justified. Tray ARGB indexing uses chunks_exact(4), so pixel[0..3] is
  justified. Pixmap dimension multiplication/resource limits need architecture-
  aware overflow and oversized-payload tests (not demonstrated on this x86_64 run).
- **Generated parser/serializer indexing:** entry[0] sites are guarded by length
  checks; node indexes come from the same vector's position; list reuse checks
  idx<len. Retain round-trip/malformed-document tests around templates.
- **Widget tree children[0]:** grow/card/button/scrollable and neo_surface assume
  Iced calls diff to establish exactly one child before layout/update/draw. This
  is a framework lifecycle invariant, not proof from the index expression. Needs
  lifecycle tests for creation, replacement and reuse; do not classify as safe
  solely because the tree usually has a child.
- **Editor removes/inserts:** rely on active count, key list and press indices
  staying consistent while state is reused. Existing reorder tests help, but
  config change during drag and stale Press cases need coverage.
- **Launcher typo indexes:** windows(2) accesses are locally justified; token
  prefix indexing depends on nonzero pattern-length policy and preceding length
  guards. Preserve configuration validation and test empty patterns.
- **Main Result errors:** config/logging/display/database/bus setup can exit
  programs without panic. For long-running components classify fatal local
  configuration failures separately from transient external-service failures.
- **Detached tasks/subscriptions:** a successful process health check does not
  establish live workers. Nightlight controller, provider startup, tray workers,
  Spotify/HA disconnect watchers and ended Bluetooth/network streams need owned
  cancellation/completion paths. See the investigation report for specific code.
- **Explicit process::exit:** CLI main.rs:280,346,395 are one-shot CLI error
  exits, not daemon recovery. No first-party process::abort/unreachable_unchecked
  site was found. Tokio abort() calls cancel tasks and are not process aborts.

## Maintenance rule to adopt with fixes

For a retained production panic require an adjacent invariant or a ledger entry
plus a regression test of its precondition. External service availability,
well-formed remote data, a live consumer, and successful I/O are not invariants.
Use safe constructs for proven branches where possible; unsafe unwraps need a
safety proof, not merely a statement that crashing is unlikely. Re-run the
inventory after changes and update line references and verdicts.
