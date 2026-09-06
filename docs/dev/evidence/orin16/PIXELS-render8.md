# PIXELS — render8 (orin 17, executor PIXELS)

The glass half of the render8 flight, read at full resolution. [`FLIGHT-RESULT-render8.md`](FLIGHT-RESULT-render8.md)
read exactly two things off the card — the A19 band on `SCREEN0` and a 25 % thumbnail of `SCREEN3` — and
left every other pixel leg owed. This document reads all four captures and settles the seven rows that
were waiting on them.

Everything below is printed by [`pixels-render8.py`](pixels-render8.py), committed beside this file, which
carries the same stdlib PNG decoder as [`../orin14/A19-pngband.py`](../orin14/A19-pngband.py) (there is no PIL
and no ImageMagick on this host). Run it against whatever directory holds the harvest:

```
python3 docs/dev/evidence/orin16/pixels-render8.py <dir-with-SCREEN0..3.PNG>
```

The full-size captures are deliberately NOT in git — 6 913 793 B each, 27 MB for four — exactly as
render7 and render8 handled them. What is committed is the four 25 % reductions and the scorer.

| capture | sha256 | taken | thumbnail |
|---|---|---|---|
| `SCREEN0.PNG` | `423202885f0568cc0e167a1d202f8d35c839a33afec7fba7f41140638f5db4f7` | ~15:43Z | [`render8-SCREEN0-small.png`](render8-SCREEN0-small.png) |
| `SCREEN1.PNG` | `256d9b556583a79425336d896e964c7ebce0b378ca6e6bb4a2a6df4b5382b206` | ~15:43Z | [`render8-SCREEN1-small.png`](render8-SCREEN1-small.png) |
| `SCREEN2.PNG` | `3bbae794eb52d2cc7a8b41d1fe6834d09cedbd7001198a8e4ca24d9f85f3f475` | ~15:43Z | [`render8-SCREEN2-small.png`](render8-SCREEN2-small.png) |
| `SCREEN3.PNG` | `0b2b83be79f2e8734ef2381d5e7e6c3ba756e5e7f3fe9eae0fe1f76eada7166e` | ~15:52Z | [`render8-SCREEN3-small.png`](render8-SCREEN3-small.png) |

All four shas match [`render8-card-harvest.sha256`](render8-card-harvest.sha256) byte for byte, so these are
the files the flight wrote. All four decode as 1920×1200, colour type 2 (RGB), every scanline filter 0.

## How a window's rect is measured

Two independent scans, and they agree on every window:

1. **The box scan.** Per row, the maximal runs of non-`DESKTOP_BG` (`#2d2b55`) merged across gaps ≤ 24 px,
   printed only where the signature changes. A window's top, bottom and side edges fall straight out of it.
2. **The chrome scan.** The close disc is flat `#ff5f57` and appears nowhere else on the glass. Every one of
   the eleven disc groups measures **24×24 px** and sits at exactly **box + 17 px** in x and **box + 10 px**
   in y, with the amber and green discs at box + 53 and box + 89. So one disc gives the box origin, and it
   reproduces the box scan's edges in every case.

Window *titles* are read as glyphs (the scorer prints the cells; the labels below were read off the
rendered bitmap): the menu bar's two cells, each window's title-bar text, and the four dock tile labels.

## SCREEN0 / SCREEN1 / SCREEN2 — ~15:43Z

The three early captures are the **same scene**. Pixel-diffed against each other they differ only in:

* the pulse window's LED band (`x45..1252, y959..1108`) — the animation, running; and
* **`x760..766, y208..216` in `SCREEN2` alone — the pointer sprite** (see A35).

Nothing else on 2 304 000 px moves. Z-order, top to bottom: **shell → quarry → console → pulse**.

| # | window | box | surface / content | notes |
|---|---|---|---|---|
| — | menu bar | `1920x34 at (0,0)` | — | app cell `Shell` glyphs `x13..55 y10..21`; `View` cell `x171..206 y10..21`; crystal glyph `16x22 at (1904,6)` |
| 1 | **shell** | `970x510 at (923,61)` | interior painted `#2d2b55`, the desktop colour | title `Shell`; the jd2 banner on two lines at `y112..118` and `y124..130` (`x940..1424`), and an **8×8 pure-white block caret at (1092,136)** |
| 2 | **quarry** | `1162x764 at (379,183)` | tree pane `x385..742`, divider `x743..745`, list pane `x746..1534` | `/usb` listing; occludes the console from `x379` |
| 3 | **console** | `1305x780 at (307,158)` | content `1295x736 at (312,197)`, black | only `x312..378` and `x1541..1606` are unoccluded; the left sliver carries **monospaced grey-on-black text** |
| 4 | **pulse** | `1290x212 at (10,914)` | surface `x15..1294`; title bar `y915..952`, LED band from `y953` | gutter `x15..44` `#0e0d22`; LED cells 10 px wide on a 13 px pitch from `x45` to `x1299` |
| — | dock | `444x52 at (738,1136)` | four tiles | labels `console` / `quarry` / `pulse` / `shell` |

`SCREEN0`'s box scan is also the control that makes the A19 band result non-vacuous: rows `y34..60` are
empty across the whole panel, and the first window edge is at `x923, y61`.

## SCREEN3 — ~15:52Z

Z-order, top to bottom: **quarry → console → pulse**. The shell window is **gone**.

| # | window | box | surface / content | notes |
|---|---|---|---|---|
| — | menu bar | `1920x34 at (0,0)` | — | app cell `Quarry` glyphs `x12..65 y13..24`; `View` cell `x171..206 y10..21`; crystal glyph `16x22 at (1904,6)` |
| 1 | **quarry** | `1162x764 at (64,89)` | tree pane `x70..427`, divider `x428..430`, list pane `x431..1219` | title `Quarry` at `x189..241 y107..118`; tree selection row `#4a73aa` at `y193..212` |
| 2 | **console** | `1305x780 at (307,195)` | content `1295x736 at (312,234)` | **solid black — 0 non-black px in 387 354 unoccluded px** |
| 3 | **pulse** | `1290x212 at (10,914)` | unchanged from the early captures | its title bar is cut at `x306` by the console box's left edge — see SO11 |
| — | dock | `444x52 at (738,1136)` | four tiles | the `shell` tile's running dot is grey |

The console box has moved **`(307,158)` → `(307,195)`**, +37 px in y, same `1305x780` — it is the cascade
re-placing the window that was closed at `:7950` and re-minted by the dock press at `:8258`.

## The rows

### A31 — the `View` drop-down's placement and typeface

The bar's `View` cell measures `x171..206` (36×12 glyphs, `y10..21`). render8's own witness — the one SO2
asked for, and it flew — prints `[winmenu] open title=View items=2 at (171,34) title-x=171 font=chrome20-bold
kind=title owner=3`. **The drop-down origin equals the measured left edge of the title that owns it.** The
app menu agrees: `title-x=12` against a `Quarry` cell whose glyphs start at `x12` (`SCREEN0`'s `Shell` starts
at `x13`, the `S` glyph's own left bearing on the same 12 px cell origin). render7's `at (193,34)` — 22 px
right of the title — does not recur.

The typeface half is **not readable from these four**: no capture has a drop-down open, so there is no
rendered drop-down glyph to compare against the bar's. What the wire now says is that both kinds carry the
same token, `font=chrome20-bold` on `kind=app` and on `kind=title` alike.

```
A31 pixels: View title cell x171..206 (36x12) vs wire title-x=171 / open at (171,34); app title cell x12 vs wire title-x=12 -> PASS (placement); typeface DATUM (no capture has a drop-down open)
```

### A35 — the pointer sprite's size

The pointer is on the glass in **`SCREEN2` only**. Isolated as the one small 4-connected component of pure
`#ffffff` (window borders use `#ffffff` too, so the size filter is what finds it), it is a **6×8 px core of
28 white pixels** whose full anti-aliased footprint — the pixel-diff bbox against `SCREEN0`, which has no
pointer there — is **`x760..766, y208..216`, i.e. 7×9 px**. It is sitting on the quarry window's title bar,
a tip-at-top-left arrow.

A 9-px-tall arrow is the **`compositor=9x9` regime** of the two the `[sprite]` witness named
(`size=18x18 scale=2 … compositor=9x9 backbuffer=18x18 same=0`), and it is the regime that applies **over a
window**. No capture puts the pointer on the desktop backdrop — `SCREEN0`, `SCREEN1` and `SCREEN3` have no
sprite anywhere — so the "grows over the backdrop" half is measured on one side only. The datum is still
worth having: it pins the window-side sprite at 9 px on metal, which is the half of the divergence the
`pal.rs` fix must *not* change.

```
A35 pixels: pointer arrow present in SCREEN2 only, 6x8 core / 7x9 footprint at (760,208) over a window; absent from SCREEN0/1/3 -> DATUM (window regime = compositor 9x9; no capture puts it on the backdrop)
```

### A33 / SO4 — where the crystal glyph actually sits

R25 asks for the x of the crystal glyph's left edge in every capture. It is the same in all four:

| capture | crystal glyph | left edge x | gap_right |
|---|---|---|---|
| `SCREEN0` | `16x22 at (1904,6)` | **1904** | 0 |
| `SCREEN1` | `16x22 at (1904,6)` | **1904** | 0 |
| `SCREEN2` | `16x22 at (1904,6)` | **1904** | 0 |
| `SCREEN3` | `16x22 at (1904,6)` | **1904** | 0 |

Exactly the wire's `glyph=16x22+1904 bar_w=1920 gap_right=0`. In a 1920-wide bar the glyph's left edge is
**1904 px from the left**, and the leftmost ink in the bar is the app title at `x12`. R25 rules the crystal
is the Mac menu and belongs top-**left**; the flown image puts it hard against the right edge. CRYSTALFIX
(flush LEFT, drop-down at `x=0`) is unflown, and this is the glass measurement that says so.

```
A33/SO4 pixels: crystal glyph left edge x=1904/1904/1904/1904 (16x22, gap_right=0) in a 1920-wide bar, all four captures -> FAIL (R25 puts the crystal top-LEFT; it is at the right edge)
```

### A29 — window id / chrome

**Chrome.** Eleven title-bar disc groups across the four captures (4 / 4 / 4 / 2 — `SCREEN3` has two because
the console's own title bar is under the quarry). Every group is identical: a 24×24 `#ff5f57` close disc at
box + 17 x / box + 10 y, amber at + 53, green at + 89. No window on any capture is missing chrome, and no
window's chrome is drawn at a different offset from any other's.

**The shell.** The `970x510 at (923,61)` window titled `Shell` — jd2's banner and a live block caret — is
present in `SCREEN0`, `SCREEN1` and `SCREEN2` and **absent from every pixel of `SCREEN3`**. The box scan
finds no fourth window; the disc scan finds no fourth disc group. Meanwhile the console **did** come back,
re-placed 37 px lower. Three shell-pin presses, three drains, and no shell surface: this is the glass proof
that SO10's `route=already-live` drains resolve to the console's id and mint nothing.

```
A29 pixels (chrome): 4/4/4/2 title-bar disc groups, each 24x24 at box+17 / +53 / +89 x and +10 y -> PASS (chrome identical on every window instance)
A29 pixels (shell): shell box 970x510 at (923,61) in SCREEN0/1/2, ABSENT from SCREEN3; console box 1305x780 moves (307,158) -> (307,195) -> FAIL (the shell never comes back)
```

### SO11 — chrome width against surface width

**SO11 does not reproduce. The ~300 px reading is occlusion, and the row as filed is a misreading of the
same class as SO4's.**

The pulse window's surface spans `x15..1294`, 1280 px. Counting light (`min(rgb) ≥ 0xd0`) pixels along the
title-bar rows inside that span:

| row | SCREEN0 / 1 / 2 | SCREEN3 |
|---|---|---|
| `y923` | 1280 px, `x15..1294` | 296 px, `x15..311` |
| `y930` | 1223 px, `x15..1294` | 239 px, `x15..311` |
| `y940` | 1216 px, `x15..1294` | 232 px, `x15..311` |
| `y950` | 1280 px, `x15..1294` | 296 px, `x15..311` |
| `y952` | 1280 px, `x15..1294` | 296 px, `x15..311` |

(The 1223/1216 rows are the same full-width bar with the title text and disc glyphs punched out of it.)

On the three early captures the chrome runs the **entire** width of its own surface: 1280 px of chrome on a
1280 px surface. On `SCREEN3` the same rows stop at `x311`, and the very next pixel is `#000000` — the
console window's content. The console's box left edge is `x307`; `x308..311` is its anti-aliased border. The
pulse chrome is not truncated, it is **covered**, by a console window that was closed, re-minted by the dock
and therefore raised above the pulse in `SCREEN3` where it was below it at 15:43.

```
SO11 pixels: pulse title-bar chrome x15..1294 = 1280 px on SCREEN0/1/2 against a 1280 px surface; x15..311 = 296 px on SCREEN3, truncated at the console box edge x=307 with #000000 past it -> PASS (chrome == surface; SO11 is OCCLUSION)
```

### A26 — is the console window solid black?

Counted over the console's content rect, skipping every pixel a window above it covers (counting the whole
rect would be scoring the occluders, not the console):

| capture | content rect | unoccluded px | black | non-black |
|---|---|---|---|---|
| `SCREEN0` | `1295x736 at (312,197)` | 73 204 | 67 825 | **5 379 (7.35 %)** |
| `SCREEN1` | `1295x736 at (312,197)` | 73 204 | 67 825 | **5 379 (7.35 %)** |
| `SCREEN2` | `1295x736 at (312,197)` | 73 204 | 67 825 | **5 379 (7.35 %)** |
| `SCREEN3` | `1295x736 at (312,234)` | 387 354 | 387 354 | **0 (0.00 %)** |

`SCREEN3` is **exactly** black: not "no text I can see", but zero pixels differing from `#000000` over
387 354 of them. The `FLIGHT-RESULT`'s reading of the thumbnail is confirmed at full resolution.

**But the early captures are not.** The console carried monospaced light-on-black glyphs at 15:43 —
`#b6b6b6`, `#8e8e8e`, `#171717` anti-aliasing, in text rows on a regular pitch. A control on the same
capture separates text from window edges: the two strictly interior slivers (`x315..375 y215..915` and
`x1545..1602 y580..915`, no border, no chrome) hold 3 594 non-black px in 62 249, and **all of them are in
the left sliver** — the right one is solid black, because the text starts at the window's left margin.

Only the leftmost 67 px of the console are ever unoccluded on these captures, so *what* the text says is not
readable and this is not by itself a verdict on whether it was the kernel-log mirror. What it does settle is
that `mirror=off` at line 1 did **not** leave the console window text-free, and that the window's emptiness
in `SCREEN3` arrives **with** its close and dock re-mint, not before it. The emptiness and the route drop are
one finding, not two.

```
A26 pixels: console content 1295x736 — SCREEN0/1/2 (312,197) non-black 5379/73204 = 7.35% (monospaced grey-on-black glyphs); SCREEN3 (312,234) non-black 0/387354 = 0.00% -> DATUM (the console is NOT empty before its close; it is EXACTLY empty after the reopen)
```

### The dock

The dock is `444x52 at (738,1136)` and carries **four tiles in every capture**, boxes `x751..844`,
`x859..952`, `x967..1060`, `x1075..1168` — 94 px wide on a 108 px pitch. Their labels, read off the glyphs at
`y1156..1169`, are in order **`console`, `quarry`, `pulse`, `shell`**. Each tile's running dot is a 6×6 blob
at `y1179..1184`, centred at `x795 / x903 / x1011 / x1119`.

| capture | console | quarry | pulse | shell |
|---|---|---|---|---|
| `SCREEN0` | lit `#4a73aa` | lit | lit | lit |
| `SCREEN1` | lit | lit | lit | lit |
| `SCREEN2` | lit | lit | lit | lit |
| `SCREEN3` | lit | lit | lit | **grey `#cbcbcf`** |

Two things follow. The `FLIGHT-RESULT`'s reading is right on the dot — the **shell** tile, the fourth, is the
one that goes not-running, and it is the tile whose pin was pressed three times. And the dock **never fell to
three tiles on the glass**: it is four tiles wide in all four captures, including `SCREEN3`, taken nine
minutes after the console close that the report read `[dock] press … tile=2/3` as having shrunk it. `tile=2/3`
is an index, not a count.

```
dock pixels: 4 tiles (console/quarry/pulse/shell, boxes x751..844 / 859..952 / 967..1060 / 1075..1168, pitch 108) in all four captures; running dots 6x6 at y1179 — SCREEN0/1/2 4 lit + 0 grey, SCREEN3 3 lit + 1 grey -> PASS (the dock never fell to three tiles; the shell tile alone goes grey)
```

## Two corrections to FLIGHT-RESULT-render8.md

Both are readings of the `SCREEN3` thumbnail that the full-resolution capture does not support. Neither
changes a verdict in that document; both change a row on a ledger.

1. **SO11 is occlusion, not a chrome defect.** Filed as "the pulse window's title bar is ~300 px wide against
   a ~1290 px surface". Measured: 1280 px of chrome on a 1280 px surface in `SCREEN0/1/2`, and the 296 px
   reading in `SCREEN3` ends exactly at the console box's left edge with the console's own black past it. The
   row asks for a witness printing the chrome rect beside the surface rect; that witness would have printed
   equal widths and the divergence would have stayed unexplained. **No fix is owed.**

2. **Quarry's `MODIFIED` column is populated.** Filed as "`MODIFIED` column blank for every entry in
   `SCREEN3.PNG` while `SIZE` is". Measured over the column at `x985..1219`: **21 list rows carry ink, 18 of
   them a 199-px-wide date-time string** (`2026-09-06 15:33` and friends, at the same glyph scale as `SIZE`)
   and 3 a 12-px `--` — the three directory rows, which show `--` in `SIZE` as well. The `SIZE` column is
   inked on the same rows. Nothing is missing.

A third reading is confirmed rather than corrected: `[dock] press … tile=2/3` did **not** mean the dock had
fallen to three tiles (see the dock section).

## What is still owed on the glass after this

* **A31's typeface half** — needs a capture taken with a drop-down open. Three of the four PRTSCR presses
  landed with no menu on screen; the fourth (`SCREEN3`) has the quarry focused and the bar idle. A render9
  step that opens `View` and *then* presses PrintScreen closes this in one capture.
* **A35's backdrop half** — needs the pointer parked over the desktop backdrop when the shutter fires. The
  one sprite that was captured is over a window.
* **A34** — unflown, unchanged by this document.
