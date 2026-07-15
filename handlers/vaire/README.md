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

1. **SYNC — the Alignment.** Bring the workspace to a coherent state across the
   drives it lives on: mirror committed history and copy the manifest-bounded
   untracked penumbra. *(Implemented for the dev-tree Bolt — see below.)*
2. **STATUS — the God View.** Report the branch, commit, and **Crystal Color**
   of every managed unit. *(Implemented — see below.)*
3. **SNAP — the Forging.** Crystallize a point-in-time workspace state as an
   atomic, rebuildable capture (git bundles + penumbra). *(Implemented for the
   dev-tree Bolt — see below.)*

## The dev-tree Bolt (BOLT-1) — STATUS / SNAP / SYNC

The Loom's first *real* managed unit is UnaOS's own development tree: a source
tree that must exist coherently on two physical drives — the **live** internal
"narino" drive where work happens, and a **target** copy on a removable volume
(`/Volumes/40G`). The `devtree` module adds three coherence verbs; the live-copy
flip (**SWITCH**) is arc 2 and is deliberately absent here.

The unit boundary is a **versioned, commented manifest** — `bolt.manifest.toml`
— that the crate reads (serde/toml). It declares the live/target roots, the git
units (the main repo plus its sibling worktrees), the untracked penumbra roots
(the `~/.claude/plans/unaos` tree), and the exclusion rules.

- **STATUS** (`vaire status`) — pure-read coherence truth: which copy is live
  (arc-1 fact: always narino, no flip), per-repo git truth (branch, dirty files,
  unpushed commits, worktree-pointer flag), the penumbra delta
  (same / only-live / differs), the last-weave stamp, and the **Crystal Color**:
  - **Green** — target present and fully coherent.
  - **Amber** — target present but drift exists (a dirty working tree, a target
    mirror behind live HEAD, or a penumbra difference).
  - **Red** — target volume absent (unsyncable).

  Git worktrees (whose `.git` is an absolute-path pointer file) are surfaced as a
  flagged **RED-for-switch** row and **never rewritten** — repointing them is
  arc 2's job.
- **SNAP** (`vaire snap`) — a stamped point-in-time capture written to the
  target under `<target>/.vaire-snaps/<UTCstamp>/`: a `git bundle --all` per repo
  plus a manifest-bounded penumbra copy. Refuses honestly if the target volume
  is absent.
- **SYNC** (`vaire sync [--apply]`, narino→40G one-way) — **dry-run is the
  default**: it prints the exact would-do plan (mirror each repo, copy each
  new/changed penumbra file, and every excluded skip). `--apply` is required to
  write, and an apply **always SNAPs first**. Git history moves through a bare
  mirror on the target (`git push --mirror`, all refs incl. track branches); the
  untracked penumbra is copied file-by-file strictly off the manifest — **never**
  a bare `rsync -a` of the tree.

### The narino-never-written invariant

No code path in this arc writes, deletes, renames, or chmods anything under the
live (narino) tree. This is provable by construction: the only write surface is
the `devtree::Target` type, built solely from the manifest's `target_root`
(there is no constructor accepting the live root as a write destination). The
live side is only ever opened read-only — STATUS shells read-only `git` queries
and reads penumbra bytes; SNAP reads the live repos to write bundles *into the
Target*; SYNC pushes live refs *into the target mirror* and copies penumbra
files *into the Target*.

### The exclusion manifest (security-adjacent, default-deny)

The penumbra is filtered by two exclusion classes declared in the manifest and
reported distinctly in STATUS / dry-run (never silently dropped):

- **junk** — `target`, `node_modules`, `.DS_Store`, `*.o`, `*.elf`, `*.rlib`.
- **credentials** — **default-deny**: `*.pem`, `*.key`, `*_rsa*`, `*.p12`,
  `*token*`, `*secret*`, `*credential*`, `.env` / `.env.*`, `.netrc`,
  `*.keychain`. The crate merges a built-in credential floor on top of whatever
  the manifest lists, so forgetting a pattern cannot leak a credential.

Additionally, **symlinks are never followed** (dereferencing one would copy
content from outside the manifest boundary into the mirror). Arc-1 skips them —
but never silently: each symlink is surfaced as its own reported class in
STATUS, the SYNC dry-run plan (`symlink (not followed)`), and SNAP reports.

> **First real run is attended.** Every automated test runs against tempdir
> fixtures. The first real `vaire status` and the first `vaire sync --apply`
> against the live narino tree / `/Volumes/40G` are run by Peter — they are this
> arc's mini metal gate, not the executor's.

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

- **SWITCH** — the atomic live-copy flip (repointing which drive is "live",
  rewriting worktree `.git` pointers). That is **arc 2**; arc 1 hardcodes
  "live = narino" as a manifest fact.
- Two-way / 40G→narino sync — arc 1 is narino→40G one-way only.
- In-memory manifests for the other `BoltKind`s (`GitRepo`, `Vault`,
  `NleProject`) still register programmatically; only the dev-tree Bolt reads a
  declarative on-disk manifest so far.
- UnaFS-native version control (the Destiny) — behind UNAFS-F1.
- An async `ignite(...)` entry point / Synapse subscription loop — Vaire is
  driven synchronously through `handle_message`.
- Unified line-level diffs — the diff is a per-path change summary.
