# PULSEOCCLUDE — fbcon.patch rationale (2026-09-05, executor PULSEOCCLUDE, base 2000608a)

Patch: `fbcon.patch` in this directory. Apply with `git apply fbcon.patch` at the repo root against
`2000608a` (verified: `./arroyo check` exit 0 with it applied, all legs including `arm-tegra-render`;
then reverted — the two files it touches are shared `video/` files, outside the tegra lane).

## Case that applied: (b) — both sites are shared, neither has a tegra arm or a parameter

* The console box for the `orinconwin` seam is computed entirely in shared `video/fbcon.rs`:
  `win_content_extent` (`fbcon.rs:1853-1871`) sizes the content, `panel_console_window_open`
  (`fbcon.rs:1966-1972`) centres the outer box in the work area. The tegra-owned seam
  `arch/aarch64/display_tegra.rs:2571 orin_conwin()` only calls `fbcon::panel_console_window_open()`
  (`:2656`) — no size or position parameter exists to pass.
* The pulse placement is shared `video/pulsewin.rs:449-456 open()` — bottom-left of the work area,
  no tegra arm, no parameter. Its only tegra-side lever is `pulsewin::arm()` at `main.rs:8178`
  (PAINTPULSE), a latch; the open runs inside `pulsewin::service()` (`:578-605`) on the render pass.
* A tegra-owned `wm::move_to` after either open was considered and rejected: (1) `wm::create_at`'s
  stated invariant (`wm.rs:~855`, SPAWN-PLACE) is that no pixel is ever presented at a position the
  window will not occupy — a post-create move re-introduces the frame-then-jump and the vacated box
  the rMBP s41 metal sitting recorded; (2) on 1920x1200 there is no position for a 1290x212 pulse
  box that avoids a 1305x780 console box centred at (307,158): above it 158 px, below it 148 px,
  beside it 307/308 px — every strip is too small. Only a size change fixes it, and the size is
  fbcon's.

## Why the Pi coexists (and does not, quite)

Same 1920x1200 panel on the bench Pi (`~/unaos-bench/capture/line-acm0/pi.log`):
`[wc-x] console-window win=1 … box=1305x780 at (307,158) … rows=46` and
`[pulsewin] open win=3 panel=1920x1200 surf=1280x120 box=1290x164 at (10,922)`.
The difference is the CORE COUNT: `pulsewin::content_extent` sizes the window as
`line_h + 2*pad + ncpu * 3*cell_h` = 12 + 12 + ncpu*24 → 120 px content at 4 cores (Pi), 168 at 6
(Orin); box = content + TITLE_H 34 + 2*BORDER 10 → 164 (Pi) / 212 (Orin); placed at
`ph - chrome_h(1200)=104 - gap 10 - box` → y 922 (Pi) / 874 (Orin).
So the Pi overlaps too — 16 rows (922..938 of the console box), i.e. the bottom 11 px of text row 45
(surface y 197..933) — but it goes unnoticed because the Pi's console window is FROZEN by the GUI
handoff's `fbcon::detach()` before the pulse window opens (`desktop_firmware.rs:~247` ledger), so
its last row is blank and it never damages the pulse window: pi.log shows `[wcn] win=3 … drg=0`
and no `[wc-h] win=3 … band=yes`. The Orin's console is LIVE (`orinconwin` guards the detach), the
prompt lives on the bottom row, and every printed line damages the band under the pulse window:
`drg=N`, `span=64 band=yes`.

## The arithmetic (cell 7x16, TITLE_H 34, BORDER 5, budget 1,048,576 box px)

Work area on 1920x1200: `top_chrome_h` = 0 (no bar on either aarch64 board), `chrome_h(1200)` =
`band_h` 92 (1200/13) + `line_h` 12 = 104 → work area y 0..1096, `avail_h` = 1096.

Before (both boards): h = 1096*7/8 = 959, w = 1680; budget loop (15/16 per step, both axes):
(1680,959)→(1575,899)→(1476,842)→(1383,789)→(1296,739) stops at 1306*783 = 1,022,598 ≤ budget;
cells: 1295x736 = 185 cols x 46 rows; box 1305x780; oy = (1096-780)/2 = 158 → y 158..938.

After — `pulsewin::reserve_h` = pulse box + gap(2*BORDER):
* Orin (6 cores): reserve 212+10 = 222. `fit_h` = 1096-222-44 = 830 < 959 → h starts at 830.
  Loop: (1680,830)→(1575,778)→(1476,729)→(1383,683) stops at 1393*727 = 1,012,711.
  Cells: 1379x672 = **197 cols x 42 rows**; box 1389x716; oy = (1096-222-716)/2 = 79 →
  console box y **79..795**, text rows y 118..790; pulse box y **874..1086**. Clearance 79 px.
* Pi (4 cores): reserve 164+10 = 174. `fit_h` = 878. Loop: (1680,878)→(1575,823)→(1476,771)→
  (1383,722) = 1,067,038 > budget →(1296,676) stops at 940,320. Cells: 1295x672 =
  **185 cols x 42 rows**; box 1305x716; oy = (1096-174-716)/2 = 103 → console y **103..819**;
  pulse y **922..1086**. Clearance 103 px.
* rMBP 2880x1800 (x86 `wc`, both windows compiled): `fit_h` is not binding (1420 > 7/8), so the
  console keeps 1232x688; centring moves it up 87 px (oy 453 → 366) and it already cleared the
  pulse box (1464..1628) before. No overlap either way.
* QEMU 640x480: `dock::Layout::for_panel` declines the console window on that panel, so only the
  pulse window opens — the reservation has no effect on the gate.

## Expected wire on the next Orin render boot

* `[wc-x] console-window win=2 panel=1920x1200 surf=1379x672 box=1389x716 at (265,79) cell=7x16
  cols=197 rows=42 pulse_res=222` (the new trailing field is the reservation; no spec REQUIREs the
  old wording — the `[wc-x]` lines in `x86-witness.spec`/`pi4-regression.spec`/`jetson-sync1.spec`
  are comments only).
* `[pulsewin] open win=3 … box=1290x212 at (10,874)` unchanged.
* NO `[wc-h] win=3 … band=yes` line; `[wcn] win=3` lines absent (a static row with `drg=0` prints
  no line) or `drg=0`.
* The console's `[wcn] win=2` line loses its `dout=2 dkpx=…` dual-output shape.

## Cost stated

The reservation applies whenever `pulsewin` is compiled (its module gate at `video/mod.rs:667` is
exactly `panel_console_window_open`'s), not only when the window WILL open: at console-open time the
`ARMED` latch is not yet set on any board (Orin arms at the render seam after `orin_conwin`; the Pi
arms after opening the console). So an Orin image with `orinconwin` but without `orinrender` gives up
4 text rows to a window it never opens. A board-feature `cfg` in a shared file was rejected (name by
subsystem, never board); if that cost matters, the right shape is a `pulsewin::will_open()` predicate
set by the seams before the console opens — a second shared edit, not taken here.
