# Subniri Iced

## Spotify

Enable Spotify and pair through `snowconf` at `spotify.com/pair`. Spotify Premium
is required; no user-created client ID or secret is needed. Saved-track changes
arrive live over WebSocket without registering a playback device. Native collection
reads/writes respect shared cooldowns and do not use the public Web API; legacy
`client_id` settings are deprecated and ignored.

See [Spotify integration and developer API maintenance](docs/development/spotify-api.md)
for recovery policy and offline replay/live opt-in diagnostic commands.

## System menu quick tiles

See [quick-tile configuration and editing](docs/design/quick-tiles.md) for row-based
layouts, responsive widgets, resizing, and animation behavior.

## `iceout` Launcher

`polarbar` launches the logout/power menu through the `iceout` binary.

By default, it looks for `iceout` next to the running `polarbar` executable. During development, this means `target/debug/polarbar` will launch `target/debug/iceout`.

Packagers can override this at compile time with `SUBNIRI_ICEOUT_BIN`:

```sh
SUBNIRI_ICEOUT_BIN=/nix/store/.../bin/iceout cargo build -p polarbar
```

This avoids relying on the user's `PATH`.
