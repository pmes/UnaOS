# gneiss_pal — Gneiss Platform Abstraction Layer

The shared platform abstraction layer for UnaOS userspace: the common foundation
that handlers and vessels build on instead of re-implementing host services.

## Responsibilities

`gneiss_pal` collects the cross-cutting plumbing that more than one component
needs — talking to external services, locating on-disk state, persisting
conversation history, and a handful of pure utilities. Keeping these in one crate
prevents each handler from carrying its own copy of the same host-facing code.

Today the crate covers four concrete areas:

- **External service clients** — an LLM client (Google Vertex / Gemini) and a
  GitHub client.
- **Filesystem layout** — the canonical on-disk directory tree for UnaOS state.
- **Persistence** — saving and loading message history to JSON.
- **Contracts and utilities** — a memory-mapping trait, a UI handler trait, and
  small text helpers.

## Key public types and functions

| Item | Module | Description |
| --- | --- | --- |
| `ResilientClient` | `api` | Async LLM client for Google Vertex (`generate_content`, `embed_content`). Fetches credentials via `gcloud` ADC and retries once on a 401 by refreshing the token. |
| `Content`, `Part`, `FileData`, `UsageMetadata` | `api` | Serde request/response types for the Vertex content API. |
| `api::format::format_network_log` | `api::format` | Renders a raw JSON network-log line into a human-readable string for display. |
| `ForgeClient` | `forge` | GitHub client built on `octocrab`; reads `GITHUB_TOKEN`, exposes `get_user_info`, `list_repos`, `get_file_content`. |
| `UnaPaths` | `paths` | Resolves the UnaOS state tree (`root`, `vault`, `cortex`, `config`, …) honoring `UNA_ROOT` with per-OS defaults; `awaken()` creates the directories. |
| `BrainManager`, `SavedMessage` | `persistence` | JSON-backed load/save of a message log, plus an active-directive lookup. |
| `MemoryMappedRegion` | `io` | Pure-Rust trait for a contiguous mapped byte region (`as_slice`, `as_str`), decoupled from any OS mapping crate. |
| `AppHandler` | `app_handler` | Trait a UI handler implements: `handle_event(SMessage)` and `view() -> DashboardState`, bridging to the Bandy message bus. |
| `calculate_truncation` | `utils` | Computes the byte index at which to truncate text given line/character limits. |

`utils` and `app_handler` are re-exported at the crate root.

## Feature flags

The crate is `std`-by-default. The `std` feature (on by default) enables the
`reqwest` and `octocrab` dependencies and compiles the host-facing modules
(`io`, `paths`, `persistence`, `app_handler`, and the `ForgeClient`). The `api`
and `utils` modules are always available.

## How it fits into UnaOS

`gneiss_pal` is one of the shared libraries in `libs/`. Handlers (domain services
such as `vein`, `matrix`, `tabula`) and vessels (the runnable apps in `apps/`,
such as `lumen`) depend on it for host services rather than reimplementing them.
It depends on `bandy` — the inter-component message bus — so that an `AppHandler`
can consume `SMessage` events and return `DashboardState`. For the full picture
of vessels, handlers, libraries, and the Bandy / Quartzite / Synapse / Spline
layers, see [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
and the canon in [`docs/CODEX.md`](../../docs/CODEX.md).

## Status

Partial. The implemented surface — the Vertex and GitHub clients, `UnaPaths`,
`BrainManager`, the `AppHandler` / `MemoryMappedRegion` traits, and the text
utilities — is functional and in use by userspace components. The broader
"great library" role described in the CODEX (geometry, DSP, windowing, and a
unified networking/serial stack living in this crate) is design-stage and not
yet present in this code.
