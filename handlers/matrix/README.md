# Matrix

Matrix is the UnaOS workspace-topology handler. It scans a project's source
tree, derives the dependency structure between files, and publishes that
structure on the message bus so the GUI can render a navigable map of the
workspace and the AI handler (Vein) can use it as context.

**Status: implemented (all-asset genesis, Rust-aware analysis, Finder browser).**
The lexical scanner, workspace indexer, async event loop, and the Finder
file-browser capability are working; the scanner/genesis paths are wired into
the `lumen` vessel and the `vein` handler. The genesis tree maps ALL assets in
the workspace — every regular file (code, docs, images, config) becomes a node —
per Matrix's charter as the spatial all-asset manager; the deeper dependency
analysis (`map_topology`) is currently Rust-source only. The **Finder** (see
below) adds navigable, spatial file browsing and the Finder verbs on top of the
same anchored workspace root. Richer asset features described in the charter
(previews, media scrubbing, tag-based smart collections) are not yet implemented.

## What it does

- **Topology scan** — `MatrixScanner::map_topology(paths, root, depth)` walks one
  or more target paths, parses each `.rs` file lexically (comment stripping plus
  `use` / `mod` / symbol extraction; no full AST), and builds a deduplicated
  dependency graph. It returns two artifacts: a compact `DICTIONARY$EDGES`
  payload for the UI, and a human-readable "semantic code topology" string for
  LLM context. `ScanDepth::Interface` captures public symbols; `ScanDepth::DeepAST`
  captures all definitions including `impl` blocks.
- **Genesis tree** — `MatrixScanner::build_genesis_tree(dir, root)` produces the
  nested `bandy::state::TopologyNode` tree the GUI renders. Every regular file
  is a node; `target`, `.git`, `node_modules`, and empty directories are
  pruned, and symlinks are never followed.
- **Graft decode/apply** — `graft::apply_graft(roots, target_id, payload)`
  decodes a `GraftTopology` payload and replaces the target node's children
  with the scanned symbols; vessels call it and re-render on `true`.
- **Workspace indexing** — `indexer::WorkspaceIndexer` recursively scans for
  `Cargo.toml` files and builds a crate-level dependency DAG (`CrateNode`), used
  by Vein's workspace cortex.
- **Finder (file browser)** — `finder::Finder` is a navigable CURSOR over the
  filesystem: `list(rel)` returns one directory's immediate children as a
  `bandy::state::BrowseListing` (dirs first, then files, each alphabetical; with
  parent + breadcrumbs for ascent/descent), and the Finder verbs `open`,
  `new_folder`, `rename`, `copy`, `mv`, `delete` each execute against `std::fs`
  and answer a `bandy::state::FsOutcome`. It is distinct from the code-topology
  DAG: the Finder shows files (including empty dirs), the DAG shows dependency
  structure (and prunes empty dirs). See "The Finder" below.

## The Finder

The Finder is Matrix's file-browser capability — a Mac-Finder-style navigable
view, rendered as a flat file list/grid rather than the dependency graph. It is
an ADDED mode on the existing genesis tree, not a replacement: `build_genesis_tree`
is untouched and its tests stay green.

**Why a per-directory cursor, not a re-scan.** `build_genesis_tree` recurses the
WHOLE subtree and prunes empty directories — correct for a code map, wrong for a
file browser (a Finder must show an empty folder, and must not pay a full-subtree
walk to step one level). So `Finder::list` does a single `read_dir` of the target
directory: the right weight for a cursor, and it deliberately does not prune.

**Navigation model.** Paths are workspace-relative (`""` = the anchored root).
A `BrowseListing` carries `path`, `parent` (`None` at the root — ascent stops
there), `breadcrumbs` (root → current), and `entries`. Descend = list a child;
ascend = list `parent`.

**Sandbox + symlink law.** `Finder::resolve` anchors every path under the
workspace root, rejects `..`/absolute escapes, and never follows a symlink
component — the genesis scan's symlink law, extended to navigation and every op.
Symlinks are shown in listings (flagged, classified by their own type) but never
descended.

**The verbs, principal-attributed.** Each verb is a `bandy` event carrying the
caller's `Origin` principal, so an in-kernel fulfilment would run with the
invoker's grants, never ambient authority (ROADMAP message-security law). The
handler stamps and logs the principal on every request. Verbs:

| Verb | `std::fs` op | Notes |
| --- | --- | --- |
| `Open` | validate (no write) | file → `Ok`; a directory is refused (navigate instead). |
| `NewFolder` | `create_dir` | into an existing dir; name validated (no separators/traversal). |
| `Rename` | `rename` | same parent; a separator in the new name is refused. |
| `Copy` | `copy` / recursive | file or dir; refuses copying a dir into itself. |
| `Move` | `rename` | into a destination dir; refuses moving a dir into itself. |
| `Delete` | move-to-trash | needs `confirmed`; REVERSIBLE — moves to `.una-trash/`, never hard-deletes. |

**Destructive-action discipline.** `Delete` with `confirmed: false` answers
`FsOutcome::NeedsConfirm` (the UI re-issues with `confirmed: true`); a confirmed
delete moves the target into the workspace `.una-trash/` (timestamp-prefixed) so
it is recoverable — nothing is ever permanently destroyed.

**Loud read-only refusal (FAT-verb posture).** A write a read-only volume or a
permission-denied directory refuses surfaces as `FsOutcome::Denied` — loudly,
never a silent no-op and never a generic `Error`. `Error` is reserved for
genuine, non-policy failures.

### FAT / UnaFS-on-metal mapping (design note)

The host build uses `std::fs`; on metal the same Finder maps onto the kernel's
typed filesystem verbs (`docs/dev/OS/09_FILESYSTEM/vfs.md`) with no change to the
event vocabulary:

- **Reads follow the program source.** `list`/`open` become the read-side verbs
  (`ls`/`cat`-shaped), each KERNEL-stamped with the caller's `IMAGE_SHA256`
  principal — exactly the principal the `Origin` field already carries on the
  bus. A caller-supplied principal is `-EINVAL`, never overwritten, so the
  host-side `Origin` is advisory and the kernel is the authority.
- **Writes refuse loudly on read-only volumes.** `new_folder`/`rename`/`copy`/
  `move`/`delete` become the write-side verbs. Where the host maps
  `PermissionDenied`/`EROFS` to `FsOutcome::Denied`, the kernel answers `-ENOTSUP`
  *before* it touches the block path (the read-only USB mount posture, PIUSB-27):
  same loud refusal, same `Denied` surface on the bus.
- **Every verb is ACL-checked under the invoker's grants** (U6/K1/K2 owner +
  `grants:*`), so the `Origin` stamp is load-bearing, not decorative: it is the
  principal the on-disk ACL is evaluated against.
- **Delete → trash** maps to an `unlink` into a trash catalog entry (or a
  future typed "trashed" attribute) rather than a destructive `remove`, keeping
  the reversible posture on metal.

The Finder therefore needs no new metal-specific event — the same `BrowseTo` /
`FileOp` / `DirListed` / `FsOpResult` vocabulary rides the bus; only the
implementation under it swaps `std::fs` for the kernel verb bridge.

## How it plugs into the bus

Matrix is a domain handler in the sense of the userspace architecture: a crate
exposing an async entry point that subscribes to the Synapse and reacts to
`SMessage`.

`ignite(synapse, absolute_workspace_root)` subscribes to the Synapse and runs an
event loop keyed on the workspace root (shared as an `Arc<PathBuf>`):

**Consumes**
- `SMessage::Matrix(MatrixEvent::FocusSector(targets))` — a space-separated list
  of workspace-relative targets to scan.
- `SMessage::Matrix(MatrixEvent::BrowseTo { principal, path })` — Finder: list a
  directory. Answered by `DirListed` (or `FsOpResult { Denied }` on refusal).
- `SMessage::Matrix(MatrixEvent::FileOp { principal, verb, path, arg, confirmed })`
  — Finder: run one file verb. Answered by `FsOpResult` (plus a refreshed
  `DirListed` on a successful mutation).

**Emits** (via `Synapse::fire_async`)
- `MatrixEvent::GraftTopology { target_id, payload }` — for a single-file scan,
  the symbol payload to graft onto an existing UI node.
- `MatrixEvent::SectorFocused { target, context }` — the semantic topology for a
  single file, for LLM context.
- `MatrixEvent::IngestTopology { ui_dag, semantic_dag }` — for a multi-target
  scan, the full UI payload plus semantic graph (consumed by Vein/the GUI).
- `MatrixEvent::DirListed(BrowseListing)` — Finder: the browse-view listing of
  the current directory (the flat file list/grid the vessel renders).
- `MatrixEvent::FsOpResult { principal, verb, path, outcome }` — Finder: the
  principal-attributed result of a file verb.

The handler does not call other handlers directly; all interaction is through
`SMessage` on the Synapse. `MatrixScanner` and `WorkspaceIndexer` are also used
synchronously by `vein` (workspace cortex) and `lumen` (initial genesis tree).

## Entry points

| Item | Role |
| --- | --- |
| `ignite(synapse, root)` | Async handler loop; subscribes and reacts to `FocusSector`. |
| `MatrixScanner::map_topology(...)` | Lexical dependency scan → `(ui_payload, semantic_dag)`. |
| `MatrixScanner::build_genesis_tree(...)` | Build the `TopologyNode` tree for the UI. |
| `indexer::WorkspaceIndexer` | Crate-level dependency DAG from `Cargo.toml` files. |
| `finder::Finder` | File-browser cursor: `list` + the file verbs, sandboxed to the root. |
| `finder::Finder::dispatch(event)` | Map a `BrowseTo`/`FileOp` request to the events to publish. |

## Vessel view (design — not yet wired)

The browse view is a NEW on-glass surface (a file list/grid distinct from the
topology tree widget). Rendering it lives in `quartzite`'s per-platform view
layer (GTK `translator.rs`, Qt `window.rs`, macOS `spline.rs`) — outside this
handler's lane — so the handler, events, and tests land here complete and the
vessel wiring is designed, not built:

- **State seam.** `WorkspaceState` already carries `ViewEntity` panes
  (`Topology`, `Stream`, `Editor`). Add `ViewEntity::Browse(BrowseListing)` (or a
  small `BrowseState { listing, selection }`) as a fourth pane kind. The genesis
  tree keeps its `Topology` pane; the Finder is a *mode toggle* to a `Browse`
  pane, so the DAG map is never displaced.
- **Down (render).** On `DirListed`, the vessel replaces the Browse pane's
  listing and repaints the list/grid (icon + name + size, dirs first). Crispy
  density: one dense row per entry, breadcrumb bar at the top, no wasted chrome —
  a Finder's density, not its pixels.
- **Up (intent).** A row double-click fires `BrowseTo` (dir) or `FileOp { Open }`
  (file); a context menu / toolbar fires `FileOp { NewFolder | Rename | Copy |
  Move | Delete }`. The vessel stamps the local user's `Origin` as principal.
  A `FsOpResult { Denied | Error }` shows an inline banner (the loud refusal);
  `NeedsConfirm` raises the delete confirmation, which re-fires `Delete` with
  `confirmed: true`.
- **Editor bridge.** `FsOpResult { verb: Open, outcome: Ok }` is the vessel's cue
  to read the file and fire `EditorLoad` — the Finder resolves + attributes;
  the editor owns the read.

## Dependencies

`bandy` (bus + shared topology state), `elessar` (workspace-root detection),
`gneiss_pal` (platform services). An optional `gtk` feature pulls in `gtk4` /
`glib` for the Linux view layer.
