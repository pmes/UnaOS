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

## The UnaFS-native Loom (VAIRE-2) — `usync` / `ustatus`

The Bolt-1 `sync` above is the direction ruling's *quick solution*: git mirrors
plus penumbra copies onto another host volume. The **real** vaire
(RECONCILIATION-2026-07, the vaire ruling) stores the managed tree as **native
UnaFS objects** on the K-line CoW machinery — not host-fs copies. The `usync`
verb family is that destination, running alongside the Bolt-1 host verbs (which
stay untouched and working).

- **`vaire usync [<image>] [--apply] [--size-mb <n>]`** — weave the
  manifest-bounded penumbra (same manifest, same `ExcludeRules` default-deny
  floor) into a UnaFS **v3** image as native objects:
  - dirs → `mkdir`, files → `create_file` + `write_data`;
  - **K6 typed attrs** per file: `vaire.size` (`Int`), `vaire.mtime` (`Int`),
    `vaire.src` (`String`), `vaire.sync` (run stamp, `String`); unit-root attrs
    (`vaire.unit` = manifest name, `vaire.githead`, `vaire.run`) plus one
    `vaire.summary.<stamp>` per run — so **runs accrue in the image itself**;
  - **one `snapshot_create(name=UTCstamp, creator="vaire")` at the end of every
    completed sync** — the retained root IS the SNAP concept, natively;
  - **incremental** on re-run: files whose `vaire.size` + `vaire.mtime` attrs
    match the live file are skipped and counted; a changed file is unlinked and
    rewritten CoW (grow-only `write_data` cannot shrink in place, so the rewrite
    is exact). Reported `written / skipped / excluded`, like Bolt-1's rows.
    One honest blind spot (the standard size+mtime limitation): mtime is stored
    at 1-second granularity, so a size-preserving edit made within the same
    second as the stored mtime is silently skipped on the next run.
- **`vaire ustatus [<image>]`** — read-only: unit attrs, the snapshot index,
  last-sync stamp, and object/byte counts vs the live tree. The image is opened
  read-only; its bytes are never touched.

**Invariants carried verbatim from Bolt-1.** The live tree is **read-only** —
the only write surface is the UnaFS image handle; no constructor accepts a
live-tree write path (proven by a `0o555` byte-compare test). The exclusion
floor is the **same default-deny**: `usync` walks the penumbra through the same
`walk_penumbra`, so credential patterns, junk pruning, and
reported-never-followed symlinks all apply identically; every skip is counted
and reported, never silent. **Dry-run is the DEFAULT** for `usync`; `--apply`
is required to write. The image is an explicit argument (never a shared/fixed
path — never `bench_vault.img`); it defaults under `~/unaos-bench/vaire/`. The
snapshot cap (16, policy) is honored honestly: a full index is **refused** with
a message naming the drop verb — `usync` never auto-drops a retained root. The
cap is checked up front, before any write, so a refused run is a true no-op on
the image (byte-identical, test-pinned).

### Measured from birth — the baseline UnaFS benchmark

The vaire ruling's corollary: a real dev-tree sync (hundreds of mixed-size files
+ typed metadata + journaled CoW writes in ONE measured run) is the first-class
UnaFS baseline benchmark, and the per-phase costs are the evidence base for the
crate-side batched-sync work that closes the ledgered ~0.7 s `with_unafs` mask
(SECURITY.md). `usync` is instrumented with the **NSSPAN pattern** (host-ported
from the kernel `NsSpanProbe`): per-phase `AtomicU64` accumulators folded by an
RAII `Instant` probe over `scan / lookup / read / write / attrs / commit /
snapshot`, plus UnaFS's own `CommitStats`. Every `usync` ends with the one-line
benchmark ledger, persisted per run as a typed attr on the unit root.

**Baseline run — `~/.claude/plans/unaos` → a fresh v3 image (239 files, 12
dirs, 4,161,266 B; 1 junk skip).** Host context (an uncontextualized baseline is
uninterpretable later): **MacBookPro16,1**, **macOS 26.5.2 (build 25F84)**,
internal **APFS** volume on a **PCI-Express SSD**. Release build; run SOLO (one
unafs-image user on the machine). Times are per-phase totals in ms.

| Run | written | skipped | scan | lookup | read | write | attrs | commit | snapshot | commits | blocks | wall* |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **cold** (fresh format) | 239 | 0 | 4.38 | 15.84 | 43.28 | 49.34 | 157.57 | 10949.77 | 46.05 | 242 | 23484 | ~11.3 s |
| **warm** (incremental) | 0 | 239 | 6.46 | 7.48 | 0.00 | 0.00 | 19.29 | 89.29 | 47.01 | 3 | 235 | ~0.17 s |

*wall = sum of the measured phases (the CLI prints the exact `BENCHMARK:`
line). Phase coverage: `scan` = live-tree walk; `lookup` = image-side lookups
(per-file incremental `ls`/attr reads + directory-chain resolution, which on a
cold run folds in the fresh `mkdir`s); `write` = `create_file` + `write_data`
(+ any stale-object unlink); `commit` = every explicit root flip, including the
final summary commit; the reported `commits`/`blocks` cover the whole run. The
one figure the *persisted* per-run attr cannot include is the commit that
persists it (inherent); the CLI's `commit_stats` are refreshed after it.

The headline: on the cold run the **`commit` phase is 97 % of the wall**
(10.95 s of ~11.3 s) across **242 root flips** — one per file, the current
batched regime (`set_autocommit(false)` + an explicit `commit()` per file).
This is the concrete motivation for the crate-side batched-sync work: all other
phases together are ~0.3 s; the cost is entirely the many small root-flip
transactions. See the finding below. The warm run confirms the incremental path
is cheap (all-skip, ~0.17 s): its measured cost is the snapshot incref walk +
the two root flips, with the per-file image lookups (`lookup` = 7.5 ms for 239
files) staying negligible.

### Finding: batched multi-file commit vs the missing bulk-write API

`usync` already batches within a file — `create_file` + `write_data` + four
`set_attribute`s land in ONE root flip (autocommit off, one `commit()` per
file). A whole-sync single transaction is likewise reachable today by committing
once at the end instead of per file; **no new UnaFS API is needed** for that
coarser batching. What does **not** exist (and is the honest batched-sync
finding for the ledger, not a blocker for this arc) is a **vectored / bulk write
or bulk-create API**: each file still costs an individual `create_inode` +
`write_data` + per-attr inode rewrite, each rewriting the full inode block and
re-serializing the parent directory. The commit-phase dominance above is a
root-flip-count cost that whole-sync batching would collapse; a bulk API would
additionally cut the per-object metadata churn. Neither is required to ship
`usync` — recorded here as the evidence the corollary asked for.

## The one-flip native sync (VAIRE-3) — the batch path adopted

The bulk-create API the finding above named as missing landed crate-side in
**UNAFS-BATCH** (`UnaFS::create_files_batch(parent_id, Vec<BatchFile>)`, API
addition only — the v3 format is unchanged). VAIRE-3 adopts it, and the
adoption is exactly the shape UNAFS-BATCH predicted: autocommit is turned off
once for the whole run, the write set is grouped by parent directory and each
group is landed with a single `create_files_batch` (the four `vaire.*` K6 attrs
ride each `BatchFile`, folded into its one creation inode write), and **ONE**
`commit()` flips the whole staged tree — every `mkdir`, every batched file, and
the unit-root attrs — as a single atomic root, followed by the
`snapshot_create` and the ledger-summary persist. All Bolt-1/VAIRE-2 invariants
carry verbatim: live tree read-only, dry-run default, incremental size+mtime
skip, stale-object unlink for an exact rewrite, one snapshot per completed sync,
cap-16 up-front refusal. A mid-sync failure unwinds the **whole** outer
transaction (the `create_files_batch` fold-documented semantics): `usync`
reports the failure and the mounted image is left at the last committed root —
no partial tree, no snapshot. (CoW may leave orphaned data blocks physically on
disk for reclamation; the logical root never moves, and `fsck` stays clean.)

**Phase-column note (columns kept comparable to the VAIRE-2 table).** The names
are unchanged, but two meanings shifted with the batch shape: `write` now covers
stale-object unlinks **plus** the `create_files_batch` staging (the former
per-file `create_file`/`write_data`), and each file's four attrs fold into the
batch creation inode — so the `attrs` phase now measures only the unit-root +
per-run-summary attrs, not per-file attrs. `commit` collapses from one flip per
file to the run's handful of flips.

### After — the measured one-flip run (same protocol, same host)

Re-run of the exact VAIRE-2 protocol, SOLO, into a **fresh v3 image** (own
scratchpad path — never `baseline.img`). Same host context: **MacBookPro16,1**,
**macOS 26.5.2 (build 25F84)**, internal **APFS** on a **PCI-Express SSD**;
release build. Source = the live `~/.claude/plans/unaos` tree at run time — it
had grown since the baseline scan (**247 files, 12 dirs, 4,211,657 B; 1 junk
skip** vs the baseline's 239 files / 4,161,266 B), so compare the *regime*, not
a file-for-file delta. Times are per-phase totals in ms.

| Run | written | skipped | scan | lookup | read | write | attrs | commit | snapshot | commits | blocks | wall* |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **VAIRE-2 cold** (per-file flips) | 239 | 0 | 4.38 | 15.84 | 43.28 | 49.34 | 157.57 | 10949.77 | 46.05 | 242 | 23484 | ~11.3 s |
| **VAIRE-3 cold** (one-flip batch) | 247 | 0 | 6.79 | 4.12 | 15.05 | 25.31 | 25.20 | 92.58 | 44.28 | 3 | 1989 | **~0.213 s** |
| **VAIRE-2 warm** (incremental) | 0 | 239 | 6.46 | 7.48 | 0.00 | 0.00 | 19.29 | 89.29 | 47.01 | 3 | 235 | ~0.17 s |
| **VAIRE-3 warm** (incremental) | 0 | 247 | 8.02 | 10.39 | 0.00 | 0.00 | 23.27 | 87.44 | 45.69 | 3 | 235 | ~0.175 s |

*wall = sum of the measured phases (the CLI's exact `BENCHMARK:` line).

**Headline:** the cold `commit` phase collapses from **10.95 s across 242 root
flips to 92.58 ms across 3**, and the whole cold wall from **~11.3 s to ~0.213 s
(≈ 53×)**. Blocks written for the cold sync drop **23,484 → 1,989** (the same
per-object metadata-churn collapse UNAFS-BATCH measured: one parent-directory +
catalog rewrite per group and one folded inode write per file, instead of a full
inode + directory + catalog rewrite per attribute). `fsck` is clean (0 leaked, 0
stale) and the snapshot index shows the retained roots.

**Finding — the cold commit sits at ~93 ms, not the harness's ~50 ms, and that
is expected, not a shortfall.** The `tools/unafs bench-batch` harness measured
~49.9 ms across **2** flips (format commit + one whole-tree commit). `usync`
inherently carries **three** flips per run: the whole-tree sync commit, the
`snapshot_create` commit (one retained root per completed sync — a VAIRE-2
invariant), and the per-run ledger-summary persist commit (the benchmark line is
recorded into the image itself). Those two extra inherent flips — the snapshot
and the self-recorded summary — are the difference; the win is *above*, not
below, the prediction for the raw two-flip tree write, and the per-flip cost
(~30 ms) is consistent with the harness. Nothing is hidden here: the surplus over
the bare harness is vaire's snapshot-per-sync + measured-from-birth design, both
carried verbatim.

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
