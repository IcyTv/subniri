# Fileberg folder identities and path entry

Fileberg gives Home and configured XDG user directories descriptive folder icons.
Common names such as `Downloads`, `Documents`, `Pictures`, `Music`, `Videos`,
`projects`, `src`, and `.config` also receive matching symbols. Names are matched
case-insensitively; configured locations use their actual paths, including
localized or relocated XDG directories.

Git repository folders receive a branch symbol. Both `.git` directories and `.git`
files (used by worktrees and submodules) are recognized. Detection happens during
directory loading, not rendering, and does not run Git commands.

## Custom roots

Add mappings to the shared Subniri configuration:

```kdl
fileberg {
    roots {
        root {
            path "~/work"
            label "Work"
            icon "briefcase"
        }
        root {
            path "/mnt/library"
            label "Library"
            icon "~/icons/library.svg"
        }
    }
}
```

The equivalent Home Manager setting is:

```nix
services.subniri.settings.fileberg.roots = [
  { path = "~/work"; label = "Work"; icon = "briefcase"; }
  { path = "/mnt/library"; label = "Library"; icon = "~/icons/library.svg"; }
];
```

Paths must be absolute or begin with `~/`, without `..` components. Labels must
be nonempty. Each path may appear only once. Custom icon paths follow the same
rules and must end in `.svg`. Files are read by the directory worker, with a
1 MiB limit. Missing or non-SVG artwork falls back to the folder symbol.
Custom SVG colors are preserved; built-in symbols follow the active palette.

Built-in folder icon names are:

`folder`, `house`, `desktop`, `download-simple`, `file-text`, `image`,
`music-notes`, `film-strip`, `users`, `copy`, `code`, `git-branch`, `pen-nib`,
`briefcase`, `smiley`, `archive`, `star`, `gear`, and `package`.

Configuration reloads update the current listing. Editing or adding an SVG file
itself requires refreshing the directory. The existing base/override configuration
rules still apply. These mappings have no graphical editor in Snowconf.

## Breadcrumbs and path entry

Breadcrumbs start at the deepest enclosing Home, XDG, Git, or custom root, unless
a deeper visible drive mount applies. At the same path, recognition priority is
custom mapping, Git repository, then Home/XDG. Common-name matches supply icons
only; a folder named `src` does not hide its repository root.

Root labels do not change filesystem names or navigation destinations. Parent
navigation can leave a displayed root; history continues to use absolute paths.

Press **Ctrl+L** or click the pencil button to replace breadcrumbs with a focused,
selected path field. **Enter** navigates to a readable directory; **Escape** or
the close button cancels. Absolute paths, `~`, `~/…`, and paths relative to the
current directory are supported. Spaces are literal; shell variables and command
substitution are not expanded. Invalid paths keep the editor open with an error
and do not alter navigation history.

While editing, a dropdown suggests matching directories and folder symlinks.
Matching ignores case but inserts the actual spelling. A bare name can also match
Home, an XDG location, or a custom root's label. Hidden folders appear only when
the last path component starts with `.`. Suggestions complete that last component
using prefix matching,
not fuzzy matching, and include up to 64 results in a scrollable dropdown.

Typing does not select a suggestion automatically. **Up/Down** selects one;
**Tab** fills the selected suggestion (or the first result) and adds `/` so you can
continue into a subfolder. Clicking a suggestion also fills rather than navigates.
**Enter** opens an explicitly selected suggestion; otherwise it opens the literal
typed path. With no suggestions, Tab retains normal focus traversal. Horizontal
caret keys and Shift+Tab retain their normal behavior. Escape still cancels editing.

Directory scans are debounced and run on workers. Obsolete results are discarded,
and suggestions never change the current directory or history by themselves.
This is a folder/path preview, not a preview of a folder's contents.

## File artwork

Document, spreadsheet, presentation, code, configuration, archive, media, font,
executable, and ebook formats receive distinct symbols. Existing extension labels
and bounded image thumbnails remain in place.

Symlinks retain target artwork or thumbnails and receive a bottom-left Phosphor
`link` badge. Unresolved targets, including link loops, use a warning-colored badge.
This presentation does not change copy, move, deletion, or symlink traversal rules.
