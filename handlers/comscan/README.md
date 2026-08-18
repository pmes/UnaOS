# Comscan

The hardware I/O and signal handler for UnaOS: a bridge between the workspace
and external hardware over serial, GPIO, Bluetooth, and software-defined radio.

**Status: partially implemented.** The crate now exists (`Cargo.toml`, `src/`,
`ignite(...)`), and its **first landed capability is the Console app** — the
system log viewer described under [The Console app](#the-console-app-macos-consoleapp-equivalent)
below. The serial / GPIO / Bluetooth / SDR scope in the next sections remains
**design-stage**: a specification, not working code.

## The Console app (macOS `Console.app` equivalent)

Comscan is the telemetry/serial handler, and the OS's log viewer belongs here:
a ring-3 program that **subscribes to the system log feed**, keeps a bounded
scrollback, and publishes a filtered, tailing view for a vessel to render. This
is the correct model for reading logs — the kernel console is plumbing behind
the facade, not a desktop window to manage; the app is a *subscriber*, never a
pinned raw console.

**The log feed — host vs metal.** The ring-3 app is transport-agnostic; only the
ingest edge swaps:

- **Host (today).** Every component that logs fires a
  `bandy::SMessage::Log { level, source, content }` onto the Synapse bus — the
  same bus every other host-native handler already reads. The Console app
  subscribes to that one feed; there is no separate log daemon.
- **Metal (the swap).** The kernel's `TERM_RING`
  (`unaos/crates/kernel/src/termring.rs`) is the console-output stream. Over a
  real kernel, each drained `TERM_RING` line becomes one `LogView::on_log(..)`
  call in place of an `SMessage::Log`. Nothing else changes — the same swap the
  matrix Finder documented for its FAT↔UnaFS backing.

**Bounded scrollback.** `logview::LogView` holds at most `cap` records in a ring
(`DEFAULT_SCROLLBACK` = 4096). It is **drop-OLDEST** — the opposite of
`TERM_RING`, which is a transport and drops the newest. A scrollback must keep
showing the present, so on overflow it evicts the oldest line and **counts the
eviction** (`dropped`), so a truncated view is visibly truncated — the same
counted-loss honesty `termring` keeps.

**Command / render contract** (`bandy::LogEvent`, carried under
`SMessage::Logs`), distinct from the `SMessage::Log` producer message:

- view→handler: `LogFilter(String)` (case-insensitive substring over
  content/source/level; empty clears), `LogSource(LogSource)` (facet by
  subsystem; `All` shows everything), `LogPause(bool)` (scroll-lock — the ring
  keeps ingesting, the view freezes).
- handler→view: `LogTail { lines, dropped, paused }` — the single bounded,
  filtered snapshot the vessel draws.

**The vessel view is WIRED (GR26).** `bandy::state::LogViewState` is the render
seam, and `bandy::state::ViewEntity::Console(LogViewState)` is now a first-class
pane. `LogView::view_state()` snapshots this handler's scrollback into that
payload; the Qt/GTK tetra bridge (`libs/quartzite/src/tetra.rs`) converts it —
`ConsoleTetra::from_log_view` — running every line through Tabula's log
sanitizer, so a stray control byte off the cable is *shown* (as a Control
Picture), never obeyed. The GTK render (`libs/quartzite/src/platforms/gtk/
console_view.rs`) is a read-only, monospace, live view: it seeds from the
snapshot and follows the feed, re-rendering on each `SMessage::Logs(LogTail)`
this handler publishes. Read-only by ownership — the pane has no input field and
the log is root-owned on the shard; the view only renders records.

**Summoned facade-natively — not a command-line flag.** The Console opens on a
gesture, the way macOS opens `Console.app`; it is emphatically not `una
--console` (those flags were removed — a Unix-style flag to open a log is the
wrong idiom for a spatial facade OS). `una` ignites this handler (the live feed)
and wires the summon: **Ctrl+`** on the host window pops the read-only Console
window that follows the feed (`quartzite::install_console_summon` /
`open_console_window`). A shell tile/menu that wants the same effect fires the
same summon. For opening a specific *named* log read-only, the static-file path
is still `handlers/tabula`'s `logview` renderer, reached with `una <log>`; the
newest-log discovery seam is `tabula::default_console_log`. The live/tailing feed
here is the richer, metal-facing successor to that static view.

Proofs live in `tests/console.rs` (bounded scrollback, live tail, pause/resume,
text + source filters, the `serve` bus round-trip) and in the `kat_logs_*`
golden KATs in `libs/bandy/tests/smessage_kats.rs` (the frozen wire shape).

## Responsibility

Comscan owns direct communication with hardware interfaces. Where most of the
system deals in files, workspaces, and rendered views, Comscan deals in raw byte
streams, link parameters, and wireless device discovery. It is the handler other
parts of the system use when they need to talk to a physical device — for
example, streaming a generated CNC/3D toolpath to a controller over USB serial.

## Scope (planned)

- **Serial / UART** — a terminal and byte pipe for microcontrollers and
  controllers (Arduino, ESP32, STM32, 3D-printer/CNC firmware), with baud-rate
  detection and a hex/ASCII view of raw traffic.
- **GPIO** — read/write of general-purpose I/O lines on supported hardware.
- **Bluetooth** — discovery and inspection of BLE devices, including raw
  advertisement data; pairing key material is delegated to the `holocron`
  secrets handler rather than stored by Comscan.
- **Software-defined radio (SDR)** — spectrum capture and demodulation for
  diagnostic and sub-GHz protocol work.

Comscan is intended to build on the serial/signal stack in `gneiss_pal`
(`src/net`) rather than re-implement host transport itself.

## Integration with the Synapse / SMessage bus

Like every UnaOS handler, Comscan is a self-contained crate that will expose an
async entry point (by convention `ignite(...)`), subscribe to the `Synapse`
broadcast bus, and react to `SMessage` variants. It does not call other handlers
directly. The planned message flow:

- **Inbound** — Comscan subscribes via `Synapse::subscribe()` and acts on
  commands addressed to it: open/close a port, set link parameters, write a byte
  stream to a device, start/stop a scan.
- **Outbound** — Comscan publishes via `Synapse::fire(msg)`: device-discovery
  results, received serial/wireless data, and link-status changes, for the GUI
  and other handlers to observe.

Dedicated `SMessage` variants for Comscan's commands and events are not yet
defined; adding them is a deliberate, reviewed change to the shared `bandy`
enum.

## Relationship to other handlers

- **Vug** (3D/CAD/CAM) generates toolpaths; Comscan streams them to the machine.
  This is the "design → make" path with no intermediate slicer or export step.
- **Holocron** holds pairing keys and other secrets; Comscan defers all key
  storage to it.

## See also

- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
  — the handler / Synapse / SMessage model.
- [`docs/CODEX.md`](../../docs/CODEX.md) — the full handler manifest.
