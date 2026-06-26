# Tabula

Text and code editing for UnaOS. Tabula owns the "Text" capability area in the
handler manifest (see [`docs/CODEX.md`](../../docs/CODEX.md)): a lightweight
editor view for source files, prose, and read-only logs.

## Status

**Partial / view-only.** The crate currently provides an embeddable editor
widget built on GTK4 and GtkSourceView (`sourceview5`). It is **not yet wired to
the Synapse**: there is no `ignite(...)` entry point, and it neither subscribes
to nor emits any `SMessage`. A vessel embeds `TabulaView` directly today; bus
integration is future work.

## What it provides

The public API lives in `src/lib.rs`:

- **`EditorMode`** — selects the editor configuration:
  - `Code(String)` — monospace, line numbers, no wrapping; the `String` is a
    GtkSourceView language ID used for syntax highlighting.
  - `Prose` — proportional font, word wrapping, page margins.
  - `Log` — monospace, read-only, word wrapping (intended for log views).
- **`TabulaView`** — wraps a `sourceview5::View` inside a `ScrolledWindow`.
  - `TabulaView::new(mode)` — builds the view for the given `EditorMode` with
    auto-indent enabled.
  - `widget() -> gtk4::Widget` — returns the scrollable container for embedding
    in a host layout.
  - `load_file(&Path)` — reads a file into the buffer and selects a highlighting
    language from the file **extension**. On a read error it writes the failure
    message into the buffer instead of panicking.

Language detection in `load_file` is extension-based (`.rs`, `.toml`, `.md`,
`.py`, `.js`/`.ts`, `.json`, `.c`/`.h`/`.cpp`; otherwise plain text). Content
sniffing / magic-byte detection is not implemented.

## How it is meant to plug into the bus

Per the userspace architecture
([`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)),
a handler is a domain-service crate that exposes an async entry point (by
convention `ignite(...)`) and communicates over **Bandy** — the `SMessage` enum
carried on the **Synapse** broadcast bus — rather than calling other handlers
directly. Tabula does not yet implement this seam. When it does, it is expected
to react to open/save requests and surface its editor view to a vessel's GUI via
that bus; the relevant `SMessage` variants are not defined here yet.

## Dependencies

- `gtk4`, `sourceview5`, `glib` — the GUI toolkit and source editor backend.
- `elessar` (workspace/context detection) and `libspelling` (spell checking) are
  declared in `Cargo.toml` but not yet referenced from `src/`; they are reserved
  for prose spell-checking and project-context awareness.

## Notes

The `.una`-style canonical naming and the wider editor vision (embedding inside
Matrix previews and Principia config views, stdin piping, log streaming from
Midden) are design intent, not current behavior. This README tracks the code in
`src/lib.rs`.
