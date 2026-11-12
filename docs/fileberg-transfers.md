# Fileberg transfers

Drag selected files or folders onto a directory, the directory background, or a
mounted sidebar destination. Grid and list views use the same transfer engine.
Dragging a selected item preserves the selection. Hovering a destination opens it
after a delay; dragging near a scrollable edge scrolls the view.

Within Fileberg, the default is move on the same filesystem and copy between
filesystems. Ctrl requests copy; Shift requests move. Cross-application drags are
copy-only, including drags between separate Fileberg instances. Outgoing native
dragging starts when the pointer crosses the window edge. The drag image shows
the number of selected entries. Filesystem work runs outside the UI thread.

## Transfer options

Set the global policy in **Snowconf → Fileberg**. Normal dragging and ordinary
Copy/Cut use that policy. **Advanced Copy** and **Advanced Move** are context
submenus with Auto, Durable, and Buffered actions. Each action prepares files
with a per-operation policy, without changing the global setting or opening a
destination dialog. These actions have stable IDs for context-menu favorites.

Copy/Cut (Ctrl+C/Ctrl+X) prepares the selected files. Prepared copies remain
highlighted; prepared moves are greyed out. Navigate to a destination, then use
the bottom selection bar's Paste button or Ctrl+V. Context-menu Paste targets the
clicked folder, or the open directory when invoked on its background. Cancel in
the selection bar clears prepared files. Copies remain prepared for repeated
pasting; successful moves clear them. This prepared state is local to Fileberg,
not an implementation of the system-wide clipboard.

Active progress appears in a scrollable sidebar list above Drives after a short
delay, so brief operations do not flash a progress bar. Each entry shows its
phase, human-readable completed/total bytes, and estimated remaining copy time.
ETA starts with “Estimating…” and uses a recent copy-only throughput window after
at least two seconds of samples. Preparation, verification, flushing, rollback,
and conflict prompts do not contribute to that rate. ETA does not predict final
verification or flush time. Completed operations and recovery actions live in
the sidebar's History tab.

Same-filesystem moves rename entries rather than reading and copying their
contents. They still record undo information, check identity and change stamps,
and perform policy-required syncs. Preparation walks metadata, not file contents.
Copies hash source bytes while copying and verify both the surviving source and
staged contents before publication. Cancellation of an unpublished partial copy
removes its private staging data without hashing that data first. Published data and replacements
retain their guarded inverse-operation recovery paths.

- **Auto**, the default, uses buffered completion only for positively identified
  fixed local storage. Removable, network-backed, or unresolved storage requires
  completion flushing.
- **Durable** requires file and namespace flushing before completion and removal
  of move sources. This is subject to filesystem and hardware guarantees.
- **Buffered** accepts delayed writeback errors and loss after power, kernel, or
  device failure. A process exit alone does not ordinarily discard kernel cache.

Recovery data and operation journals retain durable ordering in every mode.
Flushing failures do not silently downgrade the policy. Known volatile storage
requires acknowledgement before a move; use copy when originals must survive.
Measured write speed adjusts writeback pacing, not storage classification.

## Conflicts and recovery

Conflicts offer Replace, Skip, Keep both, and Cancel. Directory conflicts also
offer Merge. Merge preserves unrelated destination entries. Delayed decisions
are revalidated before mutation. Ordinary conflict choices can apply to remaining
conflicts of the same kind; storage consent is never implicitly reused.

Replace retains the previous destination for undo. Same-filesystem private
storage is preferred. If that preservation cannot be made, Fileberg offers a
central backup under `$XDG_DATA_HOME/fileberg/recovery`. The conflict dialog also
provides an explicit Central backup choice. Central recovery checks capacity and
durably copies and verifies old data before replacing it. Capacity checks cannot
reserve space against other writers or guarantee quota availability.
Known volatile central storage is rejected; it cannot provide persistent undo.

Completed recovery history normally expires after seven days or when its retained
replacement data exceeds 5 GiB. A job needing more storage asks for explicit
approval. The allowance applies only to that job and still expires after seven
days. Active jobs and incomplete recovery are not expired to meet the budget.
Fileberg never purges the desktop Trash as recovery cleanup.

## Undo and cancellation

Use Ctrl+Z or History to undo Fileberg-owned transfers, including incoming copies.
Undo survives restarts. Ordinary copies do not retain duplicate backup data:
undo removes their unchanged output. Ordinary move undo transfers surviving data
back. Only overwritten destinations require retained copies of old contents.
Fileberg cannot undo work performed by another application receiving a drag.

Cancel stops forward work and attempts full rollback. A kernel-blocked flush
cannot safely be interrupted immediately. When changed files, occupied paths,
offline storage, or errors prevent restoration, Fileberg preserves surviving
data, attempts independent reversals, and retains the journal for retry. A
Needs recovery outcome is not a claim that everything was restored.

## Supported data and limits

Local file URIs and document-portal file transfers are supported. Outgoing portal
access uses identity-checked descriptors. Symlink portal exports are rejected
rather than silently exporting their targets; URI transport remains available.
Receiving a token or reaching MIME EOF is not filesystem completion.

Regular files, directories, and symlinks are supported. Symlinks are copied as
links, without recursive traversal. Device nodes, sockets, and named pipes are
not transfer inputs. Remote URLs are not downloads. Tabs, link creation, and
external moves are outside this implementation.

Transfers preserve basic permission bits and modification times where supported.
They do not promise `cp -a` equivalence: ACLs, extended attributes, ownership,
hard-link topology, sparse allocation, and filesystem-specific metadata are not
preserved as a complete archival representation. Copy/replacement content hashes,
rename identity/change stamps, and pinned identities detect changes but are not
a filesystem-wide snapshot against
arbitrary concurrent writers. New entries are never recursively erased as part
of deleting an older recorded directory tree.
