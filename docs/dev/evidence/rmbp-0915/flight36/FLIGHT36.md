# FLIGHT 36 — image 28 on the UNAFS CARD IMAGE (rmbp12flight36, hw-rmbp@1719fe54), 2026-10-14, main bench — IN PROGRESS

Medium: `UnaOS-rmbp-esp-rmbp12flight36-20261013T2355Z-1719fe5` (.card.img sha256 2404415b…). Tree = the cloud tip 1719fe54: the flight-35 fix wave (GATESTEAL B657, DONGLEBOOT B658, WINEDGE B659, SOFTY dark, dragmouse) + STARFIELD B661 + TEMPLE's core. Line = image 27h's 56 knobs.

## Peter, verbatim (the glass)
- "next boot ready"
- "i can only sort of see the wallpaper through the flawed window borders. boot was the same"

## 1. Boot 1 (fresh card, dongle out)
`:: LOADER: 12659ms/3135ms`; `[starfield] gen wall_ms=152 stars=2997 lensed=2457 streams=3991 facets=17`; `:: BOOT: total=20817ms`; `[font] face=Inter`; login 00:09:11; `[desktop] built at=login ok bar_ms=44 … pins=11 held_ms=12`. **The screenshot (Cmd-Shift-3 at 00:10:11) SURVIVED: `PRTSCR: slice n=1..` streamed, `PRTSCR: SCREEN0.PNG 2880x1800 314346 bytes -> OK … bytes_written=314346 chunks=5`** — no steal, no wedge (flight 35's trigger). Typed: **DESKTOPCRAFT `corners=ok shadow=ok edge=ok(1px) hairline=ok black=ok -> PASS`** (+ cost UNDER); **STARFIELD `gen_ms=150 stars=2997 lensed=ok streams=3991 zircon=ok(facets=17) beam=ok shimmer_us=27 -> PASS`**; **SOFTY `… dark=19/19 accent=table pressed=ok -> PASS`** (the fold seam closed); SHOTZIP, SHOTMOUNT, IMGVIEW, AETHER PASS.

**GATESTEAL, half-fixed (00:11:49, during `tests softy` again — flight 35's boot 1 too):** `W5: nmi core=c7 taken=y`, **`[pump] REHOME: c7 is dead and carried the usb-pump (tid 9): moved ready=0 sleeping=4 left=0 current=-1 -> c2 svc=c2 try=1`**, `[wcser] GATE STOLEN from c7 by c1 after 1505ms … c7 is DEAD`, `PUMPWEDGE: age_ms=2390 last_step=store-services … rehome=none -> FAIL :: pump=9@cpu7`, REHOME try=2, `[pump] wedge cleared` once, then `PUMPWEDGE … wedges=2`, the cable rescue from 00:11:52. The re-home moved the dead core's queues (4 sleeping tasks) but **the pump WAS the current task on c7 and "the current task is in no queue and stays"** — it stayed on the corpse. The door died (`tests dragmouse` never echoed); the internal keyboard/trackpad (EHCI) stayed alive (`[ptr] fine n=4672`, a second Cmd-Shift-3 chord at 00:11:55 registered). THE REMAINING HOLE: re-home the dead core's CURRENT task too.

**The look (Peter):** "i can only sort of see the wallpaper through the flawed window borders. boot was the same" — WINEDGE's 1 px edge passes its own fixture (`edge=ok(1px)`) and does NOT read as a window border to Peter; the starfield is there but the windows' borders are the flaw he sees. His eyes rule (R89): the window frame is still owed as he sees it, not as the fixture measures it.

Not reached on boot 1 (the door died at dragmouse): winresize, menuscraft, menubar, statustray, dockpins, notify, doorsplit, ptrpress, filetypes, **gen7, kgr, wifi**, stria, quarryhover, motion, the standing list.
