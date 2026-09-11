# TEAR-DIAG — why every instrument reads `torn=0` while the operator sees "A LOT OF TEARING"

Round: orin 24. Base fold `1b50376a` (render12). Evidence: `boot-render12-A1/A2/A3.log`
(Orin metal, 1920x1200, `display_tegra` inherited firmware scanout).
Operator, at the bench on render12, verbatim: **"THERE'S A LOT OF TEARING."**

## 1. What `torn=` measures

There are three `torn=` counters on the wire and all three compute the same predicate:

| counter | site | predicate |
|---|---|---|
| `[wc-h] … torn=` (window present) | `video/wcg.rs:1045` + `video/wcg.rs:1052` | `present_us > rectscan_us` |
| `[wc-h] win=… torn=yes/no` (per sample) | `video/wcg.rs:1359` | same |
| `[wc-k] … torn=` (desktop erase fill) | `video/wcg.rs:1904` + `video/wcg.rs:1905` | same |
| `[strip] … torn=` (dock/menubar/crystal) | `video/strip.rs:1085` + `video/strip.rs:1092` | same |

with `rectscan_us = FRAME_US * rows / panel_height` and `FRAME_US = 16_667`
(`video/wcg.rs:332`). The rollup verdict is `torn_n > 0 -> AT-RISK`, else
`declines > 0 -> UNSTAGED`, else `-> TEAR-FREE` (`video/wcg.rs:1476`).

So `torn=` asks **"did this copy take longer than the beam takes to cross the rows it
wrote?"** It is a **duration** predicate.

## 2. What it cannot see: PHASE

A present writes rows `[y0,y1)` of the **live scanout** — there is no back buffer and no
flip on this path; `video/wcg.rs:91` already states there is "no vblank synchronisation
anywhere in the path", and `video/wm.rs:18566` says the same. The write therefore begins at
an unknown, uniformly distributed phase φ within the frame period. The panel shows a seam
whenever the write interval overlaps the beam's traversal of those same rows:

```
P(tear) = min(1, (present_us + rectscan_us) / FRAME_US)
```

`torn = present_us > rectscan_us` fires only when the first term alone exceeds the second —
i.e. only when `P(tear)` has **already been pinned at 1.0** and has been for a long time.
Between `P = 0` and `P = 1` the counter is silent for the entire range.

Worse, the counter is **anti-monotone in the thing that makes a tear visible**: a taller
rect raises `rectscan_us`, which *raises the bar `torn=` is measured against*, while it
*raises* `P(tear)`. A full-panel present has `P = 1` and needs `present_us > 16667` before
`torn=` will admit it. **The bigger and more visible the tear, the harder this check is to
fire.** That is the LAWS §5 shape: a zero here is a fact about the pattern, not about
the panel.

### The measured gap, from the render12 wire

| line | box | `present_us` | `rectscan_us` | `torn=` | slowdown needed to fire | **P(tear)** |
|---|---|---|---|---|---|---|
| `[wc-h] win=1` | 1305x780 | 1115 | 10833 | no | **9.7x** | **0.717** |
| `[wc-h] win=2` | 1162x764 | 1169 | 10611 | no | 9.1x | 0.707 |
| `[wc-h] win=3` | 1290x212 | 147 | 2944 | no | 20.0x | 0.185 |
| `[strip] crystal` | 170x121 | 144 | 1680 | no | 11.7x | 0.109 |
| `[strip] dock` | 444x52 | 222 | 722 | no | 3.3x | 0.056 |
| `[strip] menubar` | 1920x34 | 197 | 472 | no | 2.4x | 0.040 |

A2 boot, win=3: `whole=2161` presents over `age_ms=641884` at `P = 0.185` gives an
**expected ~400 torn frames**, ~0.6/s, on a line that prints `torn=0 … -> TEAR-FREE`.
Nothing in that verdict is a lie about its own predicate; the predicate is not the question.

## 3. The populations no `torn=` covers at all

1. **The UNSTAGED / DIRECT path.** `stage_window` (`video/wm.rs:19968`) declines to the
   pre-WC-H direct path on `DECL_GEOM`/`DECL_CAP`/`DECL_LOCK`/`DECL_ALLOC`;
   `wcg::stage_decline` (`video/wcg.rs:763`) prints `staged=no … -> DIRECT`
   (`video/wcg.rs:1365`) and **records no duration and no `torn=` at all**. A3 boot:
   `win=1 declines=1818 decl_alloc=1818`, `win=3 declines=147 decl_alloc=147`, both
   `-> UNSTAGED`. Those 1965 composites did not memcpy a finished image; they ran
   `paint_window` **straight into the live scanout** for roughly a `compose_us` (measured
   3902 µs on the staged samples of the same window), so their `P(tear)` is
   `(3902+10833)/16667 = 0.88` — and the beam can catch a *half-drawn* window (chrome
   without content), not merely a seam. **~1700 near-certain, wholly unmeasured tears in
   one boot.** `decl_alloc` is `stage.try_reserve` failing at `video/wm.rs:20054`: the
   window needs `1305*4*780 = 3.9 MB` of per-core stage buffer against
   `MAX_STAGE_BYTES = 4 MiB`, so a busy heap drops the compositor into the tearing path
   and leaves it there.
2. **The cursor repair path.** `[cursor8] repair rate … repairs=4247 flush_kb=688033`
   (A3): 4247 panel writes with no tear instrument on any of them.
3. **The compgate FOLD.** A3 `[compgate] rollup entered=31740 folds=38955 waits=2826
   reruns=2166 maxhold_us=657195`. A folded present publishes pending and defers; the
   damage reaches the glass on a *later* pass. One logical update then becomes **two**
   panel writes at two unrelated phases — two seams for one frame.

## 4. Mechanism, in one paragraph

The Orin composites into cached RAM and then row-copies into the firmware's single live
scanout with no vblank reference, no back buffer and no flip, so **every present is a
partial overwrite of the image the beam is currently reading**. Tearing is therefore the
default state of this path and its rate is `sum over presents of
min(1,(present_us+rectscan_us)/FRAME_US)`, which on render12 is of order one visible seam
per second on the window path plus ~1700 in-place, half-drawn repaints per boot on the
UNSTAGED path. `torn=` reads 0 because it asks a *duration* question — "was the copy slower
than the beam" — whose threshold on this geometry sits 9x to 20x above the presents actually
observed, and which by construction gets *harder* to satisfy as the torn rect gets taller.
The compositor gate, the strip rollups and the erase witness all inherit that same
predicate, so 300+ clean samples across three boots are 300+ answers to a question the
operator never asked.

## 5. What follows

* An instrument that can fire: the **beam-crossing exposure** `beamppk` (per-mille
  `P(tear)`), summed per window into `beamcross` (expected torn presents) — it separates a
  banded 8-row console present (6 ppk) from a whole-box desktop present (716 ppk), which
  `torn=` cannot.
* The fix that removes the largest unmeasured population: `DECL_ALLOC` must **shrink the
  band and stay staged** rather than fall into the in-place direct path.
* Not fixable in this arc, recorded as owed: there is no readable raster position on the
  Tegra path — the nvdisplay aperture is power-domain guarded, the register model is
  unconfirmed (`arch/aarch64/display_tegra.rs:1884`) and this kernel has never written the
  display block (`writes=0`), so a true vblank wait or a double-buffer flip needs a Tegra DC
  rung of its own. The x86 Kepler path already reads a live `VERT (vline, vblank_count)`
  register at `drivers/gpu/kepler_display.rs:138` and the compositor consumes it nowhere:
  phase-locked presents are buildable there first.
