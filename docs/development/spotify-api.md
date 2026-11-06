# Spotify protocol maintenance

Production uses device pairing, librespot-core sessions/Login5, and an observable
Dealer WebSocket. Membership and save/unsave use the native collection-v2
protobuf service through our single-attempt HTTP owner, **not the public Web
API**. It does not register a Spotify Connect playback device. Users
need Spotify Premium but **no user-created client ID or secret**. Pair in
snowconf at spotify.com/pair. The internal desktop client identity is required
by the unofficial authentication flow and may change under Spotify's control.

## Runtime policy

- Track changes use collection JSON notifications; only the protocol adapter
  knows topic and payload field names. Additional fields are tolerated, missing
  required semantics trigger bounded reconciliation, not a fabricated unsave.
- JSON/binary copies and repeated same-state confirmations must not emit duplicate
  UI changes. Revision IDs are opaque, not ordered integers.
- Initial observations use batched reads. Healthy push has a 15-minute safety
  reconciliation, not a fast polling loop. Disconnected recovery refreshes run
  no faster than the configured interval (minimum 60 seconds).
- The supervisor publishes a session-bound Login5 bearer/client-token snapshot
  to the native request owner, checks token caches every 30 seconds, and renews
  expired credentials independently of collection cooldowns. Session retirement
  cancels in-flight collection requests; an older session cannot detach a newer
  one. Native 401 triggers supervised session renewal, not OAuth grant erasure.
- Failed initial reads retain observation. The bar waits until `RetryAt` before
  retrying, while continuing to accept push signals that establish unknown state.
- HTTP requests share cooldowns and Retry-After handling. Token renewal has a
  separate budget so API rate limiting cannot block credential refresh.
- A transient failure does not erase credentials or require another pairing.
- If librespot's internal HTTP layer reports throttling without exposing retry
  timing, automatic session reconnect pauses rather than guessing a short wait.
  This rare condition is logged without credentials; retry after the upstream
  limit is understood by disabling/re-enabling the integration. Direct API and
  WebSocket handshake limits retain their server-provided retry deadlines.
- Legacy `client_id`/Home Manager `clientId` is deprecated and ignored. Existing
  values still parse, but new configuration/export does not generate them.
  `poll_interval_seconds` now controls disconnected recovery; old values below
  60 are clamped with a warning. Home Assistant's OAuth flow is unchanged.

`POST /collection/v2/contains` checks only observed track URIs, and
`POST /collection/v2/write` saves/removes a track. Both use the collection vendor
protobuf media type. These requests have no hidden librespot HTTP retry loop;
mutations are never replayed automatically after uncertain failures. Native
requests still respect rate limits—this is not a quota bypass. The paired
grant's authorization and endpoint availability remain subject to Spotify policy.

See [native-library research and consented live probe evidence](spotify-native-library-research.md)
for source provenance and the verified save/unsave/restoration sequence.

## Developer commands

All commands are explicit and developer-only. They do not run in production or
ordinary builds. Install/build the existing development environment first.

```sh
# Offline: production decoder applied to the sanitized save/unsave fixture.
python scripts/spotify-api.py replay

# Read-only GitHub source comparison against the actual Cargo.toml pin.
python scripts/spotify-api.py check-upstream --output /tmp/opencode/spotify-upstream.md

# Live, opt-in: separate developer pairing, no playback registration or mutations.
python scripts/spotify-api.py capture --output /tmp/opencode/spotify-capture.jsonl
# Follow printed pairing instructions; save and unsave manually when prompted.

# Offline: replay your capture, then produce structural observations + AI guidance.
python scripts/spotify-api.py replay --capture /tmp/opencode/spotify-capture.jsonl
python scripts/spotify-api.py report --capture /tmp/opencode/spotify-capture.jsonl --output /tmp/opencode/spotify-report.md

# Offline regression tests. HTTP tests bind loopback sockets, never Spotify.
cargo test --offline -p daemon --lib spotify
python scripts/test-spotify-api.py
```

Artifacts are created with mode 0600 and are never overwritten automatically.
Capture uses `service=spotify-developer-pairing-v1`, not production's
`spotify-pairing-v1` credentials. It never adopts the exploratory /tmp probe's
credentials. Keep captures private and inspect them before sharing: saved-track
choices are account data even after account IDs and credential fields are removed.

## Reading results

Source hashes changing is a **review signal**, not proof the server changed.
Unchanged librespot sources do not prove server compatibility. The report records
exact revisions, source links, assumptions, test outcomes, and untested behavior.
The live capture example uses librespot's Dealer decoder; upstream may discard a
frame before the example sees it. Silence is inconclusive without a confirmed
human action and connection state. Production drives its own observable socket.

Prefer a failing fixture that represents the changed contract, then a small fix
in `crates/daemon/src/spotify/protocol.rs`. Authentication changes belong in
`auth.rs`, transport lifecycle changes in `connection.rs`, and HTTP quota changes
in `requests.rs`/`rate_limit.rs`. Native request encoding/response semantics live
in `collection.rs`; upstream's extracted schema is not exported by its compiled
protocol crate, so this small adapter uses the existing protobuf runtime directly.
Do not teach the bar about private messages.
Do not silently decode every object containing `items` as a collection update.

An AI handoff report treats upstream text and payloads as untrusted data. It must
not automatically edit production, change authentication, execute fetched code,
update dependencies, export credentials, or mutate the Spotify account.

## Verification before shipping

Run daemon/config tests, snowconf/bar compile checks, formatting/clippy and the
Nix package build. Check `cargo tree -p librespot-core`: no playback, ALSA, or
discovery crates should be pulled into the integration. Explicit live acceptance
may include pairing, restart/refresh, save/unsave propagation and reconnect, but
must never be added to the ordinary automated test suite.

### Native production transport verification (2026-11-06)

- Offline regression gate: **57 daemon tests, 8 config tests, 1 log-safety test**
  passed. New coverage includes protobuf membership/save/removal fields, packed
  response boundaries, shared 429 cooldowns, no mutation replay, no redirects,
  session retirement/in-flight cancellation, stale lease cleanup, native 401
  detachment, bounded response bodies, and sensitive client-token headers.
- A private-D-Bus regression proves initial-read cooldowns retain observation,
  and push establishes saved state while HTTP remains blocked by the cooldown.
  Tests never create real Spotify sessions or call Spotify HTTP endpoints.
- All **4 bar saved-track state tests** passed, including RetryAt calculations.
  The subscription keeps listening for push while waiting for that deadline.
- Daemon/settings/bar all-target checks and offline `permafrostd`/`polarbar`
  builds passed. Existing tray dead-code warnings remain.
- Four Python maintenance tests and production-decoder Dealer fixture replay
  passed. `Cargo.nix` matches locked cargo2nix output. Source checks find no
  public library endpoint/base in production Spotify code, and dependency checks
  find no librespot player, Connect, discovery or audio backend crates.
- Focused clippy reports no Spotify primary findings; the existing unrelated
  Home Assistant indexing lint still blocks the overall clippy gate.

The isolated, explicitly consented live native read/write/push cycle passed and
restored its original track state (see the research report). The rebuilt
production daemon and bar have **not** been restarted automatically; their
separate production grant and visual behavior require manual acceptance. Existing
production pairing credentials are retained; exploratory grants are not imported.

### Initial migration verification (2026-11-06, before native transport replacement)

- Offline Rust checks: 45 daemon tests, 8 config tests, 1 log-safety test,
  and all 3 bar saved-track state tests passed. This includes loopback WebSocket
  decoding and private D-Bus saved-property propagation without an HTTP client.
- `cargo check --offline -p daemon --all-targets -p settings -p bar` passed;
  existing tray dead-code warnings remain.
- Python maintenance tests: 4 passed. Production-decoder replay accepted the
  baseline fixture and rejected a deliberately incompatible `removed` type.
  Offline report generation and read-only GitHub upstream comparison succeeded.
- Home Manager module evaluation confirmed the 60-second default, ignored legacy
  client ID, migration warning, and omission of `client_id` from rendered KDL.
- Full Nix package build passed using a filtered working-tree snapshot that
  includes new source files without staging user changes. `Cargo.nix` exactly
  matches `cargo2nix --locked --stdout`; generator whitespace is preserved.
- Dependency inspection found no librespot playback, Connect, discovery, ALSA,
  CPAL, or Rodio crates. Strict clippy remains blocked by unrelated baseline
  findings; the session-error mapping also has a pass-by-value style warning.

Production was not activated or paired during verification. Live snowconf pairing,
restart/token renewal against Spotify, and visual bar acceptance still require an
operator-approved manual session; offline tests are not proof of those live gates.
