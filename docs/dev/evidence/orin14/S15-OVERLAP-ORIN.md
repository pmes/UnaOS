# S15 — pulse-window / console-window overlap on the Orin, measured from code at `6cc8de8c`

Executor OCCLUDE13, seat orin 14, 2026-09-05. Base: `hw-jetson` tip `6cc8de8c`. No kernel edit; no
patch applied. Every number below is derived from the named source line and checked against the two
flown readbacks the tree already holds (`docs/dev/evidence/orin13/render2-boot.log`,
`docs/dev/evidence/orin13/render3b-boot1.log`). Inputs read: `docs/dev/evidence/orin13/PULSEOCCLUDE-RATIONALE.md`,
`docs/dev/evidence/orin13/pulseoccl-fbcon.patch`, `docs/dev/evidence/orin13/RENDER2-AUDIT.md` N1,
`docs/dev/RULINGS.md` R17, `docs/dev/OS/orin-ledger.md` A10/A18/B1.

## 0. Answer in one paragraph

On the CASCADED desktop (R17's target) the console window is opened by exactly the same shared
expression the old `orinconwin` seam used — `desktop_firmware::activate` calls
`fbcon::panel_console_window_open()` (`video/desktop_firmware.rs:237`) with the menu bar not yet
enabled (`:292`) and the pulse band not yet retired — so its box is the one render3b read back,
**1305x780 at (307,158)**. The pulse window, once its `open` runs again, lands at **1290x212 at
(10,874)** (render2 readback, and the cascade's ordering does not move it). The intersection is
**993 x 64 px**: the console's bottom border (5 px) plus text rows 42–45 (row 42 by 11 px, rows
43–45 whole) — **4 rows, the prompt row (row 45, y 917..933) wholly inside**. N1's 64 px / 4 rows is
confirmed by derivation, not merely by readback. The one thing the R17 arc will change is the
bottom reservation: retiring the strip (`retire_desktop_chrome`) takes `chrome_h` from 104 to 64,
which moves the pulse window DOWN 40 px and shrinks the overlap to **24 px / 2 rows** (console
opened before the retire, the Pi's ordering) or **28 px / 2 rows** (console opened after it) —
**the prompt row is inside the pulse box in every ordering**. rmbp's patch applies clean at
`6cc8de8c` (`git apply --check` exit 0, silent) and, at 1920x1200, caps the console at
**197 cols x 42 rows, box 1389x716 at (265,79)**, clearing the pulse box by 79 px (or 41 rows / box
1305x700 at (307,107), clearance 107 px, once the strip is retired). Recommendation: §7.

## 1. Constants, with their sources

| symbol | value | source |
|---|---|---|
| `wm::TITLE_H` | 34 | `video/wm.rs:98` = `theme::TITLE_HEIGHT` (`video/theme.rs:308`) |
| `wm::BORDER` | 5 | `video/wm.rs:105` = `theme::FRAME` (`video/theme.rs:301`) |
| `theme::GAP` = `strip::PAD` | 12 | `video/theme.rs:331`, `video/strip.rs:104` |
| `theme::BUTTON_HEIGHT` = dock `TILE_H` | 28 | `video/theme.rs:325`, `video/dock.rs:289` |
| `dock::STRIP_H` = `TILE_H + 2*PAD` | 52 | `video/dock.rs:304` |
| `video::dock_reserve_h()` = `strip::PAD + dock::STRIP_H` | 64 | `video/mod.rs:771-773` |
| `ui::BASE_CELL`, `SCALE_STEP`, `SCALE_MAX` | 8, 900, 4 | `ui.rs:36,40,43` |
| `Metrics::for_height(1200)` and `for_height(1)` | scale 1, cell 8, `line_h` 12 | `ui.rs:71-82` (`scale = h/900` floored to 1; `line_h = cell + cell/2`) |
| `ui_status::band_h(1200)` | 92 | `ui_status.rs:594-601`: `(1200/13 = 92).max(8*8 = 64).min(300)` |
| `ui_status::chrome_h(1200)` — strip live | 104 | `ui_status.rs:608-611`: `band_h + line_h` = 92 + 12 |
| `ui_status::chrome_h(1200)` — strip retired | 64 | same fn, first statement: `desktop_scene_owns_backdrop()` → `dock_reserve_h()` |
| `ui_status::top_chrome_h(1920,1200)` | 0 before `menubar::set_enabled(true)`, 34 after | `ui_status.rs:631-645` reads `menubar::strip_rect` (`video/menubar.rs:614-619`: `None` while `!ENABLED`; else `frame_flush(Top, BAR_H=34, …)` → `(0,0,1920,34)`, reservation `y+h` = 34). render3b:460 `rect=Some((0, 0, 1920, 34))` |
| console glyph cell | 7 x 16 | `video/font.rs:144-147` (`Size16`); render3b:444 `cell=7x16` |
| `fbcon::WIN_BOX_BUDGET_PX` | 1,048,576 | `video/fbcon.rs:1833` (`4*1024*1024/4`) |
| `sched::meter_cpu_count()` on tegra | 6 | `arch/aarch64/sched.rs:5543-5545` → `percpu::METER_CPU_COUNT` (`percpu.rs:156`) |
| `ui_status::PSTRIP_MAX_CPUS` | 8 | `ui_status.rs:406` |
| `pulsewin::FLOOR_CELLS` | 20 | `video/pulsewin.rs:262` |

`wm::spawn_geometry` (`video/wm.rs:831-846`) returns scale 1 for both windows: `place_scale` →
`scale_in` (`wm.rs:22447-22458`) is `(pw/2/w).min(usable_h/2/h)…max(1)` and `1920/2/1295 = 0`,
`1920/2/1280 = 0`; the `min_width_scale` floor is `ceil(CLUSTER_MIN_SRC_W / w)` = 1 for either width.
Outer box = `(w + 2*BORDER) x (h + TITLE_H + 2*BORDER)` = `(w+10) x (h+44)`.

## 2. The cascade's order of operations (what is enabled when each box is computed)

`video/desktop_firmware.rs::activate` (`:83`), reached on the Orin through `tegra_desk_cascade`
(`main.rs:2717`, `[deskcascade] arming cascade …`, render3b:415):

1. `:223` dock hostability check, then `:237` **`fbcon::panel_console_window_open()`** — the console
   box is sized and placed HERE, with `top_chrome_h = 0` (bar not enabled) and `chrome_h = 104` (strip
   live; `DESKTOP_SCENE` false — its only setter is `video::retire_desktop_chrome`, `video/mod.rs:728`,
   whose only caller is the Pi's `render_service` at `main.rs:5576`; `orin_render_service` has none,
   and render3b prints no `[realdesk]` line). Readback render3b:444.
2. `:292` **`menubar::set_enabled(true)`** — `top_chrome_h` becomes 34 from here. Readback render3b:460.
3. `:373` **`pulsewin::arm()`** — a latch only. Readback render3b:477 `[pidesk] pulse-window ARMED`.
4. The pulse window's `open` runs inside `pulsewin::service()` (`video/pulsewin.rs:578-605`) on the
   render pass — which CASCADE `a20839c6` removed from `orin_render_service`; render3b therefore has no
   `[pulsewin] open` line (A10 "flown"). R17/A18 puts that call back. Its box is computed with
   `top_chrome_h = 34` and whatever `chrome_h` is at that moment.

So "the cascade's console placement" is the pre-cascade placement, term for term: the old `orinconwin`
seam (`arch/aarch64/display_tegra.rs:2657`) called the same function under the same two reservations,
which is why render2:`[wc-x] console-window win=2 … box=1305x780 at (307,158)` and
render3b:`… win=1 … box=1305x780 at (307,158)` agree to the pixel.

## 3. The console window box at 1920x1200 (`fbcon::win_content_extent`, `:1854-1871`; placement `:1966-1972`)

```
avail_h = 1200 - top_chrome_h(0) - chrome_h(104) = 1096                         fbcon.rs:1856-1859
w0 = 1920*7/8 = 1680 ; h0 = 1096*7/8 = 959                                       fbcon.rs:1860-1861
budget loop (15/16 per step, both axes; stop when (w+10)*(h+44) <= 1,048,576):   fbcon.rs:1862-1868
  (1680,959) 1690*1003 = 1,695,070   >
  (1575,899) 1585*943  = 1,494,655   >
  (1476,842) 1486*886  = 1,316,596   >
  (1383,789) 1393*833  = 1,160,369   >
  (1296,739) 1306*783  = 1,022,598   <= stop
cells: 1296/7 = 185 cols -> 1295 ; 739/16 = 46 rows -> 736                       fbcon.rs:1869
outer box: (1295+10) x (736+44) = 1305 x 780
ox = (1920-1305)/2 = 307 ; oy = 0 + (1200 - 0 - 104 - 780)/2 = 158               fbcon.rs:1966-1972
content origin = (307+5, 158+34+5) = (312, 197)                                  fbcon.rs:1987-1988
```

Console box: x 307..1612, **y 158..938**. Text row r occupies y `197 + 16r .. 213 + 16r`, r = 0..45;
the bottom row (45) is y 917..933, and the bottom border is y 933..938. The prompt lives on the bottom
row (RENDER2-AUDIT N1). Readback: render3b:444 / render2 `surf=1295x736 box=1305x780 at (307,158)
cols=185 rows=46` — every intermediate above is fixed by those four numbers.

## 4. The pulse window box at 1920x1200 (`pulsewin::content_extent` `:236-260`; `open` `:385-466`)

```
m = Metrics::for_height(1): cell 8, line_h 12 ; ncpu = min(8, 6).max(1) = 6     pulsewin.rs:243-244
cw = min(1920*2/3 = 1280, 1920-10) = 1280                                        pulsewin.rs:248
ch = menu_h(12) + 2*pad(6) + 6*row_target(24) = 12 + 12 + 144 = 168               pulsewin.rs:249,265-278
outer: ow = 1290 ; oh = 168 + 44 = 212                                            pulsewin.rs:252
floors: cw >= 2*20*8 = 320 ok ; ow <= 1920 ok ; oh <= work_h ok ; ch < 900 ok     pulsewin.rs:253
gap = 2*BORDER = 10 ; ox = min(10, 1920-1290) = 10                                pulsewin.rs:447-448
oy = (1200 - chrome_h - 10 - 212).max(top_chrome_h)                               pulsewin.rs:449-453
   strip live    (chrome_h 104): oy = 874     -> box y 874..1086
   strip retired (chrome_h  64): oy = 914     -> box y 914..1126
```

`work_h` for the floor test is `1200 - 34 - 104 = 1062` (bar enabled by then) — not binding. The
`.max(wtop)` floor (34) is not binding. So the bar's enablement, which the cascade puts BEFORE this
open, changes nothing about the pulse box: readback render2 `[pulsewin] open win=3 … surf=1280x168
box=1290x212 at (10,874)` is what the cascade will print again while the strip is live.

## 5. The intersection

### 5.1 As the tree stands (`chrome_h` 104 at both opens)

* x: console 307..1612 ∩ pulse 10..1300 = 307..1300 → **993 px**
* y: console 158..938 ∩ pulse 874..1086 = 874..938 → **64 px** (= `[wc-h] win=3 … span=64 band=yes`,
  render2, eight such lines)
* console rows under the pulse box: rows whose span meets [874, 938): r ≥ 42 (`197+16*42 = 869`,
  869..885 meets 874 by 11 px); rows 43 (885..901), 44 (901..917), 45 (917..933) whole; border 933..938.
  Check: 11 + 3*16 + 5 = 64. → **4 rows (42 partial, 43–45 whole); the prompt row (45) wholly inside.**

**This is N1's number exactly — 64 px / 4 rows — and it is unchanged by the cascade**, because the
cascade does not move either box (§2).

### 5.2 After the R17/A18 arc retires the strip (`chrome_h` 104 → 64)

Only the bottom reservation moves. `retire_desktop_chrome` (`video/mod.rs:728-742`, witness
`bottom_reserved=104->64`) is one-way and, on the Pi, runs in the render task's backdrop hand-off
AFTER the cascade (`main.rs:5576`). Three orderings are possible on the Orin; the R17 arc picks one:

| ordering | console box | pulse box | overlap | rows under the pulse box | prompt row |
|---|---|---|---|---|---|
| (i) retire after console open, before pulse open (the Pi's) | 1305x780 at (307,158), y 158..938 | (10,914), y 914..1126 | 993 x **24 px** | border 5 + row 45 whole (917..933) + row 44 by 3 px (914..917) → **2 rows** | inside |
| (ii) retire before both opens | avail_h 1136; h0 994; loop (1680,994)→(1575,931)→(1476,872)→(1383,817)→(1296,765): 1306*809 = 1,056,554 > budget →(1215,717): 1225*761 = 932,225 ≤; cells 173x44 → 1211x704; box **1221x748 at (349,194)**, y 194..942; text origin y 233 | (10,914), y 914..1126 | x 349..1300 → 951 x **28 px** | border 5 + row 43 whole (921..937) + row 42 by 7 px → **2 rows** | inside |
| (iii) no retire (strip kept) | as §3 | (10,874) | 993 x 64 | 4 | inside |

There is no ordering in which the pulse box clears the prompt row. The overlap is a property of the
two SHARED placements (console centred in the work area, pulse pinned to its bottom), not of the
tegra seam or of the strip.

## 6. The 640x480 QEMU shape (from code; no tegra QEMU run in this tree prints these lines)

`chrome_h(480)` = `band_h(480)` `(480/13 = 36).max(64).min(120)` = 64, + `line_h` 12 = **76**.
`menubar::geometry(640,480)`: `FLOOR_W` = `4*12 + CRYSTAL_W(16) + 6*9` = 118 ≤ 640, `FLOOR_H` = 34+52+24 =
110 ≤ 480 → the bar fits; `top_chrome_h` = 34 once enabled (`menubar.rs:193,210,218`).

* Pulse: `work_h` = 480-34-76 = 370; `cw` = min(426, 630) = 426 ≥ 320; `ch` = 168; box **436x212 at
  (10, 480-76-10-212 = 182)**, y 182..394. Opens.
* Console: gated by `dock::Layout::for_panel(12, 640, 480)` (`desktop_firmware.rs:223`, `dock.rs:400-423`).
  The step-down loop at `glyphs = 1`: `tile_w` = 24 + 1*9 = 33; `w` = 24 + 12*33 + 11*12 = 552;
  `frame_centred` (`strip.rs:124-136`) requires `552 + 24 ≤ 640` and `480 ≥ 52 + 24` — both hold, so
  **the code at `6cc8de8c` says `Some`** (a 12-tile dock of one-glyph captions) and the console window
  OPENS on this panel: `avail_h` = 404, w 560, h 353 (box 570x397 = 226,290, no shrink), cells 80x22,
  box **570x396 at (35, (404-396)/2 = 4)**, y 4..400, text origin y 43. Then the pulse box (y 182..394)
  sits INSIDE the console box: overlap 411 x 212 px, rows 8 (partial) through 21 (the prompt row,
  379..395) — 14 of 22 rows.
  ⚠ `PULSEOCCLUDE-RATIONALE.md` states the opposite ("`dock::Layout::for_panel` declines the console
  window on that panel, so only the pulse window opens"), citing the `[pidesk] activate DECLINE
  reason=dock-cannot-host-full-strip panel=640x480 rows=12` line recorded in
  `docs/dev/OS/08_VIDEO/engine.md:13754` from an earlier PI-DESK tree. The arithmetic above uses the
  current chrome face (`CHROME_CELL_W` = 9 at `Size20`, `font.rs:141-150`; the rmbp7 capture's
  `:: DOCK: strip tiles=6 … w=660 glyphs=8` on 2880 resolves to the same 9 px). One QEMU boot settles
  it: `awk '/\[wc-x\] console-window|dock-cannot-host|\[pulsewin\] open/' unaos/target/serial-arm.log`
  after a `desktop_firmware`+`deskcascade` run at 640x480. If the console opens there, the QEMU gate
  can SEE this overlap (`[wc-h] win=… band=yes`) and the rMBP's patch becomes gateable on QEMU; if it
  declines, the gate stays blind to N1 as the rationale says.

## 7. rmbp's patch at `6cc8de8c`

```
$ git apply --check docs/dev/evidence/orin13/pulseoccl-fbcon.patch ; echo $?
0            (no output — both hunks in fbcon.rs and the pulsewin.rs hunk apply clean)
```

The patch (a) adds `pulsewin::reserve_h(pw, ph)` = outer box + `2*BORDER` = 212 + 10 = **222** on the
Orin (0 where `content_extent` declines), (b) caps the console content height at
`fit_h = avail_h - reserve_h - 44`, and (c) subtracts `reserve_h` from the centring span. `reserve_h`
is evaluated at console-open time, i.e. with `top_chrome_h` = 0 — `content_extent`'s own `work_h`
test passes there (212 ≤ 1096), so the reservation is 222 whether or not the bar is up.

At 1920x1200, strip live (`chrome_h` 104):

```
fit_h = 1096 - 222 - 44 = 830 ; h0 = min(959, 830) = 830
loop: (1680,830) 1690*874 = 1,477,060 > ; (1575,778) 1585*822 = 1,302,870 > ;
      (1476,729) 1486*773 = 1,148,678 > ; (1383,683) 1393*727 = 1,012,711 <= stop
cells 197 x 42 -> 1379 x 672 ; box 1389 x 716 ; oy = (1096 - 222 - 716)/2 = 79
console y 79..795 ; pulse y 874..1086 ; clearance 79 px ; 42 rows (was 46), 197 cols (was 185)
```

(the RATIONALE's own expected wire line, `[wc-x] console-window … surf=1379x672 box=1389x716 at
(265,79) … cols=197 rows=42 pulse_res=222`, reproduces.)

Strip retired (`chrome_h` 64), if the retire lands before the console opens:

```
avail_h 1136 ; fit_h = 1136 - 222 - 44 = 870 ; h0 = min(994, 870) = 870
loop: (1680,870) > ; (1575,815) > ; (1476,764) > ; (1383,716) 1393*760 = 1,058,680 > ;
      (1296,671) 1306*715 = 933,790 <= stop
cells 185 x 41 -> 1295 x 656 ; box 1305 x 700 ; oy = (1136 - 222 - 700)/2 = 107
console y 107..807 ; pulse y 914..1126 ; clearance 107 px ; 41 rows
```

Either way the patch clears the pulse box with margin; its cost is 4–5 console rows, paid on every
image that compiles `pulsewin` whether or not the window opens (the RATIONALE's stated cost; a
`pulsewin::will_open()` predicate is the shape that would remove it).

## 8. Recommendation

**Apply rmbp's patch, via a grant, in the R17/A18 arc — do not move the pulse window, do not accept.**

* *Move* is ruled out by geometry: on 1920x1200 the free strips around a 1305x780 console centred at
  (307,158) are 158 px above, 148 px below (104 of which are reserved), 307/308 px beside — none holds
  a 1290x212 box (RATIONALE, confirmed by §3/§4). A post-create `wm::move_to` is also ruled out by
  SPAWN-PLACE (`wm.rs:~855`).
* *Accept* is ruled out by R17 itself: the ruling restores the windowed pulse as THE pulse on the
  cascaded desktop, and the prompt row is under it in every ordering (§5.2). A console whose prompt is
  occluded is the defect the R17 arc exists to remove, not a cosmetic residue.
* *Apply* is the only shape that changes a SIZE, and only `fbcon` owns the size. Both files are
  rmbp's (`video/`); the seat negotiates the grant over ccd before the arc touches them. The patch is
  check-green at its base and applies clean at `6cc8de8c`; its expected wire is written above and in
  the RATIONALE, and its one open cost (rows given up on images that never open the window) is small
  and already stated. Sequence it AFTER the strip retirement in the same arc so the console is sized
  once against the final `chrome_h` (§7's second block), not twice.

Falsifiers for the next Orin render boot, with the patch: `[wc-x] console-window … rows=42
pulse_res=222` (or `rows=41` with the strip retired first); `[pulsewin] open … at (10,874)` (or
`(10,914)`); **no** `[wc-h] win=3 … band=yes`; `[wcn] win=3 … drg=0`.
