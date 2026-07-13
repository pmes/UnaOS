# Vaire — the Loom

Vaire is the **repo/workspace manager** for UnaOS: the Loom. It treats a
workspace as a set of managed units — **Bolts** — and weaves them into one
coherent, inspectable whole. A Bolt may be a Git repository, a UnaFS vault, or
(reserved) a non-linear-editing project. Vaire is built entirely on the
pure-Rust `gix` (gitoxide) library for Git — no `libgit2` / `git2` dependency.

> A "Shard" in UnaOS canon is an AI *instance*. The unit the Loom manages — a
> finished bolt of cloth under its care — is a **Bolt**.

## The charter (the destination)

The Loom exists so a workspace of interdependent units moves in crystalline
lockstep, and so its whole state can be seen at a glance and captured as
history. Three Rites define it:

1. **SYNC — the Alignment.** Bring the workspace to a coherent state: pull
   upstream, verify local ("dirty") changes are safe, re-link paths so the
   whole compiles as one unit. *(Prospective — RITES-2.)*
2. **STATUS — the God View.** Report the branch, commit, and **Crystal Color**
   of every managed unit. *(Implemented — see below.)*
3. **SNAP — the Forging.** Crystallize the entire workspace state into an
   atomic, rebuildable release manifest. *(Prospective — RITES-2.)*

**The Destiny:** version control becomes a metadata query. Vaire will
eventually bypass `.git` directories and interface directly with **UnaFS** —
the local source of truth becomes the UnaFS database, while still pushing and
pulling to standard Git remotes for collaboration. This is multi-arc work
sequenced behind UnaFS's own maturation (UNAFS-F1).

## What is implemented today (STATUS / Crystal)

- **The Bolt manifest** — `Manifest` registers managed units declaratively
  (`register(name, path, kind)`), lists them in registration order (`list`),
  and reports live status (`status_of`, `status_all`). `BoltKind` is one of
  `GitRepo`, `Vault`, or `NleProject` (reserved: registered but not yet
  status-aware).
- **`Vaire::look() -> Result<GitStatus>`** (and the path-parameterized
  `look_at(path)`) — inspects a Git repository at (or above) a directory via
  `gix::discover` and returns `GitStatus { branch, commit, is_dirty }`: the
  symbolic branch (or `"DETACHED"`), the 7-char short HEAD hash, and a **real**
  dirty flag. The dirty check compares the working tree against the index and
  the index against HEAD's tree (untracked files excluded, matching porcelain's
  tracked-change notion of "dirty"). *This replaced a hard-coded `false` stub.*
- **Crystal Color** — `CrystalColor::{Green, Amber, Red}`, the God-view
  vocabulary:
  - **Green** — clean, synced, ready.
  - **Amber** — local (tracked) changes present.
  - **Red** — detached HEAD / conflict, or an absent / unreadable / unmountable
    unit.
- **The vault as the first non-git unit** — a `Vault` Bolt's status rides
  UnaFS's fail-closed mount check, **read-only**: an absent, unreadable, or
  unmountable vault is Red (its on-disk bytes are never touched — the probe
  opens the device read-only); a vault that mounts is Green. Last-snapshot is
  not yet tracked (that arrives with SNAP). Vault *management* only — the
  engram save/query actor belongs to `vein`, not Vaire.
- **`Vaire::handle_message(&SMessage) -> Option<SMessage>`** — the bus-facing
  entry for the Git diff contract (`GetDiff` → `DiffPayload` / `Log`). It is a
  pure function; the hosting vessel owns the Synapse subscription and fires the
  returned response.
- **`create_view() -> gtk4::Widget`** — an optional status widget rendering the
  current branch/commit/dirty state. Compiled only under the `gtk4` feature.

The diff itself is a tree-to-tree comparison via `gix`: revisions are resolved
with `rev_parse_single`, peeled to trees, and walked to produce a line per
changed path (`+ Added`, `- Deleted`, `~ Modified`, `* Rewritten`).

## Bus integration (Synapse / SMessage)

| Direction | Variant | Notes |
| --- | --- | --- |
| In | `GetDiff { commit_a, commit_b }` | Request a diff between two revisions. |
| Out | `DiffPayload { diff }` | The computed change summary, on success. |
| Out | `Log { level, source, content }` | `level = "ERROR"`, `source = "Vaire"` when the diff fails. |

`handle_message` is a pure function: given an `SMessage`, it returns the
response `SMessage` to publish. It does not itself subscribe to the Synapse or
spawn a task; the hosting vessel receives messages and fires the returned
response onto the bus.

## Not yet present

- SYNC (the Alignment) and SNAP (the Forging) — RITES-2.
- A declarative on-disk manifest format and workspace discovery — the manifest
  is currently built programmatically in memory.
- UnaFS-native version control (the Destiny) — behind UNAFS-F1.
- An async `ignite(...)` entry point / Synapse subscription loop — Vaire is
  driven synchronously through `handle_message`.
- Unified line-level diffs — the diff is a per-path change summary.
