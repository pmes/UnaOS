# Obsidian

Obsidian is the UnaOS binary-inspection handler: a hex viewer/editor,
structure dissector, and lightweight disassembly view. It owns the "Binary"
capability area in the handler manifest (see
[`docs/CODEX.md`](../../docs/CODEX.md)) — the surface that opens when a vessel
encounters a file it cannot parse as text or media (a raw binary, a core dump, a
captured packet).

## Status

**Design-stage (not yet implemented).** This directory contains design notes
only — there is no `Cargo.toml`, no `src/`, and no code. Nothing described below
is built, and none of the wire format or APIs are fixed yet. The handler is not
a member of the workspace build and does not subscribe to the Synapse.

## What it will do

- **Hex view** — a scrollable hex/offset/ASCII grid over the file contents, with
  an entropy sidebar that colors regions by byte-randomness (low entropy for
  zero-fill and text, high for compressed or encrypted spans) as a coarse
  structural overview.
- **Format dissection** — recognize common binary layouts without executing
  them, using parsing services from `gneiss_pal`. Initial targets: ELF and PE
  (highlight headers, sections, and symbol tables) and `.pcap` capture files
  (highlight Ethernet / IP / TCP framing in the raw stream).
- **Non-destructive editing** — back the buffer by a memory-mapped (`mmap`) view
  of the file so large dumps open without a full read, and record edits as an
  overlay patch layer so the on-disk file is unchanged until an explicit save.
- **Lightweight disassembly** — for a selected byte range, produce a
  disassembled listing (a focused view, not a full reverse-engineering suite).
- **AI explanation** — forward a selected byte range to the `vein` (AI) handler
  over the bus and surface the returned natural-language explanation of an
  opcode sequence or structure.

## How it will plug into Bandy

Like the other handlers (see
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)),
Obsidian is intended to be a self-contained crate exposing an async entry point
(`ignite(synapse, …)`) that subscribes to the `Synapse` and reacts to
`SMessage`s rather than calling other handlers directly:

- **Consumes** — a request to open/inspect a path (the message that routes an
  unparsed file to Obsidian), and selection/analysis requests from the UI.
- **Emits** — view payloads for the hex grid and dissection overlays, and, for
  the explain feature, a prompt to `vein` whose `SMessage::AiToken(...)` reply
  the UI renders.

The concrete `SMessage` variants for these flows are not yet defined; adding
them is a deliberate, reviewed change to the `bandy` enum.

## Scope

Obsidian is a focused inspection and light-editing tool, not a full disassembler
or protocol analyzer. It is positioned to cover the common cases handled today
by tools such as Hex Fiend / HxD, `strings`, a packet-inspection view, and a
minimal Ghidra-style disassembly pane — at the depth a system inspector needs,
no more.

## See also

- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md) — userspace component model (vessels / handlers / libraries).
- [`docs/CODEX.md`](../../docs/CODEX.md) — the handler manifest.
- [`libs/bandy`](../../libs/bandy) — `SMessage` and `Synapse`.
