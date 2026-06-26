# Aether — documentation and web retrieval handler

Aether is the UnaOS handler responsible for reading and rendering documents:
HTML, Markdown, and PDF, plus local system documentation. It is a read-only
viewer, not a general-purpose browser.

**Status:** design-stage (not yet implemented). This document describes the
intended design. The crate currently contains no working code; the entry point
and message contract below are the planned interface, not a shipped one.

## Responsibility

Render documents and retrieve reference material — local manuals and remote web
pages — as plain, readable content. Aether is the "Reader": it presents a
document's text and structure and deliberately omits the dynamic application
layer of the modern web.

## Scope

- **Read-only rendering.** Aether parses and lays out HTML, Markdown, and PDF.
  It supports the subset of CSS needed for clean document layout and does not
  execute page JavaScript (no JIT). Any scripting, if added later, is expected
  to run sandboxed and only on explicit user authorization.
- **Reader-first presentation.** Content is rendered as hypertext: body text,
  headings, links, and images, with advertising and non-semantic layout
  elements stripped.
- **Unified local + remote search.** Aether is intended to treat local
  documentation (the `principia` system manuals, `gneiss_pal` API docs, man
  pages) and the remote web as one searchable corpus, resolving local sources
  before reaching the network.
- **Snapshots.** A retrieved page can be frozen and stored for offline reference
  in a `geode` archive.

Network fetches and filesystem access are expected to go through `gneiss_pal`
(its `net` and `fs` modules) rather than being re-implemented in the handler.

## Integration with the message bus

Like every UnaOS handler, Aether is a self-contained crate exposing an async
entry point (by convention `ignite(...)`) and communicates only over the
**Bandy** message bus (`libs/bandy`); it does not call other handlers directly.

- It subscribes to the **Synapse** — the shared Tokio broadcast channel — via
  `subscribe()` and reacts to relevant **SMessage** variants (e.g. a request to
  open or fetch a document).
- It publishes results back onto the Synapse with `fire(msg)` — rendered content
  and metadata for the GUI (Quartzite) to display.
- A new SMessage variant for Aether's request/response traffic is a deliberate,
  reviewed addition to the shared `SMessage` enum.

## Relationship to Vein

Aether is intended to serve as the retrieval layer for the **Vein** handler (AI
integration). When Vein answers a query, Aether fetches the underlying source
material; Vein summarizes it and Aether supplies the citations and the full
document for inspection.

## See also

- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
  — the handler/vessel/Bandy component model.
- [`docs/CODEX.md`](../../docs/CODEX.md) — the full handler manifest.
