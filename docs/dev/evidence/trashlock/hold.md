# TRASHLOCK (rmbp-ledger B523): the hold, named

Base 9c549dba (image 22's tree). Source: f28-boot1.log, 12:27:53Z to the end.

## The Trash path, read end to end: no hold of its own
- `fs/trash.rs:181` `entries()` calls `trash_core::entries` (`unaos/libs/fs/trash_core/src/lib.rs:226`), which calls `attr_entries` (:236).
- :240 `fs.query(TRASH_QUERY)` goes to `MountTable::query` (fs/vfs.rs:846), then `NativeBackend::query` (vfs.rs:2170): ONE `with_unafs` holds the crate's `query` (`unafs/src/fs.rs:2721`: `query_matches` plus `path_of` per hit, bounded by `MAX_PATH_DEPTH`) and `native_read_gate` per hit (`read_authz`, fs only). It returns, and the guard drops, BEFORE the loop below.
- :244/:248/:252 run three `fs.get_attr` per entry. Each one is `NativeBackend::get_attr` (vfs.rs:2133), its OWN `with_unafs`, taken after the query's hold is gone. So there is **no re-entrant take of MOUNT**, the first candidate is falsified.
- `with_unafs_attempt` (fs/unafs.rs:792) is guard-based: every `?` and `return` inside a closure drops the guard, and no code releases MOUNT by hand. The trash_core loops (`trash` collisions ≤ 99, `delete_tree` ≤ `max_depth`) and the crate's btree range and path walk are all bounded. An entry whose attribute row is missing is `continue`d at :245.
- The keys the Trash queries are the keys `stamp` writes (`una:trash-origin`), and no `job:`-style index is involved.

## The hold
`unafs::MOUNT` (fs/unafs.rs:567) is taken at :794 inside `without_interrupts` (IF=0). Under that hold every sector read goes through `SdSectorDevice` (handle=sdhc; f28 `:: [unafsbind] mount=native handle=sdhc ::`) and `block::read_block(s)_sdhc` into `sdhc::read_block_512`/`read_blocks_512`. Those take `sdhc::CARD` (drivers/sdhc.rs:997) at :1537/:2650 with a plain spin.

CARD is ALSO taken with interrupts ON: FAT reads and writes on the Sdhc source (fs/fat.rs:799/:836/:903, which are unmasked) and the `card_*` accessors (sdhc.rs:1008-1039). x86 tasks are CPU-pinned and preemptive (arch/x86_64/sched.rs:14-18). The cycle:
1. Task T on core W takes CARD unmasked, and the timer preempts it.
2. Task U on core W enters `with_unafs` (masked), takes MOUNT and spins on CARD with IF=0.
3. Core W never takes another tick, so T never runs. CARD is never released, and neither is MOUNT.
4. Every later `with_unafs` caller (the render task at :794) spins on MOUNT.

This matches what the log shows: `masked_ms=0` to EOF (no attempt ends), and the render task on MOUNT.

Why the Trash op set it off: `trash()` bumps `TRASH_GEN` (fs/trash.rs:196). The Dock then spawns `dock-trash` (video/dock.rs:4158) on `smp::xhci_worker_cpu(0)`, the first worker core. That is the same core `shelltask::job_cpu` (shelltask.rs:137) gives the shell job running `tests quarry2`. So a fresh UnaFS transaction lands on the core where the leg's unmasked card users run.

Why LOCKREG said nothing: the MOUNT holder's spin on sdhc.rs:997 had ALREADY been said once at 12:18:00 (`[lock] spin-wait name=sdhc.rs:997 at=sdhc.rs:2650 waiter=12@cpu1 on=14@cpu2`, `… at=sdhc.rs:1537 …`). `say_ok` speaks once per site per BOOT, and the 16-line cap was spent by 12:22:36. Both are now per minute.

Honest limit: f28 does not print T's identity, so steps 1 and 2 are inferred from the code and the wire. The next boot's per-minute `[lock] spin-wait name=sdhc.rs:997 … on=<tid>@cpu<n>` and `[lock] held-too-long name=unafs.rs:567` confirm or refute it.

## The fix at the source (OUTSIDE this brief's files, STOP-reported)
In drivers/sdhc.rs, every `CARD.lock()` (:1008, :1013, :1019, :1025, :1039, :1537, :1704, :2231, :2241, :2650, :2773, :3383, :3431, :3623) is taken inside `crate::arch::without_interrupts`. A card holder then cannot be preempted (MOUNT's own discipline, sched.rs:32), so the masked MOUNT holder's spin on CARD is bounded by one SD command.

## Wire, next boot
- Any wedge: `[lock] held-too-long name=unafs.rs:567 at=unafs.rs:794 by=<tid>@cpu<n>` once a minute while held, plus `[lock] spin-wait name=sdhc.rs:997 at=sdhc.rs:2650 waiter=<tid> on=<tid>@cpu<n>`.
- `:: RENDERWEDGE: … holder=<lock>:<label>:<tid>@cpu<n>` names a non-mount lock through `sync::waits_on`.
- Healthy: `[trash] … op=trash ok=1`, then `[dock] trash state why=change full=1`, then `[trash] … op=restore ok=1`, then `:: QUARRY2: … origin=ok(attrs) -> PASS ::`, and `:: TRASH: store=attrs trashed=2 restored=1 emptied=1 query_ok=3 -> PASS ::`.
