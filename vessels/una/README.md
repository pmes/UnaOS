# Una (UnaIDE)

The developer-environment vessel for UnaOS: a code-first workspace assembled
from handlers rather than a monolithic IDE. Una is an instance of the Elessar
workspace runtime resolved into a `Context::Code` layout — it contributes no
editor or terminal logic of its own; it acts as the host that composes the
relevant handlers into one development environment.

This README covers the **`vessels/una`** IDE vessel. It is not a handler and is
not one of the 21 locked handler names in [`docs/CODEX.md`](../../docs/CODEX.md);
it is a source vessel (see the vessel definition in
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)).

## What it is

In UnaOS terminology a **vessel** is an executable a user runs, composed at
startup from shared libraries and capability handlers rather than bundling one
feature set. Una is the code-project vessel:

- **State.** Workspace state (`WorkspaceState`, `AppState`, `ViewEntity`) is
  supplied by `libs/bandy`; signalling rides `libs/bandy`'s `Synapse` broadcast
  bus (the same seam `vessels/lumen` uses).
- **Window.** Uses `libs/quartzite` to render the native window frame. On macOS
  this is the AppKit `Backend`/`Spline` seam; the GTK4 path is Linux-only
  (`cfg(target_os = "linux")` deps in `Cargo.toml`).
- **Composition.** Uses `libs/elessar` to resolve the layout: `Context::new(cwd)`
  reads the working directory, `.layout()` maps it to a `Layout`, and
  `workspace_for(layout, roots)` builds the `WorkspaceState`. A recognized
  project directory resolves to `Layout::Code` (Topology left, Editor right); an
  unrecognized directory resolves to `Layout::Comms` (Topology left, Stream
  right). The genesis tree that `matrix` scans becomes the left Topology.

## Invocation

```
una            # workspace at the cwd
una <dir>      # workspace at <dir>
una <file>     # open <file>, workspace anchored at its parent
```

`una` takes at most one **positional** argument and **no option flags**.
Opening a log is not a mode you type: point the app at a log — `una UNAOS.LOG`
— and `TabulaDocument::open` name-routes it to the read-only Console view on its
own. Any other file opens editable. (The earlier `--console` / `--edit` flags
are gone: a Unix-style flag to open a log is the wrong idiom for a spatial
facade OS, and `--edit` was a lie — a kernel log is root-owned and cannot be
written no matter what argv says.)

A named file **overrides** the Layout's choice of right pane: a console log
lives on a mounted FAT volume or in a capture directory, neither of which
resolves to a `Layout::Code` project, so the Editor is asked for explicitly and
its `EditorState` is seeded before the first frame.

### Why a log is read-only — and why it is not a flag

The read-only Console treatment is the file's **ownership**, not an app policy.
On the shard the kernel flight recorder writes `UNAOS.LOG` as *root*, and the
UnaFS ownership ACL (`acl-<lba>-<off>` rows carrying `owner`/`grants:*` — see
[`docs/SECURITY.md`](../../docs/SECURITY.md)) refuses any write from a
user-owned vessel. `una` renders the record; it could not write it if it tried.
Attempting Cmd+S on a Console view is refused and echoed to the console pane as
`[una] <path> is a kernel log, owned by root — read-only; the shard's ACL
refuses writes, not this app`.

### Summoning the Console app (the facade-native path)

Opening the **newest** log without naming a file is the **Console app's** job,
not a command-line flag. That app is the macOS-`Console.app` equivalent backed
by `handlers/comscan`; on glass it is a desktop tile rendering the future
`ViewEntity::Console` (see comscan's
[README](../../handlers/comscan/README.md)). Its discovery seam already exists —
`tabula::default_console_log` finds the newest log across mounted volumes and
`~/unaos-bench/capture`, and `tabula`'s `logview` renders it — but wiring the
`ViewEntity::Console` variant and its AppKit/GTK widget lives in `libs/bandy` +
`libs/quartzite`, outside this vessel's lane. Until that tile lands, the
host-dev open path is the positional `una <log>` above.

The log rendering rules (NUL padding, control bytes, size cap, read-only) live
in `handlers/tabula`; see its [README](../../handlers/tabula/README.md).

## Composed handlers

Una binds the following handlers into a single workspace layout. Integration is
in bring-up; the table reflects current truth on the `UnaOS-unaide` branch.

| Handler           | Role                                              | Status                                                                 |
| ----------------- | ------------------------------------------------- | ---------------------------------------------------------------------- |
| `handlers/matrix` | Files — workspace navigation, topology grafting.  | **Live.** Dependency wired; scans the genesis tree and serves the left Topology. |
| `handlers/tabula` | Editor — document core, syntax highlighting, Console view. | **Live (core).** Portable `TabulaDocument` core is wired; files load into the Editor pane via `TabulaDocument::open`, which routes console/serial logs into the read-only Console view. GTK view is feature-gated (`--features gtk`, off by default). |
| `handlers/midden` | Terminal — shell emulation, process management.   | Pure `Midden::execute()` core is ready; console pane wiring in flight (not yet a dependency). |
| `handlers/aule`   | Builder — Cargo wrapper and task runner.          | Pure `Aule::forge()` / streamed-forge core is ready; not yet a dependency. |
| `handlers/vaire`  | Version control — Git graph, diff view.           | Builds green on macOS (SMessage-ported); una integration pending.       |

The GTK-bearing handlers were de-GTK'd (`528c213`): their GTK views moved behind
an optional `gtk` feature (default off) so their portable cores build on macOS.

## Status

**Alpha bring-up on macOS.** Running `cargo run -p una` opens a native window
with a live file tree (matrix Topology) and a click-to-load editor: activating a
file in the sidebar loads it through the portable `TabulaDocument` core and
renders it in the Editor pane (`Layout::Code`). The brain loop is minimal — no
Vein/AI cortex — and serves the structural signals a bare workspace needs
(`UiReady`, `ToggleMatrixNode`, `EditorLoad`). The console pane (midden) and the
version-control panel (vaire) are the next integration steps.

## See also

- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
  — the vessel / handler / library model and the Elessar runtime.
- [`docs/CODEX.md`](../../docs/CODEX.md) — the handler manifest and the Elessar
  protocol.
