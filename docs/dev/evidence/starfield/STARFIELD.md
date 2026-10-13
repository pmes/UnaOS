# STARFIELD — the start screen's scene, and the capture to take (rmbp-ledger B661)

Branch `exec-rmbp-starfield`, cut from `23fe38b3`. Design record: `docs/dev/OS/05_USER_EXPERIENCE/STARFIELD.md`.
Files: `unaos/crates/kernel/src/video/desktop.rs` (the generator, the shimmer, `track`'s seat, the fixtures),
`unaos/crates/kernel/src/video/wallpaper.rs` (the picture carries its shimmer), `unaos/crates/kernel/src/video/theme.rs`
(one LINE-NEUTRAL re-export), `unaos/libs/sys/palette_core/src/lib.rs` (`starfield`, appended at the tail),
`unaos/crates/kernel/src/tests.rs` (LINE-NEUTRAL registration).

## 1. Peter's words (R128, verbatim in RULINGS.md) and the clarification

The start screen shimmers; the crystal less Picasso; the Starry Night flow as dark matter on ultra black; a beam
on a temple-sized zircon; the stars lensed by the flow's mass. Clarified mid-arc ("i meant the both combined"): ONE
picture — the zircon and its beam in the FOREGROUND over the lensed starfield and the flow, in this simple tier too.
The generator composes exactly that (design record §3): the gem is 45 % of the panel's height, lower right; the
stars and strokes lie behind it and the lens bends them round the dark-matter filaments across the whole panel.

## 2. What was measured, and what is owed to the fold

Executor rule (R107): one compile, no QEMU, no suite. Measured: the x86 line `cargo +nightly check` (the arc's
command, the wave's `kfeats24.txt` with `facet`, `wc`, `witness` armed) rc=0, no warning in the touched files. Owed
to the fold: `tests starfield` and `tests desktopcraft` on x86 QEMU, then the rMBP bench.

The cost of the one-time generation, from the code (2880x1800 = 5.18 Mpx):

| pass | work | estimate |
|---|---|---|
| field | 117 x 75 nodes x 5 noise reads + 4 blur passes + curl | under 2 ms |
| haze | per pixel: 2 bilinear reads (8 loads), a vignette multiply, a dither, ~40 integer ops | 5.18 M x 40 = ~210 M ops, ~70–110 ms |
| strokes | ~4 600 seeds x ~60 steps x (4 bilinear field reads + 1–3 splats of 4 px) | ~1.5 M pixel blends, ~25 ms |
| stars | ~3 000 stars, 4–40 pixel blends each (arcs, halos, spikes) | under 5 ms |
| zircon | ~9 front faces, each its bbox x (3–4 edge adds) | ~1 M px visits, ~10 ms |
| beam | 81 px x ~830 rows + landing disc ~60 000 px | under 5 ms |
| shimmer list | sort of at most 6 000 entries | under 1 ms |

Expected `gen_ms` about 120–160 on the rMBP (WALLPAPER's old 45 ms was the haze-equivalent pass alone), under the 400 ms
budget. The shimmer tick touches at most 7 beam groups + 4 cells: ~1 600 glint re-tints and at most ~37 000 px of
fill (four 95 px cells, the 1 px column, the landing disc), expected `shimmer_us` well under 800.

## 3. The wire the fold should read

    [starfield] gen wall_ms=<~150> stars=<~3000> lensed=<n> streams=<~4000> facets=17 front=<7..9> glints=<<=6000> groups=<n> surface=2880x1800
    :: STARFIELD: detail surface=2880x1800 glints=<n> groups=<n> zircon_box=<x>,<y>,<w>,810 front=<n> beam_land=<x>,<y> lens=ok(dx,dy) lens=ok(dx,dy) lens=ok(dx,dy) covered_changed=0 changed=<n> boxes=<n> shimmer_max_us=<n> black_runs=0 literals=0 ::
    :: STARFIELD: gen_ms=<n> stars=<n> lensed=ok streams=<n> zircon=ok(facets=17) beam=ok shimmer_us=<n> -> PASS ::
    :: DESKTOPCRAFT: corners=ok shadow=ok edge=ok(1px) hairline=ok black=ok windows=3 surface=2880x1800 -> PASS ::
    :: DESKTOPCRAFT: cost … black_runs=0 shimmer_us=<n> -> UNDER ::

## 4. The capture to take (Peter's eyes decide)

1. Dark, Una, no windows: the generated desktop after login — `prtscr` the whole panel. Read: the ground ultra black
   (not grey), the flow's strokes along the filaments with three swirls (upper left large, top centre-right, low left),
   the stars cleared from filament cores and arcing along their flanks, the zircon lower right with the beam landing on
   its upper-left crown facet, the fan of fire inside it.
2. The staged desktop (`UNAOS_SHOTSTAGE`, DESIGN-LANGUAGE §4) in Dark and in Light: the windows over the scene, the
   VEIL in the bar and the dock, the gem partly behind Lumen.
3. The shimmer, by eye: move the pointer slowly over an empty part of the desktop for ten seconds (each pass carries a
   tick): stars twinkle in scattered cells, the beam breathes with a pulse running down into the stone, glints travel
   along the strokes; nothing changes under a window. Idle (no pointer), the shimmer steps about once a second.

## 5. Owed outside this arc's files (named, not done)

* **The idle cadence.** The shimmer rides the compositor's passes (`desktop::track`). On an idle desktop the only
  taker is `wm::service_damage` (about 1 Hz), so a steady 30 Hz twinkle needs a timer-driven pass from the
  compositor's owner: in `video/wm.rs`, a `desktop::shimmer_due()`-style test (now minus `SHIM_LAST` at least
  `SHIMMER_MS`, a generated picture installed) added to `service_damage`'s idle early-return, plus a caller at
  frame rate (the render service's wait given a 33 ms timeout). Out of this arc (wm.rs is GATESTEAL's live lane).
* **The boot splash.** `unaos/crates/kernel/src/splash.rs` (SPLASH-2) draws "three irregular convex shards at varied
  tilts" under a dispersing beam before the desktop exists; if "the crystal shapes are a bit too on the picasso side"
  was said of THAT screen, the shards are there, outside this arc's files. The zircon here (`sf_zircon`) is the shape to
  give it.
* **The login screen**: none of `login.rs`, `loginrender.rs`, `loginwindow.rs` reads the wallpaper or its VEIL (a grep
  for `wallpaper`/`veil`/`live_back`; DESIGN-LANGUAGE §3.9 asks for the VEIL behind it). The scene shows from the
  first desktop pass after login; putting it behind the login card is the login lane's change.
