# HIDSTALL2 (B509): flight 27, R103 §2, the pointer fixed once and for all

CHARTER: Kernel. The seams are `driver` (the EHCI HID pump, `drivers/ehci`), `wm` (the Dock `video/dock.rs` and the lag
instrument `video/lag.rs`) and kernel-by-ruling B414/B485 (LOCKREG's holder registry `sync.rs`, the arc's counters
`hidstall.rs`). No new file, knob, store or dotfile.

## What the f27 wire says (awk, `f27-boot1.log`)
1. **596 of the 617 `[lag] stall` lines read `stage=hid` and are not HID stalls.** Their `hid_gap_ms` is 1..9 (493 of
   them are 8) while `render=handler render_ms=108..700` or `pump_ms=111..118` crossed the 50 ms bound. `sec_roll` labelled
   the second with the largest of six small numbers, and the HID gap won. Only 10 seconds on the whole boot had
   `hid_gap_ms >= 16`. Five are the login minute (JOBSCAN B497 and REGISTRYCHUNK B508: masked 5249/10886 ms). The
   other five are `08:29:16-17` (`hid_gap_ms=125/311`, `pump=fatverb pump_ms=1705.9`, the fatverb storage witness, masked
   97/303) and `08:32:22-25` (`hid_gap_ms=273/447/435`, `pump=desktop-app pump_ms=1147.3` beside
   `[dock] trash state why=change full=1 took_ms=1145`, the Trash read during `tests quarry2`, masked 312/151/331).
2. **The hid pump shares its core with the `usb-pump`** (`[hid] pump task=hid-pump cpu=7`). Every masked span on that core is
   a span with no HID pass, and `masked_ms=` was the longest UnaFS attempt on ANY core, so it could not say who held the pump's
   core.
3. **The dock timeout was a late release, not a lost one.** At `08:27:33` the line `[dock] press at (2128,1715) app=shell -> armed`
   came in the same second as `[lag] stall … stage=comp stage_ms=2816 … app=113.9` and `key→echo timeout ms=3066.6`: the
   render task, which is the release's only router, was busy. The timeout (`lp_service`) runs on the `usb-pump`, a different
   lane, so it fired at 400 ms while the release waited in the pal ring. Nothing on the wire recorded when the release was
   pushed.
4. **The pointer route went dark.** QUIETBOOT moved `[tp] mt` and the `[tp] ids=` census to the bootlog, so no
   serial line shows the trackpad lane at the desktop.

## The change
- **The holder census (1, sync.rs + hidstall.rs + lag.rs + ehci).** `hid_task_start` records the pump's core. LOCKREG's
  per-core tick (`sync::isr_tick`, every 1 ms) stamps each core's tick. A masked span holds the LAPIC tick pending, and the
  CPU takes the tick the moment interrupts return, on the task that masked them. On the pump core, a tick-to-tick gap
  of more than `1 + HOLD_OVER_MS` (5 ms) is therefore a masked hold of `gap-1` ms. It is charged to a holder named
  `<task>[:<usb-pump step in flight>][@<outermost LOCKREG acquire site>]`. The step in flight comes from `lag::pump_step_now`:
  the loop's marks are in a fixed order (`pump_next`). The holder table has 16 slots, uses atomics only, and has one writer,
  the pump core's ISR. When it overflows, the extra holds are counted.
  - On the stall line: `… masked_ms=<n> holder=<name> hold_ms=<n>` (the second's worst pump-core hold).
  - Once a minute at the desktop: `[hid] stall census holder=<name> n=<n> worst_ms=<n> session_n=<n> session_worst_ms=<n>` for
    each holder seen in the minute (up to 8), then always
    `[hid] stall census holders=<k> worst_ms=<n> worst=<name> unnamed=<n> overflow=<n> pump_cpu=<c> over_ms=4 -> CLEAR|HELD`.
  - `:: HIDSTALL: …` gains `holders= hold_worst_ms= hold_worst= releases_waited= lane_max_ms= pump_cpu=`. The verdict is unchanged.
- **The honest stage (lag.rs).** `stage=` names the stall's trigger. When no stage reached 50 ms it reads
  `render-route|render-handler|render-composite` (from `render_ms`) or `pump` (from `pump_ms`), so `stage=hid` now means the HID
  gap itself. `stage_code`/`stage_word` carry the new names into the boot summary.
- **The Trash read off the pump's core (2, dock.rs).** `dock2_store_service` runs on the `usb-pump`. On a Trash change it now
  spawns a one-shot `dock-trash` task on `smp::xhci_worker_cpu(0)` (neither render nor service, which is the storage-loan rule)
  when that core is not the pump's. Without one it reads inline and says so. `TRASH_READER` keeps one read in flight.
  Line: `[dock] trash state why=<first|change> full=<0|1> took_ms=<n> on=<worker|pump>`.
- **The release lane (3, ehci + hidstall.rs + dock.rs).** Both trackpad arms (0x02 legacy and vendor) stamp each button edge
  when they push it (`note_button_edge`). At 400 ms the Dock's timeout first asks the lane. If `pal::release_edge_pending() > 0`,
  a release was pushed and not yet routed, so the press waits for it (bounded by `DOCK_RELEASE_LANE_CAP_MS = 2000`, because
  the counter is advisory) and says so once:
  `[dock] press at (x,y) app=<a> release=in-lane pend=<n> after_ms=<n> lane_ms=<n> -> waiting`. The release's own line is new:
  `[dock] press at (x,y) app=<a> release=edge after_ms=<n> lane_ms=<n> waited=<0|1> -> launched`. A timeout still happens
  when no release is in the lane, and its line now carries `pend= lane_ms=`.
- **The trackpad on the wire (4, ehci + hidstall.rs).** While a finger is down, at most one serial line per 10 s:
  `[tp] mt route=vendor ep= fingers= mover= x= y= dx= dy= buttons= frames= ids=02:<n>,44:<n>,other:<n>` (vendor) or
  `[tp] mt route=legacy ep= buttons= dx= dy= ids=…` (0x02). The ids are the census counts.
- **`tests inputstall` (5).** It prints the live minute's INPUTSTALL reading so far, `… -> PASS|FAIL :: … via=tests`, and resets
  nothing. It is registered by `hidstall::ensure` (no tests.rs line).

## What the next boot should show
- At the desktop, once a minute: `[hid] stall census holders=0 … -> CLEAR` on a quiet desktop. During fixtures or a Trash op,
  it reads `holders=<k> … worst=<task:step@file:line> -> HELD`. A holder that is still there on a quiet desktop is the next fix.
- `[lag] stall … stage=render-handler stage_ms=108 … hid_gap_ms=8 … holder=- hold_ms=0`, where flight 27 printed
  `stage=hid stage_ms=8`.
- `[dock] trash state why=first full=<0|1> took_ms=<n> on=worker`.
- A dock click: `[dock] press … -> armed`, then `[dock] press … release=edge after_ms=<~100> lane_ms=<n> waited=0 -> launched`.
  If the render lane is busy, the press reads `release=in-lane … -> waiting` followed by `release=edge … waited=1`, never
  `release=timeout` for a release that was pushed.
- `[tp] mt route=vendor …` every ≥10 s while the pad is touched.
- `tests hidstall` → `… dock_timeouts=0 … holders=<k> … pump_cpu=7`; `tests inputstall` → `:: INPUTSTALL: … via=tests`.

## Honest limits / owed
- The census NAMES holders. In this arc only the Trash read moves. The fatverb storage witness (`pump=fatverb`, a one-shot
  witness), `desktop_app_service`'s other steps and whatever the census finds on a quiet desktop are named on flight 28 and
  moved after that. The pump core's ~8 ms quantum gap (`hid_gap_ms=8`, `QUANTUM_TICKS=4` round-robin with a busy
  `usb-pump`) is not a masked hold, so the census does not count it. Raising `hid-pump` to `PRIO_HIGH` would close it, but
  that risks a raw-spinlock priority inversion on the shared core, so it is not done here.
- A tick gap also counts a long ISR or an SMI on the pump core. It is charged to the interrupted task.
- `lane_ms` reads the EHCI trackpad's stamps only. A release from an xHCI mouse is not stamped, so `lane_ms` reads 0.
- `tests inputstall` reads the minute so far. Its `strand_pct` (the vug's strands, 6..18 % on f27 while vugs ran) is not HID and
  can still FAIL it. Ten stable PASSes are for the metal to show (R78).
