# DOCKGONE (rmbp-ledger B498) — the dock strip vacated and not repainted

Branch `exec-rmbp-dockgone`, cut from 668cdd95. Flight 27 (image 20), Peter: "the taskbar disappeared at some point."

## Finding (from the wire, `f27-boot1.log`, awk on `[strip]` / `dock`)

- The wire holds ONE dock vacate, not two: `[08:29:57Z] [strip] vacate tenant=dock box=2770x130+55+1640
  uncovered_px=35620 erased=yes src=flat -> SCENE-RESTORE`. The only dock paint line is
  `[08:27:34Z] [strip] paint tenant=dock box=2222x130 … -> AT-RISK` (08:37:48 / 08:39:50 are not on this wire). Both
  lines are LATCHED once per boot (`SAID_RESTORE`, `SAID_TORN`), so the wire says "the first", never "the last".
- It is not a lease or a timer. The vacate sits inside `tests winmemory` (08:29:57): two `wmtest` windows open
  (dock 8 -> 10 tiles), then close (`tile remove … reason=close`). 2770 is the 10-tile dock, the next is the 9-tile
  dock 2496 wide at +192; 35620 = 274 x 130 = one tile's width, the two 137-px ends. The 2770 rect is the dock's own
  previous (wider) geometry — correct, not stale.
- The defect is in what the vacate DOES with that rect. `dock::compose` erased the WHOLE old box
  (`strip::erase_rect(old)`, 2770x130 flat DESKTOP_BG — including the 2496 the dock is about to repaint) and
  THEN painted. `strip::paint` declines on a contended panel/scratch lock (composites run masked: `try_lock`); on a
  decline `compose` returned with the strip's whole area flat and the only re-drive was the next `compose_all`,
  which a per-window present (the shell typing) never runs. And `restore_vacated(old)` handed the whole old box
  back to the desktop layer too. A dock gone flat with nothing owing it a pass is Peter's "disappeared".

## The seam (no new mechanism, no new knob)

`video::strip` is the furniture primitive (CHARTER: Kernel — wm); the fix is in the primitive and its dock tenant:

- `strip::vacate_by(name, old, new, owed, by, reason)` — `vacate` is now it with `by=vacate`; it ERASES AND HANDS
  BACK ONLY THE UNCOVERED BANDS (`old` minus `old ∩ new`, at most four rects), never a pixel the new strip owns. The
  vacate line names its caller: `… by=<fn> reason=<why> -> SCENE-RESTORE`.
- `dock::compose` paints FIRST, vacates the ends only after the paint landed. A declined paint vacates nothing (the
  old strip stays on the glass, the slot keeps the old rect, so the next pass retries both) and arms the dock's own
  `PASS_OWED`, which `dock2_service` already takes with `wm::composite()` on the service pass.
- The WHOLE-strip vacate (layout `None`) happens only for auto-hide (the user's pref), a panel that cannot host the
  strip, or Log Out (`vacate_off`); each is printed `[dock] gone box=… by=… reason=autohide|unhostable|logout`, and
  one with the session open and auto-hide off is counted in `GONE` (the Mac's dock never leaves on its own).

## Milestones

- M1 — design (this file).
- M2 — `strip::vacate_by` + `uncovered_bands`: ends-only erase/handback, `by=`/`reason=` on the vacate lines.
- M3 — `dock::compose` paint-then-vacate, decline -> `PASS_OWED`; `[dock] gone` on the whole-strip arms.
- M4 — `tests dockgone`: 180 simulated seconds of service passes; asserts the ends arithmetic on flight 27's own
  rects, no in-session whole vacate, the strip present at the end.

## Witness (the next metal boot)

- `[strip] vacate tenant=dock box=2770x130+55+1640 uncovered_px=35620 erased=yes src=… by=dock::compose reason=shrink -> SCENE-RESTORE`
- `[dock] paint declined after=shrink -> OWED (service pass repaints)` (once, only if it happens)
- `tests dockgone` -> `:: DOCKGONE: vacates=0 paints=<n> present=1 ends=2 ends_px=35620 -> PASS ::`
- A `[dock] gone … reason=` line with the session open and auto-hide off is the defect, by construction.

## Owed

- Peter's card does not time the disappearance; the wire's latched lines cannot prove THIS decline is the one he
  saw. The next flight's `[dock] gone` / `paint declined` lines and an un-latched read of the dock after
  `tests winmemory` decide it.
- A declined ENDS erase (`-> STALE-ENDS`) still waits for the dock's next signature change to settle (B74's
  `settle` sits after the quiet-pass return) — unchanged here.
