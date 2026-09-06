# FLIGHT-RESULT — render7 (orin 16, 2026-09-06T11:41–12:03Z)

Image `render7-20260906T0445Z-7be8155` (hw-jetson `7be81559`; branch tip at flight time `37c78ad7`, which differs
only by the orin 15 close report), kernel.elf sha256 `c6dc3960ef7ae164…`, ELF max vaddr `0x2f5e40`. Knob line =
render6's + `UNAOS_TCURX=1`; effective features
`witness,ehcihid,holocron,tegra,orinclick,tegra_el0,tegrasmp,orinrender,desktop_firmware,orinrx,tcuprobe,tcurx,deskcascade`.
One boot, one power cycle — up at 11:41:29Z, Peter cut it at 12:03:48Z ("machine off"), 22.3 minutes on the glass.
Port `/dev/ttyACM1`: the debug probe re-enumerated after a replug at 11:28Z, so the ttyACM0 butler (pid 34809) was
retired and pid 756188 took the capture. Scored per `FLIGHT.md` §C with
[`scorers-render7.sh`](scorers-render7.sh) (committed beside this file; every token in it carries its source
`file:line`, quoted per row below), verdicts in [`render7-scores.txt`](render7-scores.txt); excerpt
[`render7-boot1.log`](render7-boot1.log) (11650 lines, sha256 `a7a1eb0a1977bb08…`); marks in
[`render7-marks.txt`](render7-marks.txt); paced injector log
[`render7-paced-inject.out`](render7-paced-inject.out).

**Every excerpt line number in this report refers to the COMMITTED `render7-boot1.log`** (11650 lines, anchored —
see Pin and purity). An earlier draft numbered against an unanchored 11627-line cut; those citations were 23 lower.

**Manifest header correction.** The staged `MANIFEST` header says "NOT aboard: the xHCI dup/nobuf counters (patch,
awaiting pi re-accept)". That line is STALE. `strings` on the staged `kernel.elf` finds `DUP-DROP`, `NOBUF-DROP` and
`ARMED-NO-COMPLETION`, and the wire's `[ptrpoll]` lines carry live `dup=` / `nobuf=` fields
(`MOUSE_DUP_DROP_COUNT` / `MOUSE_NOBUF_DROP_COUNT`, `drivers/xhci/mod.rs:2385`). The A20 instrument was therefore
**aboard**. That is a statement about the image, not a verdict on the instrument — see the A20 (pointer) row: the
pointer never died on this boot, so the instrument was **untriggered** and its zero counts are unfalsified, not
validated.

## Pin and purity (§C.1–C.2)
- Boot slice starts at `orin.log` line 68180 (`MARK render7 7be8155 pre-boot … seat=orin16 at 2026-09-06T11:39:41Z
  raw=11731299 orin=68179`); burst injection at `orin=69525` (11:43:14Z), paced at `orin=69676` (11:43:34Z),
  power-off mark at `orin=79806` (12:03:48Z).
- Purity: `pi_marks=0` over the excerpt (no `RPI`/`BCM2711`/`raspi` token); 1745 lines carry a tegra/orin token.
  PURE.
- **Anchor — RESOLVED (orin 16, this commit).** An earlier draft's excerpt began at the butler's resolve line
  (`=== butler RESOLVED: unknown -> orin after 926 unidentified lines over 19.6s …`) and carried no
  `KELF min=… max=…` loader anchor, which GATE-LEDGER's boot-anchor check rejects. The anchor was not lost, only
  mis-filed: the butler had not yet identified the port when this boot started, so its first 926 lines went to
  `unknown.log`, and the loader identity is `unknown.log:16850` —
  `[ INFO]: crates/bootloader/src/main.rs@792: KELF min=0x0 max=0x2f5e40 pg=758`, matching the mark's
  `elfmax=0x2f5e40`. The committed excerpt is therefore `unknown.log` 16850–16872 (23 loader lines, anchor first,
  the orin15 convention) followed by `orin.log` 68180–79806 (11627 lines) = **11650 lines**, sha256
  `a7a1eb0a1977bb08…`. Line citations below are against this file.

## Scores
Every verdict line `scorers-render7.sh` printed, verbatim, with the source that mints its token.

| q | scorer line (verbatim) | token source | verdict | ledger |
|---|---|---|---|---|
| A15 | `A15 smp: cpu_on_success=5 cpu_on_error=0 el3_abort=0 poweroff=0 online_line=1 -> PASS (A15 pass 5)` | `ORIN-SMP-3 CPU_ON AP n -> SUCCESS` | **PASS — pass 5 of the APTEXT layout** (render3b, render4, render6 ×2, render7); 0 deaths since APTEXT | A15 |
| A1 | `A1 u7stk: arming=1 post_cascade=1 len=32768 hw=14400 (render6=15552 delta=-1152) headroom=18368 -> PASS-SHALLOWER (1152 bytes below render6 — check the quarry actually opened)` (`[u7stk] at=boot-core:post-cascade … used=240 hw=14400 headroom=18368`; pre-cascade `hw=240`) | `arch/aarch64/sched.rs:10650` | PASS, unsaturated. The scorer's caution is DISCHARGED: the quarry DID open (A8 below), so the cascade got DEEPER work and 1152 fewer bytes of stack. The four samples now read 15472 / 15584 / 15552 / **14400** — render7 is the first that is not within ~112 B of the others, and the delta is negative, so the saving is a *different call graph*, not noise: `quarry::open()` replaced whatever the knob-off `{}` stub's caller frame cost. Headroom 18368 of 32768 | A1 |
| A18 | `A18 cascade: cascaded=1 refuse=0 pulsewin_open=3 pulsewin_decline=0 strip_kept=0 census_strip=retired census_pulsewin=3 -> PASS` (`[deskcascade] -> CASCADED windows=2 bar=1 owns_pixels=1 route=ROUTED activate=false`; `[pulsewin] open win=3 … box=1290x212 at (10,914) view=Pi LED lamps`) | `[deskcascade]`/`[orinrender] census` | PASS, fourth pass. `pulsewin_open=3` is NOT three windows — it is one window opened three times, because closing it reopens it (A30 below) | A18 |
| A19 | `A19 band: band_cleared=1 shell_present=1 jd2_probe=1 -> WIRE PASS (now A19-pngband.py SCREEN0.PNG must read non-bg=0/60200)` (`[realdesk] band-cleared x=0 y=34 w=1920 h=1166 bg=2d2b55 shell=win=4 surf=960x466 box=970x510 at (515,402)`, `[realdesk] shell-present win=4 outcome=Composited`) | `[realdesk]` | WIRE PASS, second pass. Pixel leg **harvested but not yet read** — `SCREEN0.PNG` is off the card (`render7-card-harvest.sha256`); the band read still owes a run against the full-resolution file | A19 |
| A20 (routing) | `A20 clicks: arm_click1=1 orinclick_armed=1 clickroute_press=19 consumed=30 routing_census=0 -> PASS (A20 flown)` (`[clickroute] press hit furniture asid=4294967041 win=1 (was 0) consume -> shell focus`) | `[orinclick]`/`[clickroute]` | PASS — 19 presses routed, 30 consumed, all boot | A20 |
| A22 | `A22 tcu: arm=1 stop=0 census=1239 full_final=0 nbytes=0 full_edges=2 changes=4 data=[00 00 00] -> ROW2: FULL-SEEN then consumed` | `arch/aarch64/hsp_tegra.rs:310` | **ROW 2 of TCURX-DESIGN §7.** render6 parked at ROW 1 (mailbox full forever, nobody consuming); render7's drain empties it — two full-edges, four changes, `full=0` at the end | A22 |
| liveness | `HEALTH: heartbeat=1 el1=1 arm=1 armed=1 live=1237 redzone=0 exceptions=0 -> PASS` | — | PASS | — |
| A16 | `A16 tcurx2: tcurx_took=7 took_total=7 serialrx_census=1227 rx_final=11 mbox_final=7 ovrf=0 tcu_full_final=0 keys=21 -> PASS rung 2 (the consumer took 7 byte(s) and left the mailbox empty)` (`[serialrx] rx=11 (+0) polls=220504594 refused=0 ovrf=0 lsr0=0x00000200 mbox=7 -> RX-LIVE`) | `arch/aarch64/hsp_tegra.rs:391`, `arch/aarch64/serial.rs:830` | **PASS — rung 2 flown.** The CCPLEX now consumes the TCU RX mailbox: 7 bytes taken, mailbox left empty, no overrun. This is the A16 fix working | A16 |
| A16 burst | `A16 leg burst: keys=5 tcurx_took=2 rx_after=5 mbox_after=2 ovrf_after=0 -> PASS 5/5` · keys in window: `KEY 's' :: KEY 't' :: KEY 0x0d :: KEY 't' :: KEY 'e'` | `main.rs:2943/2945` | **5/5 delivered — but OUT OF ORDER.** Injected `tste\r`; UARTC delivered bytes 2,3,5 (`s`,`t`,CR) directly and the mailbox drain delivered bytes 1,4 (`t`,`e`) 16 lines LATER. The shell therefore executed `:: [midden] cmd="st" -> TerminalError ::` and the late `te` fell into the next line. See A37 | A16, **A37 (new)** |
| A16 paced | `A16 leg paced: keys=6 tcurx_took=5 rx_after=11 mbox_after=7 ovrf_after=0 -> PARTIAL 6/5` · keys in window: `KEY 't' :: KEY 's' :: KEY 't' :: KEY 'e' :: KEY 0x0d :: KEY 0x0d` | `main.rs:2943/2945` | **6 keys for 5 bytes — MECHANISM RESOLVED, not open.** The injector sent exactly five (`inject-paced done … sent=5`, one `0x0d`), so the host is exonerated. On the wire the first four keys each sit directly under a `[tcurx] took=…` line (took-total 3,4,5,6 — the mailbox path); the FIRST `KEY 0x0d` has NO `[tcurx]` line above it (the direct UARTC read) and the SECOND is `[tcurx] took=0x0d '.' … took-total=7`. **The same CR was delivered twice, once by each path; there is no dedup.** Consequence on the glass: `:: [midden] cmd="tetste" ::` then a second empty submit (`[gui] app-enter t=125s` / `app-exit dur=0s`). See A37 | A16, **A37 (new)** |
| A27 | `A27 drag: wired=1 control_absent=0 arm=8 end=8 steered=8 fed-no-move=0 no-feed=0 \| wm_drag_begin=11 wm_drag_end=11 placed=8 no-move=3 -> PASS A27 (steered 8 gesture(s), wm placed 8)` (`[dragroute] wired panel=1920x1200 desktop_firmware=1 -> READY`; `[dragroute] end win=2 via=release fed=56 applied=42 at (423,99) -> STEERED`; `[wm-act] drag-end win=3 owner=0xffffff60 at (15,953) -> no-move`) | `arch/aarch64/display_tegra.rs:5197/5246`, `video/wm.rs:16214/16216` | **PASS — A27 flown.** render6's signature (drag-end → no-move ×4, `DRAG_MOVES` stuck at zero) is gone: 8 of 11 grabs placed. The 3 `no-move` ends are the pulse window (`win=3`) dragged to `(15,953)` twice and `(384,222)` once — clamped, not dead. The applied/fed ratio (47–75%) is **the pacer's designed coalescing, not lost motion** — see the observation below the table | A27 |
| A8 | `A8 quarry: quarry_open=4 skip=0 decline=0 pidesk_true=1 deskquarry_seat=1 compiled=1 open=1 relatched=0 -> PASS A8 (compiled=1 open=1, the window is on the wire; now count 4 dock tiles on the glass)` (`[quarry] open win=1 surf=1152x720 ts=2 box=1162x764 at (379,203) volumes=1 tree-rows=1 list-rows=0 cwd=/`; `[pidesk] quarry open=true`; `[deskquarry] seat compiled=1 open=1 windows=2 relatched=0`) | `video/quarry/live.rs:1806`, `video/desktop_firmware.rs:396`, `main.rs:8943` | **FLOWN WITH A DEFECT.** The window exists, opens, closes and reopens from the dock (`[dock] press … tile=0/4 quarry=pin -> open requested`, four tiles). But it lists NOTHING: every open prints `[quarry] open census cwd=/ entries=0 dirs=0 files=0` followed by `[quarry] open census ERROR cwd=/ "/: backend error: unafs-mount"`. That is Peter's "files not showing up in quarry" — and it is NOT a quarry fault (A28) | A8, **A28 (new)** |
| A8 count | — | — | **Scorer over-count.** `quarry_open=4` counts 3 real windows (`win=2` at line 509, `win=2` at 9670, `win=1` at 11420) plus one false positive: the `[deskquarry]` witness prose at line 523 QUOTES the string `` `[quarry] open win=…` `` inside its own explanation, so `/\[quarry\] open win=/` matches it. `open census ERROR` counts 3, which is the true window count. Every token-counting scorer over this image has the same hazard | **A38 (new)** |
| A25 | `A25 winmenu: publish=5 publish_refuse=0 contend_refuse=0 open=3 last_title=View pick=2 dismiss=3 (esc=0 outside=1 pick=2) -> OPENED and dismissed, but NOT by esc (reasons above) — press Esc with the menu down` (`[winmenu] open title=View items=2 at (193,34) owner=3`; `[winmenu] pick owner=3 id=0 label=Pi LED lamps`; `[winmenu] dismiss reason=outside owner=3`) | `video/winmenu.rs:327/634/650/728` | **FLOWN.** R21 satisfied on the wire: the pulse window's `View` menu is published to the BAR, opens from the bar, and picks work (`id=1 x86 segments`, `id=0 Pi LED lamps`). Peter: "view menu works". Three defects ride along — placement/typeface (A31), the absent app menu (A32), and `esc=0`, which is a REAL NEGATIVE this flight, not an untested leg (see the A10 section) | A25, A10 |
| A25 neg | `A25 negative: in-window title_press=0 in-window menu-dismiss=0 (winmenu pick callback=2, expected>=0) -> PASS (no in-window View strip token — R21 holds)` | `pulsewin.rs@f2eae02:834/864/892/907` (tokens gone at `37c78ad7`) | PASS — the render6 in-window strip is retired, not merely hidden | A25 |
| A26 | `A26 conquiet: console_route=1 console_window=1 mirror_off=1 (at line 1) census=0 dropped=0 -> PASS-WEAK (mirror=off once; fewer than 256 lines dropped so no census line — read the glass)` (`[wc-x] console-route first-paint win=1`; `[conquiet] mirror=off since=console-window-route win=1 lines_dropped=1 knob=bootlog`) | `video/fbcon.rs:1227/3018/3025` | **PASS-WEAK — flown weak.** The announce fired exactly once and only ONE line was dropped, so the 256-line census never printed and the strong leg is unscored. Peter did not report a scrolling kernel log in the console window, which is the glass half of the answer; the strong leg needs a boot with `UNAOS_BOOTLOG` volume | A26 |
| A20 (pointer) | `A20 ptrpoll: lines=37 first_rearm=497 first_dup=0 first_nobuf=0 \| final rearm=8552 dup=0 nobuf=0 reports=8552 \| verdicts STREAMING=36 BASELINE=1 GUARD-REARM=0 DUP-DROP=0 NOBUF-DROP=0 ARMED-NO-COMPLETION=0 -> RE-ARMED (rearm=497 > 2 at census 1 — the pipeline is moving; a dead click is a ROUTING fault)` · first `[ptrpoll] t=71 rearm=497 … base=497 decoded=0 -> BASELINE` · last `[ptrpoll] t=4831 rearm=8552 … decoded=8055 -> STREAMING` | `arch/aarch64/display_tegra.rs:5389/5391/5396/5397`, `drivers/xhci/mod.rs:2385` | **Instrument aboard, UNTRIGGERED on boot 1 (pointer alive; `dup=0 nobuf=0` unfalsified, not validated).** The pointer is ALIVE for the whole of boot 1: 8552 re-arms, 8055 decoded reports, endpoint never quiet (`ARMED-NO-COMPLETION=0`). **The failure the CLICKDEAD instrument exists to name never occurred, so the instrument was never exercised** — zero counts on a fault-free boot are equally consistent with a working instrument and with one that cannot fire, and this boot does not separate them. Do NOT read this row as a pass for the instrument. render6 boot 1's death did not recur; A20 stays open as INTERMITTENT with one more live sample, and validation waits on a boot in which the pointer actually dies | A20 |
| A17 | `A17 prtscr: armed=8 capturing=7 ok=7 inflight_refusals=0 other_refusals=0 names=[ SCREEN0.PNG SCREEN1.PNG SCREEN2.PNG SCREEN3.PNG SCREEN4.PNG SCREEN5.PNG SCREEN6.PNG ] -> FAIL A17 GAP STANDS (8 presses, 7 verdicts, 1 silent — no refusal named)` | `drivers/xhci/mod.rs:4929`, `video/prtscr.rs:178/191/276/387/546` | **GAP STANDS, and it is Peter's "3 presses in a row didn't do 3".** The three-press burst is on the wire at lines 2738 / 2741 / 2742: press 1 armed and captured `SCREEN3.PNG`, presses 2 and 3 landed while that capture was in flight and produced `SCREEN4.PNG` plus **one silent swallow** — no `Refusal::InFlight`, no `capture skipped`, nothing. 8 armed → 7 files. Second half of the row is new: the wedge (A36) | A17, **A36 (new)** |

## Glass — Peter at the bench (verbatim, ~12:0xZ)

> machine wedges for a bit when prt sc pressed. 3 presses in a row didn't do 3 apparently / mouse dragging not
> perfectly fluid but pretty good / view menu works but is misplaced and different font from main app menu item font
> and main app menu does nothing. / there should be quit in apps' main menus (like under where it says pulse) at bare
> minimum / files not showing up in quarry / closing pulse reopened it immediately, but / i cannot launch apps from
> cmd line ( no core is at el1 ) / crystal menu has a gap to the left as should be seen in scr 5 and it should be all
> the way to the right edge / crystal restart/shut down not working not expecting sleep yet / mouse cursor grows when
> over desktop background / scr 6 shows how console disappeared from taskbar when closed and shell won't reopen but
> quarry closes and reopens ok / machine off

Line by line, against the wire:

| Peter's line | Wire | Row |
|---|---|---|
| "machine wedges for a bit when prt sc pressed" | Measured. `[pstrip] rollup … gapmax=` sits at 260–465 ms all boot and spikes exactly six times, once per capture group: **8614, 7804, 6789, 6066, 6541, 6101 ms** (excerpt lines 862, 1635, 2195, 2776, 8228, 10934). Across the 6.9 MB `SCREEN3`+`SCREEN4` pair the compositor managed 3 passes in 6706 ms (`[wcn] rollup … att_rate=0.4/s span=6706ms` against a normal 3.6–4.9/s), and `[comp2] rollup … max_us=67815`. The capture is a synchronous multi-megabyte card write on the calling core, and on this board that is the ONLY working core (A21) | **A36** |
| "3 presses in a row didn't do 3 apparently" | `armed=8 capturing=7 ok=7`, no refusal of any kind. The three-press burst at lines 2738/2741/2742 yielded `SCREEN3.PNG` and `SCREEN4.PNG` | A17 (gap stands) |
| "mouse dragging not perfectly fluid but pretty good" | "Pretty good" = A27's fix landed. "Not perfectly fluid" is **the pacer's designed coalescing, NOT a defect** — the `[drag]` counters show nothing is dropped. See the observation below | A27 (observation, no new row) |
| "view menu works but is misplaced and different font from main app menu item font" | `[winmenu] open title=View items=2 at (193,34) owner=3` ×3, `pick` ×2, both labels correct. Placement and typeface are GLASS-ONLY: `video/winmenu.rs:634` prints the drop-down origin but never the bar title's rect, so no scorer can see the mismatch. Pending harvest of SCREEN1–4 | **A31** |
| "main app menu does nothing" / "there should be quit in apps' main menus (like under where it says pulse)" | `[menubar] live … press=crystal crystal=16x22` — the bar registers exactly ONE press target, the crystal. Every `[winmenu] publish` this boot is `owner=3 titles=1 items=2` (the pulse window's `View`); **no window ever publishes an application-titled tree**, so there is no `Pulse` menu to press and no `Quit` item anywhere | **A32** |
| "files not showing up in quarry" | Not a quarry fault. `[quarry] open census ERROR cwd=/ "/: backend error: unafs-mount"` ×3 — and Peter's own `ls` at the shell got the identical error: `:: tegra: JD2 — OUT \| ls: /: backend error: unafs-mount ::` / `:: ls1: /: ERR /: backend error: unafs-mount ::`. **The VFS root has no backend on this image.** Zero `SDMMC:` lines all boot (`sdmmc`/`sdmmc_arm` are absent from the shipped image, F-table), so `crate::fs::unafs::with_unafs` has nothing to mount and `fs/vfs.rs:858` returns `VfsError::Backend("unafs-mount")` for every enumeration | **A28** |
| "closing pulse reopened it immediately, but" | Twice, identically: `[pulsewin] close-box win=3 at (208,868)` → `[wc-a] close win=3` → `[pulsewin] close win=3 switches=2 (surface freed; menu cleared from the bar…)` → **five lines later** `[pulsewin] open win=3 … at (10,914)`. Same at 10542→10551. The ARMED latch is never cleared by the close, and `orin_render_service`'s `pulsewin::service()` fold re-honours it on the next pass | **A30** |
| "i cannot launch apps from cmd line ( no core is at el1 )" | Known and open: A2 (no EL0 program owns a window; `bg` refused) with A21 (one core, no tick after the drop). `SCHED: load c0=87–98%` with c1–c5 folded and c6/c7 `never-folded` all boot | A2, A21 (no new row) |
| "crystal menu has a gap to the left … it should be all the way to the right edge" | `:: SHARD-MENU: crystal_press=open via=corner-zone menu=170x121+12+34 items=4 ::` ×5 — the drop-down is anchored 12 px in from the panel's LEFT edge, under a `16x22` crystal glyph in a `1920x34+0+0` bar. Peter wants the crystal group flush RIGHT. Pending harvest of SCREEN5 | **A33** |
| "crystal restart/shut down not working not expecting sleep yet" | Both verbs reached their terminus and neither did anything: `:: SHARD-MENU: crystal_pick verb=ShutDown action=real ::` (the machine stayed up for another 12 minutes) and `:: SHARD-MENU: crystal_pick verb=Restart action=stub ::`. Restart is an admitted stub; **ShutDown claims `action=real` and is not** — the aarch64 PSCI terminus is missing or silently refused | **A34** |
| "mouse cursor grows when over desktop background" | Glass-only; nearest wire proxy is the two composition regimes the cursor census prints — `[cursor12] offer scope=desktop adm=baseline … nosprite=41 hidden=41` versus `[cursor12] offer scope=live adm=window`. No token carries the sprite's size, so nothing can score it today | **A35** |
| "scr 6 shows how console disappeared from taskbar when closed and shell won't reopen but quarry closes and reopens ok" | Fully on the wire. Console close: `[wc-a] close win=1` → `[wm-act] focus-release win=0 owner=0xffffff01 … route=close-furniture shell-raise=skipped siblings=untouched` → `[wm-act] close-furniture win=1 owner=0xffffff01 closed=true **route-dropped=true**`. Compare the pulse window's own close two lines earlier: `close-furniture win=4 … route-dropped=false`. The dock then LOSES the tile (`tile=2/3`, was `0/4`) and Peter pressed the shell pin twice — `[dock] press at (1054,1161) tile=2/3 shell=pin -> reopen requested` and again at (1046,1161) — with **no window on either**. Quarry's pin on the same dock works (`[dock] press … tile=0/3 quarry=pin -> open requested` → `[quarry] open win=1`), which isolates the fault to the shell path. **This is A7/S4 confirmed on metal for the first time**: `SHELL_REOPEN` is latched and drained only by `x86_render_service` | **A29**, A7 (→ S4) now metal-confirmed |

## Observation — the drag pacer's coalescing is designed behaviour, not a defect

*(Correction from rmbp 12's review of this report. The earlier draft read the applied/fed ratio as "the pacer drops
half the frames" and filed it as fluidity residue. It is not a drop and it is not a defect; it is recorded here as an
observation and NO ledger row files it as one.)*

`[dragroute]`'s `fed` counts every motion sample handed to the router; `applied` counts the moves the window
actually took. The `[drag]` rollup prints the decomposition, and it closes exactly:

```
[drag] win=4 owner=0xffffff02 end=placed moves=94 composites=94 erase_rects=169 erase_px=698760 erase_px_pm=7433 box_px_pm=494700 flash_px=0 admitted=94 coalesced=97 -> ONCE
[dragroute] end win=4 via=release fed=191 applied=94 at (1410,606) -> STEERED
```

Gesture 1: **fed 191 = admitted 94 + coalesced 97**, and **every admitted move composited**
(`moves=94 composites=94`). The 97 "missing" samples were not dropped — they were folded into the
following admitted move, which is what the pacer is for: the pointer reports faster than the compositor can
repaint a 1920×1200 panel, so intermediate samples merge and only the resulting position is drawn.

The identity holds in all eight steered gestures, and `composites == moves` in all eight:

| gesture | win | fed | admitted | coalesced | admitted+coalesced | moves | composites | applied |
|---|---|---|---|---|---|---|---|---|
| 1 | 4 | 191 | 94 | 97 | **191** | 94 | 94 | 94 |
| 2 | 2 | 89 | 60 | 29 | **89** | 59 | 59 | 59 |
| 3 | 1 | 68 | 60 | 8 | **68** | 55 | 55 | 55 |
| 4 | 3 | 205 | 100 | 105 | **205** | 100 | 100 | 100 |
| 5 | 3 | 172 | 86 | 86 | **172** | 86 | 86 | 86 |
| 6 | 3 | 270 | 135 | 135 | **270** | 135 | 135 | 135 |
| 7 | 3 | 81 | 43 | 38 | **81** | 43 | 43 | 43 |
| 8 | 2 | 56 | 42 | 14 | **56** | 42 | 42 | 42 |

`flash_px=0` on every gesture as well — no tearing artifact was drawn. The only arithmetic not fully accounted for
is `admitted` exceeding `applied` by 1 in gesture 2 and by 5 in gesture 3 (six of eight are exact); that is a small
open question about the admit/apply boundary, not a frame loss, and it is far too small to be what Peter felt.

What Peter felt is therefore the *coalescing rate*, which is a function of panel size and repaint cost on a board
with one working core (A21) — the same single-core constraint behind the Print Screen wedge (A36). Improving
perceived fluidity is a compositor-throughput question, not a drag-path bug. **No defect row is opened for it, and
A27 stays PASS.**

## A10 — Esc does NOT dismiss the bar menu either (correction to the flight brief)

The brief for this report expected "A10 still Esc-unproven — no `dismiss reason=esc` on the wire". The second half is
true; the first is too weak. The excerpt carries exactly one Esc press and it WAS made with a bar menu down:

```
2437  [winmenu] open title=View items=2 at (193,34) owner=3
2438  [winmenu] live ... state=open owner=3 publishes=3 clears=0 opens=3 dismisses=2 picks=2 refusals=0
2459  xHCI: KEY: '' (scancode 0x29)
2460  :: tegra: JD2 — KEY 0x1b ::
2510  [winmenu] live ... state=open ...          <- 50 lines after the Esc, still open
2549  [winmenu] live ... state=open ...          <- 89 lines after the Esc, still open
2592  [winmenu] dismiss reason=outside owner=3   <- closed by a pointer BUTTON down, not by Esc
```

`state=open` is printed twice AFTER the Esc and the eventual dismissal is `reason=outside` on a click. **A10's
failure carried over from the in-window menu to the bar menu**: MENUBAR's Esc path did not fly, and `esc=0` in the
A25 line is a real negative, not a missing test. A10 stays open with metal evidence on the new path.

## New defects the wire found that the glass did not

- **A37 — serial RX double-delivers and reorders.** Rung 2 of A16 works (the mailbox is consumed) but the CCPLEX now
  has TWO readers of the same console stream — the direct UARTC RBR poll and the TCU RX mailbox drain — with no
  sequencing and no dedup between them. Burst `tste\r` arrived as `s t CR` (direct) then `t e` (mailbox), so the
  shell ran `cmd="st"`; paced `tste\r` arrived as `t s t e` (mailbox) + `CR` (direct) + `CR` (mailbox), so the shell
  ran `cmd="tetste"` and then an empty line. Fix shape: one path owns RX, or the drain tags and orders bytes.
- **A38 — a witness line that quotes another witness token inflates every token count.** `main.rs:8943`'s
  `[deskquarry]` line explains itself by quoting `` `[quarry] open win=…` `` verbatim, which makes
  `awk '/\[quarry\] open win=/'` count 4 where 3 windows exist. Same family as "a check that cannot fire": the
  scorer is not wrong about its pattern, the tree is ambiguous about its tokens.

## Not scored
- **Pixel legs — harvested, not yet read.** All seven captures came off the card after the flight; the checksums are
  committed as [`render7-card-harvest.sha256`](render7-card-harvest.sha256) and three 960-px reductions are committed
  beside this file: [`render7-SCREEN0-small.png`](render7-SCREEN0-small.png),
  [`render7-SCREEN5-small.png`](render7-SCREEN5-small.png), [`render7-SCREEN6-small.png`](render7-SCREEN6-small.png).
  Still OWED, and none of it is scored here: A19's `A19-pngband.py SCREEN0.PNG` band read (wire PASS only so far,
  and the band read needs the FULL-resolution PNG, not the reduction); Peter's SCREEN5 for the crystal gap (A33);
  SCREEN6 for the missing console tile (A29); SCREEN1–4 for the `View` placement and typeface (A31) and the cursor
  size over the backdrop (A35). Tidy the card before the next load (FLIGHT.md §A.4) or the next capture is named
  `SCREEN7.PNG`.
- **A26 strong leg.** Needs a boot that drops more than 256 console lines so `[conquiet] census dropped=` prints.
- **A20's instrument.** Untriggered this boot — validation needs a boot in which the pointer dies.
- **A23 / A12 / A24.** Separate images — they flew LATER the same day, on their own boots, and are reported in
  [`PROBES-2026-09-06.md`](PROBES-2026-09-06.md) (A21 as well): tick1 **PASS**, sdmmcwrite **PASS**, net4 **measured
  NEGATIVE**, ga10bprobe2 **PASS rung 2**.

## Card after the flight
`SCREEN0.PNG` … `SCREEN6.PNG`, seven valid 1920x1200 captures of 6.9 MB each. **Harvested** — sha256 of each is in
[`render7-card-harvest.sha256`](render7-card-harvest.sha256). No pixel verdict in this report has been read off them
yet; every one is still marked pending.

## Ledger

Ticked in `docs/dev/OS/orin-ledger.md` this flight: **A1** (hw=14400, fourth sample), **A7** (metal-confirmed),
**A8** (flown with a defect), **A10** (Esc confirmed failing on the bar menu), **A15** (pass 5), **A16** (rung 2
flown), **A17** (gap stands + the wedge), **A18** (fourth pass), **A19** (wire pass, pixels pending), **A20**
(instrument aboard, untriggered on boot 1 — pointer alive; `dup=0 nobuf=0` unfalsified, not validated), **A22**
(row 2), **A25** (R21 satisfied), **A26** (flown weak), **A27** (flown).

Opened this flight: **A28** VFS root has no backend · **A29 (→ SO1)** console close drops the route ·
**A30** closing pulse reopens it · **A31 (→ SO2)** `View` drop-down placement and typeface ·
**A32 (→ SO3)** no application menu, no Quit · **A33 (→ SO4)** crystal menu anchoring ·
**A34** crystal Restart/Shut Down inert · **A35 (→ SO5)** cursor grows over the backdrop ·
**A36 (→ SR2)** Print Screen wedge · **A37** serial RX double-delivers and reorders ·
**A38** a witness that quotes another witness inflates every count.

Ticked from the four probe boots later the same day (`PROBES-2026-09-06.md`): **A21** (flown PASS), **A23** (flown PASS), **A12** (flown, measured NEGATIVE), **A24** (flown PASS, rung 2).

`SO1`–`SO5` are new rows on `docs/dev/LEDGER.md` (orin's shared-ledger prefix per LAWS §Ledgers; `S1`–`S32` are
frozen and these are orin's first prefixed rows). **The Print Screen wedge is deliberately NOT given an `SO` row**:
rmbp had already promoted it to `LEDGER.md` as **SR2** ("Print Screen wedges the machine for the whole capture, on
every board" — 70 s per capture on the 2012 rMBP, `rmbp-ledger` A2) and had already cited THIS boot as its second
instance. A36 cross-references SR2 instead of duplicating it. Two notes for the landing seat: rmbp uses the `SR`
prefix on the same file, and the `hw-rmbp` copy of `LEDGER.md` does not yet carry `S28`–`S32` — the union is the
landing seat's to reconcile.
