# REGISTRYCHUNK (rmbp-ledger B508) — the file-type registry build, per object and off the critical path

**Finding (flight 27, f27-boot1.log 08:25:57..08:26:28).** `[filetypes] built at=login … created=30 blocks_read=20776
cmds=3599 ms=31154`, then `[desktop] built … bar_ms=31205` 51 ms later. Two causes, read from the code:

1. **The bar waited on the build.** `assoc::service()` runs in the same ~1 kHz device-service pass as
   `desktopbuild::service()` (`video/desktop_uefi.rs`). The desktop's build holds 500 ms for the first SMC sweep and
   returns; the same pass then calls `assoc::service()`, which builds the registry INLINE for 31 s; the desktop's
   build only completes on the next pass. The registry ran before the bar, on the device-service task.
2. **The cost numbers are not the build's.** `blocks_read`/`cmds` are `bootstep::io()` deltas — GLOBAL card
   counters (`drivers/block.rs` counts every SDHC command from every core). The 31 s window overlapped the jobs
   scan (`[jobs] volume=/jobs records=1675` at 08:26:14) and the HOLOCRON ring, so 20776 is the card's traffic in
   that window, not 30 types' cost. SMALLFIX6 measured the same build at 5.5 s on flight 26 with a quieter card.
   The build itself (`assoc::seed_in`): a `stat` miss and a `registrants` attribute read per type (each a full
   path resolve under one masked `with_unafs` hold), then ONE `create_files_batch` of all 30 objects — one
   masked hold that stages 30 inodes, ~150 index facts into the catalog B-trees and the commit, during which the
   card is shared with the jobs scan's claims (the masked claimant spins its bounded wait).

**Seam.** Kernel — the registry is the kernel's (FILETYPES B423: `assoc.rs` is the fulfiller of the type
database on the system volume); no new store, no second implementation. No new file, no new knob, no new verb.

**Milestones.**
- M1 — the cost named per type and scoped to the build: `bootstep::scope_*` counts only the I/O issued on the
  build's own core while it is armed (x86; all cores elsewhere), `hidstall::scope_masked_*` the longest masked
  UnaFS span on any core while armed. One line per type on a full build:
  `[filetypes] type=<mime> made=<0|1> filled=<n> blocks=<n> cmds=<n> ms=<n> masked_ms=<n>`; the summary line's
  `blocks_read`/`cmds` become the scoped counts (`io=core<n>|global` says which).
- M2 — per object: presence from ONE `read_dir` of `/system/filetypes` (no per-type `stat` miss), each missing type
  its own one-object `create_files_batch` (one bounded transaction, one root flip), each present type its own
  `set_attrs`; on x86 the task yields between types so interrupts and the HID pump run between objects.
- M3 — where it runs is JOBSCAN's (B497, exec-rmbp-jobscan 8d177783: `assoc::service` waits for the bar and the
  `jobs-scan` worker runs `build_owed` after its scan). Not moved again here; `assoc::service`/`owe` are untouched so
  that branch folds clean. This arc's chunked `seed_in` is what that worker runs; the yield between objects is taken
  wherever the caller is unmasked. A missing object's keys read no attribute (`appres::builtin_registrants`): no
  path-resolve miss per type on the first boot. The stamp gate is unchanged (a match is one attribute read).
- M4 — `tests filetypes` reports the login build's measured cost and bounds it:
  `:: FILETYPES: types=<n> build_ms=<n> blocks=<n> masked_max_ms=<n> -> PASS|FAIL :: login=<full|stamp|none> preferred=<n> resolved=<ok|why> registry=… objects=… source=… sticky=… amend=… openwith=… builds=…`
  (the ledger's head shape; PASS needs, for a full login build, `build_ms < 1000`, `blocks < 500`, `masked_max_ms < 50`;
  for a stamp hit, `build_ms <= 10` — ASSOCSTAMP's bound; `login=none` holds nothing).

**Witness (the next flight reads).** After `[desktop] built`, on a first boot: 30 × `[filetypes] type=… blocks= cmds=
ms= masked_ms=`, then `[filetypes] built at=login … blocks_read=<n> cmds=<n> ms=<n> io=core<n> masked_max_ms=<n>`; on every later boot `… skipped=stamp …`. `tests filetypes` → the `:: FILETYPES:` line above.

**Owed.** The B-tree node path is still read per inserted fact (no node cache in `btree::DeviceStore`); if the
per-type line shows the catalog insert dominates, a node cache in the unafs crate is the next cut. The one-second
total is proven only by the metal line. Where the build runs is JOBSCAN's arc (B497).

**Fold onto JOBSCAN (B497, exec-rmbp-jobscan 8d177783).** This branch leaves `assoc::owe`/`service` and the file's
tail untouched (the REGISTRYCHUNK block sits after `build_stamped`, not at EOF where JOBSCAN appends `take_to_worker`/
`build_owed`), so the two branches merge without a hunk in common (`git merge-tree` on the two tips, quoted in the
commit). Folded, the order on the wire is: `[desktop] built` → `[jobs] scan …` → `[jobs] volume=…` → 30 ×
`[filetypes] type=…` → `[filetypes] built at=login … io=core<worker> masked_max_ms=<n>`, all on the `jobs-scan` task.
Unfolded (this branch alone) the build still runs on the device-service pass before the bar, but per object with a
yield between — the bar's placement is JOBSCAN's.

**Files beyond `assoc.rs`.** `appres.rs` (+`builtin_registrants`, pure — no VFS read for an absent object),
`bootstep.rs` (the per-core scope behind `blocks=`/`cmds=`; one relaxed load per command when disarmed), `hidstall.rs`
(the scoped masked max behind `masked_ms=`; one `fetch_max` per UnaFS attempt). Each is the counter the arc's wire
names; no other file.
