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
  - `Log` — monospace, read-only, word wrapping (the Console view's shape;
    `load_log` puts a view into it regardless of the mode it was built with,
    and a later `load_file` on a non-log restores the view's built mode, so the
    Console treatment never sticks to the pane).
- **`TabulaView`** — wraps a `sourceview5::View` inside a `ScrolledWindow`.
  - `TabulaView::new(mode)` — builds the view for the given `EditorMode` with
    auto-indent enabled.
  - `widget() -> gtk4::Widget` — returns the scrollable container for embedding
    in a host layout.
  - `load_file(&Path)` — reads a file into the buffer and selects a highlighting
    language from the file **extension**. On a read error it writes the failure
    message into the buffer instead of panicking. **Log paths are routed to
    `load_log` automatically** (see below).
  - `load_log(&Path)` — the Console view: renders a console/serial log
    read-only, monospace, unhighlighted.

Language detection in `load_file` is extension-based (`.rs`, `.toml`, `.md`,
`.py`, `.js`/`.ts`, `.json`, `.c`/`.h`/`.cpp`; otherwise plain text). Content
sniffing / magic-byte detection is not implemented.

## The Console view

Tabula's first incarnation of "the Console" is the editor itself, opened on a
log. There is no separate viewer and no new `ViewEntity`: the operator's flow
is *open this file*, and the log path in `src/logview.rs` makes that safe.

UnaOS logs are not ordinary text, so opening one naively is wrong in three
specific ways. Each is handled:

| Hazard | Why it exists | What Tabula does |
| --- | --- | --- |
| Trailing NUL padding | The kernel flight recorder reserves `UNAOS.LOG` at a **fixed** 256 KiB + 512 and writes the capture as its prefix (`unaos/crates/kernel/src/flight_recorder.rs`), so most of the file is zero padding. | Trimmed before decoding; the count is reported in `LogText::padding_bytes`. |
| Interior control bytes | Serial captures carry stray C0 bytes — this is why the house rule is to inspect logs with `awk`, not `grep`. A real 256 KiB `UNAOS.LOG` on this bench carries 21 of them. | Rendered as Unicode Control Pictures (`␀`, `␛`, `␡`): visible, counted, and inert. CR/CRLF normalise to LF; tabs and newlines pass through; ANSI CSI/OSC escape sequences are stripped. Escape scanning is **bounded**: a sequence ends at its terminator, at the first C0 byte (a newline is never inside one), or after `MAX_ESCAPE_LEN`, so a stray `ESC ]` off the cable cannot open a string that swallows the rest of the log. |
| Invalid UTF-8 | A byte mangled on the cable must not cost the whole file. | Decoded lossily; `LogText::lossy` records that it happened. |
| Unbounded size | The recorder is MBs-capable and a bench capture grows all session. | Loads are capped at `DEFAULT_MAX_BYTES` (4 MiB) keeping the **tail**, with a `:: TABULA: log truncated …` banner naming the elided and kept byte counts. The cap is applied to the log, *after* the padding trim, so a reservation cannot spend the budget on its own zeros; the kept region is advanced to the next line break only when that leaves something to show, so a single enormous line still opens (mid-line) instead of rendering as an empty buffer. |

A log document is **read-only by construction**, and the deeper reason is
*ownership*, not a rendering nicety. On the shard the kernel flight recorder
writes `UNAOS.LOG` as **root**; the UnaFS ownership ACL (`acl-<lba>-<off>` rows
carrying `owner`/`grants:*` — see [`docs/SECURITY.md`](../../docs/SECURITY.md))
refuses any write from a user-owned vessel, full stop. A user app *cannot* write
that log, so an app-level "editable" mode for it would be a lie.
`TabulaDocument::read_only` **mirrors** that filesystem fact for the host viewer:
it is set, `set_buffer` is inert, and `save()` returns `PermissionDenied`. (The
rendering fidelity point still stands on top — the buffer is a *rendering* of the
file, padding trimmed and control bytes made visible, so writing it back would
destroy the record even for a log an operator did own.)

The API in `src/logview.rs`:

- `is_log_path(&Path)` — a console/serial log by name, matched on **dot
  components** rather than as a substring: a `log` component anywhere after the
  stem (`UNAOS.LOG`, `s73-UNAOS.LOG.saved`, `ttyUSB0.log`, `ttyUSB0.log.1`) plus
  squawk `*.out` transcripts (final component only). `x.logic.rs`,
  `my.logrotate.conf` and `checkout.txt` are source, not logs — this predicate
  decides whether a file opens read-only, so an over-broad match locks files
  the operator meant to edit.
- `sanitize(&[u8]) -> LogText` — pure; every rule above, no I/O.
- `load_log(path)` / `load_log_capped(path, max_bytes)` — read + sanitize.
- `console_log_roots()` / `newest_log_in(&[PathBuf])` / `default_console_log()`
  — discovery: every removable mount point (`/run/media/*/*`, `/media/*/*`,
  `/mnt/*`) plus `~/unaos-bench/capture`, newest first. Only direct children of
  a root are considered.

On `TabulaDocument`: `open(path)` is the single entry point a vessel should
call — it routes log paths to `load_log` and everything else to `load`.

### Operator flow

The `una` vessel exposes this with **no flags** — a log is a path you open, not
a mode you type:

```
una <file>     # open any file, workspace anchored beside it
```

A `.log` / `UNAOS.LOG` / `*.out` path routes through `TabulaDocument::open` to
the read-only Console view; anything else opens editable. Activating one in
una's sidebar takes the same path. This is what reads a shard's flight recorder
after the card is mounted on the host.

There is deliberately no `--edit` escape hatch: name-based routing sends a log to
the read-only view because a log *is* read-only — on the shard it is root-owned
and the ACL denies writes (above), so "open it editable" is not a capability the
filesystem grants. An operator's own `notes.log` that they truly own is the
narrow edge this trades away; it is worth it not to pretend the app decides
writability.

Opening the **newest** log without naming a file is not `una`'s job either — it
belongs to the Console app tile (`handlers/comscan`, the future
`ViewEntity::Console`). The discovery seam it reuses lives here:
`default_console_log()` (below). `una` no longer wires that to any flag.

### Tests

`tests/console_view.rs` runs against **real captured bytes** — slices of this
bench's `UNAOS.LOG` and an FTDI capture, control bytes included, provenance in
`tests/fixtures/README.md`. The unit tests in `logview.rs` pin each rule
individually.

### Not this arc

Live follow (tail -f behaviour), level/source filtering, and log **structure**
(the `LogRecord` triple) are `handlers/comscan`'s `LogViewState`. This is the
static-file incarnation only.

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
