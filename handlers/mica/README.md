# Mica — structured-data handler

Mica is the UnaOS handler for structured, tabular data: a spreadsheet and
data-grid engine for editing and querying CSV, Parquet, and SQL-backed tables.

**Status: design-stage (not yet implemented).** This document describes the
intended design. There is no working code in this crate yet; nothing below
should be read as a description of shipped behavior.

## Responsibility

Mica owns the "structured data" capability area within UnaOS — the role played
by spreadsheets and lightweight SQL tools elsewhere. It is responsible for
loading tabular data, presenting it as an editable grid, evaluating cell
formulas, and keeping derived views consistent as the underlying data changes.

## Scope (planned)

- **Large-table grid.** Virtualized rendering and memory-mapped file access so
  that large CSV/Parquet files can be browsed without loading the entire file
  into memory. Capacity and frame-rate targets are design goals, not measured
  results.
- **Formula engine.** A spreadsheet-style formula language (`=SUM(A1:B2)`, etc.)
  evaluated over a dependency graph. Cell dependencies form a directed acyclic
  graph (DAG); updating a source cell cascades to its dependents rather than
  requiring a manual recompute. Monetary and precise values use a decimal type
  rather than binary floating point.
- **Derived views.** Non-duplicating views over a source table (filters, pivots,
  charts) that update when the source updates.
- **Headless mode.** A non-interactive path for processing a file from the
  command line, for use in scripts and pipelines.

## Integration: the Synapse / SMessage bus

Like other UnaOS handlers, Mica is a self-contained crate exposing an async
entry point (by convention `ignite(...)`). It does not call other handlers
directly. Instead it participates in the `bandy` message bus:

- It subscribes to the **Synapse** (a Tokio broadcast channel) via
  `subscribe()` and reacts to relevant **`SMessage`** variants.
- It publishes results back onto the Synapse with `fire(msg)`, where the GUI
  layer (`quartzite`) and other handlers can observe them.

Concretely, Mica is expected to consume storage and file-system messages
(e.g. `StorageQuery` / `StorageQueryResult`, and `Matrix` topology events) to
locate and load data, and to emit grid state and query results as `SMessage`s
for the GUI to render. The exact set of variants Mica produces and consumes
will be defined when the message contract is implemented; adding an `SMessage`
variant is a deliberate, reviewed change.

## Dependencies (intended)

- `bandy` — `SMessage` / `Synapse` and shared state types.
- `gneiss_pal` — host services (filesystem, paths, persistence) used to read
  and durably write table files.

## Roadmap

1. Virtualized grid over large CSV/Parquet files.
2. Core formula engine (`+ - * /`, `SUM`, decimal arithmetic, the dependency DAG).
3. Embedded scripting for complex transforms.
4. Charts and visualization views.

## See also

- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
  — userspace component model (libraries / handlers / vessels) and the
  Bandy bus.
- [`docs/CODEX.md`](../../docs/CODEX.md) — system canon and the full handler
  manifest.
