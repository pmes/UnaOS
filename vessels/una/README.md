# Una (UnaIDE)

The developer-environment vessel for UnaOS: a code-first workspace assembled
from handlers rather than a monolithic IDE. Una is an instance of the Elessar
workspace runtime frozen into a `Context::Code` layout — it contributes no
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

- **State.** Uses `libs/gneiss_pal` for headless workspace state.
- **Window.** Uses `libs/quartzite` to render the native (GTK4) window frame.
- **Composition.** Uses `libs/elessar` to manage the layout (the Spline) and to
  bind handlers according to the `Context::Code` context.

## Composed handlers

Una binds the following handlers into a single workspace layout:

| Handler          | Role                                                      | Location     |
| ---------------- | -------------------------------------------------------- | ------------ |
| `handlers/tabula` | Editor — syntax highlighting (tree-sitter), multi-cursor, LSP. | Top right    |
| `handlers/midden` | Terminal — shell emulation, process management.          | Bottom right |
| `handlers/vaire`  | Version control — Git graph, diff view, commit interface. | Left panel   |
| `handlers/matrix` | Files — workspace navigation.                            | Left panel   |
| `handlers/aule`   | Builder — Cargo wrapper and task runner.                 | Left panel   |

## Status

**Pre-alpha.** Depends on `libs/elessar` and the handlers above being present in
the workspace.

## See also

- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
  — the vessel / handler / library model and the Elessar runtime.
- [`docs/CODEX.md`](../../docs/CODEX.md) — the handler manifest and the Elessar
  protocol.
