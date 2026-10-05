# BOOT80 — the 60 s silent wait on the UnaFS root (rmbp-ledger B350)

## Finding (read from f21-boots.log, not assumed)
- The users store is NOT on UnaFS: `USERS.DAT` lives on the FAT boot volume (p1) and `users::try_load`
  finished at 18:07:44 (`[login] root password unset row=created`) — before the UnaFS root even mounted
  (18:07:46). `stage_resolve` reads no medium at all: the stage is a pure function of the loaded store.
- What sits between `try_load` and `stage_resolve("store-loaded")` in `users::service` is
  `crate::fs::assoc::seed_once()` (FILETYPE, B307): on the FIRST boot of an attribute-bearing root it
  writes the type database — 2 mkdirs + 13 × (create + 3 `set_attr`) = 54 VFS mutations, each its own
  UnaFS transaction.
- Every UnaFS commit (`libs/fs/unafs/src/fs.rs` `commit`) rewrote the WHOLE refcount map: on the card's
  131072-block volume that is 128 leaves + index + inode map ≈ 131 × 4 KiB = 1048 single-sector CMD24
  writes per commit (`BlockAdapter` issued one command per 512 B sector; reads likewise). 54 commits ×
  ~1050 commands ≈ 57 000 SD commands ≈ the 60 s. That is the "full-volume" cost: the map that covers
  every block of the volume, written whole on every transaction, one sector per command.
- R80 had taken the `[users] load` / `[assoc] seed` lines (bootlog-only), so the wait had no name.

## Seam
UnaFS's own crate (`unaos/libs/fs/unafs`, the shared core both rings link) and the kernel's one coherent
mount (`fs/unafs.rs`). No second store; the assoc seed rides UnaFS's existing `create_files_batch`.

## Milestones
- M1 — name it: `fs/bootstep.rs` (new; CHARTER Kernel — fs-core): one boot line per store step,
  `[boot] step=<name> ms=<n> blocks_read=<n> blocks_written=<n> cmds=<n>`, counted at the SDHC block
  entry points; steps: users-load, root-mount, assoc-seed, stage-resolve.
- M2 — remove the cost (unafs crate, host-tested): (a) `SectorDevice::read_sectors`/`write_sectors`
  (default = the per-sector loop) and `BlockAdapter` moves a 4 KiB block in ONE call; (b) `ReadAhead`
  window arithmetic + cache (window = the SDHC multi-block command, 64 sectors); (c) `commit` re-points
  every refmap leaf whose counts did not change at its committed block and writes only the dirty ones
  (fixpoint over the leaves the commit's own allocations dirty) — format-identical.
  Kernel: `SdSectorDevice` serves `read_sectors`/`write_sectors` with CMD18/CMD25 through the 64-sector
  read-ahead window (write-through); `assoc::seed_in` stages its 13 objects in ONE transaction
  (`MountTable::create_files_batch` → `UnaFS::create_files_batch`).
- M3 — `tests boot80`: force a cold remount, resolve the users store, the stage and the type database;
  `:: BOOT80: mount_ms= mount_blocks= blocks_read= cmds= ms= -> PASS|FAIL ::`, bound blocks_read <= 64
  (the resolve) and ms <= 2000 (mount + resolve).
- M4 — the splash names the step: each step paints its name on the held splash surface and composites.

## Witness (boot 22)
`[boot] step=users-load …`, `[boot] step=root-mount …`, `[boot] step=assoc-seed … commits=1`,
`[boot] step=stage-resolve ms=0 blocks_read=0`, then `:: BOOT: … loader->desktop=` near boot 20's 10 s;
`tests boot80` → `:: BOOT80: … -> PASS ::`.

## Owed
The cold mount still reads the whole refcount map (128 leaves, by format) — now in 64-sector commands;
a lazily-paged refmap is a format/crate arc of its own. `resolve_path` reads each directory whole (the
directory IS the index for names; the name B-tree is the query catalog) — fine at these depths.
