# Fileberg

Developer orientation: [module map](../../docs/development/fileberg-modules.md).

An Iced file browser with a compact yellow identity bar, paper-colored surfaces,
outlined folders, and blue selection states. It uses embedded Phosphor icons
and no web assets. Missing context-menu operations are explicitly logging stubs.

Folder artwork and colored document cards follow the HTML prototype, with
regular-weight glyphs inset inside each card and a monospace extension label.
Supported raster images show bounded previews decoded on the directory worker;
invalid, unsupported, or oversized images retain a document-card fallback.
DM Sans and Space Mono are bundled locally with their OFL licenses in
`assets/fonts/`; the application does not fetch fonts at runtime.

```sh
cargo run -p file-manager -- /path/to/directory
```

## Browsing

- Click to select one item; Ctrl-click toggles individual items. Shift-click selects
  an anchored range in displayed order; Ctrl+Shift-click adds a range. Repeated
  range selection can shrink or cross the anchor. Left-click empty space to clear.
- Arrows select and move through items; grid Up/Down follows actual wrapped rows.
  Shift extends the range, Ctrl moves only the keyboard cursor, and Ctrl+Shift adds
  a range. Home/End select the first/last item with the same modifiers.
- Ctrl+A selects all visible items. Space selects the focused item; Ctrl+Space
  toggles it. Tab/Shift+Tab move between controls, treating files as one tab stop.
- Double-click opens the clicked item. Enter and **Open** require one selected
  item; multi-selection Open is unavailable until tab support is added.
- Files open with `xdg-open`, using their original OS paths.
- Home, standard user directories, Root, breadcrumbs, Back, Forward, and Up navigate.
- Search filters filenames in the current directory, case-insensitively.
- Typing outside a text field selects the first visible filename beginning with
  that case-insensitive prefix, without filtering the list (`f` → `faz`, then
  `fo` → `foo`). A small “Jump to” preview shows the prefix and unmatched input.
  Backspace edits it; Escape dismisses only the prefix, preserving the selection
  and scroll position. Clicks dismiss the prefix and apply normal selection rules.
  Spaces can refine a prefix without
  accidentally opening the focused file. Text fields keep their normal typing.
- Grid/list switching preserves selection and filters. Sorting keeps folders first.
- A compact selection/action strip is always present, so first selection does not
  resize the file viewport or move double-click targets. Details start collapsed;
  **Info** or context-menu **Properties** expands a fixed-height, internally
  scrollable inspector. Its expansion state survives selection changes, clearing,
  refresh, and navigation for the current window; **Hide info** collapses it.
- Grid tiles stay 160×155 pixels; resizing wraps them into additional columns
  instead of stretching or shrinking the tiles.
- Up, breadcrumbs to ancestors, and Back highlight the child folder just left.
  Forward highlights the next redo destination when it is a child, otherwise
  restoring that visit's selection. Explicit revisits also remember selections.
  Restored entries are focused and scrolled into view after the scan completes;
  stale scans and deleted entries cannot restore an invalid highlight.
- New folder opens an inline form. Existing names and path traversal are rejected.
- Ctrl+K focuses search; F5 refreshes; Alt+Left/Right navigate history.
  Escape dismisses a menu, prefix, or form first; otherwise it clears selection
  without resetting filename/tag filters. Text fields keep their own shortcuts.
- Narrow windows use an icon sidebar and a two-row navigation/search toolbar.
- Place tooltips appear only in the icon-only sidebar, after a short hover delay,
  with an opaque bordered bubble, pointer, and hard shadow.
- Breadcrumbs start with a clickable Home icon, followed by chevrons and path
  components; Home itself is shown as `Home icon > Home`. The sidebar disk card
  shows real filesystem usage and opens Root.
- Mouse clicks do not leave focus outlines behind; Tab navigation retains them.

Directory scans and file operations run off the UI thread. Stale scans cannot
replace the current directory. Read, creation, and launch errors are displayed.

## Tags

Use the sidebar **+** to create a tag and its pencil buttons to rename or change
a tag's parent. Parent choices exclude the tag and its descendants. Names must
be unique within their parent. `Starred` starts as an ordinary, editable tag.

Select a file or folder to assign tags from **+ Add tag**. Click a tag chip to
remove the assignment. The Info button shows/hides this panel. Sidebar tag filters
include descendant tags and combine with filename search, within the current
directory only. Assigning a parent does not assign its children.

Storage is centralized in `$XDG_DATA_HOME/fileberg/tags.json`, falling back to
`~/.local/share/fileberg/tags.json`. The versioned JSON contains stable numeric tag
IDs, parent IDs, and absolute Unix path bytes, including non-UTF-8 paths. Only
the tag storage module owns serialization; a future database or icepickd migration
does not need to change file views.

Writes replace the JSON atomically after syncing the temporary file. A failed
save leaves the previous UI snapshot intact and displays an error. Invalid or
future-version files are not overwritten. A lifetime file lock allows one writable
Fileberg instance at a time; other windows can still browse and retry tag loading
after the writer exits. Closing waits for pending tag/favorites saves or folder creation.

## Context menus

- Right-click an item or empty file-area space; Menu/Shift+F10 uses the selected
  entry's bounds, or a directory-menu fallback without selection.
- Right-clicking a selected item preserves the selected group; an unselected item
  replaces the selection. Background menus and dismissal preserve selection.
  Item menus snapshot every selected path and identity. Multi-item Properties
  uses that snapshot. Cut, Copy, Duplicate, Rename, and Trash remain
  logging stubs and log the full selection without modifying files.
  Open and other unsupported multi-item actions are hidden from
  the normal menu. Action search retains explanations for unavailable actions.
  Multiple-item details show a summary; per-item tags require one selected item.
- Menus are native application-parented Wayland popups. The compositor handles
  screen-edge placement and grabs; outside clicks follow native delivery, with
  no suppression or replay. Escape closes the whole menu.
- Action search uses the launcher's shared fuzzy/typo matching and aliases.
  Search flattens submenu choices and explains unavailable actions. Filename
  type-to-select is disabled for the entire lifetime of the menu.
- Arrows navigate actions; Tab also reaches pin controls. Horizontal arrows keep
  editing the caret while search is focused. Open With, Tags, Sort By, and View
  expose native submenus; search can execute their choices directly.
- Hover/highlight a command to pin/unpin it. One shared favorites list is stored
  through `crates/config` in `subniri/config.kdl`, including existing override-file
  semantics. Favorites are unique and appear only when applicable.
- Drag favorite rows to reorder them when search is empty; a six-pixel threshold
  distinguishes dragging from clicking. Hidden favorites keep their stored slots.
  Pinning and reordering keep the menu open; completed edits survive restart.
- Background updates retain the original target, query and highlighted action
  identity. A disappearing/replaced target disables its actions without closing
  the popup. An open descriptor prevents inode reuse from silently retargeting it.
- Existing Open, New Folder, tag assignment, view and sort operations remain real.
  Cut, Copy, Duplicate, Rename, Trash, Open in New Window, application selection,
  and directory Properties currently log the intended target/action only.
  They never claim success or modify files. These are deliberate prototype stubs.

Favorites writes are serialized and reload current shared settings before changing
only Fileberg preferences. Failures are displayed and roll back to confirmed
favorites. The shared config writer's existing cross-process read/write race is
unchanged; simultaneous edits from separate applications are not transactionally
locked. Favorites for particular submenu choices and filesystem drag-and-drop
remain deferred.

**Current limits:** assignments are path-based; external renames/moves are not
tracked. Tag deletion, global tag search, indexing, clipboard file
operations, Recent/Trash views, and cross-compositor decoration negotiation are
not implemented. The context menu exposes the logging stubs described above.

## Verification

```sh
cargo fmt -p file-manager -- --check
cargo check -p file-manager
cargo test -p file-manager -- --skip disks::tests::test
cargo clippy -p file-manager --all-targets --no-deps
cargo build -p file-manager --example native-popup-probe
nix-shell -p weston --run 'python3 scripts/test-fileberg-native-popup.py'
nix-shell -p weston --run 'POPUP_PROBE_SCALE=1.25 python3 scripts/test-fileberg-native-popup.py'
nix-shell -p weston --run 'python3 scripts/test-fileberg-context-menu.py'
nix-shell -p weston --run 'FILEBERG_PROBE_FAVORITES=1 python3 scripts/test-fileberg-context-menu.py'
```

The existing `disks::tests::test` is a live UDisks2/system-D-Bus probe, not a
deterministic unit test. It is excluded from the focused test command above.

Native checks reuse the existing private headless Weston→niri helpers. Keyboard
events enter through a test-only host relay so niri exercises ordinary popup grabs;
its internal virtual-keyboard protocol bypasses that path. User desktop sockets,
configuration, and data are never used by these tests. Artifacts remain in
`/tmp/opencode/fileberg-*`.
