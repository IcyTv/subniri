# Repository guidance

## Scope and sources
- Subniri is a desktop shell/environment currently targeting the niri Wayland compositor. Nix is the only packaging mechanism; Cargo builds are for development, not a replacement packaging path.
- Prefer current code, manifests, and Nix expressions over prose. Existing `docs/` and `scripts/` are mostly development artifacts: informative, not authoritative; verify their claims before relying on them.
- Put task-specific plans, scripts, captures, reports, and other outputs under `.opencode/<task>/`, not the repository root. Durable user/developer documentation belongs in `docs/`; reusable development helper scripts belong in `scripts/`.

## Development and verification
- Enter the flake environment via `direnv` (`.envrc` uses `use flake`) or `nix develop`. It supplies native libraries, `PHOSPHOR_ICONS`, and runtime library paths; a bare Rust toolchain is insufficient.
- Use `cargo check`, not Clippy, as the normal compile gate. `prek.toml` still lists Clippy, but that is not the current checking policy; do not run the aggregate hooks just to check compilation.

```sh
cargo check -p <package> --all-targets
cargo test -p <package> <test-filter>
cargo fmt --all -- --check
nix build . --no-link
```

- For deterministic Fileberg tests: `cargo test -p file-manager -- --skip disks::tests::test`. The skipped test connects to the real system D-Bus/UDisks2 service.
- Compositor tests must run through `nix-shell` with Weston, e.g. `nix-shell -p weston --run '<test command>'`, from the development environment. Use a private headless Weston host with nested niri, not the user's running desktop; isolate configuration/data and put new artifacts under `.opencode/<task>/`.
- `nix-shell -p ...` may also supply ephemeral dependencies (including Python packages); do not add task-only dependencies to the project environment.
- Nix flake builds only see Git-tracked source paths. Newly created source/assets must be included in the build input before a Nix build can validate them; Cargo seeing them is not sufficient.

## Dependencies and Nix packaging
- Fork checkouts are available locally, e.g. `~/projects/iced` and `~/projects/exwlshelleventloop`. Local dependency patches are fine during iteration. A fork change is not complete until it is pushed and this repository's pinned revision/lockfile is updated; do not ship local path overrides.
- Keep `Cargo.lock` and generated `Cargo.nix` synchronized after dependency changes. Regenerate with `cargo2nix --locked --overwrite`; never hand-edit or format `Cargo.nix`. Nix formatting excludes it: `alejandra --check -e ./Cargo.nix .`.
- All patched wgpu-family crates (including naga) must use the same fork revision in root `Cargo.toml`; mixing registry/git sources duplicates shared types.
- `flake.nix` owns the cargo2nix package set, fork build overrides, GUI runtime wrappers, and shipped package list. Adding a Cargo workspace member does not automatically add it to the shipped bundle; Fileberg is currently not in that list.

## Wiring that is easy to miss
- Cargo package names differ from binaries: `bar` → `polarbar`, `daemon` → `permafrostd`, `launcher` → `avalaunch`, `logout` → `iceout`, `settings` → `snowconf`, `indexer` → `icepickd`, `cli` → `subniri`, `file-manager` → `fileberg`. Use package names with `cargo -p`.
- `crates/bar/src/main.rs` wires the layer-shell UI through the forked `iced_exwlshell` backend, not standard Iced/winit. Bar popup orchestration is in `crates/bar/src/popups.rs`; shared widgets/styles live in `crates/neo-widgets`.
- `crates/daemon` owns background integrations exposed over session D-Bus; `crates/daemon-common` contains shared interfaces. Keep service/protocol logic out of bar widgets.
- Shared KDL configuration is owned by `crates/config` with derives in `config-macros`/`config-traits`; Home Manager rendering is in `nix/home-manager/subniri.nix`. Configuration changes may need updates on both Rust and Nix sides.
- Config defaults to `$XDG_CONFIG_HOME/subniri/config.kdl`; `SUBNIRI_CONFIG_FILE` selects another base file. `SUBNIRI_CONFIG_OVERRIDE_FILE` selects a separate editable override: reads use it when present and writes target it. Preserve this distinction when changing settings persistence.
- `polarbar` launches `iceout` from beside its own executable during development, not via `PATH`; build `logout` too when testing that menu. Nix sets the compile-time `SUBNIRI_ICEOUT_BIN` path.
