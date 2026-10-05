# GLASSLAG — rmbp-ledger B370 (branch `exec-rmbp-glasslag`, cut from exec-rmbp-merge14 at 2d4e1b12)

## Finding (flight 22, `f22-boots.log`)
- Peter at the glass: "delay in interactivity", "delay in opening when clicked in taskbar", lumen echo "with a
  delay"; `[wpace] rollup … rate=203/s -> FREE` says the compositor is not pacing — the wire had no line that
  says WHERE an event's time goes.
- The render service (the compositor's pass owner) runs `net_tick::service_tick()` on its ~5 s clock
  (main.rs, beside `emit_load_witness`). Unleased, its `dhcp_link_tick` → `dhcp_acquire` pumps the stack for
  `DHCP_WAIT_MS` = 2 s; the wire prints `:: SOCK-5: … no offer` seven times per boot, ~5 s apart (04:54:45 …
  04:55:22, again 05:58:00 … 05:58:40) — 2 s of a parked compositor each time. `witness_tick_sntp` waits on
  an NTP reply on the same thread once leased.
- "lowering brightness at first made it more brite": `backlight::LEVEL` starts at the ASSUMED default (12/16;
  "the firmware's level is not read back at boot"), so with the firmware panel below 11/16 the first Down wrote
  11/16 — brighter. At the bottom, Down at the floor re-wrote the floor even with the panel below it.
- QUIETBOOT `lines=384 bound=250 top=[kepler:263,…] -> FAIL`: 263 `:: kepler: <sub>` lines, all from
  `kepler::init` (wire lines 45..308; by sub-tag `ucode-post` 128, `FENCE` 40, `ucode`/`ucode-echo` 9 each,
  `fal-port` 8, `recon` 7, `ucode-poke` 6, `runlist-scan`/`beacon` 4, … 41 sub-tags in ~26 families).

## The seam
Kernel — `wm` (the instrument rides the router chain and the composite pass), the net drive seam
(`net_tick`), the one backlight writer (`video/backlight.rs`), the serial line layer (`serial_line`). No
handler domain is touched; no new store. New file: `video/lag.rs` (`//! CHARTER: Kernel — wm`).

## Milestones
- **M1 `[lag]`** (`video/lag.rs` + same-line hooks): one event per class (key press, pointer press) timed from
  `pal`'s enqueue funnel to the end of the composite pass that shows it, stages `queue` (ring wait) → `wm`
  (`wc_route_event` to the route: a ring-3 input ring, a kernel surface, a dock tile) → `app` (`sys_input_poll`
  read it / the router returned / the window was minted or raised / the menu opened) → `comp` (the app's present
  syscall, or the next composite pass for a kernel surface) → `present` (that pass ended).
  `[lag] key→echo|click→shown|click→window-shown|menu→open ms= queue= wm= app= comp= present= worst=`
  (keys only at ≥ 50 ms; launches, menus and clicks always), `[lag] … timeout ms= reached=` past 3 s (10 s for
  a launch), and every 5 s with any event: `:: LAG: n= key= click= launch= menu= p50= p95= max= max_kind=
  worst_stage= stage_ms=[queue:,wm:,app:,comp:,present:] timeout= orphan= coalesced= lost= span=5s ::`.
  No key value is printed (R65). Silent when idle. x86 + `wc` only; no-op elsewhere.
- **M2 service start off the compositor**: `net_tick::service_tick()` on x86 now starts the `net-tick` task
  once (`[net] tick task=net-tick cpu=<n> cadence_ms=5000`) and returns; the task runs the old body (DHCP
  link tick, SNTP witness) every 5 s off the render service. The render line in main.rs is untouched.
- **M3 BRIGHTFLOOR first step**: the first desktop pass reads the gmux register back once
  (`[backlight] seed readback=<raw> max=<m> level=<l>`) and seeds the level with the highest step at or below the
  panel; every key step is judged against the register's known value (`backlight::next_level`): Down writes a
  strictly lower value or nothing, Up strictly higher or nothing; `stage` records what the register will hold.
  KAT in `tests brightfloor`: `:: BRIGHTSTEP: raws=1024 down_ok=1 up_ok=1 flight22=1 seeded=<0|1> -> PASS ::`
  over every register value 0..=1023, plus flight 22's case (panel at 6/16, assumed 12: the old rule brightened).
- **M4 Kepler fold**: `serial_line::fold_open(":: kepler: ")` / `fold_close()` around `kepler::init` (pci.rs).
  Folded lines do not reach the wire or the QUIETBOOT count; at close at most 18 lines:
  `:: KEPLER: fold family=<f> n=<n> last=<last line of that family> ::` ×≤16, `:: KEPLER: fold rest=[…] ::`,
  `:: KEPLER: fold lines=<n> families=<n> kept=<n> dropped=<n> replay=tests keplerlog ::`. `tests keplerlog`
  prints every folded line verbatim then `:: KEPLERLOG: lines= kept= dropped= -> PASS ::`. A `census` build never
  folds. Expected boot 24: `:: QUIETBOOT: lines=~139 bound=250 census=0 -> PASS ::` (384 − 263 + 18).

## Witness (boot 24, the metal line)
- `:: KEPLER: fold lines=263 families=~26 kept=263 dropped=0 replay=tests keplerlog ::` before `:: BOOT:`;
  `tests quietboot` → `-> PASS`.
- `[net] tick task=net-tick …` once; no 2 s render stalls at each `SOCK-5`.
- Peter clicks a taskbar tile / types in lumen / opens a menu → `[lag] …` lines and `:: LAG: … worst_stage=<name>`.
- `tests brightfloor` → `:: BRIGHTSTEP: … -> PASS ::`; on the glass, the first Down darkens.

## Owed
- The LAG line names the stage; the fix of whatever it names (likely `app` for a launch: the dock's verb launch
  runs `shell::dispatch_command` and the ELF load on the render service) is the next arc's.
- `[quarry] key_route key=0x..` prints raw key codes on the wire (flight 22, 40+ lines) — a PWONWIRE neighbour,
  not this arc's.
- The 29 s valve and the login/firstboot service starts (`[boot] step` users-load/assoc-seed run in
  `users::service` on the usb-pump task) are INSTALLBARE's (B364).
- Not flown; compile legs only (R76/R78).
