# Matrix

Matrix is the UnaOS workspace-topology handler. It scans a project's source
tree, derives the dependency structure between files, and publishes that
structure on the message bus so the GUI can render a navigable map of the
workspace and the AI handler (Vein) can use it as context.

**Status: implemented (focused scope).** The lexical scanner, workspace
indexer, and async event loop are working and wired into the `lumen` vessel and
the `vein` handler. The scope is intentionally narrow: Matrix currently models
Rust source topology. Richer asset/preview features described in earlier design
notes (3D model preview, media scrubbing, tag-based smart collections) are not
implemented.

## What it does

- **Topology scan** — `MatrixScanner::map_topology(paths, root, depth)` walks one
  or more target paths, parses each `.rs` file lexically (comment stripping plus
  `use` / `mod` / symbol extraction; no full AST), and builds a deduplicated
  dependency graph. It returns two artifacts: a compact `DICTIONARY$EDGES`
  payload for the UI, and a human-readable "semantic code topology" string for
  LLM context. `ScanDepth::Interface` captures public symbols; `ScanDepth::DeepAST`
  captures all definitions including `impl` blocks.
- **Genesis tree** — `MatrixScanner::build_genesis_tree(dir, root)` produces the
  nested `bandy::state::TopologyNode` tree the GUI renders, pruning `target`,
  `.git`, `node_modules`, and any directory containing no `.rs` files.
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
