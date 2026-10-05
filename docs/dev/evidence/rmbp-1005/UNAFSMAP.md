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

**BOOT80 (B350) and this arc.** BOOT80 (merged mid-arc via merge12) made the LEGACY commit re-point
clean refmap leaves instead of rewriting all of them, keeping the one/two-level raw-pointer index
rewritten whole. The paged map subsumes it: the same dirty-leaf discipline (a leaf whose counts did not
change keeps its block; the relocation runs to a fixpoint because retiring/allocating map blocks dirties
other leaves) now covers the index nodes too (only nodes on a dirty path move), the inode map, holes,
checksums, and RAM (no whole-map views). The join kept UNAFSMAP's `refmap.rs`/`commit`; v3–v6 volumes
still get BOOT80's behaviour through the legacy shape of the same tree code. BOOT80's adapter
(`read_sectors`/`write_sectors`), `readahead.rs` and `tests/boot80_logic.rs` are untouched and green.

## Results (unflown, R78)

Commits: M1 wip dbb75beb · merge12 join d826239e · boot-22 fold merge 89506bd5 · M1 35ccc7d5 ·
M2+M3 fb5a6435 · M4 (this doc, bench-map dir spread) on the tip.

- `cargo test --release -p unafs -p unafs-cli` (repo root): exit 0, 27 targets, 204 passed, 0 failed —
  every pre-existing suite on v7 (version-pinned asserts moved: `kat_superblock_v6` keeps the v6 golden
  and `kat_superblock_v7` adds 06 → 07; the drift test re-seals the leaf sum, since an unsealed flip is
  now refused as corruption), `boot80_logic` green, new `map_tree` (7) and `map_bench` (2 + ignored rows).
- Bench, same harness both sides (`tests/map_bench.rs bench_rows`, sparse image file, release; BEFORE =
  the merge-base code ab741349, AFTER = the tip):

| volume | blocks | mount s before → after | mount peak heap before → after | 1-block write commit blocks before → after |
|---|---|---|---|---|
| 1 GiB | 262144 | 0.001 → 0.000 | 2066 KiB → 12 KiB | 261 → 7 |
| 4 GiB | 1048576 | 0.006 → 0.000 | 8228 KiB → 26 KiB | 1031 → 7 |
| 16 GiB | 4194304 | 0.044 → 0.000 | 32876 KiB → 80 KiB | 4109 → 7 |
| 64 GiB | 16777216 | 0.232 → 0.000 | 131468 KiB → 297 KiB | 16421 → 7 |
| 1 TiB | 268435456 | (needs 2 GiB of RAM: not run) → 0.002 | ≈ 2 GiB → 4632 KiB | ≈ 262,000 → 8 |

- `tools/unafs bench-map` (v7, 1 TiB sparse): format 0.036 s, mount 0.003 s, 4632 KiB mount heap, 1-block
  commit 8 blocks, 6 map blocks total. With `--files 1000000` (1,000 per directory): format+create
  35.0 s, mount 0.025 s / 12483 KiB (the inode map is held whole: 8 MiB for 1 M ids), 1-block commit 13,
  2959 map blocks, **fsck 6.43 s, 34332 KiB peak** (the reachability map; the refcount walk is per leaf).
  `--version 6` shows the migration: the first write turns a v6 image v7 and keeps its 261 / 4116 legacy
  leaf blocks.
- Kernel budget (`map_bench::kernel_budget_heap_on_a_500_gb_root`, default 4 MiB cache, the rMBP's
  500 GB p2 = 122,070,272 blocks): mount peak 2111 KiB, peak through a 64 MiB write + 2,000 files + fsck
  3916 KiB against the 256 MiB x86 heap; 40 pages peak, 20 of 119,210 leaves own a block.
- Ledger bounds as tests: `one_tib_mount_under_one_second_and_eight_mib` and `map_tree::
  one_tib_sparse_volume_is_a_handful_of_map_blocks` (commit < 16 blocks) pass.
- Installer: `GROW_CAP_BLOCKS` = 2^31 (= `unafs::superblock::MAX_BLOCK_COUNT_PAGED`, const-asserted under
  `unafs`): `grow_target_blocks` is the partition minus the scratch tail.
- x86 metal shape + `ahciroot` (`wc,quarry,ftdirx,login,loginst,nvidia-kepler-vblank,smc,usbnet,hda,
  hda-tone,facet,beam,sdw,sdwrite,sdhcblk,selfhost,linuxabi,ahci,unafs,busreg,lumen,netring3,
  prefs_reset,census,installdemo,instgui,witness,ahciroot`): exit 0. aarch64 `login,loginst,virt_el0`
  (blob head 280080d2): exit 0. charter-check: exit 0 (no new kernel file).

Expected wire on an install: `[install] stage=grow from=<b> to=<p2 blocks minus scratch> ok` (no clamp at
4194304); the next boot's root line names that `blocks=`; a v6 card mirrored to the SSD becomes v7 on the
grow's first commit. `tests unafsgrow` unchanged (`the map's own growth 0` on v7: new leaves are holes).

**Still owed.** Unflown. The inode map is persisted incrementally but held whole in RAM. A migrated
volume keeps its all-zero legacy leaves as blocks (they become holes the next time they are dirtied and
empty). The Jetson `sdmmc_tegra.rs` installer cap still sizes the old 8 B/block map (that track's row).
