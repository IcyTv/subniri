# System menu quick tiles

Quick tiles use a square grid. Width and height are positive grid spans, not
pixels. Text size stays fixed; each widget adapts its contents to its footprint.

## Configuration

```kdl
system_menu {
    row {
        widget "wifi" width=1
        widget "nightlight" width=4 height=2
    }
    row {
        widget "bluetooth" width=1
        gap width=4
        widget "script.example" width=2
    }
    row {}
}
```

- Omitted widget dimensions mean width 2, height 1; gaps default to width 1.
- Each `row` names the next physical grid row. Empty rows preserve vertical space
  when loading handwritten configuration, but edit exit removes unoccupied rows.
- A gap advances the cursor, including across cells occupied by a tall tile above.
  Collisions push widgets right until they fit and log a configuration warning.
- Legacy `widgets wifi bluetooth` and `widgets { - wifi; - bluetooth; }` lists
  remain readable. They migrate into rows of two medium tiles when saved.
- Do not combine legacy `widgets` with `row` declarations.
- Saves include explicit gaps and omit default dimensions. Duplicate widgets keep
  separate footprints. Unrelated settings are reread before saving.
- Interactive growth stops at output bounds. Oversized handwritten layouts retain
  their dimensions but are clipped; there is no scrolling.

The Home Manager setting is `services.subniri.settings.systemMenu.rows`: a list
of rows containing entries such as `{ widget = "wifi"; width = 1; }` or
`{ gap = 4; }`. An empty list denotes an empty row. The legacy
`systemMenu.widgets` list remains accepted, including Home Assistant entity IDs.

## Editing

Use the pencil button to edit. Click a selected tile to disable it, or a grey
catalog tile to add it. Slots stay in place with plus/minus indicators; unadded
tiles can also be dragged and resized. Normal device controls are inactive while
editing.

Drag a tile's body to move it or its bottom-right corner to resize it. Previews
snap to the grid. Obstructed neighbors move right locally; unrelated rows do not
repack. Release commits. Escape or losing window focus cancels. Retracting a
resize restores the original placement instead of accumulating gaps.

Dropping between rows inserts a row. Tiles starting below it move down; tall
tiles crossing the insertion keep their starting row and height. On edit exit
or dismissal, disabled footprints are compacted horizontally, authored horizontal
gaps remain, and all vertically unoccupied rows are removed. Rows covered by tall
active tiles remain. Closing persists through the same coalesced save path.

A transparent gutter lets gestures continue beyond the visible card. Leaving
the outer surface cancels a gesture; clicking idle gutter space dismisses the
popup. The gutter does not change the card's visible placement or edge constraints.

Nightlight exposes sliders when its footprint has sufficient room (at least
4×2). Toggling never changes its footprint. Off-state sliders remain visible but
disabled. Right-click offers discrete brightness and temperature values at every
footprint.

## Developer boundaries

- `crates/bar/src/modules/system_menu/spatial.rs` owns `WIDGET_SIZE` and `SPACING`.
- Widgets own responsive rules and may share `neo_tile`.
- `neo_widgets::animation::geometry` provides rectangle motion and `PopupResize`.
  `popup_surface` separates surface capacity from visible bounds; `popup_gutter`
  handles inset content, gesture forwarding, and outside dismissal.
- `PopupResize` consumes actual root-window resize events. Growth reserves capacity
  before content motion. Shrink waits for `ShellEvent::Presented` and its
  `content_updated_at`; redraws and buffer submission are not presentation proof.
  Without presentation support, retain the larger envelope rather than guessing
  a delay. Do not use this coordinator for context menus.
- `utilities::LatestWrite` serializes range writes and coalesces intermediate
  values. Nightlight shares it between slider and context-menu commands.

## Development checks

Run inside the project's development environment:

```sh
cargo test -p config -p neo-widgets -p utilities -p bar
cargo check -p bar --all-targets
cargo fmt --all -- --check
```

`nix/tests/quick-tiles.nix` checks declarative row/gap rendering and legacy
compatibility.
