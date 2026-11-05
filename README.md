# Subniri Iced

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
