# Matrix

Matrix is the UnaOS workspace-topology handler. It scans a project's source
tree, derives the dependency structure between files, and publishes that
structure on the message bus so the GUI can render a navigable map of the
workspace and the AI handler (Vein) can use it as context.

**Status: implemented (all-asset genesis, Rust-aware analysis).** The lexical
scanner, workspace indexer, and async event loop are working and wired into the
`lumen` vessel and the `vein` handler. The genesis tree maps ALL assets in the
workspace — every regular file (code, docs, images, config) becomes a node —
per Matrix's charter as the spatial all-asset manager; the deeper dependency
analysis (`map_topology`) is currently Rust-source only. Richer asset features
described in the charter (previews, media scrubbing, tag-based smart
collections) are not yet implemented.

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

## How it plugs into the bus

Matrix is a domain handler in the sense of the userspace architecture: a crate
exposing an async entry point that subscribes to the Synapse and reacts to
`SMessage`.

`ignite(synapse, absolute_workspace_root)` subscribes to the Synapse and runs an
event loop keyed on the workspace root (shared as an `Arc<PathBuf>`):

**Consumes**
- `SMessage::Matrix(MatrixEvent::FocusSector(targets))` — a space-separated list
  of workspace-relative targets to scan.

**Emits** (via `Synapse::fire_async`)
- `MatrixEvent::GraftTopology { target_id, payload }` — for a single-file scan,
  the symbol payload to graft onto an existing UI node.
- `MatrixEvent::SectorFocused { target, context }` — the semantic topology for a
  single file, for LLM context.
- `MatrixEvent::IngestTopology { ui_dag, semantic_dag }` — for a multi-target
  scan, the full UI payload plus semantic graph (consumed by Vein/the GUI).

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

## Dependencies

`bandy` (bus + shared topology state), `elessar` (workspace-root detection),
`gneiss_pal` (platform services). An optional `gtk` feature pulls in `gtk4` /
`glib` for the Linux view layer.
