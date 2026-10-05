# UNAFSMAP — UnaFS's maps become paged trees (B354)

Branch `exec-rmbp-unafsmap`, cut from 9a50a644; `exec-rmbp-merge12` (boot-22 integration, 21 arcs, with
UNAFSGROW B347 and AHCIROOT B332) merged first (ab741349). No new knob: the crate is every build's; the
installer leg rides `ahciroot`/`installdemo` + `unafs`.

## Design (written before the code)

**Finding.** The refcount map (`refmap.rs`) is two whole `Vec<u32>` views — 8 B of RAM per block — and
`commit` decrefs every map block and writes ALL leaves + index fresh each time. A 16 GiB volume rewrites
4,097 blocks per commit and holds 32 MiB; a 1 TiB volume would hold 2 GiB and write 1 GiB per commit. So
UNAFSGROW capped the installer at `GROW_CAP_BLOCKS` = 16 GiB. The inode map has the same whole-rewrite
shape (one index of ≤ 512 leaves: a hard cap of 262,144 inode ids — a system volume needs more).
NOTE on numbering: the ledger row says "v6"; the crate is already v6 (B302's B+tree catalog), so the
paged maps are **v7**, and the migrating volumes are the v6 ones every installer has written.

**Seam.** `fs-core`: everything lives in the shared no_std `unafs` crate (`maptree.rs` new, `refmap.rs`,
`fs.rs`, `fsck.rs`) that `tools/unafs`, the kernel mount and the installer all link — one implementation.
The kernel changes only the installer's cap.

**Shape (v7).** Both maps are a radix of 4096 B leaves (1024 u32 counts / 512 u64 inode slots) under
255-way index nodes: entry = `(block u64, sum u32, used u32)`, trailer = magic `UNAFSMN1` + FNV-1a of the
node. Every leaf and node below the top is checksummed by its parent; the top by its own trailer.
`block == 0` is a hole (all-zero subtree, no block): a fresh 1 TiB volume's map is a few blocks.
Levels are a pure function of the leaf count (1 TiB = 262,144 leaves = 3 levels). The root record is
unchanged (80 B): `refmap_block`/`imap_block` name the top nodes. Cap: 2^31 blocks (8 TiB).

**RAM.** The refcount map holds the tree's pointers (16 B per leaf: 4 MiB at 1 TiB) + 2 B "busy" per leaf
+ an LRU of leaf pages bounded by a budget (default 4 MiB); pages a transaction modified are pinned (with
their frozen copy) until the commit. First-fit allocation skips full leaves on the busy counts, so the
current/frozen "never reuse a block the committed tree holds" rule is unchanged.

**Commit.** Dirty leaves + every index node on a dirty path are relocated to fresh blocks (old blocks
decref'd, protected by the frozen view until the flip) — inode map first, then the refcount map's own
nodes to a fixpoint (its allocations dirty more leaves), then leaves and nodes are written bottom-up, the
barrier, the 512 B root flip. Clean subtrees keep their blocks. A paged leaf left empty becomes a hole.

**Migration.** v6 volumes mount read/write on the legacy shape (the legacy load reads every leaf once,
as before, computing sums; pages are not kept). The first commit re-shapes both maps as paged — every
leaf block is KEPT (a legacy leaf is a paged leaf), only index nodes are new — and flips the root with
`ROOT_FLAG_MIGRATE` (bit 1), then rewrites block 0 as v7, then commits with the flag clear. A cut after
the flip mounts the v6 superblock over a MIGRATE root as paged and finishes on the next commit. v3–v5
volumes (flat catalogs, cannot be stamped v7) stay legacy-shaped but commit incrementally too (only dirty
leaves + the index blocks above them; byte-compatible with every pre-v7 reader). Retained snapshot roots
taken before the migration keep their legacy inode maps: the walk sniffs the top node's magic.

**Milestones.**
- M1 `maptree.rs` + paged `refmap.rs` + `fs.rs` commit/load/migration, v7, `ROOT_FLAG_MIGRATE`; the
  whole suite green on v7 plus new KATs (`map_tree.rs`: 1 TiB sparse format, dirty-only commits,
  bounded cache, v6 → v7 migration incl. the interrupted one, checksummed leaves refused when corrupt).
- M2 fsck walks leaves (holes skipped, repair touches only differing leaves); grow appends holes;
  `tools/unafs bench-map` before/after table; the 1 M-file fsck.
- M3 installer `GROW_CAP_BLOCKS` → the format's wall (the partition); heap test at the kernel budget.
- M4 this doc's results.

**Witness.** Host-proven: `cargo test -p unafs -p unafs-cli` exit 0 + `map_bench` bounds. Kernel wire on
an install: `[install] stage=grow from=<b> to=<p2 blocks> ok` (no longer clamped at 4194304).

**Owed.** Unflown (R78). The inode map is still held whole in RAM (8 B per inode id; persisted
incrementally). The Jetson sdmmc installer's own heap cap (`sdmmc_tegra.rs`) is that track's.
