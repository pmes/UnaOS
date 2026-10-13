# QUARRYIO (rmbp-ledger B660) — Quarry's facts leave the lock, the paint reads only the model

CHARTER: Matrix — kernel-by-ruling R50 (Quarry is the Finder; the facts are `video/quarry/columns.rs`'s, beside the model).
Branch `exec-rmbp-quarryio`, cut from `45e03964`. Design written BEFORE the code (R106).

## 1. The wire (FLIGHT35 §1 RENDERWEDGE x2, `f35-boot1.log`)

```
[21:24:48Z] [folderview] enter dir=/apps src=default writable=1 ...
[21:24:53Z] [lock] held-too-long name=sdhc.rs:997 at=sdhc.rs:3623 by=12@cpu1 ms=53 waiters=0
[21:24:56Z] :: RENDERWEDGE: age_ms=5084 last_handler=panel-render holder=untracked -> FAIL :: ... render=12@cpu1
[21:24:56Z] [lock] task=12 holds=[live.rs:521,columns.rs:190,sdhc.rs:997] waits=none
[21:25:01Z] [lock] held-too-long name=attrcols.rs:69 at=attrcols.rs:197 by=12@cpu1 ms=92 waiters=0
[21:25:01Z] [quarry2] list cwd=/apps rows=32 typed=32 ...
[21:25:01Z] [quarry] press at (115,727) row=2 kind=dir -> cd
...
[21:25:29Z] [attrcols] facts dir=/apps wrote=32 -> relist
[21:25:30Z] [quarry2] list cwd=/apps rows=32 ...
[21:25:31Z] :: RENDERWEDGE: age_ms=5011 last_handler=console-launch holder=unafs:render:12@cpu1 -> FAIL ::
[21:25:31Z] [lock] task=12 holds=[live.rs:521,columns.rs:190,unafs.rs:567,sdhc.rs:997] waits=none
```

`holds=[…]` are LOCK NAMES (the `#[track_caller]` construction site, `sync.rs:43-46`), not acquire sites:
`live.rs:521` = `MODEL`, `columns.rs:190` = `COLS`, `unafs.rs:567` = `unafs::MOUNT`, `sdhc.rs:997` = `CARD`.
The `[folderview] enter` line (printed INSIDE `after_show`) opens wedge 1 and the `[quarry2] list` line (the
LAST statement of `after_show`) closes it; the press's own `-> cd` line prints after `show` returns. Wedge 2 is
the same function reached from ATTRCOLUMNS' relist.

## 2. The chain (file:line at `45e03964`)

Not the painter. `columns::paint_list` (columns.rs:568) holds `COLS` and calls only pure helpers
(`rootdisk::mark` fs/rootdisk.rs:110, `bootfat::shows_lock` fs/bootfat.rs:93, `is_executable` live.rs:708,
`appres::blit_path_icon` fs/appres.rs:547 — `try_lock` + an in-RAM decode, no read). The block I/O is the
LISTING's tail, run under `MODEL` (held by every `show` caller) with `COLS` taken at its head:

* Wedge 1 (a tree-row press, `panel-render`): `press_route` live.rs:2418 → tree arm live.rs:2652 `m.show(&p)`
  → `Model::show` live.rs:1159 (`collect_cached` live.rs:1166, the one directory read) → live.rs:1186
  `columns::after_show(self)` → columns.rs:457 **`COLS.lock()`**, then, all under MODEL+COLS:
  * columns.rs:459 `is_trash` → columns.rs:362 `vfs_mount_table()` (mount probe);
  * columns.rs:460 `folderview::enter` → folderview.rs:348 `resolve(&mt, cwd)` (the folder's and its parents'
    `una:view` attributes — UnaFS `MOUNT`, `CARD`);
  * columns.rs:465-467 `vfs_mount_table()`, `trash::entries()` (in the Trash);
  * columns.rs:471 `compute_meta` → per row (32 in `/apps`): columns.rs:368 `list_attrs(cwd)`, columns.rs:374
    `listing_mime` → columns.rs:339 `mt.get_attr(path, una:type)` / columns.rs:351 `mt.read` (the sniff);
    columns.rs:375 `appres::sight_in` → fs/appres.rs:352 `mt.stat` + fs/appres.rs:365/381 `read_block` (the
    program's resource block — every `/apps/*.ELF` is a UNAOS_ELF); columns.rs:379 `assoc::name_of_in` →
    fs/assoc.rs:157 `reg_attr` (the registry's `una:description`);
  * columns.rs:472 `attrcols::after_meta` → attrcols.rs:197 **`ATTRS.lock()`** (the `attrcols.rs:69` hold the wire
    names), attrcols.rs:201 `af::view_of`, attrcols.rs:204 `list_attrs(cwd)`, attrcols.rs:210 `scan` →
    attrcols.rs:176 `list_attrs` per row;
  * columns.rs:475 `resort` → columns.rs:447 `livedir::resplice` → livedir.rs:111 `collect_cached` + livedir.rs:118
    `compute_meta` for every expanded folder's children.
* Wedge 2 (`console-launch`, the render pass's service drain): `live::service` live.rs:3209 → `columns::service`
  columns.rs:224 → `attrcols::service` attrcols.rs:231 (`[attrcols] facts … -> relist`) → attrcols.rs:262 `relist`
  → attrcols.rs:270 `m.show(&c)` → the same `after_show` chain (and `livedir::service` livedir.rs:236 →
  livedir.rs:259 `MODEL.lock()` → livedir.rs:221 `m.show` reaches it on every namespace generation:
  `[quarry] live dir=/apps gen=… relisted=32` every few seconds in the log).

So: per navigation and per relist, ~5 reads × 32 rows on the SD card, all inside `COLS` (and `ATTRS` for the scan)
on the render task. The attribute columns' lock `columns.rs:190` is held across `CARD` and `MOUNT`: LAW of the
arc (2) is broken at columns.rs:457.

## 3. The mechanism (what this commit changes)

1. **The listing installs PENDING facts, never reads them.** `after_show` (columns.rs) does its two folder-level
   reads (`is_trash`, `folderview::enter`'s `resolve`) with NO `COLS` held (MODEL only — the listing's own lock),
   then takes `COLS` for pure work: `load`, the trash flag, the meta vector rebuilt from the previous listing of
   the same folder by NAME (a relist keeps its types on the glass) or a pending row (`RowMeta.pending`, type `...`),
   the sort, the splice. It raises the facts latch and bumps the listing epoch.
2. **The facts pass** (`columns::facts_pass`, first in `columns::service`, on Quarry's service pass): snapshot under
   MODEL→COLS (no I/O) up to `FACTS_PER_PASS` = 4 pending rows; release EVERY Quarry lock; read the folder's
   context once per epoch (mount table, `list_attrs(cwd)`, the Trash origins, ATTRCOLUMNS' `una:view` keys via
   `attrcols::begin`) and the rows' facts (`row_meta`: type, sniff, APPRES sighting, type name, attribute cells via
   `attrcols::scan`); then retake MODEL→COLS→ATTRS and install by name when the epoch has not moved. The pass that
   leaves nothing pending applies FOLDERVIEW's attribute sort, re-sorts, and prints the old
   `[quarry2] list cwd=… typed=…` line plus `[quarryio] facts dir=<d> rows=<n> passes=<n> ms=<n>`.
3. **The splice reads before `COLS`.** `livedir::read_open` reads the expanded folders' children under MODEL only;
   `resplice` takes them (or re-uses the inline rows already in the list, harvested before the strip) and gives
   them pending facts. `livedir::press` and `tests quarrylive` read first, lock second.
4. **The paint reads only its model.** `millercols::listing` answers from its cache when the caller is painting and
   latches a miss for the service pass (`millercols::service` reads it and repaints).
5. **The witness.** `columns::io_note` sits at each Quarry read seam (`live::collect`, the facts context and each
   facts row, `is_trash`, folderview's resolve, `millercols::listing`'s read): `painting` = this task is inside
   `repaint` (`PAINTER` = `arch::sched::current_id()`), `service` = inside `facts_pass`; holds = `MODEL`/`COLS`/`ATTRS`
   `is_locked()` at the read.

Lock order after: `MODEL` → `COLS` → `ATTRS` → leaves, and no Quarry read between a `COLS` acquire and its release.
The residual named, not hidden: `Model::show`'s ONE directory read (`collect_cached`) and the two folder-level reads
still run under `MODEL` in the router — single reads, not per-row; moving the listing itself onto the service pass
is the ring-3 port's model change (quarry.md §7), not this arc's.

## 4. `tests quarryio` (rides `quarry3_tests`, no tests.rs line)

Opens Quarry on a scratch folder under the home (`QIOTMP`, five files: three typed by extension, two by sniff), runs
the service pass until no fact is pending (bounded, 64 passes), then N = 8 `repaint`s, counting:

`:: QUARRYIO: paints=<n> io_in_paint=0 io_in_service=<n> holds_max=<n> cols_in_io=0 pending=0 -> PASS :: dir=<d> passes=<n>`

PASS = `io_in_paint == 0 && io_in_service > 0 && cols_in_io == 0 && pending == 0 && paints >= 8`. `holds_max` is the
most Quarry locks held at any counted read (MODEL alone on the listing's read = 1; the facts pass = 0).
HONEST LIMIT: `is_locked()` is global, so a render-task paint overlapping the fixture's read can raise
`cols_in_io` — a false FAIL that names itself, never a false PASS.
Go-red by mutation (the seat's, at the fold): put `let _ = compute_meta(&crate::shell::vfs_mount_table(), &m.cwd,
&m.list, &[]);` back at the head of `paint_list` → `io_in_paint` = rows per paint → `-> FAIL`; or delete
`facts_pass()` from `columns::service` → `pending>0 io_in_service=0 -> FAIL`.

## 5. Proof owed to the fold

Compiled only (R107). The seat's fold: `tests quarryio`, `tests quarry2`, `tests quarrylive`, `tests attrcolumns`,
`tests quarry3`, `tests columnsview`, and on the next flight the wire below.

Next flight wire: on a press into `/apps`, `[folderview] enter dir=/apps` then `[quarry] press … -> cd` within one
render pass; then `[quarry2] list cwd=/apps rows=32 typed=32 …` + `[quarryio] facts dir=/apps rows=32 passes=8 ms=<n>`
over the following passes; NO `:: RENDERWEDGE: … -> FAIL` with `columns.rs:190` in a `[lock] task=… holds=[…]`,
and no `[lock] held-too-long name=columns.rs:190 at=columns.rs:<after_show>`.

## 6. What the executor ran (R107: compile + the PLAYBOOK's pre-commit leg, nothing else)

* `cd unaos/crates/kernel && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave35/target cargo +nightly check -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target ../../x86_64-unknown-none.json --features "$(cat /root/unaos-bench/scratch/cloud/wave35/kfeats24.txt),stria,gen7r9,wifi9,usbnet,wifi10"`
  → rc=0 (kfeats24 carries `quarry`, `wc`, `witness`). Run twice: the first gates run named a new-parser key for the
  fixture's PNG-signature file in columns.rs (GATE-ARCH); that file became a sniffed JSON file and the tree was
  compiled once more, rc=0.
* `cd unaos && ./arroyo gates` → `GATES: 17 run 16 green -> FAIL`: the one red is `gate=deps` — `BEHIND tokio (latest
  stable 1.53.3)`, every host crate's lock at 1.53.2. This commit touches no `Cargo.toml` and no `Cargo.lock`
  (`git diff --stat 45e03964 -- '*Cargo.lock' '*Cargo.toml'` is empty): the red is the registry moving, not this
  tree, and is reported to the seat rather than fixed out of lane.
