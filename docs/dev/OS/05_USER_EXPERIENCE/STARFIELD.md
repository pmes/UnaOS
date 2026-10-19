# STARFIELD — the start screen: dark matter, lensed stars, the zircon and the beam

rmbp-ledger B661. Ruling R128 (Peter's words, `docs/dev/RULINGS.md`). Code: `unaos/crates/kernel/src/video/desktop.rs`
(`wm::desktop`, the STARFIELD section: `wall_gen`, `shimmer_tick`, `starfield_selftest`; the live seat is the tail of
`track`), `unaos/crates/kernel/src/video/wallpaper.rs` (the picture carries its shimmer list; `shimmer_with`), the
palette in `unaos/libs/sys/palette_core/src/lib.rs` (`palette_core::starfield`, re-exported as `theme::starfield`).
Gate: the desktop family (x86 `wc`, aarch64 `desktop_firmware`) with `facet`, exactly WALLPAPER's; knob-off nothing
compiles. Fixture: `tests starfield` (and DESKTOPCRAFT's cost line gains `shimmer_us=`).

This file was written before the code (R106): the composition, the field, the lens, the gem, the beam and the
shimmer are decided here; the code follows it.

## 1. What Peter asked for

R128: the start screen *shimmers*; the crystal shapes are *less Picasso*; the backdrop is the Starry Night flow read
as *dark matter* on an *ultra black* screen; *a beam hitting a temple-sized zircon*; the background stars are
*gravitationally lensed* by the flow's mass. Two tiers, and this arc is the SIMPLE one: the generated default
wallpaper, one-time, with only a sparse shimmer per frame. The full scene is TEMPLE's game (SR147).

## 2. The budget, and where each part runs

| part | when | cost bound |
|---|---|---|
| the picture (ground, haze, strokes, stars, gem, beam) | ONCE per install, on the worker core (`wallpaper::generate`, the probe's and the theme regen's own path) | at most 400 ms at 2880x1800, printed `wall_ms=` on the `[starfield] gen` line and `gen_ms=` by `tests starfield` |
| the shimmer list | built by the same generation | at most 6000 entries (20 bytes each, 120 KB) |
| the shimmer | at most once per 33 ms, riding the compositor's own pass (`desktop::track`, the head of `composite_inner`) | at most 11 tiny erase boxes per tick, about 40 000 px of fill; `shimmer_us=` measured by the fixture against 800 us |

Nothing of the scene is painted per frame. The compositor's frame already costs 15.2 ms of 16.7 at flight 35.

## 3. The composition (2880x1800 at scale 2; every length scales with the panel's height)

DESIGN-LANGUAGE §4 stages Aether front centre-left, Settings behind it to the right, Lumen at the right edge, the dock
at the bottom centre, and keeps the upper left and lower right free. So:

* **The ground**: ultra black `starfield::VOID` (Dark `#040612` — never `#000000`: §2.3 reserves pure black for the
  console and the media grounds, and DESKTOPCRAFT's `black=` reds a pure-black run; this ground keeps a non-zero
  blue under two overlapping window shadows).
* **The dark-matter web**: a ridged multi-octave field drawn as a faint haze (`MATTER_DEEP`) over the whole panel,
  darker toward the corners.
* **The flow**: Van Gogh strokes — short parallel streamlines of the field's curl, on a jittered seed grid, each
  stroke's luminance its density (`MATTER_DEEP` -> `MATTER_MID` -> `MATTER_HI`). Three vortices give the Starry Night
  swirls: a large one in the free upper left (0.34 W, 0.30 H, radius 0.20 H), a counter-rotating one top centre-right
  (0.62 W, 0.16 H, radius 0.11 H) and a small one low left (0.14 W, 0.62 H, radius 0.13 H).
* **The stars**: behind the flow's light, lensed by its mass (§5). About 3000 at 2880x1800: 85 % one pixel, 12 % a
  soft two-pixel point, 3 % bright with a halo and four short spikes; a third of the bright ones warm
  (`STAR_WARM`, the Starry Night yellow), the rest `STAR`.
* **The zircon**: FOREGROUND, lower right — its centre at 0.78 W, its base at 0.965 H, its height 45 % of the panel,
  clear of the dock and in the free lower right. A natural zircon's habit made smooth: an octagonal prism under an
  octagonal pyramid and a base — 17 facets, convex, projected in perspective (§6).
* **The beam**: a narrow column from the top of the panel striking the zircon's upper-left pyramid facet (the lit
  one), a soft glow where it lands, a horizontal flare across the landing point, faint dust scattered along its
  length, and inside the gem a three-ray fan (`FIRE_WARM`, `ZIRCON_LIGHT`, `FIRE_COOL`) — the dispersion a zircon is
  prized for.

**Light** gets the same generator, quieter: the same draw order and shapes over the Light pair of every token (a pale
violet ground, the strokes as darker blue ink, the stars as muted dots, the gem darker than the ground). The accent
is not part of the scene; a theme change regenerates it (MENUSCRAFT's epoch path is unchanged).

## 4. The field (integer only; the kernel has no float here)

A node grid at cell `c = max(8, H/72)` px (25 px at 1800: 117 x 75 nodes), node `i` at pixel `(i - 1) c`.

* Value noise `vn(x, y, L, seed)`: a 64-bit mix hash of the lattice corner, Q12 values, smoothstep `t^2 (3 - 2t)`,
  bilinear. Lattice lengths `L1 = H/2.5`, `L2 = L1/2.1`, `L3 = L1/4.4`, `L0 = H/1.6`.
* Ridges: `ridge(n) = 4095 - |2n - 4095|`, sharpened `r = (ridge^2)^2` (Q12) — filaments where the noise crosses its
  middle, the cosmic-web look.
* Density `D = (5 r1 + 3 r2 (r1 + 1024) + 2 r3 (r2 + 1024)) / 10` (the finer octaves weighted by the coarser
  ridge, so a filament has substructure and the voids stay empty), clamped to `0..4095`.
* Potential `Phi = D` blurred twice by `[1 2 1]^2` (the lensing potential is smoother than the mass).
* Stream function `psi = Phi + (vn(L0) - 2048) / 2 + sum_k A_k (1 - r^2/R_k^2)^2` over the three vortices
  (`A = 6000, -4000, 5000`).
* Flow `u = (d psi/dy, -d psi/dx)` by central differences, normalised to Q12 per node. Strokes therefore run ALONG
  the filaments (the curl of a ridge field follows its contours) and wheel round the vortices.
* Lens gradient `G = grad Phi` (per cell, central differences); `Gmax = max |G|` over the grid.

Per pixel the haze reads `D` and `Phi` bilinearly (per-column and per-row index tables, four loads), mixes
`VOID -> MATTER_DEEP` by `v^2` with `v = (3 D + Phi) / 4`, a vignette scales the added part only, and the result is
ordered-dithered (the 4x4 Bayer table) so an 8-bit panel shows no banding.

## 5. The lens math (weak lensing, the simple tier)

A background star has a SOURCE position `b` (deterministic hash). The flow's mass bends its light; in the thin-lens
equation `b = t - a(t)`, for mass concentrated in a filament the deflection `a` points AWAY from the mass, so the image
`t` is pushed outward from the dense filament. The simple tier evaluates the deflection at the source (first order):

    G     = grad Phi (b)                      bilinear from the node grid
    delta = -G * Dmax / Gmax                  Dmax = H/100 px (18 px at 1800); |delta| <= Dmax
    t     = b + delta                         the image position, Q8 sub-pixel
    l     = 1 + |G| * Smax / Gmax             tangential stretch (an arc), Smax = H/225 px
    tdir  = (-Gy, Gx) / |G|                   along the filament: the arc is drawn along tdir
    mu    = 1 + 0.75 |G| / Gmax               magnification of the star's brightness

So stars are cleared from the filament cores, pile up along their flanks, and the ones near the steepest gradient are
stretched into short arcs ALONG the filament — they arc round the mass. The fixture checks three known stars (the
three largest displacements among the bright compact stars clear of the foreground): `|delta| >= 1`,
`|delta| <= Dmax + 1`, `delta . G < 0` (displaced away from the mass), and a star is drawn at `t` (brighter than the
ring 5 px round it) — `lens=ok(dx,dy)` each.

What TEMPLE renders in full instead: a 3D density volume, the deflection integrated along each ray (not evaluated at
the source), multiple images and Einstein rings at caustics, a moving camera, and the gem ray-traced with refraction
and dispersion per wavelength.

## 6. The zircon (convex polyhedron, flat-shaded, Fresnel rim)

Model (Q12, unit 4096): ring radius 1, the octagon's corners at `20 + 45 k` degrees (the 20 degree yaw keeps an edge
off the centre line), prism from `y = -1.75` to `y = 0.30`, apex at `y = 1.55`. Faces: 8 prism quads, 8 pyramid
triangles, the base octagon — 17. Outward normals by cross product, flipped against the centroid. Tilt -15 degrees
about X (the top toward the camera, so the crown shows), the camera at the origin looking +z, the gem at `z = 7`.
Projection `s = (X/Z, -Y/Z)`, then one scale and one shift so the projected height is 45 % of the panel and the base
sits at 0.965 H, centre 0.78 W. Sines are Bhaskara's integer approximation (error under 0.2 %).

Visibility: a face is drawn when `N . P < 0` (it faces the camera); for a convex body the front faces tile the
silhouette without overlap, so no depth buffer is needed. Rasterisation: per face, per row, each edge's signed
distance (Q16 px, stepped by one add per pixel); an edge whose neighbour face is BACK-facing is a silhouette edge
and is anti-aliased over one pixel; an interior edge is a hard boundary and a 0.6 px highlight (`ZIRCON_LIGHT`,
faint) — the crisp facet lines of a cut stone.

Shading, per facet, one light (the beam's, from above, a little left and front, `L = (-0.22, 0.93, -0.30)`):

    ndl = max(0, N.L)    base   = mix(ZIRCON_DEEP, ZIRCON_MID, ndl)
    H = norm(L + V)      spec   = (N.H)^32          -> mix toward ZIRCON_LIGHT by 0.86 spec
    ndv = max(0, N.V)    F      = (1 - ndv)^3       -> mix toward ZIRCON_LIGHT by 0.59 F   (the Fresnel rim)
    a crown facet facing the light: a 16 % tint of FIRE_WARM / FIRE_COOL alternating (fire)
    alpha = 0.82 + 0.18 F                           (the flow shows faintly through the stone)

Within a facet the colour darkens toward the base by up to 28 % (`mix(c, ZIRCON_DEEP, y)`), and a 1.5 px rim of
`ZIRCON_LIGHT` runs along the silhouette. Not cubist: one symmetric stone with smooth, coherent light — none of the
old generator's shapes survive (it had a two-tone diagonal and one soft accent disc).

## 7. The beam

Column `x = apex - 0.16 gem width`, from the top of the panel down to the silhouette's top at that column (the
landing point). Core half-width `max(1, H/600)`, glow half-width `H/45`; brightness ramps from 0.38 at the top to
1.0 at the landing. Dust: about one mote in 23 rows, inside the glow. Landing glow radius `H/13` (cubic falloff) with
a hot core `H/200`; a horizontal flare 2.6 glow radii each side, 3 px tall. The inner fan: three rays from the landing
point down-right through the stone (angles 64, 68, 72 degrees below the horizontal), drawn only inside the silhouette.

## 8. The shimmer

Every entry is a pixel of the FINAL picture: `(x, y, rest, tint, phase, rate, amp)`. At time `t` (ms) its value is

    s   = smooth(triangle((t * rate * 8 + phase) mod 65536))      0..255
    out = mix(rest, tint, amp * s / 256)

so at `s = 0` the pixel is exactly the generated picture and the shimmer only departs from it and returns.

| entries | rest / tint | rate (period) | amp |
|---|---|---|---|
| star twinkle (1 px and 2 px stars, bright cores) | the star / the ground 3 px away | 2..6 (1.4–4.1 s) | 140–200 |
| lensed drift (stars with `|G| > Gmax/4`, compact) — a PAIR: the star pixel and its neighbour along `-G` | each the other's rest | 1 (8.2 s) | 128: a cross-fade of at most half, the star drifts at most 1 px along the lens gradient |
| streamline glints (every 7th step of a bright stroke) | the stroke / `MATTER_HI` | 3 (2.7 s), phase along the stroke: light travels down the stream | 140 |
| the beam (its centre line, every row) and its landing (a disc of radius `H/200 + 4`) | the beam / `BEAM_CORE` | 2 (4.1 s), phase falling with `y`: the beam breathes and a pulse runs down into the stone | 100 / 120 |

Entries on or next to the foreground (the gem's silhouette, the beam's glow) are dropped from the star and stream
kinds. Caps: beam 1600, twinkle 2000, drift 800 (400 pairs), streams the rest, 6000 in all.

**Groups.** Entries are sorted into cells of `max(32, H/19)` px (95 px at 1800), plus seven beam groups (six column
segments and the landing). A group's box is the bounding box of its entries.

**The tick** (`shimmer_tick`, pure; the live seat is `desktop::track`'s tail, so it runs at the head of every
composite pass, before the erase drain that paints what it queues in the SAME pass): at most once per 33 ms, only when
the erase queue is empty and no shadow fill was queued this pass (the shimmer yields to real work). It re-tints every
beam group and the next 4 star cells in a coprime-stride order (scattered, so the twinkle never sweeps), writing the
new values into the installed picture's buffer, and queues each re-tinted group's box with `defer_erase` — the
compositor's own erase route: `stage_fill` paints the wallpaper there, shades every window shadow, clips every window
box and the furniture, and handles the cursor. A group whose box meets any visible window's occluder box (its shadow
extent) is skipped: **the shimmer stops where a window covers it**, and no window is re-damaged by it. At most 11
boxes per tick (`MAX_DEFER` is 16).

**The cadence it gets.** The compositor runs passes on demand: with the pointer moving or anything drawing, the shimmer
runs at up to 30 Hz; on a desktop where nothing draws, `wm::service_damage` is the only taker (the dock's once-a-second
service pass), so the twinkle steps about once a second. A steady 30 Hz on an idle desktop needs a timer-driven pass
from the compositor's owner (`wm.rs`, outside this arc's files) — owed, named in the report.

## 9. From the first frame (STARTSCENE, rmbp-ledger B662)

R128's "i meant the both combined" asks for ONE picture. STARFIELD built the picture as the generated wallpaper.
Its fold left one thing owed: the boot splash and the login backdrop did not show it. The boot screen becomes the
desktop in four steps. Written before the code (R106):

| step | where | painted before | paints now |
|---|---|---|---|
| 1. GOP stage | `splash::gop_stage`, at boot entry, pre-heap, allocation-free | the three quartz shards and the spectrum rays | the scene's GROUND: `VOID` and the picture's own lensed stars (`splash::gop_scene` -> `wm::desktop::stars_onto`). `[splash] gop scene=ground stars=<n> ms=<n> WxH=<w>x<h>` |
| 2. the hold | `splash::hold_prepare` at the Kepler takeover (`fbcon` `takeover_blit`, then `desktop_uefi::activate_on`); parked by `hold_open` as the topmost row until `users::stage_resolve` | the same shards again, re-traced into the hold store | THE generation: `wallpaper::generate_splash` copies the finished scene into the hold store and installs it as the wallpaper. `:: SPLASH: … scene=1 wall_ms=<n> -> painted ::`, `[startscene] splash gen wall_ms=<n> …` |
| 3. the login screen | `login::open_as` opens its window over the backdrop | the flat `DESKTOP_BG` (no picture was installed yet) | the installed picture: the glass's `stage_fill` and the desktop layer's `wp_paint` both draw it, and the shimmer rides `track` round the login window's occluder box |
| 4. the desktop | `desktopbuild::build` -> `wallpaper::rearm` -> `probe` | the scene, generated for the first time here, after `[desktop] built` | the same picture, kept: `[wallpaper] probe try=1 found=0 kept=generated`. A found `WALL.PNG` still replaces it. `regen_on_theme` regenerates only on a Light/Dark change (the scene reads no accent), so a session's accent keeps the picture: `[wallpaper] regen … kept=generated reason=accent-only` |

**Where the generation runs.** It runs once, on the boot core, inside `hold_prepare`. That is the first point where
the heap exists (the hold store is allocated there), the panel's size is known (the `WRITER` handle), and the
scene's inputs are live (`theme::is_dark` and the `theme::starfield` pairs; the accent is not one of them). The
cost is §2's `wall_ms`, at most 400 ms at 2880x1800, paid once during the takeover instead of later on the worker
core. If generation fails (no memory), the hold paints the crystal as before.

**The GOP stage's stars are the picture's stars.** The generator cannot run before the heap. So the field and the
star pass are factored out of `wall_gen`, and the stage runs them without the heap:

* `sf_field_fill` computes into caller-owned tables: the heap's for the picture, or a static five-table scratch of
  12 288 nodes for the GOP stage. The stream function is skipped when there are no strokes to read it.
* `sf_stars_core` draws through a plot sink and uses the same seed (`STAR_SEED`).
* The arithmetic is identical either way, so every star lands on the same pixel.
* `stars_onto` writes onto VRAM without ever reading it back. It keeps a 4096-slot cache of what it last wrote, so
  overlapping splats accumulate as they do in the picture.
* The web, the gem and the beam arrive with step 2's full paint: a fade-in, not a cut.

**The hold shimmers.** While the hold row covers the panel, `track`'s shimmer skips everything: the row is an
occluder. So `hold_service` ticks instead. It runs on the ~1 kHz device-service pass, which is fast enough that the
33 ms floor is what binds; a boot step that holds the pass stalls the twinkle for that long. Each tick:

* is at most one `shimmer_tick` per 33 ms;
* writes into the HOLD STORE, never the picture;
* treats the caption bands as the occluder (the step's word and the OVER line, the bottom three band heights);
* damages each re-tinted box on the row, then makes one `wm::composite`.

aarch64 has no device-service caller of `hold_service`, so there the hold does not shimmer.

**Who owns the backdrop.** `wallpaper::owns_backdrop()` is true when REALDESK's latch is set OR the splash has
installed the scene. It gates `wm::wp_paint`, `take_stale` and the live shimmer. Finding: on x86 REALDESK's latch is
never set (`retire_desktop_chrome` runs only on the Pi's and the Orin's render paths). Before this arc, those three
were therefore dark on x86, including §8's live shimmer.

**The shards.** A scene build (`wc` and `facet`) no longer paints them. `advance` keeps its SPLASHRETIRE seam and
the `gui` backstop: `ANIM` is armed, `SPLASH_FB` is never published, so it paints nothing. The shard code stays,
because builds without the generator (knob-off, no `facet`) and the hold's no-memory fallback still call it. Every
`splash.rs` edit is a same-line fold, a line-count-equal rewrite inside a `wc`-gated body, or an append at the foot,
so no knob-off line moves.

**Fixture**, `tests startscene`. It checks four things:

* `gen_count=1`, and the installed buffer is the one the splash installed;
* `splash_scene=1`;
* `hold_ticks>0` whenever the hold lasted at least 66 ms;
* `gop_stars=ok`: `stars_into` on an offscreen `VOID` buffer lights all 16 of the picture's sampled star pixels
  (bright, compact, evenly spaced).

`login_backdrop=picture` is reported when the picture owns the backdrop and is installed.

    :: STARTSCENE: detail installed=<0|1> held_ms=<n> lit=<n>/<m> surface=<w>x<h> ::
    :: STARTSCENE: gen_count=1 splash_scene=1 gop_stars=ok hold_ticks=<n> login_backdrop=picture -> PASS ::

## 10. Files and seams

* CHARTER: Kernel — wm (`desktop.rs`'s own charter line; the generator is the desktop's paint). No new file in
  `video/`.
* `palette_core::starfield` — 13 `(Light, Dark)` pairs, appended at the crate's tail; `theme.rs` re-exports the module
  on an existing line (LINE-NEUTRAL: `theme.rs` is in every image).
* `wallpaper.rs` — `Wall` and its published `Snap` carry the picture's `Shimmer` (boxed, so its address survives the
  move into the lock; retired with the picture); `generate` prints `[starfield] gen wall_ms=…`; a picture FILE has no
  shimmer.
* `desktop.rs` — the generator replaces `wall_gen`'s body; `track` gathers the visible windows' occluder boxes in its
  existing table walk and calls the tick before its early return.
* `tests.rs` — `register("starfield", …)`, a LINE-NEUTRAL append beside `desktopcraft`.

## 11. The wire

    [starfield] gen wall_ms=<n> stars=<n> lensed=<n> streams=<n> facets=17 front=<n> glints=<n> groups=<n> surface=<w>x<h>
    :: STARFIELD: detail surface=<w>x<h> glints=<n> groups=<n> zircon_box=<x>,<y>,<w>,<h> front=<n> beam_land=<x>,<y> lens=ok(<dx>,<dy>) lens=ok(<dx>,<dy>) lens=ok(<dx>,<dy>) covered_changed=0 changed=<n> boxes=<n> shimmer_max_us=<n> literals=0 ::
    :: STARFIELD: gen_ms=<n> stars=<n> lensed=ok streams=<n> zircon=ok(facets=17) beam=ok shimmer_us=<n> -> PASS ::
    :: DESKTOPCRAFT: cost … black_runs=<n> shimmer_us=<n> -> UNDER ::

Evidence and the capture to take: `docs/dev/evidence/starfield/STARFIELD.md`. Peter's eyes decide.
