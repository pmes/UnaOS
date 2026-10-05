# WINDOWCAP (B378) — R90: no hard-coded cap; one dynamic limit as the emergency valve

Cut from hw-rmbp@a60219de (flight 23 flown on image 16). Executor branch `exec-rmbp-windowcap`.

## Finding (flight 23, boot 2)
After `storm` (six vugs) + console + shell + STAT + lumen + quarry + settings, `video::wm`'s table was
full: `MAX_WINDOWS = 12` fixed rows. Every later `wm::create` returned `WIN_NONE` with no line and no
alert: `[facet] refuse … reason=no-window(create-failed)`, `[fileview] refuse … reason=window create
failed`, `[login] screen open window=no (create refused — headless form)`. The process side carries the
same shape: x86 `MAX_PROCS = 10` and `WIN_MAX = 12` written as literals, aarch64 `MAX_PROCS = 6`.

## The seam
Kernel, by ruling (R90): `video::wincap` (new file, `//! CHARTER: Kernel — kernel-by-ruling`) owns the
ONE limit; `wm::create_inner` and the x86 `sys_spawn` ask it. No second table, no store.

* **The limit is derived, never written down.** `windows = min(mem, dock, ids)`:
  `mem` = half the heap free at arming ÷ the per-window kernel cost (pace shadow + mirror of the
  largest surface, 2 × 288×288×4 B, plus the row); `dock` = the most app tiles the dock can host on
  THIS panel beside its four pins (`dock::Layout::for_panel`, live, so the dock check holds for every
  table state by construction); `ids` = the compositor's id space minus the system rows (owner 0:
  the login screen, the notice, compat). `procs = min(asids, mem, windows)` where `asids` is the ring-3
  address-space pool minus its 2-slot reserve.
* **What counts:** app rows only (`dock_addressable`: used, not compat, owner ≠ 0). The login screen,
  the notice that SAYS the limit, and the compat row are never refused by the limit, so the alert can
  always open.
* **The id space** stays a compile-time array width, raised 12 → 32: 32 is the width of the per-slot
  `u32` masks (`PACE_PENDING`, `PACE_SHADOW_OK`, `NATIVE_SLOTS`, `VERIFIED`), an ABI fact, not a
  taste number. The two full-table stack copies (`composite_inner`'s snapshot and the witness
  `occ_clip_live`) move to the heap so 16 KiB task stacks do not pay 32 rows twice. x86 `WIN_MAX`
  and `MAX_PROCS` stop being literals (`= wm::MAX_WINDOWS`, `= USER_SLOTS - 2`).

## Milestones
* **M1** — `video::wincap`: the limit, armed once with `[wm] limit windows=<n> procs=<n>
  from=mem:<MiB>,asids:<n>,dock:<n>,ids:<n> (R90)`; the four dock checks (`desktop_uefi`, `main`,
  `quarry/live`, `desktop_firmware`, `display_tegra`) go parametric on `wincap::dock_rows()`.
* **M2** — the id space 12 → 32; heap snapshots; `WIN_MAX`/`MAX_PROCS` derived; deadman `ROWS`.
* **M3** — hitting it is SAID: `wm::create_inner` refuses an app row at the limit and prints
  `[wm] REFUSED create reason=limit n=<n> (R90)` (or `reason=ids`), and posts the notice
  `Too many windows open (<n>) - close one` (R70's surface, `users::screen_notice`, queue-only, opened
  by `notice_service`); the x86 ring-3 `sys_spawn` at the limit prints `[wm] REFUSED spawn
  reason=limit n=<n> (R90)` and PAUSES THE SPAWNER (sleep with doubling backoff 50 ms → 1.6 s, reset on
  a successful spawn) before `-EAGAIN`, so a runaway spawner burns its own time, not the machine's.
* **M4** — the witness at the desktop ignition (`boot::ignite`, hooked in M1): `:: WINDOWCAP:
  fixed_cap=<none|ids:28> limit=<n> procs=<n> opens_refused=<k> -> PASS ::` (PASS iff `limit >= 11` and
  nothing refused yet). `ids:28` = the 32-row id space less the 4 system rows; it is printed only when
  that term is the one that binds.

## Witness lines (x86 metal shape — ungated by any knob; `video` + `boot::ignite` are always built)
* `[wm] limit windows=<n> procs=<n> from=mem:<MiB>,asids:<n>,dock:<n>,ids:<n> (R90)` — once.
* `:: WINDOWCAP: fixed_cap=… limit=<n> procs=<n> opens_refused=0 -> PASS ::` — at `[boot] phase=desktop`.
* `[wm] REFUSED create reason=limit n=<n> (R90)` / `[wm] REFUSED spawn reason=limit n=<n> (R90)` only
  when hit; then `:: NOTICE-OPEN: title=Too many windows …`.

## Owed (said, not hidden)
* **`fixed_cap=ids:28` is printed when the id space, not memory, binds** (it will on the rMBP: 256 MiB
  heap ÷ ~650 KiB ≫ 32). Retiring the id space — the slab where each row carries its own pace stamps,
  shadow, title source and native bit instead of `[_; MAX_WINDOWS]` side tables and `u32` slot masks —
  is the next arc (WINDOWCAP-2).
* **The ring-3 address-space pool** (`USER_SLOTS`, 1.3 MiB of `.bss` per slot) is still static; the
  process limit's `asids` term is that pool. A heap-backed slot pool is the "real user-memory
  allocator" arc the memory.rs note already names. aarch64 `MAX_PROCS = 6` stays a literal (sched's
  `MAX_KILL_REQS` mirrors it); its limit line prints the same derivation.
* Stack: the remaining `[_; MAX_WINDOWS]` scratches grow 12 → 32 (DockEntry/WlRow rows ~1.5 KiB per
  scratch at 32). Unmeasured — the metal boot is the proof (R78).

## Landed (exec-rmbp-windowcap)
* M1 eb8d3cff, M2 34d3fe88, M3 714168b1 (+ this note). Compile legs, inline: x86 metal shape exit 0;
  aarch64 `login,loginst,virt_el0,lumen,desktop_firmware,quarry,facet,usbnet` exit 0,
  `tegra,login,loginst,virt_el0` exit 0, `login,loginst,virt_el0` exit 0. charter-check exit 0.
* The glass copy is ASCII: `Too many windows open (<n>) - close one` (title `Too many windows`) — the
  notice line is bytes through the kernel face; the em dash is not promised to render.
* The dock's runtime metric proof (`dock::uimetrics_assert`) is now stated for the pins + eleven app
  rows; a 32-row strip at scale 4 would exceed the 4096 px scratch, and `for_panel` refuses it, which
  is exactly why the dock term is IN the limit.
* What the next flight reads (rMBP, 2880x1800): `[wm] limit windows=<n> procs=10 from=mem:<MiB>,asids:10,
  dock:<n|none>,ids:28 (R90)` once; `:: WINDOWCAP: fixed_cap=… limit=<n> procs=10 opens_refused=0 ->
  PASS ::` beside `[boot] phase=desktop`; after `storm` + console + shell + STAT + lumen + quarry +
  settings, the screenshot / TEST.MD / holocron prompt OPEN (no `create-failed`); a deliberate overrun
  (`storm` repeated) prints `[wm] REFUSED create reason=limit n=<n>` and `:: NOTICE-OPEN: title=Too many
  windows`, and a ring-3 spawner past the process limit prints `[wm] REFUSED spawn … paused_ms=50..1600`.

## WINDOWCAP-2 (the seat's answers, 2026-10-05): memory is the only term

The seat: (1) the 32-id ceiling is not a stopping point — rows carry their own side state, the id space is
unbounded, the witness reaches `fixed_cap=none`; (2) the dock's panel width is NOT a term of the limit — the
dock shows what fits and overflows the rest; (3) the spawn backoff stands; (4) aarch64 `MAX_PROCS` derived,
`MAX_KILL_REQS` follows.

* **M5 — the table has no width.** `wm::MAX_WINDOWS` is gone. `Table.rows` is a heap `Vec<Window>` grown on
  demand (`alloc_slot`: the lowest free row, else append; id = slot + 1, so ids come from a counter with
  a free list). Every per-slot side table (pace stamps, shadow + mirror buffers, title source, native
  bit, generation, dock stamp, ctrl-decline cells, WC-N/WC-D/WC-G witness tables, the dock's DOCKID
  registry, deadman's attach counters) is a `video::rowstore::SegVec` — lock-free, segment-doubling,
  never moving, allocated on first touch — and every `u32` slot mask (`PACE_PENDING`, `PACE_SHADOW_OK`,
  `PACE_SHADOW_WEDGED`, `NATIVE_SLOTS`, `VERIFIED`, `VERIFIED_FULL`, `DO_NEIGH_MASK`, appmenu `WINS`,
  winlist's Show-Desktop mask) is a `SlotBits` or a list. The only bound left is the `WinId` TYPE
  (`wm::WIN_ID_SENTINEL_FLOOR = u32::MAX - 255`, the top kept for the dock's synthetic pin ids). Every
  `[_; MAX_WINDOWS]` stack scratch became a `Vec`: the per-pass ones are warm pooled buffers (`RowsSnap`
  for the table snapshot, `dock::ModelBuf` for the dock / bar / crystal / winmenu model) or are reserved
  BEFORE the table lock; the rest allocate on create/close/witness paths only. `OccClip`/`OccSnap`/
  `OccRows` and the span buffers grow. x86 `WIN_MAX` (ring-3 table) = `USER_SLOTS x FB_WIN_SLOTS`, the
  VA layout's count; aarch64 `WIN_MAX` the same expression (was 8).
* **M5 — memory is the limit.** `wincap::win_limit()` = half the heap free at arming ÷ `WIN_COST`; no
  id term, no dock term. `[wm] limit windows=<n> procs=<n> from=mem:<MiB>,asids:<n> (R90)`;
  `:: WINDOWCAP: fixed_cap=none limit=<n> procs=<n> from=mem:<MiB> opens_refused=0 -> PASS ::`. The
  create-time refusal reasons are `limit` (the memory limit) and `heap` (the table could not grow).
* **M5 — the dock overflows, it does not cap.** `dock::Layout::for_panel(n)` lays out every row when
  one-glyph tiles fit; otherwise the most tiles that fit at three glyphs, the LAST one the right-edge
  `+<k>` group (`Layout::overflow`). A press on `+<k>` opens the bar's Window menu (`winlist`: one row
  per live app window — the list of the rest); with no Window box on the bar it raises the overflowed
  windows in turn. `[dock] press … overflow=<k> -> window-list opened=…`. The five dock checks
  (`desktop_uefi`, `desktop_firmware`, `quarry::live`, `main`, `display_tegra`) ask `for_panel` for the
  LIVE count (`wincap::dock_rows` = live windows + pins) and now decline only a panel too narrow for one
  tile.
* **M6 — aarch64 `MAX_PROCS = USER_SLOTS - 2`** (6 on both boards today, now derived), and
  `sched::MAX_KILL_REQS` is the same expression through the same `uslots` facade (a non-EL0 build has
  no `Proc` table; its kill table keeps one inert row).
* **The spawn backoff (seat: fine).** A ring-3 `sys_spawn` refused at the process limit sleeps the
  spawner 50 ms, doubling to 1.6 s, before `-EAGAIN`. Any fixture that expects `-EAGAIN` from a full
  process table now takes **at least 50 ms** per refused spawn (the backoff resets on the next admitted
  spawn).

### Still fixed, named (not window or process caps, but tables the seat may want next)
* x86 `USER_SLOTS = 12` (and aarch64 8): the ring-3 address-space pool — `.bss` page tables + 1.3 MiB
  backing per slot. `procs` is `min(asids, mem)`; a heap-backed slot pool is the "user-memory allocator"
  arc `memory.rs` names.
* `FB_WIN_SLOTS = 4` windows per process (the per-slot FB region's VA).
* Menu publishers: `winmenu::WINMENU_MAX = 4` window trees and `appmenu::SLOTS = 4` app menus — a fifth
  publisher's menus do not attach (its window still opens).
* `wm`'s `MAX_DEFER = 16`: a coalescing erase queue (a full queue unions), not a count of windows.
