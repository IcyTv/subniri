# Fileberg module map

`crates/file-manager/src/main.rs` only declares modules and starts the app.

| Module | Owns |
| --- | --- |
| `app/` | Startup, shared application state, host subscriptions, shutdown, and top-level message routing |
| `browser/` | Browser messages and updates; directory loading, navigation history, selection, typeahead, and browsing views |
| `context_menu/` | Context-menu messages and updates, target identity snapshots, favorites, popup lifecycle, search/ordering, and rendering |
| `context_menu/actions/` | One module per action, including its metadata, availability, and execution |
| `tags/` | Tag messages and updates, persistence, editing, and tag views |
| `filesystem/` | Directory entries, disk discovery, and storage usage |
| `ui/` | Shared styling, artwork, and keyboard/focus adapters |
| `trash.rs` | Existing freedesktop trash implementation; unchanged by this refactor |

## Adding an action rule

Start with the action's file, not the view. For example:

```text
context_menu/actions/trash.rs
```

Each action registers a `Definition`: stable configuration ID, label, group,
search aliases, selection scope, multiple-selection support, execution, and an
optional additional availability check. The shared router handles selection
scope, target availability, and routing; action-specific rules live beside that
action's execution. No action registry, policy, or dispatch files need to be
edited to change an existing action's behavior.

`trash.rs` under `actions/` deliberately has an empty additional check. That is
the insertion point for trash-path restrictions; this refactor does not add them.
The existing filesystem implementation remains at `src/trash.rs`.

`context_menu/rows.rs` converts actions into ordered/searchable row data. It
consults action availability but does not define it. Normal menus hide unavailable
rows; search keeps their existing explanations. `context_menu/view.rs` paints
those rows and publishes interaction messages. Execution consults the same action
availability interface before running the owning action.

## Messages and effects

The top-level `app::Message` contains browser, context-menu, and tag message
wrappers plus host lifecycle messages and the variants required by the shell
backend macro. Each feature defines its own message enum and update handler.
Widgets convert feature messages into application messages at the UI seam;
`FileManager::update` only routes them. Feature effects return `Task<app::Message>`
so asynchronous completion returns through the same route.

Shared application state remains explicit in `app::FileManager`: directory,
selection, tags, and popup transitions interact during navigation and shutdown.
There are no hidden dereference-based state aliases or parallel copies of state.
Browser operations live in `browser/directory.rs` and `browser/selection.rs`, tag
operations in `tags/update.rs`, and context-menu operations beside their actions.

## Checks

```sh
cargo check -p file-manager --all-targets
cargo test -p file-manager -- --skip disks::tests::test
cargo fmt --all -- --check
```

The skipped test accesses the real system D-Bus/UDisks2 service. Native popup
checks require the private Weston/niri test environment described in repository
guidance. Nix packaging also requires new source files to be in the Git-tracked
build input; Cargo checks do not establish that packaging condition.
