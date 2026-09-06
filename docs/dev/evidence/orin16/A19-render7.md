# A19 pixel leg — render7 (`SCREEN0.PNG`)

Executor A19PNG, orin 16, 2026-09-06. Closes the half of A19 that
`docs/dev/evidence/orin16/FLIGHT-RESULT-render7.md` left owing: the row reads
*"WIRE PASS, second pass. Pixel leg **harvested but not yet read** — `SCREEN0.PNG` is off the card
(`render7-card-harvest.sha256`); the band read still owes a run against the full-resolution file"*,
and the A19 cell itself says *"now `A19-pngband.py SCREEN0.PNG` must read `non-bg=0/60200`"*.

## Verdict

**A19 PASS — the pre-cascade shell-text band is gone on the glass, second pass.**
`SCREEN0.PNG` band `x0-700 y34-120` reads **`non-bg=0/60200 (0.0%)`**, both controls 0
(`ctrl-right` 0/60200, `ctrl-below` 0/30000). This is the exact number the render7 flight demanded,
and it matches render6's pixel leg byte for byte. The wire leg
(`band_cleared=1 shell_present=1 jd2_probe=1`) and the pixel leg now agree.

Ledger line:

> A19 pixels (render7): `A19-pngband.py SCREEN0.PNG` → band `non-bg=0/60200 (0.0%)`, controls 0/60200 and 0/30000 — **PASS, second pass** (render6 also 0/60200; render4 read 855/15050 = 5.7%). Scorer can-fire proved this run (synthetic band text → 4800/60200 FAIL).

## Script provenance

**The script was not lost and needed no reconstruction.** It is committed in this repo at

    /home/pmes/src/github.com/pmes/UnaOS-orin/docs/dev/evidence/orin14/A19-pngband.py
    sha256 0344fa2f3e8b474b2bb79adaaaa9fa36f637915c4060f6a4f56c3b02856c25d1

introduced by `6f56eff8` *"tegra: A19 — the top-left band is the shell painting the panel AFTER the
cascade; the shell gets its own window"* (`git log --all --oneline -- '*A19-pngband*'`), and it is
the same file render6 scored with (`docs/dev/evidence/orin15/FLIGHT-RESULT-render6.md`, A19 row).
It was copied verbatim into this working directory — **not modified** — as
`~/unaos-bench/scratch/orin16/a19png/A19-pngband.py` (sha above, identical to the repo copy).

The repo checkout is read-only for this executor; nothing in the repo was touched.

Geometry and constants, from the script's own header and body (unchanged since render6):

| item | value |
|---|---|
| band rectangle | `x 0-700, y 34-120` → 700 x 86 = **60200 px** (the flight's denominator) |
| control right | `x 700-1400, y 34-120` → 60200 px |
| control below | `x 0-300, y 120-220` → 30000 px (backdrop by construction: the cascade's console box starts at x=307) |
| menubar (informational, non-bg by design) | `x 0-700, y 0-34` → 23800 px |
| background | `wm::DESKTOP_BG` = `0x002D2B55` (R=0x2D G=0x2B B=0x55) |
| tolerance | 8/255 per channel, max-abs across R,G,B |
| verdict rule | PASS iff the **band** reads 0 non-bg |

The scorer is stdlib-only — it decodes zlib + all five PNG filter types itself — so **PIL was never
needed** and `flatpak-spawn --host` was not required. The sandbox `python3` ran it directly, ~0.05 s
per 6.9 MB file.

## Inputs — sha-verified against the card harvest

Files: `~/unaos-bench/scratch/orin16/render7-card-harvest/` (1920x1200, RGB, bpp=3, confirmed by the
scorer's own IHDR read). Each sha re-derived this turn with `sha256sum` and matched against
`docs/dev/evidence/orin16/render7-card-harvest.sha256`:

| file | sha256 | matches repo record |
|---|---|---|
| `SCREEN0.PNG` | `93df21829cea6ca19f4f0a33f05ac70004644e679c22e6b990db026d2334ca8d` | yes |
| `SCREEN5.PNG` | `4c285db0e3e4f5d2ac77bc4f8242263614fc8989093bb118d822463b66970bec` | yes |
| `SCREEN6.PNG` | `fa2ccbd5da6bc36e84c0fc73a0e2d2c4caa9e7b4a49286a0648bcee5fca85368` | yes |

`HARVEST.md` in that directory records the provenance: *"TIDY at 2026-09-06T12:06:10Z: SCREEN0-6 +
UPD1-4 harvested to /home/pmes/unaos-bench/scratch/orin16/render7-card-harvest (SHA256SUMS) and
removed from card"*. These are the card's own bytes, not a re-render.

## Exact command

```
cd ~/unaos-bench/scratch/orin16/a19png
cp /home/pmes/src/github.com/pmes/UnaOS-orin/docs/dev/evidence/orin14/A19-pngband.py .
H=~/unaos-bench/scratch/orin16/render7-card-harvest
for f in SCREEN0 SCREEN5 SCREEN6; do python3 A19-pngband.py $H/$f.PNG; done
```

## Can-fire proof

A zero from this scorer is only admissible if the scorer can produce a non-zero. Both directions were
proved this turn with synthetic 1920x1200 PNGs built by `canfire-gen.py` (stdlib zlib, filter type 0,
every pixel `0x2D2B55`; the "text" variant additionally paints a white 300x16 = **4800 px** block at
`x 40..340, y 60..76`, i.e. *inside* the band rectangle):

```
$ python3 A19-pngband.py canfire-text.png
canfire-text.png: 1920x1200 bpp=3
band x0-700 y34-120: non-bg=4800/60200 (8.0%)
  rows with non-bg: y 60..75 (16 rows); x extent 40..339
  row groups (text lines): [(60, 75)]
ctrl-right x700-1400 y34-120: non-bg=0/60200 (0.0%)
ctrl-below x0-300 y120-220: non-bg=0/30000 (0.0%)
A19 scorer verdict (band must be 0 non-bg): FAIL

$ python3 A19-pngband.py canfire-clean.png
band x0-700 y34-120: non-bg=0/60200 (0.0%)
A19 scorer verdict (band must be 0 non-bg): PASS
```

**CAN-FIRE: PROVED.** The scorer counted the planted block exactly — 4800 of 4800 planted pixels,
zero spill into either control, correct row extent `y 60..75` and x extent `40..339` — and returned
FAIL. The clean control returned 0/60200 and PASS. The scorer discriminates; `SCREEN0`'s zero is a
real measurement, not a dead check.

## Results — three captures

| capture | band `x0-700 y34-120` | ctrl-right | ctrl-below | scorer verdict |
|---|---|---|---|---|
| **`SCREEN0.PNG`** | **0/60200 (0.0%)** | 0/60200 | 0/30000 | **PASS** |
| `SCREEN5.PNG` | 42249/60200 (70.2%) | 60200/60200 (100%) | 26980/30000 (89.9%) | FAIL (occluded — see below) |
| `SCREEN6.PNG` | 27660/60200 (45.9%) | 22242/60200 (36.9%) | 24400/30000 (81.3%) | FAIL (occluded — see below) |

(The `bar x0-700 y0-34` row reads 23800/23800 on all three — the menubar is non-bg by design and the
script prints it as informational only.)

### SCREEN0 — the A19 subject

`SCREEN0.PNG` is the capture A19 is about: the first Print Screen of the boot, the cascade freshly
laid out. Band and *both* controls read exactly zero. Cross-checked against
`docs/dev/evidence/orin16/render7-SCREEN0-small.png`: the top-left quadrant is unbroken desktop
backdrop, and the nearest window furniture (the console box) begins around full-res x≈310, y≈156 —
outside the band and outside both controls, which is why the controls also read 0. Compare render4's
failing signature, 855/15050 = 5.7%: sparse, low-fill, discrete row groups — the visual signature of
glyphs. Nothing of that kind survives in render7.

### SCREEN5 / SCREEN6 — contrast runs, and why their FAIL is *not* an A19 regression

Both were requested for contrast and both are **later** captures with windows moved over the
band, so the scorer's rectangle is occluded by furniture. Three independent facts separate window
fill from shell text, and all three say "furniture":

1. **Fill fraction.** Text fills ~5% of its band (render4: 5.7%). These read 46% and 70%, with
   SCREEN5's right control at a saturated **100%** — a solid opaque region, not glyphs.
2. **Row structure.** The scorer's row-group summary collapses to a *single* contiguous group in
   each — `[(34,119)]` for SCREEN5, `[(77,119)]` for SCREEN6 — i.e. every row in the range is hit.
   Text produces several disjoint groups, one per line.
3. **Geometry matches a known window.** SCREEN6's hits start at exactly `y=77, x=56` and run to the
   band's right edge. `render7-SCREEN6-small.png` shows the quarry window's top-left corner at
   preview (28,39) = full-res (56,78) — the scorer is reading quarry's title bar and body.
   SCREEN5 likewise has the quarry window dragged into the top-left corner *and* the crystal
   drop-down (`About This Shard / Sleep / Restart / Shut Down`) open directly inside the band, which
   is Peter's A33 observation.

So the SCREEN5/6 numbers are a **negative control that behaves correctly**: the scorer is not blind
to content in that rectangle at this stage of the boot, and it reports content when content is there.
They do not bear on A19, whose claim is about the *pre-cascade shell painting the panel itself* and
which is scored on `SCREEN0`.

A note for whoever reuses this scorer: it is a pure "is this rectangle the background colour"
counter with no notion of window ownership, so it is only meaningful on a capture where the band is
known to be unoccupied backdrop — `SCREEN0`. Pointing it at an arbitrary later screenshot will
report furniture as a failure. That limitation is inherent to the check and is not a defect.

## Files

- `~/unaos-bench/scratch/orin16/a19png/A19-render7.md` — this document
- `~/unaos-bench/scratch/orin16/a19png/A19-pngband.py` — verbatim copy of the repo scorer
- `~/unaos-bench/scratch/orin16/a19png/canfire-gen.py` — can-fire PNG generator
- `~/unaos-bench/scratch/orin16/a19png/canfire-clean.png`, `canfire-text.png` — can-fire inputs
