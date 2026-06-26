# Vaire

Version-control handler for UnaOS. Vaire exposes Git repository state and
inter-commit diffs to the rest of userspace, built entirely on the pure-Rust
`gix` (gitoxide) library — no `libgit2` / `git2` dependency.

## Responsibility

Vaire is the version-control (Git) domain service in the handler layer
described in [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md).
It owns the project's relationship to its Git repository: reporting current
HEAD state and computing the set of changes between two revisions. It locates
the repository by walking up from the working directory (`gix::discover`), so
callers do not pass a repo path.

## What it does today

- **`Vaire::look() -> Result<GitStatus>`** — inspects the repository at (or
  above) the current directory and returns a `GitStatus { branch, commit,
  is_dirty }`: the symbolic branch name (or `"DETACHED"`), the abbreviated
  7-character HEAD commit hash, and a dirty flag.
- **`Vaire::handle_message(&SMessage) -> Option<SMessage>`** — the bus-facing
  entry point. It pattern-matches a request message and returns a response
  message (or `None` if the message is not for Vaire).
- **`create_view() -> gtk4::Widget`** — an optional status widget rendering the
  current branch/commit/dirty state. Compiled only under the `gtk4` feature.

The diff itself is a tree-to-tree comparison via `gix`: revisions are resolved
with `rev_parse_single`, peeled to trees, and walked to produce a line per
changed path (`+ Added`, `- Deleted`, `~ Modified`, `* Rewritten`).

## Bus integration (Synapse / SMessage)

Vaire participates in the Bandy message bus through a single request/response
pair on `SMessage`:

| Direction | Variant | Notes |
| --- | --- | --- |
| In | `GetDiff { commit_a, commit_b }` | Request a diff between two revisions. |
| Out | `DiffPayload { diff }` | The computed change summary, on success. |
| Out | `Log { level, source, content }` | Emitted with `level = "ERROR"`, `source = "Vaire"` when the diff fails. |

`handle_message` is a pure function: given an `SMessage`, it returns the
response `SMessage` to publish. It does **not** itself subscribe to the Synapse
or spawn a task; the hosting vessel is responsible for receiving messages and
firing the returned response onto the bus.

## Status

**Partial / in development.**

- Implemented: `look()` (branch + short HEAD), `get_diff` over `gix`, the
  `GetDiff` → `DiffPayload` / `Log` message contract, and the optional GTK
  status view.
- Not yet implemented:
  - The dirty-state check in `look()` is a stub and always reports
    `is_dirty = false`.
  - No async `ignite(...)` entry point or Synapse subscription loop yet; Vaire
    is driven synchronously through `handle_message`, unlike handlers that own
    their own task.
  - The diff is a per-path change summary, not a unified line-level diff.

Workspace-wide synchronization, multi-repository orchestration, snapshot/tag
release flows, and any UnaFS-native version control are prospective design
directions and are **not** present in this crate.
