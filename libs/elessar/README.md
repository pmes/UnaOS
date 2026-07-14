# Elessar

Workspace and context detection for UnaOS userspace. Given a directory, Elessar
locates the workspace root and classifies the project type.

## Responsibilities

Elessar is pure logic with no user-interface dependencies. It answers two
questions about a filesystem location:

1. **Where is the workspace root?** Starting from the current working directory,
   it walks up the directory tree looking for a project anchor.
2. **What kind of project is this?** It inspects a directory for well-known
   marker files and reports a classification.

The classification — called a **Spline**, the project's type or trajectory — is
the input other userspace components use to decide how to present a workspace
(for example, which handlers and views a vessel should bind for a given project).

## Public API

- `enum Spline` — the project classification, one of:
  - `Spline::UnaOS` — the UnaOS repository itself (anchored by `MEMORIA.md`).
  - `Spline::Rust` — a Rust crate or workspace (`Cargo.toml`).
  - `Spline::Web` — a Node/web project (`package.json`).
  - `Spline::Python` — a Python project (`requirements.txt` or `pyproject.toml`).
  - `Spline::Void` — no recognized marker; unclassified.
- `fn find_workspace_root() -> std::path::PathBuf` — walks up from the current
  directory until it finds `MEMORIA.md`, `Cargo.toml`, or `package.json`. If no
  anchor is found it falls back to the current working directory and logs a
  warning.
- `struct Context { path: PathBuf, spline: Spline }` — a resolved location paired
  with its classification.
- `Context::new(path: &Path) -> Context` — classifies `path` and returns the
  `Context`.

Detection is order-sensitive: `MEMORIA.md` is checked first, then `Cargo.toml`,
then `package.json`, then the Python markers. The first match wins.

The internal `detect_spline(path: &Path) -> Spline` helper performs the marker
checks; `Context::new` is the public entry point that wraps it.

## How it fits into UnaOS

Elessar is one of the shared libraries in `libs/`, alongside `gneiss_pal`
(platform abstraction), `bandy` (the `SMessage`/`Synapse` message bus), and
`quartzite` (the GUI layer). Vessels in `vessels/` use Elessar to identify the
workspace they have been pointed at; the resulting `Spline` informs how the
workspace is assembled and rendered. See
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md)
(section 4) and [`docs/CODEX.md`](../../docs/CODEX.md) for the broader context
model.

## Status

**Partial — detection only.** Workspace-root resolution and Spline
classification are implemented and unit-tested. The broader intent of capturing
a workspace as a serializable snapshot for compilation and packaging into a
native app is **not yet implemented**; that pipeline is expected to be owned by
the `aule` handler.

## Dependencies

`gneiss_pal` (platform abstraction layer), `log`, and `async-channel`.

## Testing

```sh
cargo test -p elessar
```

The bundled test confirms self-recognition: run from inside the UnaOS tree, the
crate classifies the repository root as `Spline::UnaOS` (falling back to a
`Cargo.toml`-based check in bare CI checkouts).
