# The graphics engine (Vug) — primitive ledger

`vug` (`unaos/crates/kernel/src/vug.rs`) is UnaOS's software 3D demo **and** the
graphics engine's testbed. Canon (`docs/CODEX.md` §5): Vug is the sculptor, the
future 3D CAD app; a *vug* is a crystal-lined cavity in rock. The demo shows a
real-time, software-rendered rotating quartz crystal, and every new engine
capability lands here with a visible artifact.

This file is the **running ledger of engine primitives** — what the renderer can
do, where each primitive lives, and its invariants. It grows as the engine does.

---

## 0. THE METRICS RULE (standing directive — UI-1)

**No absolute pixel sizes in UI code.** One metrics layer
(`unaos/crates/kernel/src/ui.rs`) derives an integer `scale` from the panel's
real height (`scale = clamp(height / 900, 1, 4)` — 1 at ≤ 900p, 2 at
1800p-class panels such as the rMBP Retina, capped at 4), and **every** UI
dimension derives from the resulting [`ui::Metrics`]:

| metric | derivation | meaning |
|---|---|---|
| `cell_w` / `cell_h` | `8 · scale` | the glyph cell; the text advance; **the cursor is exactly one cell, by construction** |
| `line_h` | `cell + cell/2` | text row pitch (1.5-line rhythm); the input-line clear strip |
| `margin` | `line_h` | page inset of screen-owning views |

With a PAL in hand, `pal.metrics()` (a provided `GneissPal` method) returns the
surface's metrics; they are a pure function of panel height — nothing to
initialise, nothing to go stale. `pal::draw_text` is scale-aware: each glyph
pixel renders as a `scale`×`scale` block (the scale-1 path is the classic
per-pixel loop). `Console::page_rows` — the single source of truth for page
height, which `selftest::Pager` shares — computes from the derived `line_h`.

Evidence line, printed once per surface bring-up (`TargetPal::new`), on every
target, headless-visible:

```
:: UI1: scale=N cell=WxH line=H ::
```

The one deliberate exception: the pre-heap boot console (`video/fbcon.rs`)
keeps its own unscaled 8-px font — it exists before the allocator and is out of
the GUI's life cycle. Anything else that names a pixel count is a bug.

> **Arch-neutral, float-free.** `vug` compiles on x86_64 and aarch64 and is
> reachable from the Orin panel shell (JD2), so it carries no `cfg(tegra)` and
> uses no floating point: all geometry and maths run in **Q16.16 fixed point**
> (`type Fx = i32`; the integer value is `real * 65536`) off a 256-entry *brad*
> sine table (256 brads = one turn).

### 0a. The GUI status strip (PI-UI-2)

`unaos/crates/kernel/src/ui_status.rs` draws an always-on one-line status bar
pinned to the **bottom** line of the panel, surfacing the net/time state a bench
user would otherwise only read on serial:

```
unaos.local   ip 192.168.2.3   2026-07-22 14:31:07 UTC
```

with honest placeholders before the state settles: `no lease` until a bring-up
records an address, `unsynced` until SNTP seeds the wall clock. All geometry
derives from `ui::Metrics` (THE METRICS RULE — the band is one `line_h`, the
glyph cell is centred within it); nothing names a pixel.

**Read-only by construction.** The strip consumes only public snapshot
accessors — `clock::now()` for the wall clock and `net_phy::settled_ipv4()` for
the interface address. The latter is a lock-free atomic snapshot recorded by
`net_phy::dhcp_or_static` (the single chokepoint every arch's bring-up funnels
through), so the render core reads the address without reaching into any
driver's private `NetService` state and takes no net lock in the render path.
The hostname is the fixed mDNS/DNS-SD name `unaos.local` (net11/net17).

**Refresh.** On the Pi bare-metal path the render task (`render_service`,
`main.rs`) draws the strip **after** the console each frame (so it always sits on
top) and presents. A tiny `status_tick` task posts an `Event::Timer` to
`GUI_CHANNEL` once a second (mirroring `rx_backstop`), waking the render loop to
re-draw so the clock advances and a late lease appears without a keystroke. It is
timer-gated at spawn (its `sleep_ticks` nap needs the live timer IRQ); in QEMU
raspi4b, where no Group-1 IRQ is delivered, the tick task is not spawned and the
strip refreshes on input instead. Bring-up witness, printed once by
`render_service`:

```
:: UI2: status strip armed (host+ip+time, 1 Hz) ::
```

---

## 1. Drawing primitives (in `video/` + `pal.rs`)

Added additively — the existing `fbcon`/`Console`/`Screen` contracts (the x86 GUI
and the JD2 panel shell ride them) are unchanged.

| Primitive | `FrameBuffer` | `Screen` (damage-tracked) | `GneissPal` |
|---|---|---|---|
| `draw_line(x0,y0,x1,y1,color)` | Bresenham, per-pixel clipped | + bounding-box damage | default via `draw_pixel`; `TargetPal` → back buffer |
| `fill_triangle(a,b,c,color)` | — | scanline raster + bbox damage | default via `draw_line`; `TargetPal` → back buffer |

Endpoints are signed (`i32`) so off-screen geometry clips cleanly. All drawing
goes to the `Screen` back buffer (cached RAM); `pal.render()` presents the damaged
span **once per frame**. On the Orin the present does one `dc cvac` clean over the
damage span (the DCE scans the carveout from DRAM and does not snoop) — so large
per-frame damage is fine, but there must be exactly one present per frame.

## 2. Fixed-point 3D (in `vug.rs`)

- `Fx` (Q16.16), `fmul`, `fsin`/`fcos` (brad table lookup), `isqrt` (Newton, for
  normal normalisation).
- `Vec3` + `rotate(ay, ax)` — yaw about Y then pitch about X (two axes at
  different rates read as a deliberate tumble).
- Crystal geometry: an elongated hexagonal bipyramid (a quartz point) — 14
  vertices (two apexes + two hex rings), 24 outward-wound triangles.
- **Pipeline:** rotate every vertex → perspective project to pixels (`ppu =
  focal * dist / (z + dist)`) → backface cull by **screen-space signed area**
  (this projection flips Y, so a front-facing outward-CCW triangle has negative
  area) → painter's-order depth sort (a convex solid needs only depth order) →
  per-face integer **Lambert** shade (`N·L / (|N||L|)`, ambient floor + diffuse)
  scaling a deep-amethyst base, lilac seam highlights.

## 3. Interaction

`vug` runs an animation loop **inside** the shell command, so the outer console
pump is blocked. `pal::pump_and_poll()` lets the loop keep input flowing: it polls
the USB HID controller (x86 GUI + the Orin panel deliver keyboard/mouse through
xHCI) and, on aarch64, drains the UART, then returns one queued event. **Any key
exits** and the shell command restores the console (`took_screen` honored). The
loop `yield_now`s between frames and never sleeps (the post-EL2-drop aarch64 rule:
no timer to wake a sleeper). Modes: `vug` / `vug solid` (default), `vug wire`,
`vug bebox` (tribute screen).

> **Bare-metal Pi note.** On the Pi 4 bare-metal path keyboard input is routed
> through a separate scheduled input service / channel (`GUI_CHANNEL` in
> `main.rs`), not the PAL event queue, so `pump_and_poll` does not see it there.
> `vug`'s attended targets are the x86 GUI and the Orin panel, whose input rides
> the PAL queue; wiring the Pi channel into a full-screen demo is a future
> `main.rs`-side hook (out of this arc's lane).

## 4. Instrumentation — the load meters (M3b, redesigned UI-1)

Two small corner meters on the demo (the crystal stays the star), both drawn
through the damage-tracked back buffer (one present per frame still holds):

- **RENDER meter** — the honest software "GPU monitor" (there is no GPU; we render
  in software). Per frame it clocks the render+present span (`arch::now_cycles`)
  against the whole-frame span → a **busy %** bar, and times a ~200 ms window with
  `arch::ms()` → frame time / FPS; it also shows drawn triangles and an estimated
  filled-pixel count (sum of front-face screen areas). **Seam:** a real GPU
  utilization feed would replace `now_cycles`-derived busy % and the pixel
  estimate.
- **CPU pulse row** (UI-1, Peter's sketch) — one horizontal row of per-core
  **numbered segment bars**: `CPU 1 ▮▮▮▮▯▯ 2 ▮▮▯▯▯▯ …`, for however many cores
  `sched::meter_cpu_count()` reports. Each bar is a **fixed 10 segments**
  (`PULSE_SEGS`); filled ∝ load (rounded; any nonzero load lights at least one);
  **empty segments draw dim** — an idle core reads *alive-but-empty*, never
  blank. All geometry derives from the metrics (segments are half-cell wide, one
  cell tall in the corner form).

**The honest two-source rule (M3b, kept verbatim — do not regress).** Per-core
load lives in the shared `CpuPulse` sampler (`vug.rs`), used by both the corner
row and the full-screen `pulse` view. Per ~200 ms window, per core:

- scheduler-accounted core (`Δbusy+Δidle > 0`, i.e. the core is inside
  `sched::run()`) → the scheduler's busy fraction, from the additive relaxed
  per-CPU counters (`CPU_BUSY`/`CPU_IDLE` in both `arch/*/sched.rs`, introspection
  only). This is the Orin scheduled-pump path and the x86 APs.
- unscheduled executing core (counters frozen — the x86 GUI runs the demo in the
  inline BSP loop) → that core IS the demo core; credit it with the render loop's
  own measured busy-vs-yield fraction (the same number the RENDER meter shows),
  and log it once:
  `:: VUG|PULSE: CPU meter — core N is the demo core (unscheduled render loop, load from render busy%) ::`

**Seam:** a real per-core utilization / PMU feed would replace `meter_cpu_ticks`.

## 4b. `pulse` — the full-screen system monitor (UI-1 M3)

`pulse` (shell command; `vug::run_pulse`) is the in-kernel half of the BeOS
Pulse homage — a full-screen monitor view (a host-native `vessels/pulse` vessel is
a separate, future arc). It shows the M2 pulse widget **larger** (double-size
segments, one row per core, with the load percent), plus the honest system
lines available today: core count, uptime (`arch::ms()`), live frame counter,
and frame/present time + FPS measured while the view is open.

The loop follows the vug contract exactly: it runs inside the shell command,
pumps input itself (`pal::pump_and_poll`), presents **once per frame**,
busy-polls + `yield_now`s between frames (never WFI/sleeps — the post-drop
aarch64 rule), and **any key exits** back to the console (`took_screen`
honored; the shell repaints). Per-core loads ride the same `CpuPulse` sampler —
two-source rule and all. The bare-metal Pi input caveat from §3 applies to
`pulse` too.

Serial evidence on entry/exit (headless-gate visible when scripted):

```
:: PULSE: live — N cores ::
:: PULSE: exit clean — N frames ::
```

## 4c. PULSE-2 — the always-running pulse as a bottom instrument panel

*Supersedes PULSE-STRIP, which put the pulse inside the PI-UI-2 status line.*

**Why it moved.** On the bench panel `ui::Metrics::for_height(1200)` is `scale=1`
(the scale step is 900 rows and 1200 does not reach 2x), so the status band is
12 px and PULSE-STRIP's bars came out ~30x4 — about a millimetre tall. Peter, at
the bench: *"i meant for pulse to be in the ~20mm high gap at the bottom of the
screen below the other windows not in your fake bar at the bottom like 1mm tall
and 4mm long. don't try to fake a desktop just build test tools."*

Two rules come out of that, and both are general:

1. **This panel is a test-tool surface, not a desktop imitation.** There is no
   taskbar to dock into and no chrome to imitate. An instrument gets the room it
   needs to be read at arm's length; sizing one to fit inside existing chrome is
   how it became unreadable.
2. **The bottom gap is real estate.** `wm::place` packs window boxes from the top;
   below the lowest row there was a permanently unused strip above the status
   line. That is where the pulse belongs.

**Geometry** — a panel *fraction*, not a cell multiple. THE METRICS RULE still
holds (nothing here is an absolute pixel), but an instrument that must read across
a room is a function of the **panel**, not of the type size — which is exactly
what `cell_h`-derived sizing got wrong.

| quantity | derivation |
| --- | --- |
| band height | `clamp(ph/13, cell_h·8, ph/4)` — 92 px at 1200 rows, 64 px at 480 |
| band box | `(x0, ph − line_h − band_h, x1 − x0, band_h)`, `[x0,x1)` from `free_span` |
| row | `row_h = (band_h − 2·pad) / ncpu`, `pad = max(line_h/2, 2)` |
| LED | pitch `max(row_h/2, 3)`, `gap = max(pitch/4, 1)`, `led_w = pitch − gap`, `led_h = row_h − max(row_h/4,1)` |
| bar | every pixel between the `c<N>` label (`3·cell_w`) and the percent (`7·cell_w`) |

Measured on the gate's 1920x1200 surface: `panel=(280,1096,1480x92)`, `row_h=20`,
`bar=(x=310,w=1380)`, `leds=138`, `led=8x15`, `reserved=104` — 4 cores.

**Reserving the band honestly.** An instrument the tiler can park a window on top
of is not an instrument. `ui_status::chrome_h(ph)` (pulse band + status line) is
the reservation, and `wm::place` lays out against `ph − chrome_h` instead of `ph`:
the scale rule reads the reduced height, and a row that would still overflow is
clamped up so its box ends at the reservation. This is a **tiler bottom margin**,
not a `wcf::reserved`-style box list, because the two answer different questions —
WC-F's list lets the compositor *refuse to paint a probe* over a window already
there, while this must stop the window arriving. It also does not fight
`wm::occluders`/WC-I: occlusion decides who wins where regions overlap, and after
the reservation they do not. (Occlusion is still inherited for free — the band
draws into the `Screen` back buffer and `present_background` subtracts
`occluders()` from every damaged row — so an explicitly `move_to`-pinned window is
still handled.) `legibility_cap` still reads the full `ph`: the cap is a function
of panel *density*, not of layout area.

**The one thing the band does not get: the corners.** WC-F's own reserved boxes
live in these rows (a 264x256 ramp against the bottom-left corner, 144x64 twins
against the bottom-right) and it paints them straight to the framebuffer at the
tail of every composite. Neither can move — they are photographed by the bench
operator and their geometry is a witnessed constant. So `free_span` narrows the
instrument to the horizontal span WC-F leaves free rather than fighting it: 1480
of 1920 px on the bench panel, 200 of 640 on the QEMU surface. Outside the WC-F
build (x86, non-`baremetal`, no `witness`) the band is the full panel width.
**SETTLED (Peter, 2026-07-26, P62 sitting): the bottom corners stay with the WC-F
probes** — the 1480 px centered span is the decided design, not a compromise
awaiting a WC-F retirement. Do not re-litigate in later arcs.

### The LED bar — sensitivity from width, smoothness from gradient

Peter again, once the band existed: *"if pulse spans the entire bottom width of the
screen there will be more leds to show sensitivity. with the better graphics can
you have a gradient inside each led so it scales super smooth."*

* **Sensitivity is LED count, and LED count is width.** The cores stack as
  full-width rows rather than side-by-side quarters for exactly this reason:
  quarters would give each core ~370 px and ~37 LEDs, stacked rows give each the
  whole 1380 px and 138. Four times the resolution, at the cost of row height
  (20 px rather than 92) — which is still five times what PULSE-STRIP drew and
  comfortably taller than the 8 px label beside it. Sensitivity wins.
* **Smoothness is the fill LENGTH, and the gradient is what makes a length out of
  lamps.** `draw_led_bar` computes the load's fill as a continuous pixel length;
  every LED fully inside it burns at full intensity, every LED outside draws the
  dark track, and the one LED the boundary lands inside is lit *in proportion to
  its coverage*. A rising load brightens the next lamp continuously and then hands
  over to the one after it. Each LED also carries its own vertical lens gradient
  (`LED_BANDS = 8` bands, triangular profile floored at 60%), so a lit lamp reads
  as a lamp rather than as a flat rectangle.
* **Per-mille, not percent.** At 1380 px of bar a 1% quantum is a 14 px jump — the
  display would step where the machine is smooth. `vug::classify_load_scaled` is
  the VUG-HONESTY rule stated once at an arbitrary full scale; `classify_load` is
  that function at `full = 100`, so the existing honesty witness still covers every
  branch, and the instrument calls it at `full = 1000`.
* **Scale colour** runs green → amber → red across the bar, by **position** rather
  than by load, so a given lamp's colour is stable and only the length moves. A VU
  ramp reads before any digit does, which is the job of a bench instrument.
* `PARKED` and idle keep their meanings verbatim: a parked core draws a cool
  **dashed** track (grouped dashes, since 1-on-1-off at 138 LEDs is a grey haze),
  never confusable with a 0% bar; an idle-but-scheduled core **breathes** a
  sweeping block (PULSE-ALIVE).

**The status line goes back to text only** — host / ip / clock, as it was before
PULSE-STRIP. The miniature bars are superseded. `click1_hit_test` is unchanged:
the strip's band is still `height − line_h`, and the instrument is not a view, so
it takes neither TAB nor a click.

**Data path (superseded by PULSE-3 below — kept for the record).** Unchanged from
PULSE-STRIP: the same per-core busy/idle counters
`pulse` and `top` read (`sched::meter_cpu_count` / `meter_cpu_ticks` /
`meter_current_cpu`), lock-free relaxed loads, never on a scheduler path.
`own_load` is passed as `0` — the panel is drawn by a *scheduled* task, so its
core's counters tick like any other's and the demo-core fallback cannot fire;
feeding it a render load would fabricate exactly what VUG-HONESTY closed.

**Dirty pacing.** Two entry points, and the split *is* the pacing:

* `draw()` — unconditional, from cached loads. Taken on Key/Button, where the
  console has just repainted over both bands and they owe a redraw on top
  regardless. No resample, so a keystroke storm cannot become a telemetry storm.
* `tick()` — the paced path, taken on the 1 Hz `Event::Timer`. Samples at most once
  per `PSTRIP_PERIOD_MS`, then redraws **only if** the composed text line changed or
  a core's **lit length moved by ≥ 1 px**, and returns whether it drew. The render
  loop presents only on `true`.

The threshold moved from "quantized to a bar segment" to "one pixel of lit length"
because that is now the finest difference the display can show — finer would be
invisible, coarser would step under a gradient built to be smooth. `lit_px` is the
single source of truth for both the draw and the dirty test, so "the picture
changed" and "we redrew" cannot disagree. Idle cost is unchanged at ~0 redraws/s
(an idle core's per-mille load is a hard 0 and its length does not move); the
ceiling is one redraw per sample, i.e. 1 Hz. PULSE-ALIVE's breath is deliberately
*not* a dirty source — it is wall-clock animation on a core reading a hard 0, and
making it dirty would redraw every sample and turn `skipped=` into a constant zero.
It advances on the frames the panel draws for other reasons.

**The 1 Hz pulse itself** is `status_tick` on metal. That task is timer-gated and
is *not* spawned under QEMU raspi4b (no Group-1 IRQ), which before PULSE-STRIP
meant the strip only ever refreshed on a keystroke there. The input service's QEMU
poll-nap branch carries the same 1 Hz post itself (a wall-clock compare per
cooperative pass, gated on `SCREEN_APP_ACTIVE` exactly as `status_tick` is), so the
cadence is real on both paths and still costs no task.

**Serial evidence** (`[pstrip]`), and the gate directives in
`scripts/specs/pi4-regression.spec`:

```
[pstrip] armed cores=4 panel=(280,1096,1480x92) row_h=20 bar=(x=310,w=1380) leds=138 led=8x15 gap=2 bands=8 full=1000 strip_h=12 reserved=104 period=1000ms
[pstrip] rollup samples=10 redraws=1 skipped=9 rate=0.1/s period=1000ms
```

The `armed` line is the creation geometry — the only checkable statement about how
a panel nobody can see headless actually *looks*, and the place a hard-coded pixel
would show up as a constant that ignores `UNAOS_FBW`/`UNAOS_FBH`. `leds=` is the
sensitivity number and the spec FORBIDs a single-digit count; `full=1000` is
REQUIREd so a revert to percent is caught; `reserved=` is what the tiler subtracts.
The rollup is the pacing assertion: `samples` counts meter reads, `redraws` counts
frames actually drawn and presented, and the spec REQUIREs a rollup with a non-zero
`skipped=`. The rate is printed in **tenths** so the FORBID can bite: `rate=` at or
above the bound sustained (5.0/s as written here; PULSE-4 raised it to 6.0/s when
the cadence moved — see that section) is the panel having become a spinner on the render core,
i.e. the SCHED-6 regression re-entered through the pulse. Visual verdict is the
bench's.

### PULSE-3 — the strip was reading the wrong load source

Peter's attended verdict at P64, with the finished gradient instrument on the
bench panel: *"gradient good but pulse not real-time"*. The gradient itself is
settled — colours, geometry and the WC-F corner yield are not in question here.

**What the capture said.** In `pi4-r23s1o`, three vugs held the cores at a
sustained 99% and the vugband workers churned ~1M context switches per window.
The strip printed, for ten seconds running:

```
[pstrip] rollup samples=10 redraws=0 skipped=10 rate=0.0/s period=1000ms
[sched6] passes=1/s composites=0/s (dirty-paced strip@1Hz)
```

Ten consecutive windows in which the meter concluded that nothing on the panel had
moved, against a machine that was visibly moving. The same capture's `[spinhunt]`
fixture reported `load settled c2=53` while SCHED's own load line reported
`c2=99%` in the same window — two numbers for one core, which is the tell.

**Root cause: the feed, not the pacing.** The dirty test was correct and the 1 Hz
pace was correct. PULSE-STRIP inherited `vug`'s VUG-1 M3b feed — `meter_cpu_ticks`,
i.e. the cumulative `CPU_BUSY`/`CPU_IDLE` counters `dispatch_next` bumps once per
dispatch **pass** — and PULSE-2 carried it forward verbatim. Those are pass counts,
and the scheduler had already retired that metric two arcs earlier; SCHED-5's
standing note in `arch/aarch64/sched.rs` says so in as many words: *"TIME, NOT
PASSES … it counts scheduler activity, not CPU time"*. A core running CPU-bound
tasks back to back dispatches at a near-constant rate and never reaches the
empty-queue branch, so `db/(db+di)` pins at full scale and stays flat while the
utilization underneath it wanders. A flat source through a correct dirty test is a
frozen bar — exactly what Peter watched. It is also why `[spinhunt]` and SCHED
could not be reconciled: the strip and the console were on different feeds.

**The fix.** `live_permille(cpu)` makes `sched::core_load(cpu).busy_pct_recent` —
the SCHED-5/SCHED-7 rolling ~250 ms CNTPCT busy-**time** fraction, the number `top`
and the `SCHED: load` heartbeat already print — the strip's primary source. One
feed, so the instrument and the console can no longer disagree about one core.
`meter_cpu_ticks` stays as the fallback and keeps its full meaning: SCHED-8 reports
`tracked=false` for a core not currently inside `run()`, and such a core is
classified through `vug::classify_load_scaled` exactly as before, so a frozen
non-demo core still reads `PARKED` rather than a fabricated bar and
`parked_display_witness` still covers that branch. The tick deltas are still
consumed every window whether or not they are used, or a core that later falls back
would classify against a window minutes wide. On x86 there is no `core_load` and
the source is unchanged in every particular.

**Pacing is untouched.** 1 Hz stays; skipping a window whose values moved was the
defect, and that was never the pace gate. An idle desktop still reads a hard 0 on
every core, still moves no lit length, and still redraws nothing — the
default-quiet contract and the spec's non-zero `skipped=` both hold.

**Witnesses.** The rollup gains `srcdelta=`: windows in which the *source* moved,
printed beside the windows actually drawn. A busy window reading `srcdelta=0` is a
stale feed; a large `srcdelta` with `redraws=0` is the dirty test swallowing real
movement. Neither was legible in the P64 capture. `[pstrip] src`, emitted once at
arm time, closes the headless half:

```
[pstrip] src live=4/4 quantum=10 stepres=13px mono=yes PASS
[pstrip] rollup samples=10 redraws=3 skipped=7 srcdelta=3 rate=0.3/s period=1000ms
```

`live=k/n` counts cores returning a live number; `live=0/n` is the PULSE-3
regression exactly — every core back on the dispatch-pass fallback — and is
FORBIDden. `k == n` is deliberately *not* required: a core legitimately outside
`run()` is honestly untracked. `stepres=` is what one source quantum (the feed is a
percent, so 10‰) moves the lit length on this panel's bar; `stepres=0px` would mean
a real-time source feeding a meter too coarse to render its steps — the same frozen
bars for a different reason — and is FORBIDden too, as is a non-monotonic fill.

**A red must name the right subsystem**, which is why two of the witness's verdicts
are skips rather than failures:

* `stepres` is `bar_w / 100`, so on any panel whose bar is under 100 px it is zero
  for a *geometry* reason — a shrunken `UNAOS_FBW`, a WC-F reservation that grew, a
  layout regression. A blanket `stepres=0px` FORBID would report every one of those
  as a source regression, backwards. Below the bound the witness refuses to state a
  pixel resolution it cannot attribute: it prints `stepres=coarse` and verdicts
  `SKIP-GEOM`, and the `armed` line's own geometry FORBIDs go red instead. So
  `stepres=0px` is only ever printed by a panel wide enough to have resolved the
  step, where it means exactly what the FORBID says it means.
* **x86 is untouched by PULSE-3** — there is no `core_load` and therefore no live
  feed to be on or off — so `live=0/n FAIL` would be a standing untruth in every
  x86 log. The witness is cfg-gated to report `live=n/a … SKIP-ARCH` there. The
  pi4 spec FORBIDs `SKIP-ARCH`, since an aarch64 boot printing it would mean the
  cfg gate had inverted.

The witness is re-emitted **once** at the first rollup. Its arm-time fire is taken
the instant the panel exists, before every core has necessarily entered `run()`, so
a transient `live=0` is possible there and would otherwise stand as the log's only
word on the feed; ten seconds of settled boot later the answer is not transient.
Once only — insurance against an early fire, not a second periodic witness.

### PULSE-4 — the latency budget: a correct meter that still read dead

PULSE-3's fix works and the attended P65v2 capture proves it *mechanically*:
`[pstrip] src live=4/4 stepres=13px PASS`, rollups at `redraws=6-8/10 srcdelta=6-8
rate=0.5-0.7/s`. Peter, watching that same panel while vugs churned: **"well off
live tracking"**. Both statements are true, because *"the strip redraws"* and
*"the strip tracks"* are different claims. PULSE-4 is the second one.

**The budget, term by term.** What stood between a load changing and a pixel moving:

| term | before | worst case |
|---|---|---|
| source window (`busy_pct_recent`, rolling) | ~250 ms | 250 ms |
| wake + sample cadence (`PSTRIP_PERIOD_MS`) | 1000 ms | 1000 ms |
| dirty threshold (1 px of lit length) | 13 px per source quantum | 0 ms |
| display filter | none | 0 ms |
| **step → pixels** | | **~1.25 s** |

**The cadence was the whole of it.** A second is already past the ~250–300 ms at
which a human stops reading a meter as attached to the machine — but the sharper
failure is one an average latency hides: **a burst shorter than the sample period
could begin and end entirely between two samples and leave no mark on the panel at
all.** Sub-second vug churn was not being drawn late; it was not being drawn. Six to
eight redraws a window, every one of them of a load the operator had already stopped
seeing. That is the gap between the log and the verdict.

The other two terms were *not* the defect and are deliberately untouched:

* **The dirty threshold is already at the panel's floor** — one pixel of lit length,
  against 13 px of movement per source quantum. It cannot be why anything went
  unseen. Sub-1% wander *is* invisible, but that is the SOURCE's quantum
  (`busy_pct_recent` is a percent), not the threshold's.
* **The ~250 ms source window is scheduler-lane and is left alone.** It now sets the
  floor of this budget and is the next thing worth questioning if 250 ms cadence
  still does not satisfy the eye — but narrowing it changes what `top` and
  `SCHED: load` report, which is a scheduler decision, not a display one.
  **Flagged, not touched.**

**The fix is two changes, and they are a pair.**

1. **`PSTRIP_PERIOD_MS` 1000 → 250.** One sample per source window — the fastest
   cadence at which consecutive samples carry independent evidence; faster would
   re-read overlapping evidence and put motion in the log with no new fact behind
   it. Worst-case step→pixels falls from ~1.25 s to ~500 ms and a 250 ms burst can no
   longer fall between samples. PULSE-2's stated reason for the second ("sampling
   faster would only add reads the 1 Hz redraw could never show") was circular: the
   redraw could not show them because the sample *was* the redraw's cadence.
2. **Instant attack, ~1 s decay** (`attack_decay`). Cadence alone makes a burst
   *drawn*; the envelope makes it *seen* — a 250 ms spike at 4 Hz is one frame,
   which is technically drawn and humanly invisible. Rises are taken outright, with
   no filter of any kind, so nothing is traded for the calm; falls proceed at a
   bounded **rate** — full scale per decay constant — rather than as a geometric
   approach to the target. That distinction is not cosmetic: a geometric filter
   asymptotes, and the last few per-mille it creeps through are exactly the values
   whose lit length still moves a pixel, so one busy second would buy five or six
   seconds of decay repaints. A rate converges exactly, within one decay constant,
   and then the panel is genuinely still. A fall smaller than one step passes through
   untouched, so ordinary downward tracking is not slowed — only a fall big enough to
   be a burst ending gets the dwell. It stays inside VUG-HONESTY because the displayed
   value is always between two numbers the source actually reported — it can lag a
   fall, never lead a rise, and never exceed the highest reading the feed has given.

**The metal wake is the outer term**, and it was the trap: `status_tick`'s nap was a
hard-coded `sleep_ticks(250)` — a literal second — beside a 1000 ms period constant.
Raising `PSTRIP_PERIOD_MS` alone would have left metal sampling at 1 Hz regardless,
i.e. PULSE-4 doing nothing on the only machine whose panel anyone watches. The nap
now derives from `ui_status::PSTRIP_PERIOD_TICKS`, so the wake and the sample are the
same number by construction. (The QEMU poll-nap branch already read the constant.)

**Dirty pacing is unchanged and the default-quiet law stands.** Cadence raises the
ceiling on how fast the strip *can* respond; it repaints nothing that has not moved.
An idle core still reads a hard 0, its lit length still does not move, and the idle
panel's redraw rate is still set by the status text's seconds field at ~1/s — the
same as at 1 Hz, four times the samples notwithstanding. The decay converges to an
exact 0 (a 1‰-per-sample floor under the geometric step) rather than asymptoting, so
a machine that goes quiet stops redrawing instead of creeping forever. The spec's
non-zero `skipped=` REQUIRE still holds.

**Two margins moved with the cadence, and both are stated here rather than
re-derived later.**

* **The busy-loop FORBID went 5.0 → 6.0/s.** Two independent sources feed `rate=`:
  sample-paced load redraws, capped by `PSTRIP_PERIOD_MS` at 4/s, and the status
  *text* redraw, which is outside the period gate and fires on the composed line's
  seconds field at ~1/s. A busy window can therefore reach exactly 5.0/s
  legitimately — the old bound had zero headroom over the new legal ceiling and
  would have gone red on a correctly-paced strip. The bound is raised rather than
  the text redraws netted out of `rate=`: a spinner repainting via the text path is
  just as much the regression this catches, and dropping a term from a number is how
  a witness stops measuring the thing it is named after.
* **GUI-CLICK-2's saturation deadline went ~64 s → ~16 s.** `status_tick`'s Timer
  post is gated on `SCREEN_APP_ACTIVE` because `render_service` is blocked inside
  `dispatch_command` while a full-screen app owns the screen and cannot drain
  `GUI_CHANNEL`; at 4 Hz an ungated pulse would fill the 64-slot channel four times
  sooner. The gate is unchanged and still closes it, and the same 4x applies to the
  `gui_watchdog::poll()` that rides this task — the escape hatch returning input to
  the shell from a wedged app now fires four times as often, which is the direction
  that helps. Named so GUI-CLICK-2's sizing is not re-derived blind.

**Witnesses** — the budget as a reading, not as a bench observation:

```
[pstrip] armed cores=4 panel=(280,1096,1480x92) ... period=250ms attack=instant decay=1000ms
[pstrip] rollup samples=40 redraws=10 skipped=30 srcdelta=12 rate=1.0/s srate=4.0/s gapmax=252ms lat_max_ms=254 period=250ms decay=1000ms
```

* `srate=` is the **achieved** sample rate and `gapmax=` its worst excursion. The
  strip samples on a wake it does not own, so `PSTRIP_PERIOD_MS` is a floor and these
  are what the machine really delivered — a wake that quietly stayed at 1 Hz reads
  `srate=1.0/s` however this module is configured, which is precisely the failure
  that would otherwise present as "PULSE-4 did nothing" on the bench and nowhere in
  a log.
* `lat_max_ms=` is the worst **source-moved → pixels-on-panel** latency in the
  window, measured in-kernel. The stopwatch opens at `sample_time - gap` (the
  earliest instant the detected change could have occurred, so the number is an upper
  bound rather than a flattering one) and closes where `changed` is decided, which is
  pixels-on-panel to within one band blit. An already-open measurement is never
  restarted: a run of sub-pixel moves that only becomes visible on the fifth sample
  is a five-sample latency, and reporting it as one would hide exactly the "the value
  wandered for a second before the bar admitted it" case. `lat_max_ms=0` means no
  source movement was ever left waiting for a paint.
* `srcdelta=` keeps its PULSE-3 meaning exactly — it is measured against the previous
  **raw** reading, upstream of the envelope. Measuring it on the display would keep
  it non-zero for a second after the machine went quiet and turn the stale-source
  alarm into a tautology.

**The visual verdict remains the bench's.** QEMU can confirm the cadence, the pacing
and the latency numbers; whether ~500 ms worst-case with a 1 s decay tail actually
reads as *live* to someone watching vugs churn is a question only the panel in front
of Peter can answer. If it still does not, the budget above says where the remaining
time is: the ~250 ms source window, in the scheduler's lane.

## 5. Serial evidence

`run_crystal` emits, when invoked:

```
:: VUG: crystal live — 24 faces, solid/wire, exit clean ::
:: VUG: crystal exit clean — N frames ::
```

Headless regression gates never type `vug`, so these are GUI/panel-verified when
the demo is run (attended); the demo does not perturb headless boots.

## 6. VPERF — the fbcon scroll path (x86)

### Root cause (stride-correct)

On the mid-2012 rMBP (MacBookPro10,1) the EFI GOP surface is 2880×1800,
**stride 4096 px**, 4 bpp, Bgr, `framebuffer_size` 29,491,200 B, scanned out by
the GT 650M — every CPU **read** of it is an uncached PCIe round trip.
`FrameBuffer::scroll_up` was a full-surface `core::ptr::copy` whose *source* is
that framebuffer: one 8-px text scroll moved `1800×(4096×4) − 8×16,384` ≈
**28.0 MiB read + 28.0 MiB written**; a full screenful (225 rows) ≈ **6.15 GiB
of uncached reads**. On top of that, glyphs and fills poked VRAM per pixel.
QEMU numbers for calibration (1280×800, stride 1280): one scroll moves
(800−8)×1280×4 = 4,055,040 B; the 400-line scripted scenario = 301 scrolls =
1,220,567,040 B, pre-fix all of it VRAM-read.

### The fix (M3: cached-RAM shadow, x86-only)

`fbcon` late-attaches a heap shadow (`attach_shadow()`, called at the post-heap
usbdebug seam right before the usbdebug `clear()` — fbcon itself initialises
pre-heap by design, so init-time allocation is impossible). Once attached:
glyphs and scrolls go to the shadow (cached RAM); `flush_dirty` presents the
dirty row band to VRAM as **one contiguous sequential write-only blit** (on a
scroll: the whole viewport — every visible row changes on a scroll, so the win
is *read elimination*, not damage shrink). The shadow is **never seeded from
VRAM**; it starts blank with the cursor homed. GUI builds never attach (fbcon
detaches; `Screen`'s ~28 MiB back buffer owns the 48 MiB heap budget — a second
shadow would OOM on metal). `panic_screen()` force-detaches the shadow without
freeing (`mem::forget` — no allocator use from a panic context) so the red
backdrop and panic text always paint direct-to-VRAM. Design choice: a minimal
`Vec<u8>` + second `FrameBuffer` handle (the `Screen` back-store invariant),
*not* `Option<Screen>` — fbcon already has row-band damage tracking, and
`screen.rs` is another lane's file this round. M4 adds `encode4`/`put_raw4`
(pixel encode hoisted out of glyph/fill inner loops) and a word-wide
`fill_rows` band fill; with the shadow attached all stores are full 4-byte
pixels and VRAM receives only whole blitted rows.

### Instrumentation (`videobench` / `videocap` knobs, x86-only, default OFF)

`UNAOS_VIDEOBENCH=1` → feature `videobench` → `video/vperf.rs`: relaxed-atomic
counters (scroll bytes memmoved / VRAM-read bytes / put_pixel calls), emitted
**raw-serial-only** (a line mirrored onto fbcon would recurse through the
scroll path and contaminate screendump compares):

```
:: vperf: scroll=<B> vread=<B> px=<N> ::                        (cadence)
:: vperf: fbmem mtrr=<T>(<how>) pte=<raw>(l<N>) pat=<T> eff=<T> fb=<addr> ::
:: vperf: display <vid>:<did> bar<N> owns fb (base=<addr>) ::
:: vperf: scenario scroll=<B> vread=<B> px=<N> scrolls=<N> lines=400 ::
```

The `fbmem` line is the **effective framebuffer memory type**: MTRR coverage
(RDMSR MTRRCAP/DEF_TYPE + variable ranges, CPUID-guarded), the live
identity-map leaf PTE (PWT/PCD/PAT bits visible raw), the IA32_PAT entry those
bits select, and the SDM 11-7 combination. The kernel programs **zero**
PAT/MTRR state — the WC assumption inherited from firmware
(`arch/x86_64/mod.rs`) is unverified, and **QEMU's picture is synthetic** (TCG:
`mtrr=UC(var-range) pte=0x800000e3(l2) pat=WB eff=UC`); only the attended metal
readout is data. The display probe is read-only (no BAR sizing writes on a
live-scanned-out panel; ownership = highest display mem-BAR base at/below the
fb address within 512 MiB). `UNAOS_VIDEOCAP=1` (implies videobench) halves the
fbcon-private handle's `info.height` — the bench lever that proves scroll cost
scales with viewport height (capping the `rows` field alone is a no-op:
`scroll_up` sizes from `info.height`). Full-surface paints (init fill, clear,
panic backdrop) still cover the whole panel via an uncapped twin handle.

The scripted scenario (usbdebug loop, one-shot ≥15 s so the boot fixtures are
quiet): clear + 400 fixed-width numbered lines + counter deltas. Deterministic
final screen → QMP screendump compares (`UNAOS_QMP_SHOT=<png> ./arroyo test 40`,
port auto-bumps). Measured: pre-M3 `vread=1220567040`, post-M3 **`vread=0`**,
identical workload, screendumps pixel-identical pre/post M3 and M4.

### Ledger: the scroll runs IF-masked under the console lock

The whole fbcon mirror — scroll included — runs interrupts-masked inside
`sys_write`'s serial path under the FBCON `try_lock` (`arch::without_interrupts`
in `serial::_print`/`fbcon::_print`). Post-M3 that window is still
~10 ms-class per scrolled line on metal (a full-viewport write-only blit), and
it **delays irqstorage IRQs** (and everything else) for its duration. Known,
accepted for now; redesigning console locking is explicitly out of the VPERF
arc's scope.

### Metal-confirmed result (round-6 attended rMBP bench, 2026-07-11) ✅

Verbatim readout on the real 2012 rMBP (over FTDI serial):

```
:: fbcon: cached-RAM shadow attached — VRAM is now write-only ::
:: vperf: fbmem mtrr=UC(var-range) pte=0x900000e3(l2) pat=WB eff=UC fb=0x90020000 ::
:: vperf: display 8086:0166 class 030000 @ 0:2.0 ::            (Intel HD 4000)
:: vperf: display 10de:0fd5 class 030000 @ 1:0.0 ::            (NVIDIA GT 650M)
:: vperf: display 10de:0fd5 bar1 owns fb (base=0x90000000) ::  (GT 650M scans out — gmux default)
:: vperf: scenario scroll=… vread=0 px=… scrolls=… lines=400 ::
```

- **The M3 cached-RAM shadow is METAL-CONFIRMED**: `fbcon: cached-RAM shadow attached` fired and the
  scroll scenario measured **`vread=0`** over 400 scrolled lines — zero uncached-VRAM reads on real
  hardware, matching the QEMU 1.22 GB→0. The shadow works on silicon.
- **`eff=UC` → VPERF-WC is a GO** (no longer bench-gated): the framebuffer is UNCACHED (var-range MTRR
  UC, PAT=WB unused in the l2 PTE), NOT write-combining, so the ~10× WC follow-up is real. The metal
  fact it needed is now in hand (UC; fb in the GT 650M's `bar1` @ `0x90000000`), so **VPERF-WC becomes a
  normal codable arc verified at the next bench**, not a bench-first one.
- GPU topology exactly as the interface model predicted: Intel HD 4000 (`8086:0166`) + NVIDIA GT 650M
  (`10de:0fd5`), the GT 650M owning the framebuffer via the gmux default.

### VPERF-WC — write-combining the framebuffer mapping (x86-only, landed post-round-6)

The M3 shadow removed the *reads* (`vread=0`); VPERF-WC accelerates the *writes*. The round-6
metal readout showed the framebuffer still **effective-UC** (var-range MTRR UC, PAT=WB in the l2
PTE), so the shadow's write-only sequential blits were posted **uncombined** — each store its own
uncached PCIe transaction. Marking the framebuffer **Write-Combining** lets the CPU's WC buffers
coalesce those sequential stores into burst transactions (~10× on the write path is the metal
expectation the next attended bench measures).

**Mechanism** (`arch/x86_64/memory.rs`, called from `fbcon::init`):

- `ensure_pat_wc()` programs one **unused IA32_PAT slot** — PA4 — to WC (encoding `0x01`). Power-on
  PAT is `[WB,WT,UC-,UC]` in entries 0–3 and *duplicates* them in 4–7; no firmware/kernel mapping
  ever sets a PTE's PAT bit, so entries 4–7 are dead. Writing only PA4 leaves 0–3 — every live
  mapping — byte-identical. Programmed on the **BSP**; idempotent.
- `set_framebuffer_wc(base, len)` retypes every identity-map **leaf** covering the fb range to
  select PA4 — set the PAT bit (bit 7 in a 4 KiB PTE, **bit 12** in a 2 MiB/1 GiB leaf), clear
  PCD/PWT — under `with_page_tables_writable` (the WP-clear page-table-edit sequence), then
  `invlpg`s the span. Runs **once**. `wbinvd` is **not** issued: the fb was effective-UC, so no
  cache line holds fb data to write back.

**Scope (seat-signed).** This is a **memory-TYPE change only**, on the **fb leaves only**. It writes
no page-permission bit (PRESENT/WRITABLE/USER/NX), no MTRR, and no other mapping — **SMEP/NXE/W^X
are untouched** (they live in CR4/EFER and the U/W/NX PTE bits, none of which this writes). The
firmware maps the fb with 2 MiB huge leaves inside the GPU's own BAR aperture (device MMIO, not
RAM/heap/kernel), so the retyped span *is* the framebuffer's own mapping.

**All x86 builds** carry the retype (GUI blits benefit too), un-gated by `videobench` — it rides
`fbcon::init`, where the fb range is known. aarch64 is **byte-identical** (every symbol is
`cfg(target_arch = "x86_64")`; flat-image equivalence holds under the ratified standard —
default-features virt + `-Ccodegen-units=1` tegra flat images hash-identical base↔tip).

**APs.** An AP that also blits keeps its default PA4=WB, which under the UC MTRR is effective-UC —
so an AP blit is plain UC (no speedup). This is **correct, not merely tolerated**: WC and UC are
both uncacheable (no cache line holds fb data), so the mix is *not* the SDM 11.12.4 WB-aliasing
hazard — it is exactly the write-only-framebuffer pattern, just un-accelerated on the AP. Wiring
`ensure_pat_wc()` into `smp::ap_entry` for uniform WC is a one-line follow-up, left out to keep the
arc in-lane.

**WC drain — SFENCE at every flush seam (F1, seat-review must-fix).** WC stores buffer in the CPU's
write-combining buffers and drain only on a store fence / serializing event / buffer pressure
(SDM Vol. 3A §11.3.1). A streaming console self-evicts under pressure, but the LAST partial WC
buffer before the CPU idles has no eviction trigger — the tail of the panic screen ahead of
`hlt_loop()` is the concrete failure surface, and QEMU/TCG cannot witness it (WC buffering is not
modeled). Fix: the x86 `arch::flush_framebuffer_range` (previously a no-op) now emits **SFENCE**,
so every existing flush seam is a real drain point: fbcon's per-print `flush_dirty` (after the
shadow blit), `panic_screen()`'s `flush_all` plus each subsequent panic-text print, and `Screen`'s
per-frame present. Range-independent (SFENCE drains all WC buffers); negligible next to the blit.
aarch64's flush (`DC CVAC` + `DSB`) is untouched.

**Witness.** The `videobench` `fbmem` readout is the on-target witness (QEMU + metal): the retype
flips it from `pte=…00e3 pat=WB eff=UC` to `pte=…10e3 pat=WC eff=WC`, and a boot line
`:: x86 fb-wc: retyped N leaf(s) WC (PAT PA4) over <base>..<end> ::` fires in every x86 build.
QEMU (synthetic bochs fb) already reflects `eff=WC` after the retype — it proves the **code path**;
only the attended rMBP bench proves the **speed**. QEMU screendumps are **pixel-identical** pre/post
(same SHA-256): WC changes write buffering, not final memory content.

### VPERF-WC — metal-confirmed result (round-9 attended rMBP bench, 2026-07-12) ✅

Verbatim on the real 2012 rMBP (over FTDI serial; knob-ON usbdebug+videobench media, kernel.elf
sha `3083b467`; TWO clean boots):

```
:: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) over 0x90020000..0x91c40000 ::
:: vperf: fbmem mtrr=UC(var-range) pte=0x900010e3(l2) pat=WC eff=WC fb=0x90020000 ::
:: vperf: display 8086:0166 class 030000 @ 0:2.0 ::            (Intel HD 4000)
:: vperf: display 10de:0fd5 class 030000 @ 1:0.0 ::            (NVIDIA GT 650M)
:: vperf: display 10de:0fd5 bar1 owns fb (base=0x90000000) ::
:: vperf: scenario scroll=5167382528 vread=0 px=486060 scrolls=176 lines=400 ::
```

- **The retype LANDED on silicon.** The fbmem readout FLIPPED from round-6's `pte=…00e3 pat=WB eff=UC`
  to **`pte=0x900010e3(l2) pat=WC eff=WC`** — the fb mapping is now effective Write-Combining on the
  GT 650M's `bar1` @ `0x90000000` (15 huge leaves retyped). The `vread=0` shadow win still holds.
- **~10× scroll — subjective PASS (attended eyeball).** The 400-line scripted scroll rendered
  **dramatically faster** than round-6's eff=UC boot on the same panel — the WC write-coalescing win
  is visible, matching the ~10× expectation.
- **GUI blit win — 53.8 fps (attended, Boot 2 GUI).** The vug/quartz scale-2 Retina render clocked
  **53.8 fps**, up from round-6's ~7.6 fps (~7×) — WC accelerates the GUI present path too, exactly as
  "all x86 builds carry the retype" predicts. `:: x86 fb-wc: retyped 15 leaf(s) … ::` fired on the
  GUI build too.
- **WC panic-tail (F1 SFENCE drain) — metal PASS (attended eyeball, Boot 2 GUI).** A deliberate
  `panic` at the GUI console rendered the FULL red panic screen (`=== KERNEL PANIC === / Manual Panic
  Requested by Architect!` + location) with **NO truncated tail** — the last partial WC buffer drained
  via `panic_screen()`'s `flush_all`→SFENCE before `hlt_loop()`, confirming the F1 fix on real WC
  hardware (the one thing QEMU/TCG cannot witness). The FTDI cable shows nothing after the trigger —
  expected: the panic halts the main loop that pumps `service_ftdi`, so the tail is screen-only.

**Spec:** `round6-rmbp.spec` + `x86-fat.spec` carry the `eff=WC` / `fb-wc: retyped` PENDING lines —
seat promotes to REQUIRE at this first metal capture.

**VPERF-OBS — the raw_print console-path lesson (fix `bfa1711`, 2026-07-11).** On the FIRST bench boot
NONE of the `:: vperf: …` readout lines printed on metal, though QEMU had shown them all. Root cause:
`vperf::raw_print` wrote ONLY the SERIAL1 (16550) UART leg, which is `None` on the rMBP (no 16550 — the
FTDI console exists precisely because of that), so every readout line was computed and silently dropped;
the surviving lines used `serial_println!`, which hits the FTDI boot-capture ring leg. The fix has
`raw_print` also feed `ftdi::mirror` (the ring leg), and NEVER the fbcon or selftest mirrors — both
raw_print constraints preserved (try_lock-only, never the FBCON lock; QEMU logs byte-unchanged, since
the UART leg already reaches `serial.log` and the ring is not replayed there). **The lesson: QEMU-visible
≠ metal-visible — a diagnostic must assert the console PATH it will actually reach on the target, not
merely that its code path ran.**

## 7. Headless render-path witnesses (VWIT)

The two drawing primitives (§1) reach the framebuffer by two different paths, and each has its own
`tste` regression check (`unaos/crates/kernel/src/selftest.rs`; arch-neutral, so both fire on the x86
and aarch64 headless boots when `tste` is typed):

| tste row | path exercised | where |
|---|---|---|
| `video.geometry` | the **trait-default** `draw_line`/`fill_triangle` (Bresenham / scanline) via `GneissPal::draw_pixel` | `OffscreenPal` (a raw heap pixel buffer) |
| `video.present` | the **damage-tracked `Screen`** override: `mark` bbox accumulation, the `flush` present, and `FrameBuffer`'s pixel-format encoder | `video/witness.rs` (a `Screen` over a heap-backed offscreen `FrameBuffer`) |

`video.present` (VWIT) exists because the `Screen`/`TargetPal` override — the **steady-state on-screen
renderer** — was previously verified only *visually* by `vug` and the GUI console (attended). It builds
a `Screen` over a `Vec<u8>` (passed as the surface's raw base — `FrameBuffer` holds its base as `usize`
to stay `Send`) and asserts the real present path:

- **Format decode** — a known three-distinct-channel colour lands in `Bgr` byte order (`[B,G,R,0]`) in
  the front buffer; a raw byte-order spot-check catches an Rgb/Bgr swap the round-trip word alone would
  hide. (The `OffscreenPal` path bypasses this encoder.)
- **Damage-limited present** (the load-bearing assertion) — a sentinel poked into the front buffer
  *outside* the next draw's bbox **survives** the `flush`; a disjoint `fill_rect` is presented. Proves
  `mark`/`flush` blit only the damage rectangle, not the whole surface — the invariant `video.geometry`
  cannot observe.
- **No-op flush** — a second `flush` with no new damage leaves the front byte-stable (idempotent).
- **Clip safety** — signed off-screen `draw_line`/`fill_triangle` endpoints clip in-bounds without
  panic/overrun; the clipped line paints the on-screen span, the triangle fills a bounded interior.

Serial evidence (headless-gate visible; drive `tste` with `scripts/qmp_type.py --text tste --enter`):

```
:: VWIT: render present — format=Bgr damage=OK noop=OK clip=OK ::
:: TSTE: video.present -> PASS ::
```

These are QEMU-provable regressions only — a real GPU/present is not involved; on metal the same rows
run under `tste` and mirror to whatever console the target exposes (§4/§6 console-path rule applies).

## 8. WC — the window compositor (`video/wm.rs`, WC-A)

Before WC, `SYS_FB_PRESENT` had exactly one destination: `screen::present_surface` centered the
caller's single 32x32 surface on the panel. There was no window state anywhere, so two EL0 programs
could not be on screen at once. `video::wm` is the seam that fixes that.

### The table

A fixed array of `MAX_WINDOWS = 8` rows, statically allocated behind one `spin::Mutex`. The
compositor runs from syscall context on a non-coherent scan-out path, where a heap allocation would
be both a latency cost and a failure mode. Each row carries: id (`1..=8`; `0` is `WIN_NONE`, the
fail-closed return of every operation), owner ASID (opaque here — `wm` never reads task state),
content origin and source extent, integer scale, z-order, surface pointer + stride, a damage flag,
and a title truncated to `MAX_TITLE = 16` bytes at create time.

### Composite on present, no thread

`present(id)` marks the row damaged and composites inline, from the presenting task's own context.
This is the discipline `present_surface` has always used and it is forced: while a full-screen EL0
program owns the panel the render task is parked in `dispatch_command`, so routing through it would
present nothing. There is no compositor thread in this arc.

Two properties the pass has to get right:

- **Occlusion closure.** Repainting a damaged window would erase anything stacked on top of it, so
  the damage set is closed upwards to a fixed point — every higher-z window whose outer box overlaps
  a damaged one is repainted too, transitively — and the draw order is ascending z (ties by id, i.e.
  creation order).
- **No lock across the scan-out.** The table lock is held only to snapshot the rows and clear their
  damage flags. Every framebuffer write and every `flush_range` happens after it is released. `place`
  reads the panel geometry *before* taking the table lock, so no path ever holds both locks and there
  is no order to invert.

Each composited window gets one `flush_range` over the scanlines of its outer box — one `DC CVAC`
sweep per window rather than one per row, the same present discipline as `Screen::flush`. A no-op on
coherent targets.

### Chrome is kernel-drawn, always

Each window gets a 1-px border and a `TITLE_H = 12` title strip, painted by the compositor from the
kernel's own copy of the title (8x8 `font8x8` glyphs; non-printable bytes render as spaces). Apps
draw only inside their own surface. A hostile EL0 program therefore cannot forge another window's
frame or paint a convincing system dialog, and the presentation-modes law — never fake host chrome —
holds structurally rather than by convention. Colours are flat and deliberately un-host-like.

### Layout

`place` tiles the non-compat windows left-to-right in id order with an 8-px gap, wrapping to a new
row when the next outer box would run off the panel, and picks each window's scale as the largest
integer factor fitting half the panel in each axis (so a 32x32 surface is legible on a 1920-wide
panel and two windows still sit side-by-side), **capped at a legibility ceiling**. Layout depends
only on the live set, not on the order
of creates and closes, so it is deterministic. `move_to` pins a window out of the automatic layout.
`close`/`close_owner` erase the vacated box, relayout and recomposite; `close_owner` is what task
teardown calls, so a dead ASID can never leave a window compositing from a freed address space.

#### The legibility ceiling (WC-SCALE)

The fit rule alone is a function of the *surface*, so the smaller the surface the larger the
magnification — midden's 24×16 status readout landed at `scale=37x` on the 1920×1200 bench panel,
which is ~100 panel pixels per font pixel and reads as an abstract pattern rather than as digits.
`wm::legibility_cap` bounds it: `scale ≤ ui::SCALE_MAX · ui::Metrics::for_height(panel_h).scale`.

Both halves are pre-existing answers, not new constants — THE METRICS RULE holds, no absolute pixel
size appears. `SCALE_MAX` is already the kernel's stated limit for magnifying bitmap content
("beyond 4× legibility gains nothing and glyph blocks get blocky-huge") and `Metrics::for_height` is
how every other UI dimension tracks the panel, so the ceiling is *four font-scale steps*: 4× on a
≤ 1799-row panel, 8× on an 1800p-class one. It is the same **kind** of cap `screen::present_surface`
applies on the compat path (there: ~40 % of the panel's shorter dimension); that path keeps its own
rule, because a compat surface is a full-screen app's whole canvas and wants to be as large as it
comfortably can, whereas a tiled window is one of several and wants to be as *readable* as it can.

The cap shrinks the window's outer box, so tiling stays exactly as deterministic as before — no
letterboxing, no reserved slot. Nothing here can move a checksum: `[wc-c]`/`[wc-d]` `cksum=` is FNV
over the **source** `surf_len`, which no scale change touches. Concretely: on the 640×480 gate panel
128×128 stays 1× and 64×64 stays 3× (both already under the cap, so the gate log is byte-identical);
on the 1920×1200 bench panel 128×128 stays 4×, 64×64 comes down 9× → 4×, and 24×16 comes down
37× → 4× (96×64 panel pixels — a status readout at a readable size).

### The compat shim

`screen::present_surface` keeps its geometry math verbatim (legacy centering + the UVUG-7 integer
scale rule) and hands the blit to `wm::compat_present`, which creates one `compat` window on first
call and updates it in place afterwards. A compat window draws **no** chrome and flushes exactly the
rows `[y0, y0+dh)` the legacy path flushed, so pre-window apps are byte-identical: the `kernel8-test`
panel capture matches the pre-WC baseline `pi-screen.png` sha256 exactly, with MBENCH 46/46 and 0
forbidden.

### Serial evidence (`witness` feature)

```
[wc-a] composite windows=1 drawn=1
```

That is the line a `kernel8-test` run emits today: the compat window presenting through the shim. The
other three formats are **not yet exercised** — nothing calls `create`/`close` until WC-B lands the
window syscalls — so they are given here as formats, with the values a 32x32 surface on the QEMU
640x480 panel would produce (scale = `min(640/2/32, 480/2/32)` = 7, origin = the first tile slot):

```
[wc-a] create win=1 asid=0x1 surf=32x32 stride=128 scale=7x at (9,21) z=1
[wc-a] close win=1
[wc-a] close_owner asid=0x1 closed=1
```

`composite` witnesses the **first** pass only — a mini-vug run presents ~300 frames and per-frame
lines would drown the log (the same reason `[uvug2]` fires once).

### Treating window geometry as hostile

Everything the app supplies — `w`, `h`, `stride`, the `move_to` origin — is attacker-controlled once
WC-B exposes the verbs, and the compositor is the one place that turns those numbers into memory
reads. The memory-safety lens on WC-A found six defects here; all are fixed, and the invariants they
established are the ones to preserve:

- **Surface-extent contract.** `create` takes `surf_len`, the real byte length of the mapped slot,
  supplied by the mapping code and never derived from EL0 dimensions. Geometry that would read
  outside it (`w * 4 > stride`, `h * stride > surf_len`, saturating) is rejected at create. Without
  this, a `w=h=10000, stride=40000` window over a 4 KiB slot has the compositor read ~400 MB of EL1
  memory and paint kernel bytes onto the panel — `put_pixel` clips *writes*, never the source read.
- **Panel-clipped loops.** Every composite loop is clipped to the panel intersection before it runs.
  Per-pixel clipping keeps the writes safe but still iterates: the same hostile window would spin
  ~1e8 clipped pokes per present, from syscall context.
- **Saturating geometry.** The kernel builds with overflow checks off, so `w * scale` wrapping would
  silently produce a small damage box for a large paint. All of it saturates, and `move_to` clamps
  the origin on both bounds against the panel.
- **Ids are slot aliases.** There is no generation counter, so "is this still my window?" cannot be
  answered by liveness: a closed row's id is immediately valid again under a new owner. The compat
  shim therefore keys on the row's `compat` flag — a property no recycled row can accidentally have —
  and re-checks it under the lock. (A generation-widened `WinId` was the alternative; the flag is
  exact for this hazard and keeps the id a plain slot number for WC-B's syscall ABI.)
- **Teardown drains in-flight blits, behind a phase barrier.** `composite` checks a drain flag and
  registers itself as in-flight in the *same* table-lock critical section, so both are ordered
  against any later teardown's lock acquisition. `close`/`close_owner` clear their rows, raise the
  barrier, and spin until the in-flight count reaches zero. A composite that takes the lock while the
  barrier is up skips entirely — correct, since the rows it would have drawn are gone and teardown
  recomposites when it is done, and *necessary*, because it is what makes the wait terminate: the
  in-flight count can only fall while the barrier is up. A plain "wait for idle" loop would not
  terminate under continuous presents, and the teardown path (`sched::exit` → `clear_handle_row` →
  `close_owner`) spins IRQ-masked and unpreemptible, so that livelock would be a dead core rather
  than a slow one. Today the underlying race is a stale read; under WC-B's per-ASID surface mappings
  it becomes an EL1 abort mid-blit.
- **The compat window's check-and-create is serialised.** `COMPAT_WIN` alone is check-then-act: two
  `SYS_FB_PRESENT`s on different cores could each create a compat row, and the loser's would be an
  ownerless, unreferenced window nothing can close — F3 again, through a race.
- **The compat window has a lifecycle.** It has no owner ASID, so `close_owner` can never reap it;
  `wm::close_compat()` exists for that and WC-B must call it from the EL0 teardown seam. Otherwise
  the row is immortal and every later composite re-blits a dead app's buffer.

### Seam for the window syscalls (WC-B)

`SYS_WIN_CREATE` / `SYS_WIN_PRESENT` / `SYS_WIN_MOVE` / `SYS_WIN_CLOSE` are thin fail-closed wrappers
over `create` / `present` / `move_to` / `close`; the per-ASID ownership gate reads `owner_of`, and
`clear_handle_row` calls **both** `close_owner` (for the ASID's own windows) and `close_compat` (for
the ownerless shim row). `create` must be passed the mapped slot's real byte length as `surf_len`.
`SYS_FB_MAP` / `SYS_FB_PRESENT` stay as compat wrappers over the compat window.

### WC-INT — the wired seam

The two units are joined. WC-B's `WINDOWS` table stays authoritative for id allocation and ownership;
`wm::create` mints its own `WinId` out of its own table, and WC-B stores that id in the row's `wm_id`
field. The two id spaces are never assumed to line up — every later `wm` call goes through `wm_id`.
`WIN_NONE` is a legal value everywhere, so a window the compositor refused (full table, or geometry
the extent contract rejects) simply has no compositor presence and its verbs still succeed at the
WC-B layer. `surf_len` is WC-B's `pages * 0x1000`, the real mapped-slot byte length, never a
recomputed `h * stride`.

`SYS_FB_MAP`'s compat row deliberately gets **no** `wm` window (`wm_id` stays `WIN_NONE`): the compat
window is minted lazily by `compat_present` and reaped by `close_compat`. A `wm::create` there would
mint a chrome-bearing, tiled window and the pre-WC UVUG panel output would change.

**One forwarding, not two.** Before integration, both the syscall present body and
`screen::present_surface` ended in a present. Now the two cases are exclusive: `wm_id != WIN_NONE`
damage-marks and composites through `wm::present` and never touches the ELF-3 hook; `wm_id ==
WIN_NONE` forwards to the hook, whose target computes the legacy geometry and hands off to
`compat_present`. Both sit *below* the focus-guarded `EL0_FOCUSED_PRESENT_COUNT` bump and the
`FB_PRESENT_COUNT`/checksum witness, so the UVUG-8 suspension cap and the ELF-3 fb-test `present == 1`
verdict are driven by one accounting block whichever path renders.

#### Lock order across the seam

The global order is **`WINDOWS` ⊃ `wm::TABLE` ⊃ `video::WRITER`**, and it is verified, not assumed:

- `WINDOWS` (WC-B's `SpinMutex`, always taken IRQ-masked) is the outermost lock. `sys_win_present`
  holds it across the whole present *including* the composite — deliberately, so the id handed to the
  compositor is provably the id the ownership gate validated (a `close` + `create` pair on other cores
  could otherwise recycle the id in the gap and land the caller's pixels under a new owner's identity).
- Nothing in `video/wm.rs` or `video/screen.rs` references the syscall layer at all, so **no path
  acquires `WINDOWS` from inside `wm`**. The edge is one-way by construction, and the cycle that would
  make the held-across-composite hold dangerous cannot be formed.
- The `TABLE ⊃ WRITER` half is stronger than stated: the two are **never held simultaneously**. Every
  `WRITER` acquisition in the lane is the statement form `let fb = *WRITER.lock();` — `FrameBuffer` is
  `Copy`, so the guard is a temporary dropped at the end of that statement, and what survives is a
  value. `move_to`, `erase`, `composite`, `place` and `present_surface` all read the panel geometry
  that way *before* or *after* their table critical section, never inside it. So a present never holds
  the window lock across a scan-out cache clean, and there is no order to invert.
- `COMPAT_CREATE` sits strictly outside `TABLE` (it is taken only in `compat_present`, around the
  check-and-create) and is never taken while `TABLE` is held.

**The drain barrier is safe under this order.** `close`/`close_owner` spin until in-flight composites
finish. Every caller invokes them with `WINDOWS` *released* — `sys_win_close` and `win_close_asid`
collect the `wm_id`s inside the lock and destroy outside it, and `clear_handle_row` calls
`close_owner`/`close_compat` after `win_close_asid` has returned. Even if a drain *did* run under
`WINDOWS`, it would still terminate: a composite blocked waiting on `WINDOWS` has not yet reached
`wm`, so it has not incremented `BLIT_ACTIVE` and is not a member of the drain set. Registration
happens inside `wm::TABLE`, strictly after `WINDOWS` is acquired. Combined with `DRAIN_PENDING` —
which makes any composite that takes `TABLE` after the barrier goes up skip without registering — the
wait set is fixed at entry, finite, and every member is running a bounded panel-clipped blit. **The
drain set is closed.** That matters because teardown (`sched::exit` → `clear_handle_row` →
`close_owner`) spins IRQ-masked and unpreemptible: a livelock here would be a dead core, not a slow one.

Teardown order is WC-B first, then `wm`: `win_close_asid` unmaps the surfaces and frees the rows, then
`close_owner` sweeps any compositor row whose WC-B row a racing `sys_win_close` already freed, and
`close_compat` reaps the one row that has no owner ASID to match on.

### WC-C — the desktop, the clients, and focus

WC-A built the compositor and WC-B the verbs; WC-C is the arc where real programs use them.

**The desktop is painted, not left blank.** `wm::erase` filled vacated boxes with black, which on a
panel is a *colour*, not an absence: every closed window left a black rectangle over the console's
Moonstone background and the kernel chrome's close-box left a black hole mid-desktop. `erase` now fills
`wm::DESKTOP_BG`, and the WC-INT panel residue is gone — the `kernel8-test` capture is a clean desktop
plus the status strip. `DESKTOP_BG` restates the console's private `Console::BG` rather than importing
it, so the compositor owns its own theme value (the crispy theme will replace it with real desktop
data); a drift between the two is immediately visible as a rectangle where a window used to be.

**UVUG is windowed.** `crates/user-vug` no longer maps the 32×32 compat surface. It calls
`SYS_WIN_CREATE(128, 128)` — `boot::FB_WIN_MAX_W/H`, exactly one 64 KiB window slot — reads its surface
at its own window base + `0x5000` (window region slot 0, the VA `SYS_FB_MAP` used to return), and
presents with `SYS_WIN_PRESENT`. `FOCAL` scales 6 → 24 so the crystal keeps the same framing at 4× the
linear resolution. The UVUG-8 takeover/cap line is unaffected: `sys_win_present` runs the *same* present
body as `sys_fb_present`, so the focus-guarded `EL0_FOCUSED_PRESENT_COUNT` bump still happens per
window, and the QEMU run still reaches `exit=0` through `run_user_image`'s deadline.

> **Spec change, deliberate.** The 300-frame auto-path checksum is a pure function of the surface, so a
> 128×128 surface necessarily produces a new one: `:: UVUG: frames=300 threads=2
> checksum=0xe68285b85121ac7c ::`, replacing the 32×32 `0x48221e4101db3924`. This is the *second* of the
> two options the brief allowed — the spec is updated with the new value rather than the geometry being
> kept compat — because a compat shim here would mean shipping the 32×32 render forever to protect a
> constant. The invariant that *is* preserved byte-for-byte is the **compat path**: `SYS_FB_MAP` +
> `SYS_FB_PRESENT` still produce the identical centred, chrome-less blit (the ELF-3 fb test's
> `mapped=32x32 … checksum=0x8d99530ca96d4b25` is unchanged).

**midden has a window.** `crates/user-blob/src/midden.rs` creates a 24×16 window and renders its own
bus stats into it: two rows of four hex digits (the live witness bitmask, and the number of legs passed)
from a 3×5 bitmap font packed one `u16` per glyph, blitted 1:1 at EL0. The kernel draws the border and
the title strip and *nothing else* — app content is app-drawn, chrome is kernel-drawn, and neither can
forge the other. The compositor's integer upscale (13× on the QEMU 640×480 panel, ~30× on a 1920-wide
one) is what makes it legible, so midden never learns the panel geometry. A refused `SYS_WIN_CREATE`
makes every repaint a no-op, so midden's bus witnesses never depend on the compositor. Fitting this in
the flat blob's single 4 KiB code page needed the crate's release profile to move from `opt-level = "s"`
to `"z"` (3792 B, ~300 B of headroom); the per-blob page assertion in `arroyo kernel8` is what catches
a regression.

**Focus is a key.** TAB is reserved *by the window system*, intercepted at `user_input_enqueue` — the one
choke point every event bound for an EL0 ring passes through, so no app can hold focus hostage by not
implementing it. It walks `wm::focus_ring` (the distinct owner ASIDs of the live non-compat windows, in
window-id order — a stable rotation, not a reordering stack) and hands focus to the next entry via
`user_input_set_active`, which is still the only way focus moves: the incoming ring is reset, the
interactive-takeover latch is cleared, and the UVUG-8 cap therefore keeps holding *per window*. The
matching KeyUp is swallowed on the same predicate (a lone release edge for a press the app never saw is
exactly the shape UVUG-6 removed from the typematic path). With fewer than two windows in the ring the
key falls through as an ordinary TAB.

The ring carries **one slot beyond the windows: the shell** (`USER_INPUT_ACTIVE == 0`). Without it the
cycle is a closed loop over the live apps — an operator who tabs into a window can never get the keyboard
back, and the wedge watchdog becomes the only exit from a perfectly healthy app. So "no app has focus" is
a position in the rotation, not an absence.

**WC-TAB closed the loop.** WC-C shipped the shell slot as a one-way exit: with focus 0,
`route_input_to_active_el0` is not called at all (`main.rs` gates on `user_input_active() != 0`), so no
TAB reached the seam — the ring could be left but not re-entered. `pump_usb_into_gui` now calls
`syscall::wc_shell_focus_key` from both of its non-routing paths. That is a second *entry point* onto
the same `wc_focus_key` body, not a second implementation: same predicate, same `user_input_set_active`
move, same `[wc-c] focus tab-cycle` witness. With focus 0 in no window's slot the cycle takes its
"unknown focus" arm and lands on the ring's head, the first window in window-id order. The `n < 2` guard
is shared, so with one window (or none) TAB remains an ordinary key at the shell too — deliberate
symmetry, since a lone window does not consume TAB and pushing focus into it would re-create the trap.
Nothing was clobbered: the console's `handle_key` ignores byte 9 outright, so TAB had no shell binding
before this.

Which path carries it is the whole substance of the fix. `handle_key` sets `SCREEN_APP_ACTIVE` around
`dispatch_command`, and `run_user_image` parks the shell task for the entire EL0 run — so while apps are
live *and* focus is at the shell, both the `SCREEN_APP_ACTIVE` branch and the shell drain are in play,
and the former returns first. The interception therefore sits inside that branch's existing
peek/requeue scan: a consumed TAB is simply not requeued, and on a real focus cycle the rest of the
buffer is dropped rather than forwarded (a swallowed release edge keeps the buffer — it is requeued and
the scan continues). It must not be sent onward — `render_service` is blocked inside the same
`dispatch_command`, so pushing into the 64-slot `GUI_CHANNEL` would saturate it and block the pump task,
exactly what that branch exists to prevent. Dropping the buffer is not a new policy either: it is what
`user_input_set_active` would have done to those same events itself, since it drains `pal::EVENT_QUEUE`
on every real focus change; they are outside the queue only because the uncounted peek is holding them.

**Scope, plainly:** as WC-C already conceded, the boot's own programs do not overlap in time, so a ring
of two or more windows arises today only under the `el0-wcb` fixture that creates them deliberately.
This is the mechanism made whole and symmetric — not yet a workflow a metal operator falls into. A
single-window boot sees no behaviour change at all: TAB stays an ordinary key end to end.

#### The side-by-side witness

The arc's claim is two windows composited together. A screenshot shows that to a human and proves
nothing to a gate, and the `[wc-a] create` lines say only that rows *existed*. So `composite` emits, once,
from inside the pass that actually drew them:

```
[wc-c] side-by-side windows=2 drawn=2
[wc-c] win=1 asid=0x1 surf=128x128 scale=1x at (9,21) z=3 cksum=0xfabe809492cf2325
[wc-c] win=2 asid=0x1 surf=64x64 scale=3x at (147,21) z=4 cksum=0x591f6cbe80502325
```

The per-window checksum is FNV-1a over `surf_len` — the mapping-code length, the same bound
`draw_window` reads under — so a window that is present-but-blank, or that composited a stale or
recycled surface, is distinguishable from one that drew real content.

**Honest limit on what drives it.** The boot's real programs cannot overlap: `uvug_witness` and
`bandy_rt_launcher` each run their program to completion, so UVUG's window and midden's window are never
live at the same instant. The vehicle is therefore the `el0-wcb` fixture, widened from 10 witness bits to
13: it creates a second 64×64 window filled with a different byte and presents it **while the first is
still live and still holding its content**, which is the only state in the boot where two real windows
composite in one pass. Window A is re-presented before the closes, so `FB_PRESENT_CHECKSUM` at verdict
time is still A's 128×128 `0xC3` surface and the pre-WC-C verdict line is unchanged. Two live EL0
programs at once is a scheduler question, not a compositor one, and is left to a later arc.

#### WC-C gate results (2026-07-24, QEMU raspi4b)

`./arroyo check` green both arches · `kernel8` builds (midden 3792 B ≤ 4096) · `kernel8-test 120`
MBENCH **49/49 required, 0 forbidden** (the arc adds three `pi4-regression.spec` directives — the UVUG
checksum, the `witness=0x1fff` ledger, and the side-by-side line, which is what gates the per-window
checksums below it) · `:: EL0: window verbs — … witness=0x1fff … :: PASS ::` ·
`:: EXEC-UVUG: … exit=0 -> PASS ::` · all four BANDY verdicts PASS.

`target/pi-screen.png` is **re-baselined by design** — the WC-INT residue this arc removes was the whole
point of the desktop repaint. New sha256 `2686a884320dbc389d6c33b1f37b097fa15eba769b51a751449e2c91a986bc19`.

### WC-D — the scan-out verdict, and why a 640×480 gate could not see the bench

The P56 bench reported the 128×128 windowed crystal rendering **garbled on the panel** while every serial
witness was green. That combination is the whole problem: `[wc-c]`'s per-window checksum hashes the SOURCE
surface, so it proves the app drew something and says nothing whatsoever about the pixels that reached the
panel. Everything between the surface and the scan-out — the upscale's indexing, `put_pixel`'s colour
encoding, the clip, and the cache clean that makes the non-coherent HVS see the result — was unwitnessed.

**Why the gate was blind by construction.** QEMU raspi4b's mailbox panel is 640×480; the bench Pi drives
1920×1200. `wm::place` derives each window's integer upscale FROM the panel, so those are not the same code
path: a 128×128 window lands at **scale 1** on 640×480 and **scale 4** on 1920×1200. The gate was running a
1:1 copy while the bench ran a 4× nearest-neighbour expansion — every scaled-blit defect was invisible to it
for free. `mailbox::FORCE_W` / `FORCE_H` (`UNAOS_FBW` / `UNAOS_FBH`, default off) close that gap: they
override the firmware-queried mode so the gate can be run at the bench's geometry.

**The instrument.** `wm::verify_window` runs once per window, from inside the composite pass that drew it,
and re-derives every destination pixel of the content rect from the source surface, comparing it against the
scan-out buffer **twice**:

| verdict | meaning |
|---|---|
| `bad_cache=0` | the compositor computed the right pixels — stride/pitch arithmetic, upscale indexing, colour encoding, clipping |
| `bad_cache>0` | the **blit** is wrong, or another painter overwrote the rect mid-composite; `first=` names the pixel |
| `bad_ram>0` | the pixels did not reach the memory the HVS scans — a cache-maintenance defect |

The line also carries `cksum=` (the `[wc-c]` FNV over the source slot) and `nonzero=` (destination pixels that
are not black), so a blank surface faithfully blitted onto a blank rect is distinguishable from a verified
crystal instead of reading as an equally green PASS. `first=none` is printed on a clean verdict, so a real
black-on-black mismatch at the origin cannot hide behind an all-zero placeholder. A window refused by a
guard emits `-> SKIP` with its reason rather than silently consuming its one-shot latch.

**`bad_ram` uses a bare `DC IVAC`, and that is load-bearing.** The first cut of this witness used
`DC CIVAC`. It was wrong in exactly the way that mattered: `CIVAC` writes dirty lines back before
invalidating, so had `draw_window`'s trailing `flush_range` been missing or short, the witness would have
cleaned those very lines to RAM and then read the repaired result — printing `bad_ram=0` for a panel that
garbles. The falsifier is concrete: delete the `flush_range` call and the `CIVAC` form still passes. A bare
`IVAC` discards un-cleaned lines instead, so a short flush surfaces as stale RAM.

**The falsifier was run, and it is INCONCLUSIVE off-metal — say so.** The `flush_range` call in
`draw_window` was deleted and the gate re-run at forced bench geometry; the verdict stayed
`bad_cache=0 bad_ram=0 -> PASS`. That is **not** evidence the instrument is broken — it is evidence that
QEMU raspi4b does not model a non-coherent framebuffer at all, so CPU stores reach guest memory whether or
not `DC CVAC` ran, and `bad_ram` has nothing to detect. The consequence is worth stating plainly: **the
`bad_ram` column is unvalidated in BOTH directions off-metal.** Its correctness rests on the primitive being
right by inspection (`IVAC` discards, `CIVAC` would have repaired), not on a passing gate. The next bench
boot is its first real exercise, and a `bad_ram=0` there is the first datum that means anything.

> **Hazard — an instrumented build is not a neutral observer.** `IVAC` discards anything dirty-and-unclean
> in those full scanlines, which in a correct build is nothing (`draw_window` just cleaned a strict superset)
> but in a broken one can drop pixels — and the invalidated extent is full-width panel scanlines, not the
> window's columns, so the concrete exposure is `fbcon`'s deferred dirty band (`mark_rows`/`flush_dirty`):
> unflushed console glyphs on those rows are discarded and the redraw restores only the window, not them.
> `verify_window` therefore redraws and re-flushes the window before returning. Consequence: in the presence
> of a flush defect a `witness` build can differ visibly from a default build, in either direction. Do not
> read a witness build's panel as evidence about a default build's panel.

**What the gate established, and what it did not.** At the bench's exact geometry — `panel=1920x1200`,
`surf=128x128 scale=4x at (9,21)`, matching the P56 log line for line — the verdict is
`checked=262144 bad_cache=0 bad_ram=0 -> PASS`. What that **earns** is the `bad_cache` half: the scaled
blit's indexing, the stride/pitch arithmetic and the pixel format are correct at the geometry that garbled,
so those three suspect classes are excluded. `bad_ram` passing on QEMU earns nothing about metal — QEMU does
not model the Pi's non-coherent scan-out, so that column is only meaningful on the bench, which is precisely
what it is there to report.

**Flush extent is excluded by inspection, not by `bad_ram`.** `draw_window` flushes rows
`[by, by+bh)` of the window's `outer_box`, which spans the title strip and both borders, and `flush_range`
works in whole scanlines at the panel's full stride — a strict superset of every pixel the blit touched.
That argument, not the QEMU verdict, is what rules the extent out.

**Scope of the pitch claim.** Verified against the firmware-reported pitch of 7680 B for a 1920-wide,
4-bytes-per-pixel panel — i.e. pitch == width × bpp with no padding. A bench panel whose firmware returns a
*padded* pitch is not covered by this run; `put_pixel` and `flush_range` both derive from `info.stride`, so
the arithmetic is expected to hold, but it is untested at a padded pitch.

**One thing the rect is not guarded against.** `composite` copies the `FrameBuffer` handle and releases the
window-table lock before drawing, so nothing prevents another core's painter (the console, the render task)
from writing into the rect between the blit and the verification. Such an overwrite is reported as
`bad_cache>0` and is indistinguishable from a genuine blit defect; `first=` is the disambiguator to reach for.

The boot-time "indecipherable" auto-launch surfaces were a **separate, cosmetic** matter and not this defect:
`wm::place`'s scale rule maximised legibility without bound, so midden's 24×16 surface landed at `scale=37x`
on a 1920×1200 panel — a few characters at enormous magnification, exactly as designed. **WC-SCALE fixed
that with the cap described under Layout above** (`wm::legibility_cap`), not with a correctness change.

#### WC-D gate results (2026-07-25, QEMU raspi4b)

`./arroyo check` green both arches · `kernel8` builds · `kernel8-test 120` MBENCH **50/50 required,
0 forbidden** (the arc adds the `[wc-d] … -> PASS` REQUIRE and the `-> FAIL` FORBID) · `test-arm` green ·
and, at forced bench geometry (`UNAOS_FBW=1920 UNAOS_FBH=1200`), `[wc-d] verify win=1 surf=128x128 scale=4x
at (9,21) panel=1920x1200 checked=262144 bad_cache=0 bad_ram=0 -> PASS`.

### WC-E — the garble was never in the pixels: two writers, one scan-out, no ordering

WC-D left the defect cornered and unexplained. The composited pixels were byte-correct in the RAM the HVS
scans (`bad_cache=0 bad_ram=0 nonzero=full` on every window, at the bench's exact geometry), and the panel
still garbled. The remaining suspects were all scan-out geometry — pitch, depth, pixel order, virtual size,
viewport offset, base alignment. WC-E retired every one of them and found the cause somewhere else.

**What the evidence actually said.** Three facts, taken together, leave exactly one explanation.

1. **The geometry is self-consistent and correct.** P57 metal: `pitch=7680B` for a 1920-wide 32-bpp panel
   (`= width × bpp`, no padding), `size=9216000` (`= 1920 × 1200 × 4`, exact). Nothing to shear a row
   against. Suspect 1 is dead on the wire, and 3 and 4 with it.
2. **The console renders correctly while the windows garble.** Console text and window content go through
   the *same* `put_pixel`, the *same* `info.stride` and the *same* framebuffer base. Any scan-out geometry
   error is common to both and would shear the text identically. A defect that afflicts one and not the
   other cannot live in the geometry — it must live in what is *different* about the two paths.
3. **The composited bytes are identical on QEMU and on metal.** The gate at forced bench geometry produces
   the same `[wc-c]` source checksums as the P57 metal boot, window for window
   (`0xa1cf4a91b6138449`, `0xfabe809492cf2325`, `0x591f6cbe80502325`, `0x377ffc1fbd89557d`). The content is
   not corrupted anywhere. Identical bytes, different panel ⇒ the divergence is **temporal**, not spatial:
   the question is not what was written, it is who wrote *last*.

And point 2's difference between the two paths is the answer. **The console is back-buffered; the windows
are not.** `Screen` owns a back buffer in cached RAM, and `flush` copies its damaged rectangles into the
scan-out. The window compositor writes windows **directly** into the scan-out from the presenting task's
syscall context — `screen::present_surface` documents this choice, and reasons it sound for the case it was
written for (a *full-screen* EL0 program parks the render task, so nothing else is flushing). WC-C's
*windowed* apps break that premise: the desktop keeps rendering alongside them. The two writers never knew
about each other, and nothing ordered them.

The bench log measures the collision precisely. `[vugfps] 20.3 fps 3996743 bytes/frame flushed rects=4
union=1920x1200` — the render task presents ~20 times a second with damage unions spanning the **entire
panel**, for the whole life of the boot. So: a window presents; the compositor pokes it into the scan-out;
`verify_window` reads it back microseconds later and correctly reports `bad_cache=0 bad_ram=0`; and then,
within 50 ms, the desktop flush copies desktop content over every pixel of it. Repeat at 20 Hz, in four
parallel bands whose boundaries fall wherever the scheduler puts them, and the window region on the panel
is a shimmer of window content and rotating VUG scene. "Noise-like indecipherability" is exactly what that
looks like. Every witness stays green because every witness reads the framebuffer back *inside the present
that wrote it*, long before the next desktop frame lands. WC-D even named the mechanism as an unguarded
possibility — another core's painter writing into the rect — and only missed it because it looked for the
overwrite *during* the verification window rather than after it.

**The fix is the layering the code always implied.** The desktop is the background layer; the window
compositor is the layer above it. `Screen::flush` now presents the background and then calls `wm::repaint`,
which marks the live windows damaged and re-composites — background, then windows, in one call, so
"windows are above the desktop" is continuously true instead of true only until the next frame.
`composite` already closes the damage set upwards over occlusion, so the restored stack is correct
back-to-front.

**COMPAT windows are excluded, and that exclusion is load-bearing.** A compat row is the full-screen
present path, and while a full-screen EL0 program owns the panel the render task is parked and is not
flushing — there is no second writer to order against, so a repaint there fixes nothing. It also is not
free: the first cut repainted compat rows too, and re-blitting UVUG's 32×32 surface at 15× on every
desktop frame pushed its 300-frame run past the `EXEC-UVUG` deadline —
`45/51 required, 6 forbidden` (the UVUG timeout cascading into all four BANDY verdicts). Scoping the
repaint to *real* (windowed) rows is what the collision actually requires and what restores the gate. On
a windowless desktop frame the whole mechanism costs one table-lock acquisition and a return.

> **Residual, stated plainly.** Window pixels are now overwritten and repainted *within the same present*
> rather than never being overwritten. A scan-out that lands between the two steps can still catch a window
> mid-repaint, so a single-frame tear remains possible. Removing that needs the flush to skip the rows a
> window owns, or a fully double-buffered composite; what this change removes is the *unbounded, every-frame
> erasure*, which is the difference between a window that flickers and a window that is unreadable.

**`[wc-e]` — the scan-out ground truth, on every boot.** `init_framebuffer` programs five setters and then
keeps the values it *requested*; the firmware is free to clamp, round or refuse any of them. `mailbox::
witness_fb_geometry` now asks the firmware what it actually settled on — `GET_PHYS_WH`, `GET_VIRT_WH`,
`GET_VIRT_OFFSET`, `GET_DEPTH`, `GET_PIXEL_ORDER`, `GET_ALPHA_MODE` — and prints one unconditional line
carrying `req=` beside every firmware answer, plus base, size, base alignment, and two derived identities:
`row_ok` (`pitch == virt_w × bpp`, the identity a row-phase garble breaks) and `fit_ok` (the allocation
holds the whole visible image). Query-only tags; a failed call prints a FAIL line and returns, because a
diagnostic must not be able to take down the boot it is diagnosing.

This exists because of a specific blind spot WC-D had. `verify_window` reads the framebuffer back through
the **same `info.stride` it wrote through**, so it agrees with itself no matter what the display pipe is
doing: a witness that asks our numbers can never falsify our numbers. `[wc-e]` asks the firmware's. When it
agrees with `req=`, the entire scan-out geometry suspect surface is retired for that boot and anything
downstream can be blamed honestly; when it diverges, it names the field.

#### WC-E gate results (2026-07-25, QEMU raspi4b)

`./arroyo check` green both arches · `kernel8` builds · `kernel8-test 120` at BOTH default (640×480) and
forced bench geometry (`UNAOS_FBW=1920 UNAOS_FBH=1200`), MBENCH **51/51 required, 0 forbidden** each
(the count includes this arc's own `[wc-e]` REQUIRE; the lens re-measured both) · `test-arm` green. The pre-fix
baseline run at the same geometry is what established fact 3 above (identical `[wc-c]` checksums to P57
metal). **The fix's effect is not observable in QEMU** — the collision is a race between the render task
and the presenting task, and the gate's screenshot samples one instant — so bench verification at the next
Pi 4 boot is what confirms it, with the `[wc-e]` line as the standing geometry check on the same wire.

### WC-F — an independent read of what the HVS actually scans

WC-E fixed a real two-writer ordering bug, and the bench panel garbled the 128×128 crystal anyway on the
P58 boot. Content byte-correct in RAM (WC-D), writers ordered (WC-E), garble persisting: the remaining
surface is scan-out-side, and it is the side both prior witnesses are constitutionally unable to see. Both
address the framebuffer through `info.stride`, the same number the blit writes through.

**What independence is not.** The first cut of this arc failed its own review, and the failure is the most
useful thing in this section. It compared `stride × bpp` against the firmware's pitch — but
`init_framebuffer` *defines* `stride = pitch / 4`, so those are the same number for every possible
firmware, and the check could not fail. It then re-asked `GET_PITCH` and compared that to the allocation's
pitch: the same firmware answering the same question, which confirms the reply and nothing about the
hardware. Both twin paths ended up computing byte-identical offsets, `comp_bad`/`direct_bad` were
structurally zero, and the photograph's "left garbled, right clean" arm was unreachable. **A witness whose
failing branch cannot be reached is decoration.**

Two constructs here are genuinely independent of the pitch reply:

**1. A row step derived from geometry.** `virt_w × 4` is what one row of the reported virtual width
occupies. It equals the pitch when the firmware allocates unpadded and diverges *by the padding* when it
does not — so `rowstep_match` (`stride × bpp` vs `virt_w × 4`) is a real question with a reachable `false`,
and `pad=` states the divergence in bytes. The right-hand twin block is addressed with it, so on a padded
framebuffer the two blocks genuinely land at different addresses and the panel shows it.

| field on `[wc-f] scanout` | the identity |
|---|---|
| `base_match` | the mapping we store into **is** the buffer the firmware allocated |
| `rowstep_match` | `stride × bpp` equals `virt_w × 4` — false exactly when the firmware pads a row the compositor ignores |
| `pad` | that padding, in bytes per row |
| `pitch_match` | the allocation pitch survives an independent re-query (weak alone; a divergence would be decisive) |
| `panel_match` | the geometry `wm::place` lays windows out in is the mode being scanned |
| `fits` | the mapping covers every row the pipe scans |

**2. A marker whose photographed slope IS the hardware's row step.** Every number on the serial wire —
ours and the firmware's alike — is downstream of the firmware's own claim, so no serial line can observe
*"the HVS steps differently than it reported"*. The ramp can. It writes one two-pixel mark per row by pure
byte arithmetic, each `k_row + 4` bytes after the last. If the hardware advances a row by exactly `k_row`,
the marks photograph as a diagonal moving exactly one pixel right per row; if its real step is `k_row + d`,
the marks drift `d/4` px per row and the diagonal visibly bends. 256 rows, ticked white every 16th so rows
can be counted off the photo. **This is measured off the panel, not off the log.**

**The twin probe.** One known 16×16 pattern rendered twice at the bench's 4× upscale, side by side: left
through `put_pixel`/`info.stride` (byte-for-byte `wm::draw_window`'s inner loop), right through raw stores
stepped by `virt_w × 4`. Each block is cross-read through the other path's addressing. The direct path
stores **three** bytes, exactly as `put_pixel` does, so the blocks cannot differ for the non-addressing
reason of one having zeroed the fourth byte under an alpha mode that reads it.

- **left garbled, right clean** ⇒ the compositor's addressing; the defect is in the blit path.
- **both garbled** ⇒ the HVS or the pitch; the blit is faithfully filling a surface nobody scans right.
- **both clean, crystal still garbled** ⇒ neither addressing nor geometry; the defect is specific to the
  window path (surface contents at scan time, or a writer WC-E did not order).

In the **divergent** case the right-hand block deliberately leaves its rectangle: `pad > 0` means
`geom_row < k_row`, so each row lands at a lower offset and the block drifts *upward* out of the bottom
strip. That is the displacement the probe exists to make visible — clamping it would erase the finding —
so it is accepted rather than fixed, at the cost of the overlap guard's coverage on a padded boot.
`direct_lands=(col,row)` states where the block's first row actually comes out under the kernel's row step — a padded boot displaces it both vertically and sideways — so
the operator knows which part of the panel to photograph and which garble is the probe's own.

**A comparison that cannot be made is counted, never dropped.** An out-of-range offset increments
`skipped`; `PASS` requires `skipped == 0`, `lost == 0`, and `checked` equal to the full expected count
(8192). Without that, a probe whose every read fell outside the mapping would report `comp_bad=0` — which
reads exactly like agreement, the one way a witness can lie.

**Every line is one-shot, and a retryable condition never prints SKIP.** The probe fires from a path
that runs hundreds of times per boot, so this is not tidiness. `SKIP` is reserved for **terminal** causes
— no framebuffer, no recorded firmware truth, a layout with no 4-byte RGB pixel, a panel too small — which
nothing later in the boot can change; it prints once and latches. A window sitting over the probe strip is
**retryable**: it emits a one-shot `-> DEFER` (which the spec deliberately does not forbid) and the probe
keeps trying every composite. Conflating the two broke twice over: a run that deferred early and passed
later would carry both lines and trip the unconditional `FORBID … -> SKIP` while being perfectly healthy,
and a window that never moves — a full-screen compat surface — would emit one line per composite pass,
unbounded. The geometry report latches independently of both, so it fires on the first pass regardless.

**Cost and collision discipline.** The probe runs only from composite passes that already drew something,
so its ~12 K stores and row cleans fall where work is happening anyway rather than being charged to every
idle repaint — it instruments the path it runs in, and perturbing that path is a real cost. It refuses to
paint while any live window overlaps its region (it paints last, so it would win, and a window under it
would silently show WC-F's pattern instead of the app's content). Both marks sit in the bottom strip —
twins hard right, ramp hard left — because stacking them put the ramp where `wm::place` lands its first
window at 640×480, and a witness that habitually skips is no witness. The cleaned ranges are invalidated
**separately, never as one convex hull**: when `geom_row > k_row` the hull spans rows this probe never
wrote, and a bare `DC IVAC` over those would discard another writer's still-dirty lines — manufacturing
the exact garble we are hunting. The one-shot latch is set only by a pass that produced a verdict, so a
transient skip cannot silence the witness for the boot.

`witness`-gated **and** aarch64-only: knob-off, `video/wcf.rs` does not compile, the flashable Pi media
are byte-identical (zero `wc-f` strings), every x86 artifact untouched.

Firmware is not re-queried from composite context — the property mailbox has one unlocked static buffer
and is safe only during single-core boot — so the read happens at bring-up and is recorded
(`mailbox::scanout_truth`).

#### WC-F gate results (2026-07-25, QEMU raspi4b, forced bench geometry)

`./arroyo check` green both arches · `./arroyo kernel8` clean, zero `wc-f` strings (armed build: six) ·
`UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 120` → MBENCH **55/55 required, 0 forbidden**, on a
spec grown by this arc's two REQUIREs. Both verdicts PASS.

A QEMU PASS carries no news and is not the arc's evidence: QEMU allocates unpadded, so `geom_row == k_row`
there and the divergent branch is unexercised by construction. What the gate establishes is that the probe
runs, addresses both paths, counts what it could not check, and states a verdict — so a bench divergence
appears as a number rather than as silence. **The evidence is the metal boot.**

Carried forward from WC-E and still unactioned: the P58 boot reported `alpha=0` on metal where QEMU reports
`alpha=2` (ignored). `[wc-f] scanout` repeats the field so the divergence stays on the wire, but this arc
does not act on it — alpha mode cannot shift a row's phase, and the compositor composites opaquely.

### CURSOR-1 — the system cursor

The Pi 4 mouse pipeline reached the shell in PIUSB-39 / UVUG-10 (reports decoded, routed, `drop=0`,
click-to-exit working on metal) with **no cursor on the panel**: the operator was pointing blind. The
sprite code existed — `pal::cursor` has owned an arrow, a hot spot and an auto-hide clock since
CURSOR-HIDE — but it draws through a `GneissPal`, i.e. into whatever surface the caller owns, and on the
windowed desktop that surface is the render task's `Screen` **back buffer**. `wm::composite` blits windows
straight into the front framebuffer, so a back-buffer sprite is on top of the console only, until the next
present. `video/cursor.rs` draws where "on top" is structural instead: into the front framebuffer, as the
last painter of every pass.

**The split of duties is the point.** `pal::cursor` remains the pointer's STATE — position (`move_rel` /
`set_abs`, fed from the same `pal::EVENT_QUEUE` drain the `[uvug10]` counters watch, clamped to the panel)
and visibility (`visible()`: some report has ever arrived, and the last one is younger than the ~1.5 s
auto-hide). `video::cursor` is only a painter, and reads both. `click1_dispatch`'s hit test keeps reading
`pal::cursor::pos`, and the sprite's box ORIGIN is that same position — the arrow's tip is the box's top-left
pixel — so the click lands where the tip is drawn. That equality is why the sprite is **clipped** at the panel
edge rather than shifted inward: shifting the box to keep it whole would move the drawn tip up to `side - 1`
px away from the hot spot the click uses, and near the right or bottom edge the operator would be aiming with
an arrow that is not where the click goes. Clipping draws less of the tail and keeps the tip exact.

The arrow SHAPE is shared with `pal::cursor`; the SIZE is not. `pal::cursor` magnifies by `scale + 1` (a
deliberate step above the text scale for a full-screen demo), `video::cursor` by `scale`, which makes the
desktop sprite exactly one glyph cell. The two never coexist — a demo owns the screen while it runs — so this
is a size change across modes, not two cursors at once.

**Three calls, and everyone who paints uses them.** `undraw()` restores the pixels under the sprite and
forgets them; `repaint()` is `undraw()` then save-under-and-draw; `armed()` is the witness latch. The
callers are exactly the front-framebuffer painters:

| painter | bracket |
|---|---|
| `wm::composite` | `undraw()` before the table lock, `repaint()` after the last `draw_window`/`verify_window` — the pass body is a private `composite_inner`, so every early return keeps the ordering |
| `wm::erase` (desktop repaint on close) | `undraw()` first; the `composite()` each caller runs next redraws |
| `render_service` (Pi) | `undraw()` / `repaint()` around `pal.render()` (`Screen::flush` overwrites the front buffer's damaged rects); `repaint()` on each real pointer report; `undraw()` on the auto-hide edge |
| `fbcon` (boot console mirror) | **no bracket, by exclusion** — `fbcon::detach()` at GUI takeover stops it painting the framebuffer, and it is out of the GUI's life cycle thereafter. A panic re-attaches it, which is a state where a stale cursor patch is not the problem to solve. This is the one front-buffer painter that is ordered by *not running* rather than by a bracket, and the reason both of this module's `serial_println!`s are emitted with the sprite lock released |

Composed with **WC-E** the bracket count goes up sharply: `Screen::flush` now calls `wm::repaint` → `composite`
on every desktop present, so the cursor is undrawn and redrawn ~20×/s on the bench even with the pointer at
rest. That is what forced the save-under to read back only the pixels the arrow PAINTS (~50 at scale 1) rather
than the whole 36×36 box (1296 `read_pixel` calls/frame). The composed per-frame cost of the save-under is a
**bench-read item** — it has never been measured on hardware.

**Damage: save-under, sized by the metrics.** The sprite is 8×8 blocks at `Metrics::scale`, i.e. exactly
one glyph cell (`cell_w`×`cell_h` — the derivation `ui.rs` already states for the text cursor), plus one
block of drop shadow: 36 px square at the 4× cap and nothing named in pixels anywhere (THE METRICS RULE).
Only the pixels the arrow PAINTS are saved with `FrameBuffer::read_pixel` before drawing and written back on
`undraw` — the rest of the box was never modified, so restoring it would be a write, and a race, with nothing
to fix — and only its scanlines are cleaned for the non-coherent HVS. The alternative — marking the box damaged and driving a
desktop+window recomposite — would run a composite pass at HID report rate (~125 Hz) to move an arrow three
pixels. Pointer motion therefore does **not** set the render loop's `dirty` flag at all: the sprite bypasses
the `Screen` and its flush entirely, which also removes the pre-CURSOR-1 erase-to-`0x1E1E1E` (neither the
desktop colour nor on top of a window).

A panel whose format has no colour inverse (`read_pixel` returns `None` — the lossy `U8` layout) **disables
the cursor for the boot** with a one-line reason, rather than restoring pixels it could not read. Fail-closed:
no cursor beats a trail of wrong pixels across the desktop.

**The race is the RESTORE, not just the sprite — say it plainly.** The front framebuffer has no single owner,
and WC-E makes the collision routine rather than exceptional: the compositor repaints the window layer on
every desktop flush, from another core's syscall context. The dangerous ordering is not "a window covers the
arrow" but its inverse — the render task draws the sprite at P and saves PRE-window pixels; the compositor
then draws a window over P and verifies it; the compositor's own bracket then undraws, and a naive restore
**stamps those pre-window pixels back INTO the verified window rect**. Nothing would repair it (a composite
repaints damaged windows, not arbitrary rows) and a later `[wc-d] verify` could read it as `bad_cache > 0` —
a line the Pi spec FORBIDs. Three mechanisms, together, close it:

1. **Atomicity.** Every entry point holds the sprite lock across the whole restore → save → draw sequence. The
   earlier split (`repaint` calling a self-locking `undraw`, then re-locking to save) left a window in which
   another core's draw could be captured AS the save-under — which would stamp a permanent white arrow into
   whatever was beneath it.
2. **A colour-guarded restore.** A saved pixel is written back only if the framebuffer still holds the exact
   colour the sprite painted there. A window drawn over the sprite fails that test, so its pixels are left
   alone: the restore cannot touch a rect another painter has taken.
3. **Damage repair.** Whatever rect a restore touched is handed to `wm::damage_intersecting`, which marks every
   overlapping window damaged so the next composite redraws it from its source surface. This closes the colour
   guard's residual hole — a painter whose new content happens to be exactly the sprite's own `FILL` or
   `SHADOW` is indistinguishable from the sprite. It marks only, never composites (composite brackets this
   module; compositing from here would recurse), so the repair lands within one WC-E frame.

Lock order is stated once and must hold: **`SPRITE` → `TABLE`, never the reverse** — nothing in `wm` calls
into the cursor while holding the window table, and the repair runs with the sprite lock released.

**Why no checksum or witness moves — two independent reasons.** *Ordering:* no verified pixel is ever read
with the sprite on the panel (`composite` puts it back only after the last `verify_window` returns), and
`[wc-c]`'s checksum hashes the SOURCE surface, which this module never touches. *Arming:* the sprite is drawn
only while `pal::cursor::visible()`, which requires a real pointer report — QEMU raspi4b delivers no HID
pointer input, so on the gate this code writes zero pixels and prints nothing for the whole boot. The
`kernel8-test` capture and every `[wc-c]` / `[wc-d]` / UVUG checksum are therefore unchanged by construction,
which is what the gate below confirms.

Witness, printed once, at the first draw of the boot (input-driven, so quiet boot is preserved):

```
[cursor] armed x=320 y=240
```

#### CURSOR-1 gate results (2026-07-25, QEMU raspi4b)

`./arroyo check` green both arches · `kernel8` builds · `kernel8-test 120` MBENCH **50/50 required,
0 forbidden**, with every pre-existing checksum and witness byte-identical (the arc adds no spec directive:
`[cursor] armed` cannot fire without a pointer device, and a REQUIRE for it would be a line the gate can
never satisfy) · `test-arm` green.

The **evidence that the cursor is inert on the gate** is the strongest form available off-metal: the
`target/pi-screen.png` capture is sha256
`2686a884320dbc389d6c33b1f37b097fa15eba769b51a751449e2c91a986bc19` — bit-identical to the WC-C baseline —
and `[wc-c] win=… cksum=`, `[wc-d] verify … -> PASS`, `:: UVUG: frames=300 … checksum=0xe68285b85121ac7c ::`
and the compat `mapped=32x32 … checksum=0x8d99530ca96d4b25` are unchanged. Not one `[cursor]` line appears.
What QEMU therefore cannot exercise is the sprite itself: the first draw, the save-under restore and the
`[cursor] armed` line are **unverified until the next bench boot**, exactly as WC-D's `bad_ram` column is.

#### FLICKER-2 — the two P79 flickers: one fixed, both instrumented

P79 (bench, storm 6, mouse otherwise good) reported two residual flickers: (a) a slight cursor
flicker on a ~5 s "pulse", and (b) an occasional flicker of the vug window *under* the pointer.

**Symptom (b) — root cause found and fixed: restore-before-install.** A session-owning pass updates
`sp.saved` (the sprite's save-under) only at its tail (`adopt_overlay`'s coverage install). Between
`compose_into` delivering the arrow inside a window's freshly presented rows and that install, the
covered pixels' true under-content — the window's *new* frame — exists only in the session's layer
save (`ov.saved`); `sp.saved` still holds the previous frame. Any full undraw arriving in that window
(a pointer move's `repaint` on another core, `wm::erase`, or the WC-L drain) found the panel holding
the arrow's colour there, passed the colour guard, and restored `sp.saved` — stamping last frame's
window content into a live window for one frame. The interleave fires at the `[cursor5] adopt_incoh`
rate (~1/s on the P79 capture, mouse motion being the usual trigger); the visible subset is the
occasional flicker. `undraw_locked` now consults the open session under `SPRITE → OVERLAY` order
(`try_lock`, never blocking a `compose_into` inside the blit guard): if the session coherently
describes the live sprite (epoch + geometry, the `adopt_overlay` predicate), covered pixels restore
from the layer save. `[flick2] sess_undraws=`/`sess_px=` count the fixed path running; `sess_lockmiss=`
counts the bounded `try_lock` fallback.

**Symptom (b), second mechanism, also removed: the drain's whole-sprite bracket.** `drain_deferred`
took the FULL sprite down before painting any deferred erase box, wherever those boxes were — so a
deferred erase anywhere on the panel cost a whole-sprite restore→repaint over the window under the
pointer, each restore an extra roll of the colour-guard residual. The drain now tests its queued
boxes against `sprite_box()` first: disjoint boxes leave the sprite entirely alone
(`[flick2] drain_skip=`), intersecting ones take the masked `undraw_within_nosession` handback
(`drain_masked=`), whose generation bump already protects a concurrent core's open session.

**Symptom (a) — instrumented, not provable off-metal.** The `[wcn]` rollup burst (measured 5003–5005 ms
cadence on the P79 capture, matching the reported pulse) emits *between* passes, holds no compositor
lock (`wcn_emit`'s `TABLE` snapshot drops before its first print), and SERWIT-1's staging ring means a
contended core stages and returns rather than blocking. What remains is wall time on the *winning*
core: each line is ~13 ms of IRQ-masked 115200-baud polling, the holder also drains up to 64 staged
lines, and the timer-IRQ witness sites (`[pulse5]`/`[spread4]`/`[prio]`) can fire on a core that is
mid-composite with the arrow off the glass — stretching that bracket by the whole burst. QEMU cannot
show any of this (no drawn sprite, instant UART), so the next bench boot reads it directly from the
`[flick2]` line: `down_max=` (longest full-undraw→draw interval per rollup window), `down_slow=`
(intervals ≥ 20 ms — a visible blink), `down_last_at=` (monotonic ms of the last one, to place against
the nearest burst), and `burst_last=`/`burst_max=` (the `[wcn]` block's own measured wall time). If the
flicker rides the pulse, `down_slow` events cluster at burst positions with `down_max` ≈ `burst_last`;
if both read low while Peter still sees the pulse flicker, the cause is outside the bracket path and
the next suspect is the vug present cadence itself.

Gate scope, stated plainly: `[flick2]` reads `UNWITNESSED` on QEMU by construction (the sprite is
never drawn), so the QEMU gates prove no-regression only; both verdicts above are argued from the
interleave analysis and await the next attended bench boot for pixel-level confirmation.

#### FLICKER-3 — the P80 residuals: the flush bracket and the masked stale restore

P80 (bench, attended, FLICKER-2 aboard and improved) reported two residuals: (a) "the core idle
bars cause mouse to flicker when they move", and (b) "vug can still get disturbed by the mouse",
occasionally.

**Symptom (a) — the CURSOR-13 desktop bracket was unconditional.** The status strip's per-core load
bars are desktop furniture: `ui_status` paints them into the `Screen` back buffer and marks their
one-line band as damage, and the render task's `pal.render()` → `Screen::flush()` presents it.
`flush` opened the CURSOR-13 bracket — a FULL `cursor::undraw()` before `present_background`, a
full `cursor::repaint()` after — on every present, wherever the damage was; so every bars repaint
(~1/s idle, faster when loads move) took the whole sprite off the glass and redrew it, and the
operator saw the arrow blink in time with the bars. Same shape FLICKER-2 removed from
`drain_deferred`, one layer up. `Screen::flush` now asks `bracket_needed()` first: a present whose
damage rects are all provably disjoint from a live, visible sprite skips the bracket entirely and
runs with the arrow on glass. The skip is deliberately narrow — no sprite on glass (the repaint may
owe a recovery draw), a lapsed CURSOR-HIDE visibility (this bracket's repaint is what takes a
timed-out arrow down), and a pending `FULL_PRESENT` all keep the unconditional bracket.
`present_background`'s CURSOR-6 probe (`desktop_over=`) now tests per-damage-rect overlap on both
arms and doubles as the detector for a skip decision the sprite outran (moved into the damage
mid-present — re-established by the mover's own `repaint`, exactly the `drain_deferred` argument).
`[flick2] flush_undraw=`/`flush_skip=` count the live-sprite decision; with the pointer parked away
from the strip, `flush_skip` should dominate.

**Symptom (b) — the masked undraw restored a stale save.** The P80 wire ruled the FLICKER-2
`try_lock` fallback out (`sess_lockmiss=0` across the whole attended phase) while `[cursor5]`
climbed steadily (`stale_compose=1468`, `adopt_incoh=1386`, `masked_nosession=68`): concurrent
passes were routinely active inside a session owner's compose-to-install window. FLICKER-2 gave the
session-fresh restore to `undraw_locked` (full undraws) only; `undraw_within_locked` — the masked
path behind `undraw_within_nosession`, i.e. the sessionless composite arm and the WC-L drain —
still restored `sp.saved` unconditionally. Inside that window a covered pixel's `sp.saved` is last
frame's window content, the colour guard passes (the session owner's present put our colour there),
and the masked undraw stamped the stale pixel into the live vug under the pointer — occasional
because it needs the cross-core interleave, which is P80's (b) exactly. The masked path now takes
the same `SPRITE → OVERLAY` `try_lock` and restores covered pixels from the session's layer save;
a contended read falls back for one undraw and feeds the shared `sess_lockmiss`. The caller's
conditional generation bump is unchanged — a handback still retires the owner's session.
`[flick2] mask_sess=` counts the lifted path running; `sess_px=` now aggregates layer-restored
pixels from both entry points.

Gate scope: unchanged from FLICKER-2 — `UNWITNESSED` on QEMU by construction; both fixes are argued
from the wire correlation plus interleave analysis and await the next attended bench boot. One
reading note for that boot: `down_max=`/`down_slow=` include CURSOR-HIDE spans (a full undraw with
no draw until the next pointer report — the P80 capture's 150 s `down_max` is a parked pointer, not
a blink), so judge (a) by `flush_skip` dominating and by the chair, not by `down_max` alone.

### FOCUS-VIS — focus you can SEE, and a shell you can read

P59 (bench, 2026-07-25) put two backgrounded windows on the panel — the UVUG crystal and `STAT.ELF` —
and produced two observations that turn out to be one defect:

1. **TAB provably cycled focus and the panel never moved.** `[wc-c] focus tab-cycle 0->1->2->0 (ring of
   2 + shell)` fired on every press, and `STAT` stayed entirely covered by the UVUG window.
2. **The shell was unreadable.** With windows up, the console — prompt, command line, command output —
   was underneath them. The operator could type and could not read.

The common cause: **focus was a pure input-routing fact.** `user_input_set_active` moved where keystrokes
went and nothing else, and the shell had no position in the z-order at all — it was the surface the
window layer got painted *onto*. Nothing raised a focused window, and nothing could put the console in
front of anything, because "in front of the console" was the only thing a window could be.

#### The shell is a member of the z-order

`wm::SHELL_Z` is the shell's z, allocated out of **the same monotonic `next_z` counter** every window
raise uses. That single fact makes both halves the ordinary comparison:

* a window with `z > SHELL_Z` is above the shell and composites normally;
* a window with `z < SHELL_Z` is below it and is **not drawn** — the console owns those pixels.

`SHELL_Z` starts at `0` ("the shell is at the very bottom"), which is exactly the pre-FOCUS-VIS
behaviour, so a boot that never TABs composites byte-identically to before. That is every QEMU gate run:
raspi4b has no HID.

#### One seam: `wm::focus_changed(asid)`

Called by `wc_focus_key` immediately after `user_input_set_active`, with `asid == 0` meaning the ring's
SHELL slot.

| `asid` | what happens |
|---|---|
| a window owner | **every** live non-compat window of that ASID takes a fresh z (above all windows *and* above the shell), is marked damaged, and the pass composites. All of the owner's windows, because the ring is keyed by ASID — raising a subset would leave an app half in front. |
| `0` (the shell) | `SHELL_Z` takes the fresh z. Every window is now below it: their outer boxes are erased to `DESKTOP_BG` **immediately** (instant response), and `screen::request_full_present()` asks the desktop layer to repaint the whole panel so the console's *text* comes back over that erase. |

`request_full_present` is a flag rather than a call because the `Screen` is owned by the render task —
there is no global handle, and inventing one would hand a second core a `&mut Screen`. The compositor
raises it from syscall context; `Screen::present_background` consumes it, on its own thread, before it
reads its damage set. Latency floor is the 1 Hz status-strip tick; the erase is what makes the response
look instant regardless.

Compat rows are exempt from the shell test (`above_shell`): a compat window IS the full-screen present
path, it carries owner ASID `0` and is not addressable as a focus target, so it could never be raised
back above a shell that overtook it — hiding it would strand a full-screen app's output permanently.

> **Residual, stated as a limitation.** The exemption is sound in its own terms and it is not free: a
> **backgrounded** compat (full-screen) app still composites over the console after a TAB to the shell,
> so the "read your command's output" case is only fully delivered for *windowed* apps. The foreground
> case is unaffected — a full-screen app run in the foreground parks the shell, so there is no prompt to
> read behind it — and this is the same shape as the residual BGRUN-1 already recorded on `wm::repaint`
> (a bg compat app shimmers while the operator is TABbed into some other app). The cause is shared and
> so is the fix: a compat row has **no owner ASID** to key on (the `SYS_FB_PRESENT` hook signature
> carries none), so it cannot be made a focus target, cannot be raised, and therefore cannot honestly be
> lowered either. Giving compat rows a real owner is a change to the `SYS_FB_PRESENT` seam, not to the
> z-order — flagged here rather than worked around, because every available workaround keys on something
> coarser than ownership and would strand a full-screen app's output in some other state instead.

#### Three smaller defects the same arc closes

* **`create` now composites.** A window's kernel chrome reaches the panel when the row exists, not at the
  owner's first present (which, for an app that maps a surface and then blocks, may be never). It also
  puts window creation *inside the cursor bracket*: before this, the first thing to touch the panel after
  a create was the owner's `draw_window` on another core, with the sprite down and its save-under holding
  pre-window pixels. Not on the compat path — `create_inner` is only the first half of `compat_present`,
  and a composite there would flash the surface once at the row's defaults (1× at the origin).
* **`move_to` now erases the box it vacates.** The compositor draws windows and never the desktop, so a
  moved window used to leave a full copy of itself — content, border and title strip — at its old
  position forever. Same treatment `close` gives a vacated box, because it is the same event.
* **The system cursor survives an app holding focus.** `route_input_to_active_el0` returns after routing,
  so the shell loop's `Mouse`/`MouseAbsolute` arms — the only code that moved `pal::cursor` and repainted
  `video::cursor` — were unreachable while any app had focus. The sprite froze and auto-hid 1.5 s later,
  and no amount of mouse movement brought it back. The router now updates the shared pointer state and
  repaints the sprite alongside delivery (delivery itself is unchanged), gated on
  `pal::cursor::has_reported()` so the boot-time `input_router_selftest`'s **synthetic** `Event::Mouse`
  cannot arm a cursor on a gate that has no pointer.

#### WC-D's one-shot needed a gate

`create` compositing means WC-D's per-window read-back latch would otherwise be claimed by the
create-time pass and verify a **blank** surface — a vacuous `-> PASS` that satisfies the spec's REQUIRE
while the app's real content is never checked. `Window::presented` (set by `present` / `compat_present`)
makes the verdict wait for content the owner actually put there.

#### The witness: `[wc-fv] focus-vis` — a READ-BACK, not a state dump

> **Tag:** `[wc-fv]`, not `[wc-g]`. `WC-G` belongs to the concurrent garble arc (`video/wcg.rs`); the
> regexes would not cross-match, but two subsystems answering to one letter in the ledger is a cost paid
> at every future integration rather than once here.

Every pre-existing focus witness is a statement about kernel state, and `[wc-c] focus tab-cycle` printed
correctly on the bench for a panel that never changed. `wm::focusvis_selftest` never asks the table who
is in front; it asks the **framebuffer what colour is actually there**. Two solid-colour 8×8 windows are
placed at the SAME origin, so exactly one of them can own the probe pixel:

| leg | action | expected pixel |
|---|---|---|
| `stack` | B created after A | B's colour (baseline) |
| `raise` | `focus_changed(A)` | A's colour — **defect 1** |
| `shell` | `focus_changed(0)` | neither colour — the window layer stopped owning those pixels |
| `reraise` | `focus_changed(B)` | B's colour — the shell is a POSITION in the rotation, not a terminus |

Self-cleaning (both windows closed, boxes erased, `SHELL_Z` returned to 0), placed upper-middle so it
cannot collide with WC-F's reserved probe boxes at the bottom edge, and run from the tail of
`wcb_launcher` so it cannot burn the one-shot `[wc-c] side-by-side` / `[wc-d] verify` latches.

```
[wc-fv] focus raise asid=0xf0a windows=1 top_win=1 z=7 shell_z=0
[wc-fv] focus shell z=8 hidden=2 exempt=0
[wc-fv] focus raise asid=0xf0b windows=1 top_win=2 z=9 shell_z=8
[wc-fv] focus-vis at (641,314) a=0xff2020 b=0x20ff20 stack=0x20ff20/true raise=0xff2020/true shell=0x2d2b55/true reraise=0x20ff20/true -> PASS
```

The `shell` leg reads `0x2d2b55` — `DESKTOP_BG` exactly — which is the erase landing, i.e. the window
layer genuinely stopped owning those pixels rather than merely being reordered among itself.

##### FV-EXEMPT — `hidden=` has been counting rows that were never hidden

The shell arm collects its erase set with `r.z < z`. That is **not** the hiding predicate:
`above_shell` is what decides whether a row stops compositing, and it **exempts compat rows** on
purpose — a compat row is the full-screen present path, carries owner ASID 0, is not a focus target,
and could never be raised back over a shell that overtook it, so hiding it would strand a full-screen
app's output for the rest of the boot. Its `z` still falls below the shell, so it is collected,
erased to `DESKTOP_BG`, and then repainted by the `composite` at the end of the same call — having
never been hidden at all.

Reachable, and on the desktop path: a **background** full-screen app (`bg` — the BGRUN-1 case) is on
the panel while the operator TABs to the shell. The visible cost is a whole-box desktop fill followed
by an immediate repaint of the same pixels; the durable one is that `erase` may **defer** that fill
under `STAGE` contention (WC-L), in which case the drain paints `DESKTOP_BG` over the live app one
pass *after* the repaint has already put it back.

`exempt=` is that contradiction on the wire — the part of `hidden=` that was erased-and-repainted
rather than hidden. It is derived from `above_shell` rather than from `r.compat`, so a future
exemption added there is carried automatically. `hidden=N exempt=0` is the reading the field always
implied.

**Counted, not changed.** Narrowing the erase set to `!above_shell(r, z)` is a one-token edit and is
deliberately not taken: what a bg full-screen app's pixels should do across a shell TAB is a panel
question, the headless gate has no HID to TAB with, and it interacts with BGRUN-1's two-writer
shimmer. The gate reads `exempt=0` (no compat row is live when the fixtures TAB), which is the honest
headless outcome and not evidence either way — the bench sitting is where this reads non-zero.

#### FOCUS-VIS gate results (2026-07-25, QEMU raspi4b @ `UNAOS_FBW=1920 UNAOS_FBH=1200`)

`./arroyo check` green both arches · `kernel8` builds clean · `kernel8-test 60`
**57/57 required, 0 forbidden** (56 → 57: the one new `[wc-fv] focus-vis` REQUIRE, with a matching
FORBID).

> `./arroyo test-arm` does **not** build at this tip, and not because of this arc: the aarch64-virt
> profile is `witness` **without** `baremetal`, and `video/wcf.rs` is gated on
> `all(target_arch = "aarch64", feature = "witness")` while importing `arch::aarch64::mailbox`, which is
> gated on `baremetal`. Both files are untouched by FOCUS-VIS (last changed by WC-F, `49e7d5f5`);
> recorded here rather than fixed, since the fix is a WC-F cfg correction outside this arc's lane.

Every pre-existing witness is byte-identical: `:: UVUG: frames=300 threads=2
checksum=0xe68285b85121ac7c ::`, `:: EL0: window verbs — … witness=0x1fff …
checksum=0xfabe809492cf2325 :: PASS ::`, the compat `mapped=32x32 … checksum=0x8d99530ca96d4b25`,
`[wc-c] side-by-side windows=2 drawn=2`, and every `[wc-d] verify … bad_cache=0 bad_ram=0 -> PASS`
including the two the selftest's own 8×8 rows now add. No `[cursor]` line appears — the router
keep-alive is `has_reported()`-gated, so the synthetic router-selftest `Event::Mouse` still arms
nothing on a gate with no pointer, exactly as CURSOR-1 promised.

#### The bench operator test (what must pass on the next boot)

`bg` two apps → TAB to the shell, **type a command and read its output** → TAB to a window, the window is
visibly front → the cursor is visible throughout. The raise, the shell raise and the cursor's survival of
a window create are all gate-checkable; the *legibility* of the console under the full present, and the
sprite itself (QEMU has no pointer), remain bench-only.

### FOCUS-HL — the focused window's chrome says so

FOCUS-VIS made focus *positional*: the focused window raises, the shell raises above everything. That is
the right primitive and it is not, on its own, an answer at the panel. With two windows that do not
overlap — which is the normal `bg` × 2 layout, side by side — raising the focused one moves nothing
visible, so the operator still cannot tell which window has the keyboard without typing into it.

Chrome is the natural carrier, because it is already kernel-drawn and already repainted on every present
(see WC-A: an app draws only inside its surface, the border and title strip are the compositor's). So the
indicator costs no new pixels and no new pass.

* **State.** `wm::FOCUS_ASID` — the ASID that holds focus, `0` for the shell. Written only by
  `focus_changed`, which is already the single seam every focus move passes through, so there is no
  second place for the compositor's idea of focus to diverge from the router's.
* **Read once per pass.** `composite` snapshots `FOCUS_ASID` alongside `SHELL_Z`, for the same reason:
  one pass judges every window against one focus owner, so a pass structurally cannot paint two
  highlights.
* **Two colours, no geometry.** `draw_window(fb, r, focused)` swaps `CHROME_BORDER` →
  `CHROME_BORDER_FOCUS` (`0x8C8CB4`) and `CHROME_TITLE_BG` → `CHROME_TITLE_BG_FOCUS` (`0x3A3A5A`). The
  `outer_box` is unchanged, so focus never moves a pixel — no reflow, no re-damage of neighbours beyond
  what focus already causes, and the highlight is free relative to a present that was going to fill those
  rects anyway. The colours stay in the flat, deliberately un-host-like family WC-A chose: this marks
  focus, it does not imitate a title bar.
* **Shell focus highlights nothing.** `focus == 0` matches no window, which is the honest reading — no
  app has the keyboard. The explicit `!= 0` test also stops a compat row (owner ASID `0`) matching by
  accident, though a compat window draws no chrome to highlight in the first place.
* **The losing window repaints too.** `focus_changed` already damages the windows it *raises*; it now
  also damages the windows of the ASID losing focus. That set is disjoint from the raise (and empty on
  the shell branch, which raises nothing), so without it the previous holder would keep the bright chrome
  until something unrelated damaged it — the same "both ends repaint" property FOCUS-VIS established for
  position, now extended to colour.

Damage is still focus-change-scoped: no per-frame cost, and a boot that never TABs — every QEMU gate run,
since raspi4b has no HID — composites byte-identically to before, which is why the existing surface
checksums are untouched.

**And that is also the limit of what the gate can say about this fold.** raspi4b has no HID, so no QEMU
run ever changes focus, so no QEMU run ever draws the highlight. The gate proves the change is *inert*
where it should be inert (59/59, every checksum unchanged); the highlight itself is **bench-only** and
unverified. The bench test is the FOCUS-VIS recipe with one addition: `bg /fat/VUG.ELF` → `bg
/fat/STAT.ELF` → TAB, and at each stop the front window's border and title strip must be visibly
brighter than the other's, with **neither** highlighted at the shell stop.

#### VUG/STAT gate results (2026-07-25, QEMU raspi4b @ `UNAOS_FBW=1920 UNAOS_FBH=1200`)

`./arroyo check` green both arches · `kernel8` builds clean, staging `VUG.ELF` (12568 B) and `STAT.ELF`
(8472 B) under their new names (the persistence app was `KVUG.ELF` at that arc; STAT-NAME later restored
`STAT.ELF`, byte-identical at 8472 B) · `kernel8-test 60` **59/59 required, 0 forbidden** (unchanged — this arc
adds no spec pattern and renames none) · `./arroyo test-arm` reaches
`>>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<`.

> Correction to the FOCUS-VIS note above: **`./arroyo test-arm` does build and pass at this tip.** The
> `wcf.rs` / `mailbox` cfg mismatch recorded there was fixed by a later arc; the note is kept for the
> history but should not be read as a live defect.

The renamed load paths are witnessed in the log rather than inferred:
`:: EXEC-UVUG: run /fat/VUG.ELF — loaded 12568 bytes, entry 0x300000, exit=0 -> PASS ::`, all three
`BGRUN-ST` legs PASS, and `:: UVUG: frames=300 threads=2 checksum=0xe68285b85121ac7c ::` is byte-identical
to before the arc.

**VUG-BG was verified by measurement, not by reading the diff.** A temporary probe in `BGRUN-ST`'s
kill leg dwelled 4 s before killing the backgrounded `VUG.ELF` and printed
`:: TEMP-VUGBG: after 4s bg VUG alive=true ::`. Four seconds is several times the ~1 s that same boot
takes to run 300 frames in the foreground, so "still running" cannot be "has not finished the auto path
yet". The probe was reverted and the gate re-run clean on the reverted tree.

### WC-G — the window-path garble, localized

WC-D, WC-E and WC-F between them exonerated everything global. On silicon the scan-out identities are all
true, both twin blocks are clean at 8192/8192, and the ramp photographs straight. And still a live 128×128
window garbles, in horizontal bands, less badly when the app cycles faster.

Every instrument in that chain shares one property: it measures **converged** content. A one-shot
read-back, a static twin, a photographed ramp — each samples after the writing stopped. A window that
repaints forever never converges, and that is exactly the population that garbles. WC-G instruments the
non-converged case: the present path *while it is running*.

#### The instrument

Four checksums of the same surface taken at four moments around one blit, plus a read-back of what that
blit landed, plus the blit's duration:

| leg | when | a divergence means |
|-----|------|--------------------|
| `app` | at `SYS_WIN_PRESENT` entry, owner parked in the syscall | the frame the owner declared finished |
| `blit` | immediately before `draw_window` reads it | `app != blit` — the surface moved between present and copy |
| `civac` | after `DC CIVAC` over the surface | `blit != civac` — **coherency**: the kernel's lines did not match the coherent view |
| `after` | immediately after `draw_window` returns | `blit != after` — **race**: the owner wrote it *mid-copy* |
| `fbbad` | source re-derived and compared against `read_pixel` over the content rect | non-zero — the **blit/upscale** path landed a wrong pixel |
| `us` vs `rectscan_us` | wall clock of the blit vs the beam's time on the window's own rows | `slow=yes` — the scan-out is **guaranteed** to overtake the copy inside the rect |

`blit != civac` and `blit != after` cannot both be explained by one mechanism, which is what makes the
coherency and race suspects separable rather than a shrug.

**Why `CIVAC` here, when WC-D was required to use a bare `IVAC`.** WC-D's rule was about the framebuffer,
which the kernel itself writes: cleaning before reading would have written the blit's own dirty lines out
and healed the short-flush defect the witness existed to catch. The surface is the mirror image — the
kernel only ever *reads* it, so there are no kernel-dirty lines to write back and `CIVAC` cannot repair a
compositor defect; what it can do is force the read from the coherent view, which is the question. A bare
`IVAC` would additionally risk discarding the owner's un-cleaned lines, destroying app data to answer a
question `CIVAC` answers safely. The rulings differ because the buffers differ.

**`own=`** records *why* the window was blitted. `own=yes`: the blit follows this window's own present, so
its owner is inside the syscall and cannot be writing. `own=no`: collateral repaint — the damage set is
closed upward over occlusion, so presenting window A repaints every higher-z window overlapping it, and
B's owner is running free at EL0 with nothing serialising it against the copy of its surface. `own=no`
with `blit != after` is source tearing caught in the act. Both cases occur in the gate.

**`rectscan_us`** is the threshold that matters, and it is derived, not chosen: the beam only has to cross
*this rect* to latch it part-old and part-new, so the criterion is `frame_us × rows_on_panel ÷
panel_height`, not a whole frame period.

Budgeted at four samples per window id (the checksums are 64 KiB reads and the read-back is one probe per
source pixel, from present context at EL0 frame rates — an unbudgeted version would perturb the timing it
reports). Per-window, not per-window-0, so the shared-path claim is provable on the wire. `witness`-gated
and aarch64-only like `wcf`: knob-off, `video/wcg.rs` does not compile and the flashable media are
byte-identical.

#### What the gate found

`UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 60`:

```
[wc-g] win=1 seq=1 own=yes scale=4x app=0xa1cf…8449 blit=0xa1cf…8449 civac=0xa1cf…8449 after=0xa1cf…8449 fbbad=0/16384 us=12808 rectscan_us=7111 slow=yes -> CLEAN
[wc-g] rollup win=1 scope=window samples=4 coher=0 race=0 blit=0 clean=4 slow=4 maxus=13052 frame_us=16667 -> CLEAN+SLOW
[wc-g] win=2 seq=1 own=no  scale=4x … fbbad=0/4096 us=3060 rectscan_us=3555 slow=no -> CLEAN
```

All three named suspects are **negative**: no `civac` divergence, no source race (in either the `own=yes`
or the `own=no` case), no blit-path or upscale error — `fbbad=0` over every probe. Every byte was correct
at every moment.

`coher=0` should be read narrowly. The compositor's read is a normal cacheable read of Normal
Inner-Shareable memory and the caches are PIPT, so against another core's cacheable writes to the same PA
it is coherent *by construction* — that was never in doubt and needs no witness. What the `civac` leg
tests is the part that is not guaranteed: an **alias-attribute mismatch**, the surface reached through two
mappings whose attributes or shareability disagree (the EL0 `user_data_page` leaf vs the kernel identity
leaf), or a non-cacheable alias in the chain. So `coher=0` means "no alias-attribute mismatch on this
surface", not "coherency is fine in general".

The residual is timing, and the code says why. `draw_window` writes **per-pixel, with `put_pixel`, directly
into the front framebuffer** — the live scan-out — with no vblank synchronisation anywhere in the path.
The desktop does not: it reaches the panel through `Screen`'s back buffer and a contiguous per-row
damage-rect flush, which is why direct desktop writes are clean and only windows garble. That asymmetry is
structural and is the finding; it does not depend on any measured number.

The number puts a size on it, and needs its provenance stated. **The machine was QEMU raspi4b at forced
bench geometry (`UNAOS_FBW=1920 UNAOS_FBH=1200`) — not silicon.** There, a 128×128 window at 4× copies for
13.3 ms against 7.1 ms of beam time on its own rows: a **1.9× overtake**. That figure is QEMU wall clock
and it is not stable across runs — a lens re-run of the same build measured **1.71×**. What both runs
agree on, and all the arc claims, is that **the ratio exceeds 1**, which is the condition for the scan-out
to overtake the copy inside the rect and latch it part-old/part-new at whatever scanline the beam held —
a horizontal band boundary, the shape in the photograph. It also explains "cycling faster looks a little
better": a faster cycle does not remove the tear, it shortens the interval any one torn frame stays on the
panel. The 64×64 window sits right at its own threshold (3.2 ms vs 3.6 ms), consistent with smaller
windows looking better.

**On an A72 the ratio may fall below 1**, and if it does, that falsifies *the number*, not the mechanism:
an unsynchronised per-pixel copy into a live scan-out tears whenever it loses the race, and a ratio under
1 on metal would mean only that it loses less often than QEMU suggested — the bench photograph already
establishes that it loses. `rectscan_us` is itself conservative in the same direction: `frame_us` includes
blanking while `ph` counts only visible lines, so the rect is credited with beam time the beam does not
spend on it, every `slow=yes` understates the problem, and a `slow=no` near the threshold may still tear.

WC-G fixes nothing — it is the localization. The fix is a design question for the next arc (double-buffer
the window layer, or bound the copy under the rect's scan time), not a knob.

#### WC-G gate results (2026-07-25, QEMU raspi4b, forced bench geometry)

QEMU raspi4b, forced bench geometry — **no metal in this arc**.

`./arroyo check` green both arches · `./arroyo kernel8` clean, **zero `wc-g` strings** knob-off (armed
build: two) · `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 60` → MBENCH **58/58 required,
0 forbidden**, on a spec grown by this arc's two REQUIREs and three FORBIDs. The REQUIREs assert that
the instrument *ran*, deliberately not what it found: the finding is the arc's output.

**There is no global summary line, and that is the result of three failed attempts rather than an
omission.** (1) Fire when the first window spends its budget: printed the summary *before* window 2 was
sampled at all, including before its `own=no` collateral-repaint sample — a metal race there would have
sat on the wire underneath a green summary contradicting it. (2) Fire when every *sampled* window has
spent its budget: the same bug in new clothes, since the sampled set only holds windows seen so far, so it
is trivially true the instant the first window finishes; it reproduced exactly, printing
`scope=exhausted samples=4 windows=1` before window 2 existed. (3) Fire on quiescence: the gate's two apps
start more than 3 s apart, so an idle gap is not evidence that sampling is over — it fired early too, at
`idle_us=3011902`.

The lesson is structural, not a tuning problem: **nothing observable inside a boot distinguishes "sampling
is finished" from "the next app has not launched yet."** Any global summary is therefore a completeness
claim the instrument cannot support, and one that overstates its scope is worse than none — it is the one
artifact that can make later contrary evidence look already accounted for.

So the rollup is scoped to **one window** and fires when that window spends its budget: deterministic, no
timer, and its scope is exactly what its `win=` says. The job the global line was reaching for — "no
suspect fired anywhere, ever" — moved to the spec as **FORBIDs on `-> COHER`, `-> RACE`, `-> BLIT`**. A
FORBID needs no completeness claim: it catches an anomaly in any window at any point in the boot, including
one appearing long after every rollup has printed. `CLEAN` and `CLEAN+SLOW` stay green, because the timing
finding is this arc's result and not a regression.

Unlike WC-F, the QEMU result here is **not** a formality. The three checksum legs are cache- and
race-sensitive and QEMU models neither caches nor the HVS, so their `CLEAN` verdict on the gate is weak
evidence and must be re-read on metal — a coherency or race defect can only appear there. The timing leg is
the opposite: `us=` is measured against `CNTVCT` on the same per-pixel code path the bench runs, and metal
is expected to be **slower**, not faster (real non-coherent framebuffer stores plus a real `DC CVAC`
sweep). The overtake the gate measures is therefore a lower bound on the bench's.

### WC-H — the window layer gets a back buffer

WC-G ended with a verdict and no fix: `coher=0 race=0 blit=0 clean=4 slow=4 -> CLEAN+SLOW`. Every byte
of the surface was correct at every moment around the copy, the scan-out read-back matched the source
everywhere, and the panel still garbled — because the copy took `us=15524` while the beam crosses the
window's own destination rows in `rectscan_us=7111`. Being overtaken was not a risk, it was arithmetic.

The mechanism is not re-litigated here (see §WC-G). What matters for the fix is *where* the copy landed:
`draw_window` wrote the **live front framebuffer**, one `put_pixel` at a time, upscaled — at 4x, sixteen
separate bounds-checked stores per source pixel — into memory the HVS was scanning at that moment, with
no vblank synchronisation anywhere in the path. The desktop has never had this problem, and not by luck:
it draws into `Screen`'s cached back buffer and reaches the panel through a contiguous per-row damage-rect
flush. Two layers, one scan-out, and only one of them was buffered.

#### The design trade

Three routes were on the table.

**(a) Route window blits through `Screen`.** The "do it right" answer on its face, and structurally
blocked — not merely inconvenient. There is no global `Screen`: each render task constructs its own
(`main.rs` builds four, `video::witness` a fifth) over the shared `WRITER` framebuffer, and none is
reachable from the compositor. The compositor runs from the presenting task's syscall context on an
arbitrary core, while the render task that owns a `Screen` may be parked inside `dispatch_command` and
flushing nothing at all — which is exactly why `present_surface` never routed through it either. Taking
this route means promoting one `Screen` to a global, giving it a lock acquired from syscall context, and
making every window present wait on the desktop's frame cadence. Three structural changes, one of them a
new cross-subsystem lock on a hot path, to buy a property route (b) buys outright.

**(c) Beam-race avoidance.** Bound or schedule the copy against the scan-out position. Rejected as the
brief anticipated: it needs a scan-position source the Pi 4 path does not expose, it degrades rather than
eliminates (a copy that must finish inside a bounded window either fits or tears), and it would make
correctness a function of window size and CPU load.

**(b) A dedicated window back-layer — chosen.** Compose the whole window (chrome, title strip, upscaled
content) into a cached-RAM layer sized to its clipped outer box, then present that box as contiguous
full-width row copies followed by the same single `flush_range`. This is the *same discipline* as the
desktop path — bulk sequential `blit` per row onto the framebuffer, one cache clean over the span — with
no ownership change and no new lock. The layer is one reused buffer (`wm::STAGE`), grown on demand with
`try_reserve`, capped at 4 MiB.

Every decline falls back to the pre-WC-H direct path rather than failing: a compat row, a box over the
cap, another core holding the buffer (`try_lock`, never `lock` — a present must not queue behind another
core's copy), or an allocator that cannot grow it. A window can never be lost to the back-layer.

**Compat rows keep the direct path deliberately.** A compat window is the full-screen `present_surface`
shim; its box is the whole panel, so staging it costs a panel-sized allocation and a full extra panel copy
per present — on the evidence of the `repaint` compat exclusion that preceded it, enough to blow the
`EXEC-UVUG` frame deadline. It also does not need it: while a foreground full-screen EL0 program owns the
panel the render task is parked, so the two-writer contention this arc removes is not present. WC-F's
direct-path probe likewise still bypasses the compositor, unchanged and intentionally.

#### The upscale's vertical runs, written once

A nearest-neighbour upscale emits `scale` **identical** destination lines per source row. On the front
buffer there was no cheap way to exploit that; on the back layer the lines are contiguous byte runs at a
known stride, so the first is composed per-pixel and the rest are produced with one bulk copy each. The
compose phase roughly halved (win 1: ~21000 µs → ~6300 µs). The replication is applied **only** on the
staged path: on the front, copying a composed line would carry that line's existing pad bytes onto the
others (`put_pixel` writes 3 bytes of 4), so the fallback stays a pure per-pixel write and remains
byte-for-byte the pre-WC-H path. On the back layer every pad byte is 0 by construction — the same zero pad
`Screen`'s back buffer has always presented — so the copy reproduces exactly what the per-pixel loop
would have left.

The back layer carries no residue between windows because the chrome fill is dense over the whole clipped
box: every pixel the present copies was written by the pass that composed it.

One caveat on the byte-for-byte claim above, which is about the *fallback* path and not the staged one:
the staged present writes **4 bytes per pixel** (the composed colour plus a zeroed pad byte) where the
direct path wrote 3 and left the pad as it found it. That is not pixel-visible — the pad byte is ignored
by the HVS on an XRGB scan-out, and it is the same zero pad `Screen`'s back buffer has always flushed to
the front — and WC-D is unaffected, since `read_pixel` decodes the three colour bytes. But the staged
output is byte-identical to the direct output only in the colour bytes, not in every byte of the
framebuffer.

#### Declines are samples, not non-events

`stage_window` has four fall-back exits — box over the cap, `try_lock` lost, allocator refusal,
degenerate geometry — and each runs the direct, pre-WC-H path: the tearing regime. The first cut of
`[wc-h]` fired only on staged success, which made its verdict an overclaim. The failing scenario is
concrete, not hypothetical: if 96 of 100 composites lost the lock to a concurrent desktop flush (a ~6 ms
hold window, which is exactly the contention the `try_lock` exists to sidestep), the window would have
torn continuously and the four staged samples would still have printed `TEAR-FREE`, with the FORBID
never firing. The same blind spot would have hidden a window whose box exceeds the 4 MiB cap falling back
*permanently*.

So a decline spends sample budget, prints its own line with its reason, and forces the rollup verdict to
`UNSTAGED` — which the spec FORBIDs alongside `AT-RISK`. `UNSTAGED` is not a softer finding than
`AT-RISK`: it says composites reached the panel unbuffered, so the tear-free claim the staged samples
support does not describe what the window actually did. The cap fallback becomes loud for free.

```
[wc-h] win=1 staged=no reason=fixture -> DIRECT
[wc-h] rollup win=1 scope=window samples=4 torn=0 declines=0 fixture=1 maxpresent_us=1444 ... -> TEAR-FREE
```

#### The fallback fixture — coverage that was nearly traded away

Before WC-H, every `[wc-d] verify` read a directly-drawn window. Afterwards every one of them read a
staged present, and for non-compat windows the fallback path stopped being verified against the scan-out
at all (WC-D skips compat rows, which are the only rows still on the direct path in normal operation).
That is a real regression in coverage, and it would have been silent.

A witness-only global one-shot latch forces the *first* composite WC-D is about to verify onto the direct
path, armed on the same predicate `composite_inner` tests afterwards so the fixture and the verification
cannot land in different passes. The gate runs two windows, so exactly one is verified on each path — the
log above shows `win=1` verified after its `reason=fixture` decline and `win=2` verified on a staged
present. `fixture` is counted apart from `declines` (the kernel asked for it) and printed separately so
the exclusion is visible rather than assumed, and a REQUIRE asserts the fallback was actually exercised.

#### `[wc-h]` — the witness, and why WC-G's `slow=` was not re-scoped

`[wc-h]` splits the operation into the two halves that now mean different things:

```
[wc-h] win=1 box=514x526 bytes=1081456 compose_us=6075 present_us=1084 rectscan_us=7305 torn=no -> BUFFERED
[wc-h] rollup win=1 scope=window samples=4 torn=0 declines=0 fixture=1 maxpresent_us=1444 frame_us=16667 -> TEAR-FREE
```

`compose_us` happens off-screen where no scan-out can observe a partial result. `present_us` is the row
copies — the only phase that can still tear — and `torn=` compares *that* against the beam's time on the
box, computed exactly as WC-G computes `rectscan_us` and with the same deliberate bias toward **not**
reporting a tear (`frame_us` includes blanking the beam does not spend on visible rows, so the box is
credited with more beam time than it gets; a `torn=no` near the threshold is not a proof of safety).

WC-G's `us=`/`slow=` leg and all three of its FORBIDs are **unchanged**. Its bracket still contains the
copy and nothing else — `blit` is still the surface as the copy found it, `after` as the copy left it —
but the copy is now two phases, so `slow=yes` says "the whole operation outran the beam", most of which
the beam cannot see. It no longer implies a torn panel. Deleting or re-scoping the leg would have damaged
the only instrument separating a source race from a coherency fault to fix a sentence of interpretation;
the tear question moved to `[wc-h] torn=` instead, which is narrower and true.

**The witness had to be moved out of its own measurement.** The first cut printed the `[wc-h]` line from
inside `stage_window` — inside `draw_window`, inside WC-G's clock — so every serial character of it was
charged to `[wc-g] us=`, which rose from a baseline max of 15524 to 23468 on the same work while the
compose and present figures the line reported summed to about half that. The sample is now *recorded*
where it is taken and *printed* from `wcg::stage_flush`, which the compositor calls after `wcg::end` has
stopped the clock. Consequence, stated: there is one pending slot per window id, so two cores compositing
the same window concurrently lose one printed line — the per-sample lines are a best-effort trace and can
be fewer than the rollup's `samples=`. The rollup's counters are updated at record time and miss nothing,
which is why the tear assertion is pinned on the rollup verdict (`FORBID -> AT-RISK`).

#### WC-H gate results (2026-07-25, QEMU raspi4b, forced bench geometry)

QEMU raspi4b, forced bench geometry — **no metal in this arc**.

`./arroyo check` green both arches · `./arroyo kernel8` clean, **zero `wc-h` / `wc-g` / `BUFFERED`
strings** knob-off · `./arroyo test-arm` → xHCI MISSION SUCCESS · `UNAOS_FBW=1920 UNAOS_FBH=1200
./arroyo kernel8-test 60` → MBENCH **62/62 required, 0 forbidden** (59 before this arc → **62** with its
three REQUIREs; two new FORBIDs, `-> AT-RISK` and `-> UNSTAGED`).

Cost, on the wire, same geometry, before → after (`[wc-g] us=` is the whole copy; `[wc-h] present_us=`
is the panel-facing part of it):

| window | box | WC-G `maxus` before | WC-G `maxus` after | present max after | `rectscan_us` | verdict |
|---|---|---|---|---|---|---|
| 1 (crystal, 128x128@4x) | 514x526 | 15524 µs | 11294 µs | **1444 µs** | 7305 | TEAR-FREE |
| 2 (`stat.elf`, 64x64@4x) | 258x270 | 4088 µs | 3279 µs | **306 µs** | 3750 | TEAR-FREE |

The panel-facing exposure fell **~11x** (15524 → 1444 µs for the crystal) and now sits at **20%** of the
beam's time on the box, where before it was 218% of it. The total copy did not merely stay within a modest
budget — it got *faster*, because the row-run replication more than paid for the extra copy: window 1's
ceiling dropped from 15524 µs to 11294 µs, and window 2's `[wc-g]` rollup went from `slow=1` to `slow=0`
(`-> CLEAN`, no longer `CLEAN+SLOW`).

`[vugfps]`, honestly: **it does not appear on this gate and cannot**. It is emitted by the `vug` render
loop, which is reachable only through the interactive `vug` shell verb (`shell.rs`), so a headless
battery never runs it — the baseline capture has zero `[vugfps]` lines, before and after. The bandwidth
question it would answer is instead answered directly by `[wc-h] bytes=` (1081456 per staged composite for
window 1) plus the compose/present split above, which is a finer-grained account of the same cost on the
path this arc actually changed.

#### Metal watch-list

* **The crystal is SHARP.** The money reading, and the only one that settles the arc: a 128x128 window at
  4x, repainting continuously, with no horizontal band boundaries. WC-G's photograph is the before.
* `[wc-h] rollup ... -> TEAR-FREE` for every window. Metal is expected to be **slower** than QEMU on both
  phases (real non-coherent framebuffer stores, a real `DC CVAC` sweep), so `present_us` will rise; the
  margin to watch is 1444 µs against 7305 µs — a 5x headroom on the gate that metal must not consume.
  An `AT-RISK` rollup on the bench means the row copies alone are still losing the race and the next step
  is fewer bytes per present (damage sub-rects within the window box), not a bigger buffer.
* `[wc-h] rollup ... declines=0`. A non-zero `declines=` on the bench means composites are reaching
  the panel unbuffered — most likely `reason=lock`, i.e. real contention with the desktop flush that
  QEMU's timing does not reproduce. That is the one number most likely to differ on metal, and it now
  fails the gate rather than hiding behind the staged samples.
* `[wc-g]` verdicts stay `CLEAN` or `CLEAN+SLOW`. A `COHER` or `RACE` on metal would be a *new* finding
  about the surface, unrelated to this arc — the back layer changed the destination, not the source.
* The chrome: border and title strip must land in the same place they did, since they are now composed
  through a different origin. A one-pixel offset would show as a shifted frame.
* Cursor and FOCUS-VIS are unchanged above the buffering: the sprite is still taken off the panel for the
  whole pass, and `erase` still fills the desktop directly. `erase` is *not* staged — it is a one-shot
  solid fill on close/raise rather than a continuous repaint, so it can flash but cannot produce the
  standing tear this arc removes. If the bench sees a torn erase, that is the follow-on.

### WC-I — the periodic full-desktop repaint, and the cursor bracket that rode on it

P60 (Pi 4, 1920x1200, attended) confirmed WC-H: a single vug window is crystal sharp, `[wc-h] rollup
... torn=0 declines=0`, present ~1.2-1.6 ms. It also produced two symptoms WC-H does not cover, and
neither is reproducible in QEMU (one needs the bench's timing, the other needs HID):

1. with **several** vug windows, a fuzz blip in **every** vug window **simultaneously**, slightly faster
   than once a second, while the stat window, the desktop and the console stayed clean;
2. the mouse cursor visible but **spotty and flickering**.

#### The blip: one periodic painter, named

The synchronization across windows and the ~1 Hz period are the fingerprint of a single caller that
repaints the whole window layer, and there is exactly one:

* `main.rs::status_tick` posts an `Event::Timer` to `GUI_CHANNEL` once a second (metal only — QEMU
  raspi4b has no Group-1 IRQ, so the task is not spawned there at all, which is why the gate cannot see
  this);
* the render task's `Event::Timer` arm sets `strip_dirty`, `ui_status::draw` recomposes the PI-UI-2
  status strip, and `pal.render` → `Screen::flush` presents;
* `Screen::flush` then called `wm::repaint()`, which marks **every** live window damaged and composites
  the lot.

That is WC-E's own stated residual — "the window pixels are overwritten and repainted within the same
present rather than never being overwritten at all" — with a period and a trigger attached. Two
consequences, not one:

* every window is erased to desktop content and restored once per tick, so a scan-out landing between
  the two steps catches all of them at once;
* the repaint runs a composite from the **render** core while the vug windows present from **theirs**,
  so `wm`'s single `STAGE` back layer is contended. `try_lock` declines, and a declining window takes
  the pre-WC-H direct path — per-pixel writes into live scan-out, the tearing regime. One window rarely
  collides; N windows collide N times per tick, which is why the symptom needed several vugs.

The stat window reads clean for a mundane reason: it presents rarely, so it is almost never mid-present
when the repaint lands. The desktop and the console are single-writer surfaces and were never at risk.

**The fix removes the overwrite instead of sequencing it.** `wm::occluders` publishes the panel boxes
the window layer owns (live, non-compat, above the shell — the same population `stage_window` and
`wcg::begin` scope to), and `Screen::present_background` subtracts them from its own damage: each
damaged row is copied in the sub-spans no window covers (`next_visible_span`, a linear walk over at
most `MAX_WINDOWS` boxes, at most `2n+2` steps per row, no allocation). Desktop pixels are then never
written where a window is, for any interval however short.

With nothing to undo, the blanket re-blit goes too: `Screen::flush` now calls `wm::service_damage`,
which composites only rows something *else* marked — chiefly `cursor::repair`, whose "marks only, never
composites" contract needs a pass within a frame. `wm::repaint` survives for the one path that can
still intrude, and `present_background` reports which it took.

VUG-PAR's band-parallel flush is taken only when the window layer is **empty**. The band workers copy
whole clipped rects and know nothing about occluders; the case that needs the subtraction (several
windowed apps) is also the case where the desktop's own damage is a strip or a console line, which the
parallel path declines as too little work anyway. The full-screen VUG frame it was built for has no
windows and is byte-for-byte unchanged.

#### The cursor: an unconditional bracket at present rate

`wm::composite` was `cursor::undraw()` → pass → `cursor::repaint()`, unconditionally. `composite` runs
once per window present from the presenting task's own core, so with several high-rate windows the
sprite was in a restore→save→draw cycle on one core or another essentially continuously — and
`cursor::undraw_locked`'s colour guard *declines* to restore a pixel another painter has taken, which
under that contention is most of them. The panel shows a sprite with holes that move from frame to
frame. The cost was paid on every present regardless of where the pointer was.

WC-I makes the bracket **conditional**: `composite_inner` takes it only when `cursor::sprite_box()`
intersects a live window above the shell — plus WC-F's reserved probe boxes, which paint at the tail of
the pass and lie outside the window layer, so `repair` could never mend a sprite pixel they took. The
tail is `cursor::repaint()` when the pass disturbed the sprite and the new `cursor::ensure_drawn()`
when it did not (one lock and a boolean in the common case; the work it does do covers `erase`, which
takes the sprite down and leaves the following composite to put it back).

**Where the decision runs is a correctness constraint, not a preference.** It is taken at the very top
of `composite_inner`, *before* the snapshot that registers the pass as an in-flight blit. `undraw`
takes the SPRITE lock, and F4's drain barrier is a teardown spinning IRQ-masked and unpreemptible
until `BLIT_ACTIVE` reaches zero; acquiring SPRITE inside the `BlitGuard` window would add a second
lock to that wait set, so a core preempted while holding SPRITE would stall the draining core
indefinitely instead of for the length of a bounded blit. Deciding before the guard keeps the drain's
wait set exactly what its termination argument assumes.

The consequence is that the test is deliberately **conservative in two directions**: against every
live window above the shell rather than only the damaged ones (the dirty set is closed upwards over
occlusion *inside* the pass, which this pre-pass cannot see), and against the sprite's **box** rather
than its painted mask (the box is snapshotted without the sprite lock, so it must be the outer
extent). A false positive costs the pre-WC-I behaviour for one pass; there is no false negative,
because every pixel the sprite paints lies inside the box it reports and every window the pass can
paint is a window the test considered.

Nothing WC-D depends on moved: a window this pass paints is a window whose intersection test ran, so
`verify_window` still never reads a rect with the sprite on it.

#### Witnesses

`[wc-i] rollup scope={fixture|desktop} windowed_flushes=N intrusions=N cursor_passes=N
cursor_brackets=N -> {CLEAN|INTRUDED|UNWITNESSED}`

* `intrusions` — desktop presents that wrote background pixels inside a live window's box. The blip is
  this number being one per strip tick; the fix makes it 0.
* `cursor_brackets`/`cursor_passes` — before WC-I these were equal by construction.
* `scope=fixture` fires at the end of the window-verb witness block and proves the counters are wired.
  `scope=desktop` fires once the desktop has presented over a live window layer 64 times, which is the
  first point the verdict means anything. The verdict is `CLEAN` only when the desktop ran over a
  window layer **and** never intruded; a boot where that never happened reports `UNWITNESSED`, so an
  empty run can never be read as a pass.

`[wc-i] reopen closed=W reopened=W survivor=W both=B reopen=B survivor_px=B -> PASS|FAIL` — the
close→reopen scan-out read-back (see below).

#### The close→reopen / undying-vug cluster: what this arc can and cannot say

P60 also reported that a relaunched vug shows an **empty** window, that no further vug then displays,
and that those same relaunched vugs are **unkillable** (`kill armed but unconfirmed`, no
`[skill] killed ... confirmed=1`). A hypothesis put to this arc was that WC-H's back layer carries
per-window state that teardown fails to rebind, so a recycled slot presents into a dead layer.

**That hypothesis is refuted, twice over.**

*By inspection*: `STAGE` carries no window identity at all. It is one global `Vec<u8>`, `try_lock`ed
per composite, grown-only, and `paint_window`'s opening `fill_rect` covers the whole clipped box dense
before the present — so every byte a present copies was written by that present. Nothing in `close`,
`close_owner`, `close_compat` or `win_close_asid` touches it, and there is nothing there to rebind.
The only per-id compositor state is `VERIFIED` (a WC-D one-shot latch, already cleared in
`create_inner` precisely because ids are recycled slot aliases) and `wcg`'s per-id sample budgets;
neither writes a pixel.

*By read-back*: `wm::reopen_selftest` (`[wc-i] reopen`) drives the exact sequence in QEMU — create A
and B, present both, close A, create C into the slot A vacated (the `close win=1` / `create win=1`
aliasing the bench log shows), present C — and reads the **scan-out** back at each content origin. It
reports `PASS` on the gate: the reopened window's pixels are on the panel and the untouched survivor's
still are.

So the compositor is not where those pixels are lost. The bench evidence points the same way once the
fourth symptom is added: the relaunched vug never crosses the kill boundary, and an app that never
reaches its checkpoint also never reaches its first `SYS_WIN_PRESENT`. A window whose owner has not
presented shows kernel chrome over a **zeroed** surface — `boot::build_slot` scrubs the slot's FB
region on recycle — which is precisely "an empty window". `[wc-d] verify ... nonzero=262144 -> PASS`
in the bench log is not a contradiction: it is the *one* vug that did present.

**Out of lane, reported not touched.** The remaining thread is where a relaunched vug on a recycled
slot/ASID spins before its first present, and why an armed kill never confirms against it. That lives
in the EL0 launcher / slot-recycle / task-teardown path (`arch/aarch64/boot.rs::build_slot` +
`teardown_user_slot`, `sched.rs`'s kill boundary, the shell's `skill`), not in the WC-* files this arc
owns. It needs its own arc and its own witness.

#### WC-I gate results (2026-07-25, QEMU raspi4b, forced bench geometry)

QEMU raspi4b, forced bench geometry — **no metal in this arc**; final verification is attended metal.

`./arroyo check` green both arches · `./arroyo test-arm` → xHCI MISSION SUCCESS ·
`UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 60` → MBENCH **63/63 required, 0 forbidden**, with
every `[wc-g]` / `[wc-h]` / `[wc-d]` / `[wc-fv]` witness unchanged and green.

```
[wc-i] reopen closed=1 reopened=1 survivor=2 both=true reopen=true survivor_px=true -> PASS
[wc-i] rollup scope=fixture windowed_flushes=0 intrusions=0 cursor_passes=335 cursor_brackets=0 -> UNWITNESSED
```

`UNWITNESSED` is the correct and expected gate verdict, not a weak pass: `status_tick` is not spawned
under QEMU and the headless battery delivers no input, so the desktop never presents over a live window
layer there. `cursor_brackets=0` against 335 passes is likewise wiring only — raspi4b delivers no HID
pointer report, so the sprite is never drawn. Both numbers carry their claim only on the bench.

#### Metal watch-list

* **The blip is gone.** Several vugs running: no simultaneous per-second fuzz. `[wc-i] rollup
  scope=desktop ... intrusions=0 -> CLEAN` on the wire is the assertion; a non-zero `intrusions` means
  a desktop path is still writing into a window box and names it as the cause rather than leaving it to
  a photograph.
* **`[wc-h] rollup ... declines=0` should now hold with several windows**, not just one. The
  `reason=lock` declines WC-H anticipated were largely the desktop repaint compositing on a second
  core; with the blanket repaint gone, that contention source is gone with it. Residual declines are
  genuine two-app concurrency.
* **The cursor is solid.** `cursor_brackets` far below `cursor_passes` is the number; the operator
  reading is a sprite that stops flickering while vugs render.
* The status strip and console must still repaint normally *around* windows — the subtraction is
  per-row-span, so a window overlapping the strip should show the strip either side of it and never
  through it.
* `erase` is still unstaged and still fills the desktop directly (unchanged by this arc), so a torn
  erase on close/raise remains the WC-H follow-on it always was. **Settled by WC-K**, which staged the
  fill through the same back layer; this item is kept as the record of the debt, not as open work.

### CURSOR-3 — the sprite rides the present

**P61 (attended, Pi 4 1920×1200) confirmed WC-I's cursor half and bounded what it left.** The verdict
list: the cursor no longer flickers over the desktop or the status strip — but it is still *spotty
over a vug*, i.e. specifically while the pointer sits on a live window presenting at ~60 fps. That is
not a regression in WC-I; it is the case WC-I documented as remaining, arriving on the bench with a
name.

#### The remaining case is a duty cycle, not a race

WC-I made the bracket conditional: `composite` takes the sprite off the panel only when
`cursor::sprite_box()` intersects a live window above the shell. Over the desktop that is never, and
the flicker went with it. Over a window it is *every present* — and between the `undraw` and the
`repaint` sits the whole of `draw_window`: a full off-screen compose plus `bh` row copies,
milliseconds during which the sprite is simply **not on the panel**. At 60 presents a second the
sprite is absent for a large and irregular fraction of every second, and with two vugs the two cores'
brackets interleave. No amount of care *inside* the bracket shortens that interval, and
`undraw_locked`'s colour guard — correctly — declines to restore pixels the other presenter has since
taken. Holes, moving. The bracket's cost is structural: it is a hole in time, and the fix has to
remove the hole rather than narrow it.

#### The mechanism: compose the cursor into the back layer

WC-H already composes each window into a cached-RAM back layer and presents it as contiguous
full-box rows. CURSOR-3 paints the sprite **into that layer**, after the window is composed and
before the rows are copied:

```
stage_window:   paint_window(layer)            # window chrome + upscaled content
                cursor::compose_into(layer)    # <- CURSOR-3: sprite, last into the layer
                for y in 0..bh { fb.blit(row) }  # unchanged WC-H present
```

The cursor therefore reaches the panel **inside the same row copies the window does**. There is no
undraw phase for those pixels and no interval in which they are window-only: the present is atomic to
exactly the degree the window's own pixels are, which is WC-H's whole claim. Nothing extra is written
to the front buffer, and the trailing `flush_range` already covers the sprite because it is inside
those rows — WC-H's contiguous-row present discipline is untouched.

The save-under must come from the **layer**, and that is the load-bearing detail. The pixels the
sprite hides are the window's, and at overlay time they exist only in the layer — the front still
holds the previous frame. Reading them back from the front afterwards (what `draw_locked` does) would
capture the sprite's own `FILL`, and the next restore would stamp a white arrow permanently into the
window's rect. So `compose_into` saves from the layer into a published plan (`cursor::OVERLAY`), and
the composite's tail *installs* that plan instead of painting.

#### Three tails, and the new one writes no pixels

`composite_inner` now returns `CursorTail`:

| tail | meaning | cost |
| --- | --- | --- |
| `Untouched` | the pass never went near the sprite (WC-I) | `ensure_drawn` — one lock, one bool |
| `Repaint` | the bracket ran to completion (WC-I) | restore → save → draw |
| `Adopt` | the pass carried the sprite through a staged present | install the plan; **zero framebuffer writes** in the common case |

`adopt_overlay` takes `SPRITE`, installs the plan (geometry + the layer-derived save-under) and marks
the sprite drawn — the panel already holds it. Two things are then normalised under the same
acquisition: a sprite another core drew in the meantime is taken down *before* the plan is installed
(its `saved` is front-derived and correct, so restoring it first leaves exactly one sprite on the
panel), and a pointer that has **moved** since the plan was taken falls through to the ordinary
`refresh_locked`. Installing the plan first is precisely what makes that second case safe: the module
now knows those panel pixels are the sprite's and what the window had under them, so the undraw puts
the window's pixels back instead of a save-under capturing the overlay's own `FILL`.

#### Whole-box containment, deliberately

The overlay is taken only when the sprite's box lies **entirely** inside the window's clipped outer
box. A straddling sprite would need per-pixel bookkeeping of which pixels came from a layer and which
from the front, merged across the several windows and the desktop one sprite can span — bookkeeping
whose failure mode is a stamped arrow. A straddling sprite keeps WC-I's bracket unchanged: correct,
and no worse than before. The sprite is one glyph cell plus a shadow block (36 px at the 4× cap), so
it is wholly inside the window it is pointing at for all but its own width at the frame.

#### Every WC-I invariant, preserved and stated

* **`SPRITE` never joins F4's drain wait set.** The plan is snapshotted (and the sprite undrawn) in
  exactly the place WC-I put the bracket decision — *before* the `BlitGuard` registration.
  `compose_into` runs inside the guard but takes only `OVERLAY`, and only with `try_lock`, which is
  the same discipline and the same reason WC-H's `STAGE` uses one: a contended pass declines the
  overlay and falls back to the bracket. `adopt_overlay` takes `SPRITE` in the tail, after the guard
  has been dropped. Lock order `SPRITE → OVERLAY`, never the reverse.
* **The no-intersect path is unchanged.** A pass with no sprite on the panel does one
  `sprite_plan()` — the same acquisition `sprite_box()` was, now also carrying the block scale so the
  geometry is one snapshot rather than two — and takes the `Untouched` tail.
* **`ensure_drawn` semantics are untouched**, including `erase`'s contract that the composite which
  follows puts the sprite back.
* **No verified pixel is ever read with the sprite on it.** `wcg::end`'s `fbbad` count and
  `verify_window`'s scan-out verdict both read this window's destination pixels back and compare them
  against its *source* surface, and a cursor legitimately composited into those pixels would read as
  a blit defect. So the plan is withheld from any window with a live WC-G probe or an unspent WC-D
  bit, and from a sprite overlapping a WC-F reserved box (the probe paints into the front *after* the
  pass, so it would overwrite pixels the plan claims). All three are budgeted one-shots: they cost
  those few passes WC-I's bracket and nothing else.

#### Witnesses

```
[cursor3] present tail=adopt offers=1 taken=1 -> COMPOSED     # first 8 passes that touched the sprite
[cursor3] rollup scope={fixture|desktop} planned= offers= taken= adopt= repaint= ensure= -> VERDICT
```

`planned` counts passes that took the bracket *and* had an eligible plan to hand down; `offers`/`taken`
are per-window overlay attempts and successes (`offers - taken` is a straddling sprite, a contended
plan lock, or an unreadable layer — every one a **missed improvement**, never a defect); `adopt` /
`repaint` / `ensure` are the three tails. Verdict: `COMPOSED` when the mechanism ran, `BRACKETED` when
it was offered and never landed, `UNWITNESSED` when it was never offered, `INCOHERENT` if `taken`
exceeds `offers` (an overlay that landed without being offered — a wiring defect). The rollup is
printed alongside `[wc-i]`'s, from the same harness and at the same two scopes.

#### Gate scope, stated honestly

```
[cursor3] rollup scope=fixture planned=0 offers=0 taken=0 adopt=0 repaint=0 ensure=335 -> UNWITNESSED
```

**QEMU cannot witness this fix, and the verdict says so.** raspi4b delivers no HID pointer report, so
`pal::cursor::visible()` is false for the whole boot, the sprite is never drawn, `sprite_plan()` is
always `None`, and every counter is 0. The gate proves **no-regression only**: the window path still
runs its passes (`ensure=335` — all of them through WC-I's cheap tail), settles every one through a
tail, and never took an overlay it was never offered. `UNWITNESSED` is the correct outcome, and it
exists so a pointerless run can never be read as evidence for the mechanism.

`UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 60` → MBENCH **64/64 required, 0 forbidden**;
`./arroyo test-arm` → MISSION SUCCESS; `./arroyo check` green both arches. Every `[wc-d]` / `[wc-g]` /
`[wc-h]` / `[wc-i]` witness unchanged.

#### Metal watch-list

* **The verdict is `taken > 0` with `offers == taken` while the pointer rests on a vug**, and the
  operator reading is a cursor that stays solid — no holes, no flicker — while the window presents
  under it.
* `adopt` should dominate `repaint` while the pointer is over a window. A high `repaint` with
  `offers > 0` means the overlay is being offered and declined; `offers - taken` names which of the
  three declines, and a straddling sprite at a window edge is the expected benign one.
* Dragging the pointer *across* a window edge should degrade smoothly to WC-I's behaviour and back —
  a visible step in flicker at the frame is the containment rule doing its job, not a defect.
* A cursor left over a window while WC-D or WC-G spends its one-shot budget will bracket for those
  few passes. Early-boot only, and `[cursor3] present tail=repaint` names it.

> **Superseded by CURSOR-4 (below).** Whole-box containment is gone: the straddling sprite is now
> composed in part and settled per pixel, and the "visible step in flicker at the frame" that this
> watch-list called expected behaviour was, on the P62 evidence, the flicker itself.

### CURSOR-4 — the split sprite: the straddle stops costing the overlay

CURSOR-3 removed the flicker for a sprite lying *wholly* inside one window. P62 (attended bench)
showed Peter still seeing flicker over vugs, and the wire quantified it:

```
[cursor3] rollup scope=desktop planned=2481 offers=4181 taken=2427 adopt=2427 repaint=121 ensure=92247 -> COMPOSED
```

58% of sprite-touching presents composed through the overlay and were clean by construction. The
other ~42% declined and fell back to WC-I's undraw/redraw bracket, and **every declined pass is a
visible sprite gap**. The decline rate *is* the flicker. CURSOR-3's own documentation named the
dominant reason: whole-box containment. A pointer resting on a window's border straddles on every
single pass, indefinitely — plausibly the exact pointer position P62 was run at.

#### Why the bracket had to become partial, not just wider

The undraw exists for one reason: a pixel some painter in this pass is about to overwrite must be
handed back *before* the overwrite, or the save-under is stale and the restore stamps pre-pass
content into a window's rect. **That reason applies per pixel, not per sprite.** CURSOR-3 applied it
per sprite, so a sprite half over a window and half over bare desktop had its desktop half taken down
too — for a pass that provably could never write there. That half then blinked at present rate for
nothing.

So the bracket is now masked. `composite_inner` already derived the conservative set of extents the
pass may paint (every live window above the shell whose outer box meets the sprite); it now collects
that set rather than reducing it to a boolean, and hands it to `cursor::undraw_within`, which restores
**only** the sprite pixels inside it and leaves the rest on the panel untouched. Pixels handed back
are recorded per pixel in `Sprite::off`.

#### Three provenances, named per pixel

Every painted sprite pixel ends a CURSOR-4 pass in exactly one of three classes:

| class | save-under comes from | when it reaches the panel |
|---|---|---|
| **composed** | the window's **back layer** (`compose_into`) | inside that window's row copies — no undraw phase at all |
| **remainder** | the **front buffer**, read in the tail (`redraw_off_locked`) | after every window has presented |
| **untouched** | unchanged — it was never taken down | it never left |

The provenance hazard CURSOR-3 declined to take on is closed by *where* each read happens, not by a
per-pixel colour heuristic:

* The **layer** read cannot capture the sprite's own `FILL`: the layer is private to this pass,
  `paint_window` fills the whole clipped box densely immediately above, and `compose_into` is the
  first and only writer of the sprite into it.
* The **front** read cannot capture the sprite's own `FILL` either, because it happens only in
  `adopt_overlay`'s aligned branch and only for pixels whose `off` bit is still set — i.e. pixels the
  masked undraw took down and *nothing* has put back. This is exactly `draw_locked`'s provenance,
  narrowed to a subset.
* The **untouched** class needs no argument: neither the panel nor `saved[i]` changed.

`undraw_locked` (the full undraw) now skips `off` pixels rather than restoring them. It must: their
`saved` entry is stale by construction, and the colour guard cannot detect that — the guard passes on
any pixel the pass left alone.

#### The dangerous middle case, and how it is closed

A pixel composed into a *lower* window's layer and then overwritten directly by a *higher* window
would be marked "delivered" while actually holding window content — and the tail would decline to
repaint it, leaving a hole. So coverage is not only accumulated, it is **revoked**:
`overlay_uncover` clears the coverage bits inside the box of every drawn window that did *not*
compose the sprite into it (direct path, instrument exclusion, compat row, contended plan lock,
unreadable layer). Windows are drawn back-to-front, so the **topmost painter of each pixel is the one
whose verdict the tail acts on**. A window that never draws this pass never claims coverage, so its
pixels fall to the remainder class — correct by default.

Note the consequence for the instrument exclusions: an excluded window is still handed the plan's
*geometry*, and only the compose is suppressed. Withholding the plan entirely (CURSOR-3's shape)
would leave that window unable to revoke coverage.

#### One session per pass

Coverage accumulates across the several windows of one pass, so the overlay can no longer be a
last-writer-wins slot. `overlay_open` makes it single-owner for the pass: a second concurrent
`composite` on another core finds it busy, takes no plan, and runs CURSOR-3's whole-sprite bracket
(counted under `lock=`). Merging two passes' coverage would let pass B's reset erase pass A's bits,
and pass A's tail would then "restore" pixels already carrying the sprite from B's layer — reading
`FILL` as the under-pixel and stamping the arrow permanently. The session is closed in
`adopt_overlay`, and `composite` routes **every** exit through its tail, so it cannot leak.

A `Sprite::epoch` counter — bumped by every full undraw and every draw, and carried on the `Plan` —
retires any plan whose sprite has since been taken down and put back elsewhere by a concurrent
`repaint`. The mismatch is detected and falls back to a whole-sprite refresh rather than merging.

#### Tail selection changed with it

`tail_of` now keys on **"does this pass own the overlay session"**, not "did some window carry the
sprite". Forced, not cosmetic: a pass that opened a session has left sprite pixels off the panel
whether or not any layer took them, and only `adopt_overlay` knows how to settle them (install the
covered ones, repaint the remainder, close the session). A `Repaint` tail there would leak the
session for the boot. `adopt` may therefore now exceed `taken`, which is why the rollup's old
`adopt > taken → INCOHERENT` clause is gone.

#### Every invariant, still preserved

* **`SPRITE` never joins F4's drain wait set.** `overlay_open` and `undraw_within` run in exactly the
  place WC-I put the bracket decision — before the `BlitGuard` registration. Inside the guard,
  `compose_into` and `overlay_uncover` take only `OVERLAY`, and only with `try_lock`. `adopt_overlay`
  takes `SPRITE` in the tail, after the guard is dropped. Lock order `SPRITE → OVERLAY`, unchanged.
* **The no-intersect path costs the same.** One `sprite_plan()` and the `Untouched` tail, exactly as
  before; nothing on that path was touched.
* **No extra front-buffer writes.** The composed class writes zero front pixels (unchanged from
  CURSOR-3); the remainder class writes a *subset* of what CURSOR-3's bracket repainted; the
  untouched class writes none where CURSOR-3 wrote all of them. CURSOR-4 is a strict reduction.
* **No verified pixel is ever read with the sprite on it.** The masked undraw takes down every sprite
  pixel inside *every* overlapping window's box — including the one being verified — and the tail runs
  after `wcg::end` and `verify_window` have returned. WC-G / WC-D / WC-F exclusions are unchanged in
  effect; they now suppress the compose rather than the plan.
* **Save-under provenance is sound for the split sprite**, per the three-class table above.
* **`ensure_drawn` and `erase` semantics are untouched.**

#### Witnesses

The shape **as CURSOR-4 left it** (see §CURSOR-6 for the current line, which appends
`disjoint= partial=` after `straddle=` and `stale=` after `budget=`):

```
[cursor3] rollup scope={fixture|desktop} planned= offers= taken= adopt= repaint= ensure= straddle= lock= budget= -> VERDICT
```

The three new fields are the **decline breakdown** CURSOR-3 lacked — its `offers - taken` was one
number covering three causes, so it could not say which was worth fixing. Appended after the existing
fields, so the spec's `[cursor3] rollup` assertion and the `[cursor3] present` sample line are both
unchanged.

* `straddle=` — offers where the sprite met the window's box only partially. Under CURSOR-3 every one
  of these was a whole-sprite decline; under CURSOR-4 they are composed **in part**, so this is now a
  measure of how often the split mechanism runs, not of a loss.
  **Superseded by CURSOR-6:** this field does NOT mean what this paragraph says. `stage_window` offers
  to every staged window, so a window the sprite is nowhere near produces `missed > 0` and was counted
  here identically to a real partial carry — which is how the "48 % decline" reading arose. The
  honest split is `disjoint=`/`partial=`; `straddle=` is retained only as their sum, for continuity
  with P62/P64 captures. See §CURSOR-6.
* `lock=` — a contended plan lock at either end: a `composite` that found another pass owning the
  session, or a `compose_into` that could not take `OVERLAY` inside the guard. Both fall back whole;
  neither spins under the guard.
* `budget=` — an instrument forbade the compose (live WC-G probe, unspent WC-D bit) or a WC-F reserved
  box overlapped the sprite. All one-shots.

#### Gate scope, stated honestly

```
[cursor3] rollup scope=fixture planned=0 offers=0 taken=0 adopt=0 repaint=0 ensure=349 straddle=0 lock=0 budget=0 -> UNWITNESSED
```

**QEMU still cannot witness this fix, and the new counters read 0 for the same reason as the old
ones.** raspi4b delivers no HID pointer report, so `pal::cursor::visible()` is false for the whole
boot, the sprite is never drawn, `sprite_plan()` is always `None`, no session is ever opened and no
offer is ever made. `UNWITNESSED` is the correct and only honest verdict here; the gate proves
**no-regression only**.

`UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 90` → MBENCH **77/77 required, 0 forbidden**
(2184 lines scanned); `./arroyo test-arm` → **MISSION SUCCESS**; `./arroyo check` green both arches.
Every `[wc-d]` / `[wc-g]` / `[wc-h]` / `[wc-i]` / `[wc-k]` witness unchanged.

#### Metal watch-list

* **The line to read: declines near zero with the pointer parked ON a window border over a live vug.**
  Expect `taken` to track `offers` closely and `straddle > 0` — a straddle is now a *composed* pass,
  not a declined one. The operator reading is a cursor that stays solid on the border, where before it
  blinked on every present.
* `lock=` should stay small. If it is material, the next step is a per-window plan slot instead of one
  global session — measured first, which the breakdown now allows.
* `budget=` is early-boot only (one-shot instruments) and should stop growing once WC-D and WC-G have
  spent their budgets.
* A cursor over bare desktop is unchanged: `ensure` tail, no session, no bracket.
* Watch for a **stamped arrow** — a white glyph frozen inside a window's rect. That is the failure
  mode the provenance argument above exists to prevent, and it would mean a save-under captured
  `FILL`. The gate cannot exercise it; it is the first thing to look for on metal.

### WC-J — a closed window gives its panel rows back

#### The observation (P61, attended bench)

The operator launched several background vugs, killed some, and watched: "one vug crashed, two frozen,
at least one still going". `jobs` then reported all four pids `exited 0 (reaped)` — corroborated by a
panel photograph. The kernel's process story was completely clean, which means the "frozen" windows were
not frozen processes at all. They were **ghosts**: window pixels still on the panel for owners that had
already exited and been reaped. Nothing alive was drawing them, so the defect is in the window layer by
elimination before any code is read.

#### Why WC-I made it permanent (and why it is not a WC-I regression)

The tiler owns a real window's position. Nothing in EL0 moves a window — `move_to` is what PINS a row,
and only kernel-side fixtures call it — so every real window is laid out by `place`, whose layout is a
function of **how many windows exist**. Closing one window therefore re-tiles all the survivors, and
creating one does the same in the other direction.

`close`/`close_owner` erased the box the *closed* window vacated. Nothing erased the boxes the
*survivors* vacated by moving. That set is invisible to every caller: only the tiler ever sees both the
old and the new geometry.

Before WC-I this was covered by accident. The desktop presented its whole damage set each tick and
`wm::repaint` re-blitted the entire live set on top, so an abandoned tile was overwritten within about a
second whether or not anything had reclaimed it. WC-I removed both — correctly; that is what killed the
1 Hz blip — by subtracting the window layer from the desktop's damage and replacing the blanket re-blit
with `service_damage`. The abandoned tile then belongs to nobody: not to the window (which moved), and
not to the desktop (whose damage for those rows was discarded while a window still sat there). It stays
for the rest of the boot. Killing one of four vugs leaves three full window copies standing where the
windows no longer are — exactly the "two frozen vugs" the operator reported.

#### The mechanism

`place` now RETURNS the boxes it took from the windows it moved — the one place that can see both
geometries — and every call site reclaims them through `wm::reclaim`, which does three things:

* **erase** — desktop colour on the panel immediately, so the ghost is gone within the call rather than
  at the desktop's next tick (the same immediate-response argument `focus_changed` makes for its hidden
  boxes);
* **`damage_intersecting`** — a survivor whose box overlaps a reclaimed one just had a bite taken out of
  it by that erase, so it is re-damaged for the composite the caller runs next;
* **`screen::request_full_present`** — only the desktop can put its OWN content (console text, status
  strip) back under a departed window; `erase` can paint nothing but `DESKTOP_BG`. This is the hand-off
  flag FOCUS-VIS already built for precisely this case, consumed by the render task's next flush on its
  own thread. Raised only when something was actually reclaimed, so a windowless boot never asks.

Three call sites, all covered:

| path | trigger | reclaimed |
| --- | --- | --- |
| `close` | `SYS_WIN_CLOSE` → `wc_shim::destroy` | the closed window's box **+** every re-tiled survivor's old box |
| `close_owner` | exit teardown: `clear_handle_row` → `win_close_asid` | same two sets — **this is the P61 path**, a bg app reaching its exit with a window open |
| `create_inner` | `SYS_WIN_CREATE` | the re-tiled survivors' old boxes (no drain barrier: no row is freed and no surface unmapped) |

The close paths reclaim INSIDE their existing F4 drain barrier — the pixels must not race an in-flight
blit of the old geometry — and composite after dropping it, unchanged from WC-A.

#### The witness

`[wc-j] vacate` (two legs) and `[wc-j] retile`, driven from `reopen_selftest`'s tail so they inherit its
preconditions (every one-shot per-window latch already spent, shell z restored, live set repainted).
Read-back against the scan-out, byte-for-byte against `DESKTOP_BG`, no tolerance — kernel state was
already correct in the bench report, so only the framebuffer is asked.

* **vacate/close** and **vacate/owner** — present a window, prove the panel took its colour, close it by
  each of the two paths, and read the vacated box at five points (content origin, two diagonals, title
  strip, lower border; chrome is kernel-drawn and leaks exactly as visibly as content).
* **retile** — two tiled windows; close one; the survivor must have MOVED (else the leg proves nothing),
  must still reach the panel at its new box, and must have left desktop behind at its old one.

#### WC-J gate results (2026-07-25, QEMU raspi4b, forced bench geometry)

The vacate legs **passed on the unfixed tip** — a lone close does reclaim its own box — and are kept as
the regression floor. The retile leg is where the defect lives, and on the unfixed tip it read:

```
[wc-j] vacate close_painted=true close_desktop=true (5/5) owner_painted=true owner_desktop=true (5/5) -> PASS
[wc-j] retile survivor=2 moved=true painted=true live=true old_desktop=false (0/3) -> FAIL
```

`0/3` — the survivor's abandoned tile held its window content at every sampled point. With the reclaim
in place, on the same geometry:

```
[wc-j] vacate close_painted=true close_desktop=true (5/5) owner_painted=true owner_desktop=true (5/5) -> PASS
[wc-j] retile survivor=2 moved=true painted=true live=true old_desktop=true (3/3) -> PASS
```

#### The c1=99% measured alongside the ghosts — not the window layer

P61 also measured `SCHED: load c1=99%` sustained while at least one ghost was on the panel, with every
vug exited. Nothing in the window layer can account for it, and this is stated as a negative result
rather than force-fitted: `video/` contains no loop outside the framebuffer's init poll; the compositor
is entirely event-driven (`present`, `close`, `focus_changed`, and `service_damage`, which returns after
one eight-row table scan when nothing is damaged); the GUI/render task parks in `GUI_CHANNEL.recv()` and
burns nothing at idle; and a ghost is by construction inert — the rows are freed (`[wc-a] close_owner`
prints, and the vacate legs prove those rows stop compositing), so no live loop stands behind those
pixels. The remaining candidates are all outside this lane (a task still runnable on c1 after its
parent's exit, or a poll cadence in the shell/scheduler); the bench measurement that would name it is
`top` while a ghost is present, which reports the last task per core.

#### Metal watch-list

* Kill one of several background vugs: the survivors re-tile and leave **nothing** behind at their old
  positions — the reported "frozen vug" must not appear.
* Under the re-tile, the console text and status strip come back where the abandoned tile was, within
  about a second (the `request_full_present` hand-off), not merely flat `DESKTOP_BG`.
* Launching a vug while others are up re-tiles them too, and leaves no residue either — `create` is on
  the same reclaim path as `close`.
* `erase` is still unstaged (unchanged by this arc), so a torn erase on a large reclaim remains the WC-H
  follow-on it has always been. **Settled by WC-K** — the reclaim's fill is staged and row-contiguous;
  kept here as the record of what WC-J left behind.

### WC-K — the desktop fill gets the back buffer too, and the last direct writer is gone

WC-G's verdict was about a **shape**, not about a writer. Per-pixel `put_pixel` into the live front
framebuffer, with no vblank synchronisation anywhere in the path, is structurally overtaken by the
scan-out — measured at ~2x the beam's time on the rect — and what the panel latches is part-old and
part-new, split at whatever scanline the beam held. WC-H removed that shape from a window's own pixels
by giving the window layer a back buffer.

`wm::erase` kept it. Its `fill_rect` is `w * h` bounds-checked pokes straight into the memory the HVS
is scanning, and it is the fill that repaints a vacated box on every close, every move and every
re-tile. WC-I's own section named it as outstanding debt ("`erase` is still unstaged and still fills
the desktop directly"), and WC-J then made the debt heavier rather than lighter: `reclaim` — whose
first step is that fill — is now reached from `close`, `close_owner` and `create_inner`'s re-tile, over
boxes as large as a whole tile. `erase` was the last unstaged front-buffer writer in the window
lifecycle.

#### The writer survey (what was left, and what WC-K did to it)

| writer | destination | before WC-K | after |
|---|---|---|---|
| `wm::erase` → `fill_rect` (`wm.rs`) | FRONT | direct per-pixel over the whole box | staged via `stage_fill` |
| `wm::draw_window` → `stage_window` | FRONT | staged (WC-H) | unchanged |
| `wm::paint_window` (fallback leg) | FRONT | direct — WC-H's documented last resort | unchanged |
| `cursor::undraw` / `draw` | FRONT | direct, 12x20 sprite under the OVERLAY lock | unchanged (see below) |
| `Screen::flush` / `present_background` | FRONT | back buffer + contiguous row flush | unchanged |
| `fbcon` | FRONT | pre-heap boot console, bulk `blit` restores | unchanged |
| `wcf` twin probe | FRONT | deliberate raw-addressing fixture | unchanged |

#### The staging, and why it is not a third discipline

`stage_fill` reuses WC-H's machinery outright: the same `STAGE` buffer, the same `try_lock`, the same
`MAX_STAGE_BYTES` cap, the same four decline reasons, and the same present primitive — bulk
`copy_nonoverlapping` runs, one per scanline, which is the only part of the operation the scan-out can
catch mid-flight.

One thing differs, and it is the fill's own nature rather than a new rule: a solid fill's rows are
**identical by construction**, so the composed artifact is ONE row, presented `h` times. That is not a
shortcut. Staging `w * h` bytes to hold `h` copies of one row would put a full-panel erase (7.6 MB at
the bench's 1920x1200) over the 4 MiB cap, declining it straight back into the tearing regime for
exactly the largest boxes — the ones that tear worst. The composed row is zeroed before it is filled,
because `put_pixel` writes 3 of 4 bytes and the previous tenant of `STAGE` is a window's staged pixels;
the pad is not scanned out, but the back layer's "every byte the present copies was written by this
pass" invariant is worth one row of `memset` to keep true.

**Row-contiguity is checked, not asserted.** The tear-free property rests on the shape of the present,
not on the presence of a staging buffer: a staged path whose runs fragmented, or overhung into the next
scanline, would report perfectly good compose/present numbers and still be back in the convicted
regime. `stage_fill` verifies per fill that each run is exactly `w * bpp` bytes, that it fits inside
its scanline (`x * bpp + row_bytes <= fb_row`, so no run wraps), and that consecutive runs step by
exactly one panel row. `[wc-k] contig=` reports the result and the spec FORBIDs `contig=no`
independently of the timing verdict.

#### Cursor coherence

Unchanged, deliberately. `erase` still calls `cursor::undraw()` before the first byte of any fill
reaches the panel and still leaves the repaint to the `composite()` every caller runs next; staging
moves where the *composing* writes go, never when the *panel* writes happen relative to that bracket.
WC-J's noted interaction (reclaim → erase brackets the cursor via `undraw`) therefore holds
byte-for-byte.

The staged fill does **not** take CURSOR-3's overlay, and that is the same decision the present path
takes rather than a new one. CURSOR-3 composes the sprite into a back layer only when the compositor
handed `draw_window` a `cursor::Plan` — a claim on the sprite's state machine, taken under the OVERLAY
lock by a pass that undrew the sprite itself. `erase` is not a compositor pass, holds no such plan, and
inventing one here would make it a second, unsynchronised writer of the save-under. So `stage_fill`
takes the branch `stage_window` takes when `cur` is `None`: compose no sprite, leave the repaint to the
following composite. CURSOR-3's own fallback, unchanged.

#### The witness: `[wc-k]`

Per staged fill — box, composed row bytes, run count, the contiguity verdict, the compose and present
phases separately, and the present measured against `rectscan_us` (computed exactly as `[wc-g]` and
`[wc-h]` compute it, with the same bias toward *not* reporting a tear). Plus one rollup.

Two divergences from `[wc-h]`, both corrections rather than style:

* **Decline lines are unbudgeted** (to a 16-line spam bound), where `[wc-h]` shares one budget between
  successes and declines. That sharing leaves a boot that starts declining *after* sample 4 silent
  behind an already-printed rollup — survivable for a window, which composites continuously, and not
  survivable here, because "a direct fill happened" *is* this arc's verdict. `FORBID -> DIRECT` stays
  reachable for the whole boot.
* **`scope=fills`, not `scope=boot`.** WC-G's lesson repeated: nothing observable inside a boot can
  distinguish "the erase path is finished" from "the next app has not closed a window yet", so the
  rollup claims only the fills it has seen and the completeness question belongs to the FORBIDs.

#### WC-K gate results (2026-07-25, QEMU raspi4b, forced bench geometry)

`UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 90` — **68/68 required witnesses, 0 forbidden**:

```
[wc-k] erase box=480x480 staged=yes rowbytes=1920 runs=480 contig=yes compose_us=143 present_us=1378 rectscan_us=6666 torn=no -> BUFFERED
[wc-k] erase box=514x526 staged=yes rowbytes=2056 runs=526 contig=yes compose_us=48 present_us=1883 rectscan_us=7305 torn=no -> BUFFERED
[wc-k] erase box=130x142 staged=yes rowbytes=520 runs=142 contig=yes compose_us=10 present_us=154 rectscan_us=1972 torn=no -> BUFFERED
[wc-k] rollup scope=fills samples=4 rows=1290 torn=0 noncontig=0 declines=0 maxpresent_us=1883 frame_us=16667 -> TEAR-FREE
```

Zero declines: every fill in the boot reached the back layer, including the 514x526 tile. The present
phase runs at roughly a quarter of the beam's time on the same rows (1883 vs 7305 µs on the largest
box), and the compose phase — one row, whatever the box height — is 10–143 µs, so the trade WC-H paid
for windows (a full extra box-sized copy) does not exist here at all: the total is *cheaper* than the
direct fill it replaces, because `h - 1` of the `h` rows cost a bulk copy instead of `w` bounds-checked
pokes. `[wc-h]`, `[wc-j]`, `[wc-g]` and `[wc-d]` are unchanged and green, and WC-J's read-backs still
find `DESKTOP_BG` byte-for-byte at 5/5 and 3/3 — the staged fill lands the identical pixels.

`./arroyo test-arm`: `MISSION SUCCESS`. `./arroyo check`: both arches clean.

#### Metal watch-list

* Close or kill a large window and watch the vacated box: the fill must appear as one clean rectangle,
  with no horizontal band boundary and no flash of a partially-filled box.
* Re-tile with several large windows up (launch or kill one of four vugs): every survivor's abandoned
  tile fills at once; a torn *reclaim* would show as a band across an abandoned tile, distinct from a
  torn *window paint*.
* `[wc-k] rollup ... -> TEAR-FREE` with `declines=0` on the bench's real 1920x1200 geometry — a decline
  there would most likely be `lock` (a concurrent desktop flush holding `STAGE`), which is loud by
  construction and falls back correctly rather than losing pixels.
* The cursor over a closing window: the sprite must reappear after the fill, never be erased into the
  desktop and never leave a stale patch — the `undraw`/composite bracket is the thing being watched.

## 9. CRISPY-PI — the theme table (`video/theme.rs`)

The Crispy desktop theme now exists kernel-side as a `const` table:
`crates/kernel/src/video/theme.rs`. It carries the 21 palette roles (chrome face,
the two bevels, frame keyline, the four title-gradient stops, the two title inks,
button face/pressed/ink, content fill/ink, scroll track/thumb, accent, and the
three circular title-bar controls) plus the gloss highlight and its three scalars,
and all thirteen metrics (`frame`, `bevel`, `title_height`, `corner_radius`,
`widget_radius`, `well_radius`, `scrollbar_width`, `button_height`,
`button_pad_x`, `gap`, `control_box`, `text_px`, `line_height_pct`).

**The shared-source law.** The source of record is `kits/crispy/theme.json` @
`us-crispy-modern` `0787ba9f`, read on the host by `libs/quartzite/src/theme.rs`. Both
arches — aarch64 Pi 4 and x86_64 — source their chrome and desktop constants from
this one table, and the table mirrors that one json. No per-arch invented numbers.
A value changes in the kit json first, then is re-lifted here.

**Representation and the pinned rounding rule.** Colours are packed `0x00RRGGBB`:
the json palette carries no per-colour alpha, so the top byte is zero rather than
an invented `0xFF`. Each literal was produced from its json triple by quartzite's
own converter, `to_u8(v) = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8` — clamp,
×255, +0.5, truncate toward zero, evaluated in `f32` at every step. Doing that
rounding at authoring time is what keeps the kernel free of float at runtime.

**How far the fidelity claim reaches.** Bit-for-bit agreement with a
quartzite-drawn pixel holds for **flat fills** — any surface painted with a palette
role at full opacity. It does *not* extend to the gloss ramp. Two independent
reasons: a quantized endpoint is not the host's f32 (`0.5` as `u8` would be `128`,
i.e. `0.50196`); and, more fundamentally, the host never rounds the endpoints at
all — it interpolates in f32 across the ramp and rounds each *composited per-pixel*
alpha, which no table of endpoint constants can reproduce by itself. Matching it is
a property of the interpolator the wiring arc writes. The three gloss scalars are
therefore carried at **Q16** (`value × 65536`, same round-half-up rule) rather
than `u8`, cutting endpoint error from ~2e-3 to ~4e-6 so the only residual error
sits in the interpolator where it belongs, and leaving no lossy `u8` in the table
as a trap. The title gradient *stops* are exact colours; only the interpolation
between them carries the same caveat.

**What is not lifted.** The json's `content_surface.Paper` block (`base_rgb`,
`algo: "Laid"`, `amplitude: 0.02`, `scale: 4.0`, `octaves: 3`, `seed: 4223012511`)
is deliberately absent, not an oversight. It is a *material* — quartzite's
`surface.rs` layer, a procedural paper texture a content region composites under
its content — not chrome, and not a constant: lifting it means porting a
multi-octave noise generator, a rasterizer concern. It belongs beside the
surface/material code when it lands, reading `base_rgb` from `CONTENT_FILL` (the
two agree by construction). This table stays palette + metrics only.

**Taste gate is CLOSED — APPROVED** (iteration 3, Peter, 2026-07-26). The visual
verdict has been taken on the kit these numbers come from, so they are no longer
provisional. Because every consumer will read the names and never the literals, a
later verdict change still edits that one file.

**Wiring is a follow-up arc.** Nothing consumes the table yet; `wm.rs`,
`screen.rs` and fbcon are untouched by CRISPY-PI. The module is byte-inert by
construction (all `const`, no statics, no code, compile-time-only assertions), and
that was verified rather than assumed: `target/pi_baremetal/kernel8.img` hashes
identically with and without the change. `./arroyo check` clean on both arches;
`./arroyo kernel8-test 90` MBENCH 80/80 required witnesses, 0 forbidden.

#### CRISPY-PI-2 — the re-lift from the approved kit

CRISPY-PI lifted `us-crispy` `08b42ede` with the taste gate open, and said in as many
words that a verdict change would edit that one file. The verdict came in on
**iteration 3** (Peter, 2026-07-26) and moved the source of record to
`us-crispy-modern` `0787ba9f`. CRISPY-PI-2 is that edit, and it is the shared-source
law working as designed: the kit changed first, the table followed, and no consumer
had to be touched because there are still no consumers reading literals.

Nothing structural changed. The rounding rule was **re-verified at the new commit**,
not assumed: `libs/quartzite/src/theme.rs` @ `0787ba9f` still converts a channel with
`(v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8`, so every literal was re-derived in `f32`
by that same rule. Colours are still `0x00RRGGBB` with no invented alpha, the gloss
scalars are still Q16, the Paper block is still not lifted for the same
material-versus-chrome reason, and the fidelity claim still reaches exactly as far as
flat fills.

What moved:

* **Palette.** 19 of the 21 roles changed value; `content_fill` and `gloss.highlight`
  are byte-identical to CRISPY-PI's. The kit as a whole moves lighter and cooler — the
  chrome face goes `0x00D6D5D3` → `0x00ECECEE`, the frame keyline lifts from near-black
  `0x004F4E4D` to a mid grey `0x00B4B4B9`, and the title gradients compress into a much
  shallower ramp.
* **The controls.** `CONTROL_BOX`'s single square role is gone; iteration 3 draws
  **three circles**, each with its own fill — `CONTROL_CLOSE`, `CONTROL_MID`,
  `CONTROL_ZOOM`, a ramp of the accent hue from darkest to lightest. The json carries no
  separate radius key, so the geometry stays one lifted number (`control_box`, now read
  as a diameter) with `CONTROL_RADIUS` derived from it.
* **New metrics.** `widget_radius` = 8 and `well_radius` = 15 — the kit now distinguishes
  a raised widget's corner from a recessed well's, where before every corner was the
  window's.
* **Gloss.** `0.14 / 0.5 / 0.0`, down from `0.34 / 0.55 / 0.06`: a much subtler
  highlight that now dies out completely at the bottom instead of leaving a floor.

**The assertions were re-derived, not carried.** The const-assert blocks state
relations the *new* json implies. Two are worth naming because they are exactly the
traps a mechanical re-lift sets. `BEVEL_LIGHT` and `GLOSS_HIGHLIGHT` are both pure
white at this commit, so no distinctness is asserted between them and a comment says
why — asserting it would fail the build, and silently dropping it would lose the fact.
`GLOSS_BOTTOM_ALPHA_Q16` is exactly `0`, which makes the old `<= Q16_ONE` bound on it
vacuously true, so it is removed in favour of the direction assertion
(`TOP > BOTTOM`) that still does real work. `CORNER_RADIUS < TITLE_HEIGHT` survives on
the new numbers (12 < 34), as does `CONTROL_BOX < TITLE_HEIGHT`; new bounds cover the
two new radii and the circular control.

**Still byte-inert, still verified rather than assumed.**
`target/pi_baremetal/kernel8.img` hashes `104981f1864f2bbf…` both with and without the
change — the table has no consumers, so a wholesale swap of every literal in it must
not move a single byte of the image, and does not.

##### CRISPY-PI-2 gate results (2026-07-26, QEMU raspi4b)

`./arroyo check`: both arches clean. `./arroyo kernel8`: clean.
`./arroyo kernel8-test`: **84/84 required witnesses, 0 forbidden**, 3123 lines scanned.

One process note worth keeping. At the brief's 90 s window the harness returned
84/84 with 0 forbidden but marked the capture `TRUNCATED (INCONCLUSIVE)` — the boot
was cut mid-line at 1991 lines. Every witness had in fact printed, so it was tempting
to read it as a pass; it is not one, by the harness's own rule, and it was re-run at
150 s to get the conclusive 3123-line result above. A green count inside a truncated
capture proves only that nothing had failed *yet*.

### WC-L — the staged erase loses its DIRECT fallback

WC-K staged the desktop fill but kept `fill_rect` as the last resort, and reported it. The metal
answered on the P64 attended boot (capture `pi4-r23s1o`, lines 2041/2054):

```
[wc-k] erase box=514x526 staged=no reason=lock -> DIRECT
```

Twice, both on focus tab-cycle transitions (`[wc-c] focus tab-cycle`, `[wc-fv] focus shell hidden=2`)
under ~99% core load. Every other erase that boot was `staged=yes … -> BUFFERED` and the rollup said
`TEAR-FREE`. The spec FORBIDs both `staged=no` and `-> DIRECT`, so this was a structural red, not a
cosmetic one: under the exact conditions the discipline exists for — contention — the erase path was
writing the desktop fill straight into the buffer the HVS was scanning, which is the writing shape
WC-G convicted and WC-K existed to remove. **The fallback did not make WC-K robust; it made WC-K
conditional on nothing else wanting the lock.**

#### Deferred damage, not a direct write

There is no direct fill behind `stage_fill` any more. The four decline reasons split by whether a
retry can ever succeed, and the split is the design:

* **`lock`, `alloc` — transient.** Another core holds `STAGE` right now, or the heap could not grow
  right now. The box is pushed onto `DEFER` as *deferred damage* — a desktop-colour repaint owed —
  and `drain_deferred` erases it through the staged path on the next composite pass.
* **`geom`, `cap` — permanent for that box.** A degenerate rect and an over-cap row are the same next
  pass as this one, so deferring them would queue work that can never come off and the drain would
  re-defer forever. They are dropped, counted as `declines`, and reported `-> LOST` so the existing
  `-> UNSTAGED` FORBID catches them. Neither is reachable on any panel this kernel drives (a single
  row is at most `width * 4` bytes, three orders of magnitude under `MAX_STAGE_BYTES`); the branch
  exists so that if one ever becomes reachable it is a loud red rather than a silent spin.

The drain is **WC-J's `reclaim` shape reused, not a second queue with its own rules**: erase,
`damage_intersecting`, `request_full_present`. It runs at the head of `composite_inner`, *before* the
dirty-set snapshot, so the windows the deferred paint reached are repainted by that same pass — a
deferral costs one frame of latency and never a second composite. It runs *before* the F4
`BlitGuard` for the reason the WC-I cursor bracket states: it acquires `SPRITE` (through `undraw`)
and `TABLE`, and the drain barrier's termination argument requires that a draining teardown wait only
on bounded blits, never on another core's lock.

**Lock order.** `DEFER` is a leaf. It is acquired only by `defer_erase` and `drain_deferred`, held for
an array copy, and no other lock — `TABLE`, `STAGE`, `WRITER`, `SPRITE` — is ever taken while it is
held. The queue is emptied into a local snapshot before any staging is attempted, and the `alloc`
path drops the `STAGE` guard before queueing. That is what lets `DEFER` use a blocking `lock()` where
`STAGE` uses `try_lock`: a deferral has nowhere left to fall back to, so losing a box would lose the
repaint outright, and a leaf held across no acquisition cannot invert anything. No new inversion is
introduced.

On a full queue (`MAX_DEFER` = `MAX_WINDOWS`) a box is unioned into an existing entry rather than
dropped. Sound for the same reason `reclaim` is: the drain re-damages every window the painted box
intersects, so enlarging it costs repaint work and can never leave a window with a bite taken out of
it. Dropping would leave a dead window's last frame on the panel for the rest of the boot — the P61
ghost. The victim is the entry whose union adds the **least area**, not slot 0: always unioning into
slot 0 grows without bound, dragging one entry's corners out until it covers most of the panel and
every later drain repaints most of the panel and re-damages every window on it.

#### What guarantees a deferred box is ever painted

The drain runs at the head of `composite_inner`, so the ordinary liveness argument is "some window
presents, which composites, which drains". That argument is weakest exactly where it is least
obvious: **a box is deferred because a window was torn down, and if it was the last window there is
nothing left to present.** The only thing still running is then WC-E's periodic desktop
flush → `wm::service_damage`.

Checking that path showed the first cut had a real hole, not merely an undocumented assumption:
`service_damage` early-returned unless some row was `used && damaged`, and with no windows left no
row is either. A box deferred by the last closing window would have sat on the queue for the rest of
the boot, showing that window's final frame — the exact P61 ghost WC-J removed, re-entering by a new
route. `service_damage` therefore now also runs the pass when `DEFER_N` is non-zero.

So: **the desktop's flush cadence, not a window present, is what bounds a deferral's latency in the
general case.** The dependency is load-bearing and worth restating — if WC-E's periodic flush were
removed or made conditional on a live window layer, the deferred queue would lose its only guarantee
in precisely the case it was built for. The cost on the idle path is one relaxed atomic load ahead of
a table-lock acquisition that was already happening.

**Arch-neutral.** The deferred-erase path — queue, drain, defer/drop decision, the `DEFER_*` reason
constants — is uncfg'd and compiled on x86 too, because removing a direct front-buffer write is a
correctness change and `wm.rs` is shared. Only the *reporting* is gated: `video::wcg` is
`aarch64 + witness` only (pre-existing, `video/mod.rs`), so no `[wc-k]` line of any kind is reachable
on x86 — which also means neither `staged=no` nor `-> DIRECT` can be printed on either arch.

#### The witness

`staged=defer` and `-> DEFERRED`, deliberately **not** `staged=no`/`-> DIRECT`. Both forbidden strings
name the tearing regime; a deferral is neither, and giving it that vocabulary would either fail honest
boots or force the FORBID to be loosened — and the FORBID is the arc's verdict. Deferral lines are
unbudgeted to a 16-line spam bound, on WC-K's reasoning: what they report is a fill the panel has *not
received yet*, and a boot that starts deferring after the rollup has fired must still be visible.

The rollup gains `defers=`, `redefers=` and `coalesced=`. `defers=` and `coalesced=` do not enter the
verdict precedence: a deferral that *arrives* came through the staged path one pass late, so it
neither tore nor went direct, and demoting `TEAR-FREE` for it would make the honest report of
contention indistinguishable from the regime the arc removed.

`redefers=` does enter it. A requeue is a repaint that has **not happened**, and past `E_REDEFER_MAX`
(8, one full queue's worth) the honest reading is that the erase path is not draining — which on the
panel is a dead window's frame where the desktop should be. Printing `TEAR-FREE` over a visible ghost
would be WC-K's mistake repeated in a new place: a verdict describing the samples it liked rather
than the panel. The new `-> STARVED` sits below `-> UNSTAGED` in the precedence, because a starved
box may still arrive (delayed, not lost) where a dropped one provably never will.

Starvation also gets its own one-shot `scope=starve` line. The sampled rollup fires at fill 4, and
starvation by its nature arrives late — it needs a loaded, long-running desktop, not a boot's first
four fills — and a rollup that has already printed cannot retract. Same reasoning that makes the
deferral lines unbudgeted: a FORBID is only worth having if the boot can still trip it.

#### The cursor, and why the drain returns "sprite disturbed" rather than "painted"

`drain_deferred` returns whether it took the sprite off the panel, **not** whether it painted a box.
The first cut returned `painted`, which was a bug of the same family as the one the arc fixes: the
drain undrew the sprite, every box then re-deferred, `composite_inner` saw `disturbed = false`, took
the `Untouched` tail — and the sprite was removed and never restored, every pass, for as long as the
contention lasted. That is, precisely the conditions this arc exists for. A pointer that vanishes
under load is not a lesser failure than a torn erase.

The undraw is still lazy, because undrawing on every drain would re-create WC-I's spotty sprite (an
undraw/repaint per composite, on every core, is what WC-I removed). A `STAGE.try_lock()` probe runs
*before* the queue is emptied or the sprite touched: if the staging lock is unavailable — the
dominant contention case, and the one P64 caught — nothing can be painted this pass, so the queue is
left intact and the sprite is never disturbed. The probe is advisory, not a reservation; the guard is
dropped and `stage_fill` takes the lock itself. Losing that race costs one wasted pass with the
bracket taken, which is benign in the direction that matters: it can cost a repaint, never skip one.

#### The QEMU fixture, and what it does not prove

QEMU has no contention of its own, which is exactly why WC-K shipped a fallback nobody had seen fire.
A witness build therefore forces **one** deferral per boot: a one-shot latch in `stage_fill`, the WC-H
fallback fixture's shape, gated on `!requeued` so the drain's retry is guaranteed to take the real
staged path and the queued box is provably delivered rather than cycling.

It proves the `-> DEFERRED` line, the queue round trip, the drain's re-damage, and the `BUFFERED`
erase one pass later. It does **not** prove behaviour under genuine lock contention; that proof rides
the next metal boot, and `redefers=` is where it will show.

#### WC-L gate results (2026-07-26, QEMU raspi4b)

`./arroyo kernel8-test 90` — **81/81 required witnesses, 0 forbidden**, 1886 lines scanned:

```
[wc-k] erase box=192x192 staged=defer reason=lock requeued=no -> DEFERRED
[wc-k] erase box=192x192 staged=yes rowbytes=768 runs=192 contig=yes compose_us=178 present_us=542 rectscan_us=6666 torn=no -> BUFFERED
[wc-k] erase box=130x142 staged=yes rowbytes=520 runs=142 contig=yes compose_us=7 present_us=182 rectscan_us=4930 torn=no -> BUFFERED
[wc-k] rollup scope=fills samples=4 rows=618 torn=0 noncontig=0 declines=0 defers=1 redefers=0 coalesced=0 maxpresent_us=542 frame_us=16667 -> TEAR-FREE
```

The forced 192x192 deferral comes back `BUFFERED` on the next pass — the round trip, end to end.
`declines=0`, `redefers=0`, `coalesced=0`. `./arroyo check`: both arches clean.

#### Metal watch-list (WC-L)

* The two P64 `-> DIRECT` lines must not recur — they are now unreachable by construction, so their
  return would mean a fallback was reintroduced, not that contention got worse.
* `defers=` non-zero on the bench is *expected and fine*; `redefers=` non-zero is the signal that the
  staging lock is held longer than a composite interval.
* Tab-cycle focus transitions under load: a deferred erase is one frame late, so a vacated box may
  show its old contents for a single frame. That is the accepted cost. What must never appear is a
  *partially* filled box or a horizontal band boundary.
* `coalesced=` non-zero would mean more than `MAX_WINDOWS` boxes owed at once — worth understanding,
  though the union keeps it correct.

### WC-M — the staging cap stops being a present cap

WC-H bounded the window back-layer with `MAX_STAGE_BYTES` (4 MiB, `video/wm.rs`) and made an over-cap
box **decline to the DIRECT path** — the pre-WC-H, per-pixel, scan-out-visible upscale that WC-G
convicted. That was defensible while every window was small. It stops being defensible the moment the
console becomes a window: the bench panel is 1920x1200 ARGB, so one full-panel box is ~8.8 MiB, over
twice the cap. The largest and most conspicuous present in the system was precisely the one guaranteed
to tear, and `[wc-h] win=… staged=no reason=cap -> DIRECT` was the whole of the design's answer.

This is the same mistake WC-K already fixed once, in the fill path and for the same reason. WC-K's own
comment says it outright: refusing to allocate `w * h` for a full-panel erase "is what makes a
full-panel erase fit under `MAX_STAGE_BYTES` at all instead of declining on the cap and falling
straight back into the tearing regime for the largest boxes, which are exactly the ones that tear
worst." A fill escaped by composing one row; a window cannot, because its rows all differ. So it gets
the other escape: **stage it in bands.**

#### The banding

`chunk_rows = MAX_STAGE_BYTES / row_bytes` — how many whole rows of the box fit under the cap. The
pass composes band 0 into `STAGE` and presents its rows, then composes band 1 into the *same* buffer
and presents those, until the box is done. Three properties carry the design:

* **The band is the only thing that changed about the compose.** `paint_window` is still called with
  the whole box (`bx, by, bw, bh`) so the chrome and the upscale keep their true geometry across a
  seam; what moves is the *destination origin*, which is now the band's first panel row. That makes
  the destination-local vertical origin **signed** — every band after the first has the box's top
  border, its title strip and its first source rows above its own row 0 — so `lby`/`cy` are `isize`,
  the three chrome writes go through a clipping `fill_rect_v`, and `draw_title` takes an `isize` `y`.
* **The present is bit-for-bit WC-H's present.** One bulk `copy_nonoverlapping` per row, at the
  panel's row stride, over the band's rows. Nothing about the panel-facing half was redesigned.
* **A banded present costs ONE compose, not one per band.** `paint_window` starts each band at the
  first source row that lands in it (`sy_first`, which also handles a source row straddling the seam
  under `dup` replication) and `break`s at the first row past it. Without that the scheme would be
  quadratic in band count.

**A single band is the pre-WC-M path, exactly.** When the box fits the cap, `chunk_rows == bh`, the
loop runs once at origin `(bx, by)`, the vertical origins are non-negative, the buffer is the size
WC-H allocated, and the sprite offer is unconditional as before. Every window on the bench today and
every window in the QEMU regression takes that branch, which is why the small-present fast path is
unchanged rather than merely equivalent.

The only cap decline left is a box whose **single row** exceeds 4 MiB — a 1 048 576-pixel scanline,
unreachable on any panel this kernel can address. `DECL_CAP` is kept for it rather than deleted.

#### The visibility window, stated honestly

**A banded present is not atomic, and this arc does not claim it is.** Band 0's rows are on the panel
while band 1 is still composing, so for the length of one band's compose the panel can hold the new
top of the window over the old bottom of it. What is guaranteed instead:

* **Every seam is a full ROW boundary.** A band is a whole number of complete rows, each still
  delivered in one bulk copy, so no scanline is ever half-old and half-new. The horizontal tear WC-G
  convicted and WC-H removed cannot return through this path.
* **The seam count is bounded and known**: at most `ceil(bh / chunk_rows) - 1`, at fixed row offsets,
  for one compose each. A 1920x1200 box at 4 bytes/pixel is 546 rows per band — 3 bands, 2 seams.
* **Nothing is lost or duplicated.** Each row is composed by exactly one band and presented once, and
  `paint_window`'s dense border fill covers each band completely, so WC-H's "every pixel the present
  copies was written by this pass" invariant holds per band.

The atomic alternative — one buffer the size of the whole box — *is* the panel-sized allocation the
cap exists to refuse. What banding replaces is not an atomic present; it is the DIRECT path, whose
entire scattered upscale was visible to the scan-out from the first poke to the last. A bounded number
of row-aligned seams is strictly better than that, and that is the whole of the trade.

One mitigation, named as a mitigation and not a guarantee: `draw_window`'s single `flush_range` still
runs once, after all bands, over the box's whole row span, so on the non-coherent Pi 4 the bands tend
to become visible together. A cache line may be evicted at any point, so the seams above are described
as real.

#### What did NOT change

* **Deferred erase is untouched.** WC-K/WC-L's `stage_fill` / `defer_erase` / `drain_deferred` path is
  not modified by this arc — same one-row compose, same transient/permanent decline split, same
  absence of a `fill_rect` fallback. `MAX_STAGE_BYTES` still bounds `stage_fill`'s single row, which
  is the one place the constant is still a hard refusal.
* **No new lock, no new lock order, no new spinning.** `STAGE` is taken once, with `try_lock`, and
  held across all bands of one present — the same guard, the same duration class, the same holder. No
  lock is acquired inside the band loop that was not acquired inside WC-H's single pass, so the
  `BlitGuard` window's audited-exception list (WEDGE-1) is unchanged and needs no new entry. The hold
  is *longer* on a banded present, which is a hold-time change and not a class change: the holder
  still cannot block unboundedly, which is the invariant the drain barrier actually needs.
* **No protection weakened.** No page permission, checksum or bounds check is relaxed; the band layer
  is bounded by its own `len` and `height`, which is a *tighter* fence on the compose than WC-H's
  full-box layer was.
* **No witness signature moved.** `wcg::stage_note` is called once per present as before, with the
  two halves accumulated across bands, so `[wc-h]` reports the same two quantities for the same
  operation. That keeps `video/wcg.rs` out of the diff — deliberate, because the x86 tree wants this
  change and the smaller the arch-neutral surface, the cleaner that port is.

#### WC-M gate results

**NOT RUN — recorded as not run.** This session's environment has no `cc` (the host build scripts for
`heapless`/`smoltcp`/`compiler_builtins` cannot link), no libc development files, and no
`qemu-system-aarch64`. `./arroyo check` fails with ``error: linker `cc` not found`` before reaching
any kernel source, and `./arroyo kernel8-test 150` cannot start. **No claim is made about either
gate.**

What *was* run: the four changed functions (`fill_rect_v`, `paint_window`, `draw_title`,
`stage_window`) were extracted against a stub `FrameBuffer`/`Window` and type-checked with
`rustc --emit=metadata`, in both the plain and the `aarch64 + witness` arm (cfg attributes stripped,
witness callees stubbed). Both compile clean, no errors and no warnings. That covers the
signed/unsigned arithmetic, which is this arc's only real compile risk. It is **not** a substitute for
either gate: the next session on a complete toolchain must run `./arroyo check` (both arches) and
`./arroyo kernel8-test 150` and record the verdicts here before this is treated as landed.

Note for whoever runs it: the QEMU raspi4b panel is 640x480, so every window in the regression is a
single band and the expected result is a *no-change* pass — `[wc-h] … -> BUFFERED`, the
`reason=fixture -> DIRECT` line, and the `-> TEAR-FREE` rollup, unchanged. To exercise the banding at
all the run needs the bench geometry: `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 150`.

#### Metal watch-list (WC-M)

* **A `[wc-h] … -> BUFFERED` line whose `bytes=` exceeds 4 194 304 is the arc working.** No such line
  could exist before WC-M; it is the banding's own witness, and no new instrument was needed to get
  it. `box=` alongside it names the geometry that banded.
* `reason=cap -> DIRECT` should now be **unreachable**. If it appears, a box's single row is over the
  cap — which would mean a panel width this kernel was never supposed to see, and is worth stopping
  for rather than explaining away.
* `compose_us`/`present_us` on a banded present are sums over the bands. `torn=` compares the *total*
  present against the *whole box's* scan time, which is the right comparison for "did the panel outrun
  us", but it cannot see a seam. **The seam is a visual check, not a counter** — a full-panel window
  redrawing under load should show no horizontal band boundary settling in at a fixed row. If one is
  visible, the mitigation above is failing and the honest fix is a vblank-aware present, not a bigger
  cap.
* The sprite is offered to at most one band, and only to a band containing its box outright, so a
  pointer resting on a seam takes the WC-I repaint tail instead of riding the present. Expect overlay
  take-rates to dip slightly while the pointer sits on a seam row of a banded window; a large dip
  would mean the seam placement wants to avoid the sprite, which this arc deliberately does not
  attempt.

### CURSOR-5 — the flash was our own arrow, and the lock class stops costing the whole sprite

CURSOR-4 closed the 42 % straddle decline and Peter's attended **P64** verdict narrowed what was left
to two sentences: *"mouse still spotty [over vug] and causes a flash in the vug display here and there
if you tweak the mouse just so."* Two symptoms, two mechanisms. One is a proven ordering defect; the
other is a duty cycle that CURSOR-3 and CURSOR-4 both left in place for the contended case.

#### Mechanism A — the flash: a full undraw from inside an open overlay session

**Proven by construction, not inferred from a symptom.** `composite_inner` ran in this order:

1. `cursor::sprite_plan()` → `cursor::overlay_open(&p)` — the pass takes ownership of the overlay
   session and snapshots the sprite's geometry and generation into `OVERLAY`.
2. `cursor::undraw_within(&paint[..])` — the masked undraw hands back only the pixels this pass may
   paint over.
3. **`drain_deferred(&fb)`** — WC-L's deferred-erase drain, which calls `cursor::undraw()`: a *full*
   undraw that clears `Sprite::drawn` and bumps `Sprite::epoch`.
4. the window loop, each `stage_window` calling `cursor::compose_into(&layer, …, plan)`.

Step 3 destroys the premise of step 4, and step 4 could not see it. `compose_into` may not take the
`SPRITE` lock — it runs inside `wm`'s `BlitGuard` window, and F4's drain barrier spins IRQ-masked
until every registered blit retires, so a second blocking lock in that wait set breaks its termination
argument. It therefore validated the plan against `overlay_matches`, which compares the plan against
*the session's own copy of the plan*. That copy is untouched by step 3. Every downstream compose
matched, painted the arrow into the window's back layer, and the present put it on the panel — while
the sprite module believed itself off-panel.

The tail then completes the damage. `adopt_overlay` finds `!sp.drawn`, declares the session
incoherent, and falls back to `refresh_locked`: `undraw_locked` is a no-op (nothing is drawn), and
`draw_locked` reads the **front** for its save-under — where the overlay's own `FILL` is now sitting.
The arrow is captured as "what was underneath", and it stands in the window's rect until something
else damages that window. `repair` cannot mend it: `draw_locked` restored nothing, so no rect is
handed to `damage_intersecting`.

Its trigger set is exactly Peter's sentence. It needs the `DEFER` queue non-empty (an empty queue
costs one relaxed load and returns before touching the sprite), which under VUGPAR means the focus
tab-cycles at ~99 % core load that WC-L shipped for — and it needs the pointer over a presenting
window. Rare, load-dependent, pointer-dependent: *here and there, if you tweak the mouse just so.*

**Two fixes, one for each half of the race.**

* **The drain moves ahead of the bracket.** With `drain_deferred` first, a drain that undraws leaves
  `sprite_plan()` empty, no session is opened at all, and the pass's `Repaint` tail re-establishes the
  sprite from the finished front buffer. Everything WC-L's placement argument required survives: the
  drain still runs before the dirty-set snapshot, so the windows its `damage_intersecting` reaches are
  repainted by *this* pass; it still runs outside the `BlitGuard` window, so neither `SPRITE` nor
  `TABLE` enters the drain barrier's wait set; and it still reports `disturbed` when it undrew rather
  than when it painted (WC-L MUST-FIX 1 — a drain whose boxes all re-defer has still taken the sprite
  down, and an `Untouched` tail there would leave the pointer missing for as long as the contention
  lasts).
* **`compose_into` gets a lock-free generation check.** The ordering fix cannot reach the callers that
  are not this pass: `wm::erase` on another core, and `repaint` from the render task. `cursor::EPOCH`
  mirrors `Sprite::epoch` into an `AtomicU64`, bumped by the one `bump_epoch` helper every existing
  bump now routes through, and `compose_into` compares `live_epoch()` against `plan.epoch` with a
  `Acquire` load — no wait and no entry in the drain barrier's wait set. A mismatch declines the
  compose *totally*: nothing is written to the layer, and
  the coverage bits inside this window's box are cleared so the tail repaints those pixels from the
  finished front. That is CURSOR-3's fallback for one window, which is always available and always
  correct.

  **What the generation check is not.** An earlier draft of this section claimed "a stale read is a
  conservative read". That is false, and the falsity matters: the failure mode is not a reader seeing
  a value that is too new, it is a reader seeing an OLD value that still equals `plan.epoch` — exactly
  the retired plan the check exists to reject, waved through. The check **narrows** the window between
  a retiring undraw and a compose; it does not close it, and no lock-free test of a concurrently
  changing value could. What closes the residual is the layer behind it, which was always there:
  `adopt_overlay` re-checks `ov.epoch == sp.epoch` with the sprite lock *held*, so a compose that
  slipped through is caught at the tail and settled by a whole-sprite `refresh_locked` rather than by
  an install. The check turns a certainty into a race; the tail turns the race into a repaint;
  `selfsave` is what would show the residual actually biting. The store is `Release` and the load
  `Acquire` because the thing a reader must not reorder against the generation is the undraw's *pixel
  writes* — the ordering does real work even though the value itself is advisory.

#### Mechanism B — the spottiness: the lock class took the whole sprite down

Under CURSOR-4 a pass that lost the overlay session to another core fell all the way back to
CURSOR-3's **whole-sprite** bracket — `cursor::undraw()`, the entire arrow off the panel for the
length of the pass. That is precisely the duty cycle WC-I and CURSOR-4 spent two arcs removing,
reinstated in full every time two cores composite at once. Under VUGPAR (vugband pinned at 99 % on
three cores, the P64 reality) two-cores-at-once is not the exceptional case, it is the steady state.

The undraw's reason has never been the session. A pixel must be handed back before a painter *in this
pass* overwrites it, or the save-under goes stale — and `paint` is already the conservative union of
exactly those extents. That argument is independent of who owns the overlay, so the mask applies to
the sessionless path too: pixels this pass can reach come down, pixels it provably cannot stay on the
panel. **WC-L's shape, exactly — defer the sprite operation, never drop the sprite.**

**The sessionless mask is a different operation from the session owner's, and the first cut missed
it.** CURSOR-5's initial commit routed the declining pass straight into `undraw_within`, on the
argument that the mask's justification is independent of who owns the session. The *pixel* argument is
sound; the *bookkeeping* one is not, and the gap reintroduced both P64 symptoms on the new path.
Pass A owns the session and has composed and presented sprite pixel `P`; pass B mask-undraws `P`, the
colour guard passes (the panel really does hold the sprite's colour there — A just put it there), and
B writes its OWN stale `saved[P]` into A's window rect: **the flash**. `undraw_within_locked` does not
bump the generation, so A's `adopt_overlay` still finds its session coherent, installs `ov.saved[P]`
over B's damage and clears the off bit — the module now believes the sprite is on the panel where B
has painted something else: **the hole**. CURSOR-3 and CURSOR-4 were immune only by accident, because
their decline branch took a full undraw, which bumps.

So the sessionless path is its own entry point, `cursor::undraw_within_nosession`, and it bumps the
generation whenever it actually hands a pixel back. Any such pixel has changed owner behind the
session-holder's back, and the generation is precisely the channel that says so: A's `compose_into`
then declines, A's `adopt_overlay` goes incoherent, and A's tail is the whole-sprite refresh —
CURSOR-3's already-proven fallback, now bought on purpose and only when a pixel really moved. The bump
is conditional rather than unconditional because the common case is a pass whose paint set never meets
the sprite, and a pass that hands nothing back has not disturbed anyone's premise. Clearing the open
session's coverage bits instead was rejected as strictly weaker: it repairs the hole but leaves B's
stale pixel inside A's window rect, since A would repaint `P` from the front where that value sits.

The tail stays `Repaint`, not `Adopt`: no session means no coverage to install, and `refresh_locked`
settles both classes in one acquisition — `undraw_locked` skips the pixels the mask handed back (their
`saved` is stale by construction, which is what `Sprite::off` records) and restores the rest, then
`draw_locked` re-establishes the whole sprite from the front, where every pixel now holds whichever
painter's content is final.

WC-F's reserved-box case keeps the full undraw and is now a separate branch rather than an `else`:
that probe paints the **front** at the tail of the pass, outside any window box, so the paint set does
not describe what it will touch and `repair` — which damages *windows* — could not mend a sprite pixel
it took.

#### The residual, made legible

```
[cursor5] rollup scope={fixture|desktop} stale_compose= adopt_incoh= selfsave= masked_nosession= drain_insession= -> VERDICT
```

Printed immediately after `[cursor3]`'s rollup, same scopes, same harness, so a bench capture reads
"how often the mechanism ran" and "what it cost when it raced" as one block.

* `stale_compose` — composes declined on a generation mismatch. **Mechanism A, caught rather than
  painted.** Non-zero means the interleave still *happens* and is now absorbed; it measures contention,
  not damage.
* `adopt_incoh` — tails that found their session incoherent and fell back to the whole-sprite refresh.
  Before CURSOR-5 this was the branch that stamped the arrow; it now merely costs a repaint.
* `selfsave` — save-unders that read back **exactly** the colour the sprite paints at that pixel while
  the module believed the sprite was not on the panel there. An **upper bound** on self-capture, not a
  proof of it: window content is free to contain `FILL`-white or `SHADOW`-dark pixels of its own and
  this cannot tell them apart. A boot where it stays 0 has provably not stamped; a boot where it
  climbs in step with `stale_compose` is showing the mechanism still leaking.
* `masked_nosession` — mechanism B's fix firing. Every one of these is a whole-sprite bracket that did
  not happen.
* `drain_insession` — the direct detector for mechanism A's ordering. **Must be 0**, because the
  reorder makes it structurally impossible; a non-zero count means someone put the drain back inside
  the bracket. It is the one line item that reads as a defect rather than as load, and the verdict says
  `REGRESSED` on it. The spec `FORBID`s both the verdict and the field.

  The invariant it guards is **per-pass**, and scoping it correctly is what keeps it from lying. The
  first cut tested the global session flag and counted a contended `try_lock` as busy — so a healthy
  metal boot, where another core is legitimately mid-session while this one drains (the VUGPAR steady
  state this whole arc is about), would have driven the counter up and reported `REGRESSED`. A false
  red costs Peter a bench boot chasing a bug that is not there, which is worse than no counter at all.
  The test is therefore scoped to the core that *opened* the session: a pass composites on one core and
  its drain is on that same core, so "the open session belongs to this core" is exactly "this pass is
  mid-session", and nothing wider. Cross-core sessions are invisible here by construction and correctly
  so — they are absorbed by the generation check and counted as `stale_compose`, where they read as
  load. `Overlay::owner_cpu` is diagnostic only; nothing in the mechanism reads it.

Verdicts: `REGRESSED` (drain in session), `UNWITNESSED` (the sprite was never armed — QEMU, always),
`RESIDUAL` (`selfsave > 0`), `COHERENT` otherwise.

#### CURSOR-5 gate results (2026-07-26, QEMU raspi4b)

`./arroyo kernel8-test 90` — **82/82 required witnesses, 0 forbidden**, 2192 lines scanned:

```
[cursor3] rollup scope=fixture planned=0 offers=0 taken=0 adopt=0 repaint=1 ensure=348 straddle=0 lock=0 budget=0 stale=0 -> UNWITNESSED
[cursor5] rollup scope=fixture stale_compose=0 adopt_incoh=0 selfsave=0 masked_nosession=0 drain_insession=0 -> UNWITNESSED
```

`[cursor3]`'s rollup gains a fourth decline bucket, `stale=`, fed by `Composed::stale`. A generation
decline is a real class and `offers - taken` is the number a reader reconciles the breakdown against,
so leaving it visible only in `[cursor5]` would have read as an unexplained gap in the mechanism
rather than as the absorbed race it is.

`./arroyo check`: both arches clean. `./arroyo kernel8`: clean.

**What the gate does and does not prove, stated rather than implied.** QEMU raspi4b delivers no HID
pointer report, so `pal::cursor::visible()` is false for the whole boot, the sprite is never drawn,
and every CURSOR-5 counter is 0 by construction — `UNWITNESSED`, exactly as CURSOR-3's and CURSOR-4's
rollups are. The gate proves no-regression: the window path still settles every pass through a tail,
the reordered drain still delivers its deferred box `BUFFERED` one pass later (WC-L's fixture is
unchanged and still passes), and nothing new is printed on a quiet boot.

Mechanism A is proven **in code**, by the ordering above, and its fix is proven by the same reading.
Neither mechanism can be *reproduced* on the gate. What rides the next metal boot is the observation,
not the argument.

#### Metal watch-list (CURSOR-5)

* `drain_insession=0` — if this is ever non-zero, stop; the ordering has regressed.
* `stale_compose` non-zero with `selfsave=0` is the **good** outcome: the race is happening and is
  being declined instead of painted.
* `selfsave` climbing with `stale_compose` would mean a third writer reaching the sprite's pixels that
  neither the ordering nor the generation check covers. That is the next thread to pull.
* `masked_nosession` non-zero on the bench is expected under VUGPAR and is the direct measure of
  mechanism B's fix; `[cursor3]`'s `lock=` counts the same passes, so the two must agree.
* The attended question is unchanged in shape: hold the pointer over a live vug under `vugband` load,
  tweak it, and report whether the flash recurs and whether the spottiness is gone or merely reduced.

### CURSOR-6 — the counters were measuring the module, not the panel

CURSOR-5 shipped five counters and P65v2 (attended, capture `pi4-r23s1o`) returned every one of them
silent — `[cursor5] rollup scope=desktop stale_compose=0 adopt_incoh=0 selfsave=0 masked_nosession=0
drain_insession=0 -> COHERENT` — while Peter still saw a spotty cursor and a vug-window flash "if you
tweak the mouse just so". A `COHERENT` verdict alongside a visibly broken panel is not a contradiction
to be explained away; it is the shape of the arc's next question, and the answer is that every
CURSOR-3/4/5 counter is taken from **inside the sprite module's own bookkeeping** — plans, sessions,
generations, coverage bits. A painter that overwrites the arrow's pixels without ever consulting the
module leaves all of that bookkeeping perfectly self-consistent. The instruments could not have seen
the symptom no matter how bad it got.

#### The `48 % decline` was an artefact, and it sent the last two sittings the wrong way

The P64-era desktop wire reads `offers=33790 taken=16111 straddle=18443`, which invites "the overlay
mechanism declines half the time, and straddling is why". It is not what those numbers say. An offer
is made to **every staged window of a pass that holds a plan** — `stage_window` has no overlap test,
deliberately, because a window that paints over sprite pixels without composing them must still clear
their coverage (`overlay_uncover`). The sprite is over one window; every other window in the pass is
offered a sprite it misses **entirely**, and `Composed::missed > 0` counted that identically to a
genuine partial carry. With `offers / planned` ≈ 1.6, the arithmetic is not a mechanism failing — it
is one window taking the sprite and its neighbours being asked.

`[cursor3]`'s rollup therefore splits the class and keeps the old total beside it:

* `disjoint` — `taken == 0 && missed > 0`. The sprite was nowhere in this window. Not a decline, not a
  loss; the shape of the offer set.
* `partial` — `taken > 0 && missed > 0`. A real straddle, composed in part — how often the CURSOR-4
  split actually runs.

`straddle` is retained as `disjoint + partial` so P64/P65 captures stay comparable line-for-line.

#### The live box: the sprite's geometry, readable by painters

Measuring "a painter took the arrow's pixels" needs the sprite's box readable from inside `wm`'s
`BlitGuard` window and from the desktop's row loop. Neither may take `SPRITE`: F4's drain barrier
spins IRQ-masked until every registered blit retires, and a blocking sprite lock there is exactly the
wait that argument excludes. So the box is **mirrored into relaxed atomics** beside `EPOCH`
(`cursor::LIVE_ON` / `LIVE_BOX`, read through `cursor::live_box_relaxed`), on the same discipline and
with the same honesty about what a lock-free read can promise: the answer may be one pointer report
stale. Nothing in the mechanism reads it — **no pixel decision is taken from it** — and the publish is
deliberately biased: `draw_locked` publishes *before* the pixels go down and `undraw_locked` retracts
*after* the restore, so the window in which the mirror disagrees with the panel is always the one
where it claims a sprite that is not yet there, never the one where it denies a sprite that is. A
diagnostic that can over-count is usable; one that can under-count the thing it exists to find is not.
A masked undraw does **not** retract — part of the sprite is still up, and retracting would blind the
counter to exactly the straddle case.

#### PROVEN and fixed — the dropped coverage clear

`overlay_uncover` `try_lock`s `OVERLAY` (it runs inside the blit guard) and CURSOR-4 discarded a
contended clear silently, on the argument that "the only writer that could be holding it is another
pass, which has already declined to share the session — its coverage is not ours to correct". **That
is not the only writer.** `overlay_open` and `adopt_overlay` both take `OVERLAY` with a blocking
`lock()` from outside the guard, and another core runs one of those on every composite; a pass merely
*probing* for the session it is about to be refused holds the lock for as long as it takes to read
`ov.session`. A clear can therefore be lost while **our** session is the live one.

What that costs is not a missed optimisation. The window has just painted its own content over pixels
a lower window's `compose_into` claimed, and the coverage bit for those pixels is still set.
`adopt_overlay` then installs the lower layer's save-under for them and clears their `off` bit: the
module now believes the arrow is on the panel where this window's content is (**the arrow is
missing** — spotty), and the next undraw's colour guard sees the lower layer's saved value and writes
it back into the upper window's rect (**a stale patch inside a live window** — the flash). Both P65
symptoms, from one dropped `try_lock`, and invisible to every CURSOR-5 counter because neither the
generation nor the geometry moved.

The fix is the fallback the arc already trusts everywhere else. `overlay_uncover` is now
`#[must_use] -> bool`; a `false` answer calls `cursor::note_uncover_lost()`, one relaxed store inside
the guard (no lock, so the drain's wait set is unchanged). `adopt_overlay` **swaps** the flag — per-pass
state, retired by the tail on every exit — and folds it into the `coherent` term, routing the pass to
`refresh_locked`, which re-establishes the whole sprite from the finished front buffer. That is
CURSOR-3's fallback: always available, always correct. A session mismatch still returns `true`, because
for *that* case CURSOR-4's argument does hold — there is nothing of ours to clear.

The two coherence questions are **AND-ed for the decision and counted separately for the evidence.**
`c5_coherent` (session, drawn, generation, geometry) is computed on CURSOR-5's own terms with no
reference to the lost clear, and `adopt_incoh` is bumped on `!c5_coherent` alone; `coherent` is then
`c5_coherent && !uncover_lost`. The arc's first cut suppressed `adopt_incoh` whenever a lost clear
coincided, which would have hidden a **real** CURSOR-5 incoherence behind a CURSOR-6 one. A pass that
hits both now appears in both, which is the truth and is what lets a bench reader add them up — a
silent counter is exactly what made P65v2 unreadable, and the fix for that must not create another.

**The fix has a price, and the rollup prints it.** Every dropped clear costs the pass a
`refresh_locked` — the whole-sprite duty cycle CURSOR-4 and CURSOR-5 spent two arcs removing — so
`uncover_lost` is reported as `lost/planned`, the fraction of sessions paying it. A few per thousand
is the absorbed race the fix exists for. A large fraction under VUGPAR would mean the `try_lock` is
contended often enough that the right answer is to make `overlay_uncover` not need the lock at all,
rather than to keep buying correctness with refreshes; that is a next-arc decision the number makes
available rather than one this arc pre-empts.

**Residual, bounded at one pass rather than zero.** Only the session owner can set the flag and only
`adopt_overlay` clears it, so the ordinary path sets and retires it within one pass. A pass that sets
it and then settles on a `Repaint` tail (the session lost between the set and the tail) leaves it
standing for the *next* session, which takes a `refresh_locked` it did not earn — one spurious
whole-sprite repaint, one pass late, and then clear. It cannot accumulate (an `AtomicBool`, not a
count) or persist (the next adopt always swaps it down). A spurious repaint is the same cost this fix
pays deliberately everywhere else, so this is the honest ceiling, not a hole.

#### INSTRUMENTED — the two overlap counters, and the fifth decline class

`[cursor6] rollup scope=… present_over=N masked=N desktop_over=N mismatch=N uncover_lost=N/planned -> V`

* **`present_over`** — a window present whose front-buffer row blit covered part of the live sprite box
  while the pass held **no overlay plan at all**, so nothing had handed those pixels back first. This
  is the spotty mechanism stated as a measurement: they held the arrow before the blit and hold window
  content after it, and no path tells the sprite module. Taken in `draw_window`, after the blit, from
  `live_box_relaxed` and two relaxed atomics.
* **`masked`** — the denominator: the same event with a plan in hand. A plan means `composite_inner`'s
  masked undraw ran over every window that meets the sprite (the paint set is built independently of
  the per-window `may_overlay` exclusion), so those pixels were handed back before the blit and the
  tail owes them a repaint. Healthy. Without it `present_over=0` would be unreadable — no defect and
  "the pointer was never over a presenting window" look identical.
* **`desktop_over`** — the same question for the desktop layer, taken in `Screen::present_background`
  over the spans that **survive** WC-I's occluder subtraction, and separately on the VUG-PAR band path
  (which returns before the serial loop and would otherwise be blind on the windowless full-screen
  frame it exists for). Latched per present, not per span — the unit that means something is "one
  desktop present erased the arrow", and a per-span count would report the arrow's height.
  **Not a gate FORBID, and that is a decision.** The render task brackets its own flush
  (`cursor::undraw` → `pal.render` → `cursor::repaint`), which is why the first cut forbade any hit.
  But the mirror is deliberately over-count-biased and the HID router calls `cursor::repaint` from its
  own core, so an arrow *arriving* while a flush is mid-loop registers a real, healthy, transient
  overlap. Failing a correct metal boot on that would cost Peter a sitting chasing nothing — the same
  trap CURSOR-5's `drain_insession` core-scoping was written to avoid. It is a verdict term
  (`-> UNBRACKETED`) and a watch-list item: a reader looks there first, and a **sustained** count, not
  a handful, is what would mean the bracket is genuinely broken.
* **`mismatch`** — a `compose_into` that declined because the open session did not describe *this* plan.
  CURSOR-4 left this exit silent, so it was the one decline class absent from `offers - taken`.
* **`uncover_lost`** — dropped clears, each now absorbed by a whole-sprite refresh rather than left to
  corrupt a session.

Verdicts: `UNBRACKETED` (`desktop_over > 0` — look there first; sustained, not a handful),
`UNWITNESSED` (the sprite was never armed — QEMU, always — or no present ever met it, `over == masked
== 0`), `OVERWRITTEN` (`present_over > 0`), `INTACT` otherwise.

#### CURSOR-6 gate results (2026-07-26, QEMU raspi4b)

`./arroyo check`: both arches clean. `./arroyo kernel8`: clean. `./arroyo kernel8-test 150`: full PASS,
0 forbidden.

```
[cursor3] rollup scope=fixture planned=0 offers=0 taken=0 adopt=0 repaint=1 ensure=348 straddle=0 disjoint=0 partial=0 lock=0 budget=0 stale=0 -> UNWITNESSED
[cursor5] rollup scope=fixture stale_compose=0 adopt_incoh=0 selfsave=0 masked_nosession=0 drain_insession=0 -> UNWITNESSED
[cursor6] rollup scope=fixture present_over=0 masked=0 desktop_over=0 mismatch=0 uncover_lost=0/0 -> UNWITNESSED
```

Same caveat as every cursor arc, and it is load-bearing here: QEMU raspi4b delivers no HID pointer
report, so the sprite is never drawn, `live_box_relaxed()` is `None` for the whole boot, and every
counter above is 0 by construction. The gate proves the wiring and no-regression. The dropped-clear
fix is proven **in code**, by the lock-holder argument above; the two overlap counters are proven only
in the sense that they are wired and quiet. What rides the next metal boot is the observation.

#### Metal watch-list (CURSOR-6)

* `desktop_over` — a handful is the arriving-arrow race and is expected; a count that tracks the
  desktop's flush rate means the bracket is broken and that is the whole mechanism, no further
  hypothesis needed. Deliberately not a gate FORBID (see above).
* `present_over` non-zero **names** the mechanism: window presents are erasing an unbracketed arrow.
  The fix then belongs in the plan snapshot — `composite_inner` takes `sprite_plan()` once, before the
  dirty set, and a sprite that arrives after it is invisible to the whole pass.
* `present_over=0` with `masked` large and the symptom still present retires this hypothesis too, and
  the remaining writers to the arrow's pixels are outside both present paths. That is the next thread.
  `masked=0` means the question was not asked — get the pointer over a presenting vug and re-run.
* `disjoint` should account for essentially all of the old `straddle`; `partial` is the honest count of
  how often the CURSOR-4 split runs. If `partial` turns out to be large after all, the straddle class is
  back on the table — the P65 reading was simply not able to say.
* `uncover_lost=N/planned=M` is the fix's PRICE as well as its measure: each one is a session that
  would have installed a stale save and now takes a whole-sprite refresh instead. A few per thousand
  is the absorbed race; a large fraction under VUGPAR means `overlay_uncover` should stop needing the
  lock rather than keep buying correctness with refreshes.
* `adopt_incoh` and `uncover_lost` overlap freely and are counted independently, so they may be added
  up. Neither suppresses the other.

### CURSOR-7 — the window present stops erasing the sprite

CURSOR-6's counter was answered on silicon. P67v2, instrumented boot:

```
[cursor6] scope=desktop present_over=9 masked=519 uncover_lost=0/524 -> OVERWRITTEN
```

`uncover_lost=0` retires CURSOR-6's own fix as a live mechanism and CURSOR-5's paths with it: nothing
is leaking there. What remains is `present_over` — window presents landing on a live arrow that
nothing had handed back, about nine per second — beside the bench symptom, heavy tearing on pointer
motion and a spotty cursor under a six-vug storm. This arc closes that class.

#### The two things `present_over` was counting

**One was a misclassification, and naming it is not a softening.** CURSOR-6 split the two classes on
`cur.is_some()` — whether an overlay PLAN reached `draw_window`. But a pass that loses `overlay_open`
to a concurrent composite (the VUGPAR steady state, and CURSOR-5's whole subject) takes no plan and
still calls `undraw_within_nosession` over its paint set and still settles on a `Repaint` tail. Those
pixels *were* handed back and *are* owed a repaint; they were being charged as defects. The split is
now taken on the pass's `disturbed`, which is exactly "this pass's tail owes the sprite a repaint" —
`Adopt` and `Repaint` both do, `Untouched` does not.

**The rest was real, and its shape is the plan snapshot.** `composite_inner` calls `sprite_plan()`
once, before the dirty-set snapshot; a sprite that ARRIVES or MOVES after that instant is invisible to
the entire pass. The HID router repaints the arrow at report rate (~125 Hz) from its own core while
presents run from the presenting task's core, so pointer motion is precisely the regime that maximises
the race — which is why the bench symptom is tearing *on movement*. The pass then blitted window
content over the arrow, took an `Untouched` tail (`ensure_drawn`, a no-op while `sp.drawn`), and
**nothing repainted it until the pointer moved again**. Every CURSOR-5 counter reads `COHERENT`
throughout, because the sprite module's own bookkeeping never learns anything happened.

#### The mechanism: arm inside the guard, repair at the tail

The natural fix — hide the sprite before the blit, restore after — is not available where the blit
happens. `draw_window`/`stage_window` run inside `wm`'s `BlitGuard` window, and F4's drain barrier
spins IRQ-masked and unpreemptible until every registered blit retires: a blocking `SPRITE`
acquisition there is exactly the wait its termination argument excludes (§WEDGE-1's audited-exception
list is law, and this arc adds nothing to it). CURSOR-3/4/5 spend their whole design on that
constraint; the only sprite work admissible inside the guard is `OVERLAY.try_lock` and relaxed atomics.

So the detection stays where it was and gains a consequence:

* `draw_window`, after the present, tests `live_box_relaxed()` against this window's clipped outer box
  — two relaxed atomics, no lock, unchanged. The call is **no longer `#[cfg(feature = "witness")]`**:
  it is mechanism now, and a production build that skipped it would keep the defect.
* When the pass is not `bracketed`, `note_present_over_sprite` sets `cursor::PRESENT_DIRTY`, one
  relaxed store. No lock, no allocation, no serial, nothing added to the drain's wait set.
* `wm::composite` — outside the guard, on exactly the footing the `Repaint` tail has had since WC-I —
  `take_present_dirty()`s the flag and runs a whole-sprite `cursor::repaint()`. That is
  "composite the sprite on top after the blit": `undraw_locked`'s colour guard declines the pixels the
  present overwrote (so no stale content is stamped into a window's rect), and `draw_locked` then
  re-establishes the arrow and its save-under from the finished front buffer.

Three details are load-bearing:

* **The flag is read BEFORE the tail runs.** A pass already headed for `Repaint` does not repaint
  twice; a pass headed for `Untouched` is upgraded to `Repaint`.
* **`Adopt` is never downgraded.** `adopt_overlay` is the only closer of the overlay session — a pass
  that skipped it would leak the session and lock the overlay mechanism out for the rest of the boot.
  The repair is appended *after* it instead.
* **The flag is global and coalescing.** The sprite is global and the repair is whole-sprite, so a
  pass on core B repairing an arrow core A trampled is the right outcome, not a crossed wire. N
  unbracketed presents cost at most one repaint per tail — the fix must not reinstate the per-present
  duty cycle WC-I and CURSOR-4 spent two arcs removing.

#### The residual, stated rather than implied

The arrow is absent from the panel between the offending blit and the tail — the rest of the window
loop, bounded by one composite pass. This does **not** make the present atomic with the sprite; the
path that is atomic (`compose_into`, the sprite riding the present inside the back layer) is unchanged
and still carries the bracketed case. What changes is that the absence is bounded by a pass instead of
being unbounded, and the module's belief about the panel is re-established every time it is falsified.

Cost, in the steady state: a stationary pointer over a live vug makes `hit` true, so `disturbed` is
true, so nothing arms — the fix is silent. Arming happens when the sprite arrived or moved after the
plan snapshot, i.e. during motion, where the HID path is repainting the arrow anyway. The extra
repaints are co-timed with the symptom they fix.

#### What the counters mean now

`[cursor6] rollup scope=… present_over=N masked=N repaired=N desktop_over=N mismatch=N uncover_lost=N/planned -> V`

* **`masked`** now includes the sessionless masked undraw (split on `disturbed`, not `cur.is_some()`),
  so it will be larger on the next metal boot than P67v2's 519 and `present_over` correspondingly
  smaller. That is the reclassification, not an improvement, and the two must be read together.
* **`present_over`** is no longer an unrepaired overwrite. Each one now arms a tail repaint, so it is
  a **cost** counter — how often the sprite must be re-established from the finished front — rather
  than a damage counter.
* **`repaired`** is new: tail repairs actually taken. The flag coalesces, so this counts repair
  PASSES, never presents or pixels; `repaired <= present_over` always, and a ratio well under 1 means
  several presents per pass met the arrow and were settled together — the fix working, not a gap.
* **Verdicts.** `UNBRACKETED` (`desktop_over > 0`, unchanged, look there first); `UNWITNESSED`
  (`over == masked == 0`, or the sprite was never armed — QEMU, always); **`OVERWRITTEN` now means
  `present_over > 0` AND `repaired == 0`**, i.e. presents met the arrow and nothing repaired it, which
  is a regression in `wm::composite`'s tail rather than a steady state; `REPAIRED` (`present_over > 0`
  with repairs running); `INTACT` otherwise.

#### CURSOR-7 gate results

Not run in this session's environment: the container has no C compiler (`cc`/`gcc` absent, no glibc
link objects) and no `qemu-system-aarch64`, so `./arroyo check` cannot build the host build scripts
(`heapless`, `smoltcp`, `compiler_builtins`) and `./arroyo kernel8-test` cannot boot. Both files were
parse-checked with `rustfmt` and the change is reviewed by construction. **`./arroyo check` and
`./arroyo kernel8-test 150` are owed before this arc is merged.** As with every cursor arc, the QEMU
gate can only prove wiring and no-regression: raspi4b delivers no HID pointer report, the sprite is
never drawn, `live_box_relaxed()` is `None` for the whole boot, and every counter above is 0
(`UNWITNESSED`).

#### Metal watch-list (CURSOR-7)

* `repaired > 0` with the tearing gone is the arc landing. `repaired` climbing while the symptom
  survives means the repair is running and is not sufficient — the next thread is the plan snapshot
  itself (make the pass see a sprite that arrives mid-pass), not another counter.
* `present_over > 0` with `repaired == 0` is `OVERWRITTEN` and means the tail is not consuming the
  flag. That is a code regression in `wm::composite`, not load.
* `present_over` should fall sharply against P67v2's 9/s purely from the reclassification, with the
  balance appearing in `masked`. If it does not, the sessionless path is not the bulk of it and the
  arriving-sprite race is larger than assumed.
* Watch `[cursor3]`'s `repaint=` beside `repaired=`: the tail repaint is a whole-sprite duty cycle,
  and if it starts tracking the present rate rather than the motion rate the fix has become the
  churn WC-I removed and belongs behind a rate limit or in the plan snapshot instead.

> **That last bullet was the arc's own tripwire, and P69 tripped it.** See §CURSOR-8.

### CURSOR-8 — the repair is a request, not a command

CURSOR-7's watch-list named the failure mode and P69 met it, minutes after the bench boot: *"mouse
worse than ever over a vug — unusable"*, plus *"keystrokes visible in the background cause flashes in
the vug"*. The tail repair was tracking the PRESENT rate.

#### The cost model that was wrong

CURSOR-7 reasoned: *a stationary pointer over a live vug makes `hit` true, so `disturbed` is true, so
nothing arms — the fix is silent, and arming only happens during motion.* That is true of a SINGLE
compositing core and false of the VUGPAR steady state, which is the only state the symptom lives in.
The unbracketed-and-live combination is common there, and structurally so:

* a masked undraw on core A deliberately does **not** retract `LIVE_ON` (part of the sprite is still
  up; a reader that concluded "no sprite" would undercount exactly the straddle case CURSOR-6 was
  built to see), so the box stays advertised for the whole of A's bracket;
* meanwhile core B enters `composite_inner`, its own `sprite_plan()` comes back empty or its own
  window set does not meet the sprite, so B's `disturbed` is false;
* B blits, `live_box_relaxed()` says the box is live, `bracketed` is false — B arms;
* `verify_window` also passes `bracketed = false` unconditionally, by CURSOR-7's own conservative
  choice, so every read-back verify arms too.

Six vugs at ~50 fps is several hundred armings a second, and CURSOR-7 granted every request.

#### The loop, and why the keystroke report is the same bug

The repair is not merely expensive; it is **self-sustaining**:

1. a vug presents; the present meets the live sprite box unbracketed (above);
2. `PRESENT_DIRTY` is armed; the consuming tail runs a whole-sprite `cursor::repaint()`;
3. `repaint` ends in `cursor::repair`, which calls `wm::damage_intersecting` over the restored rect —
   marking every window UNDER the sprite damaged. That is by design and predates this arc: the colour
   guard can leave stale pixels inside a window's rect and only a redraw from the app's surface mends
   them;
4. the next composite therefore re-blits that whole window from its surface — and that present meets
   the sprite again, at (1).

The cursor is unusable because it spends its life mid-restore — precisely the duty cycle WC-I and
CURSOR-3 exist to remove — and the vug flashes because every turn of the loop re-blits it whole.

**The keystroke flash is the second-order view of the same loop, and is NOT a separate seed.** A
keystroke echoes to the console; the console is the DESKTOP surface, not a window; `Screen::flush`
ends in `wm::service_damage`, which exists to service exactly the damage `cursor::repair` leaves
behind. So each keypress *cashes* the sprite damage the repair storm had already queued, into a full
re-blit of the focused vug — one flash per keypress. No console pixel ever lands on the vug: WC-I's
`occluders` subtraction in `Screen::present_background` removes the window boxes from the desktop's
own damage before it copies, and `[cursor6] desktop_over` is the counter that would say otherwise (it
must stay 0, and a non-zero reading there would be a *different* bug and its own seed). The keystroke
supplies the CADENCE, not the pixels. Bounding the repair rate bounds both symptoms; if the flashes
survive with `[cursor8] suppressed_rate` climbing and `repairs` at the motion rate, then the console
present is an independent mechanism after all and gets its own arc.

#### What landed

Nothing about the arming changed — the flag still means what CURSOR-7 says it means, and it is still
a relaxed store inside the `BlitGuard` window. What moved is the DECISION to spend a repaint on it,
into `cursor::take_present_dirty`, which runs outside the guard where a clock and the sprite's own
generation are readable. Two tests, cheapest first, both lock-free, no new lock and no new lock order
(§WEDGE-1's list is untouched):

* **Stale** — `live_epoch() != PRESENT_DIRTY_EPOCH` (the generation recorded at arming time) means a
  full restore → save → draw cycle has run since the offending present: from the HID router's
  `repaint`, from `wm::erase`, or from the deferred-erase drain. That cycle already re-established the
  arrow and its save-under from the finished front, so the request is stale and buys nothing. This is
  the "were the sprite's pixels actually disturbed" test in the only form affordable here: the
  pixel-exact form (compare the panel against `Sprite::saved` over the box) is a read-back of exactly
  the pixels the repair would rewrite, with `SPRITE` held — it *is* the repair.
* **Rate** — at most one granted repair per `REPAIR_MIN_MS` = **8 ms**, the 125 Hz HID report period.
  The bound is the MOTION rate because motion is the only thing a whole-sprite repaint can
  legitimately track, and the router already pays a repaint per report. A parked sprite over a
  churning vug now repairs at 125 Hz worst case instead of at aggregate present rate, which puts the
  loop's gain below one. A frame bound (16 ms) would leave a visibly late arrow on a fast drag; a
  present-rate bound is the thing being removed.

**A deferred request is re-armed, never dropped.** Deferring by up to 8 ms is a bounded latency;
dropping is the unbounded absence CURSOR-7 exists to close, and the two must not be conflated. The
first pass past the floor takes it, and a pass always comes — every present runs a tail, and
`service_damage` runs on the desktop's cadence even with no window presenting.

The clock is read through a lane-local `mono_now_hz()` in `video/cursor.rs`, calling the same public
arch accessors `clock::monotonic()` calls (`arch::timer::cntfrq` + `arch::now_cycles` on aarch64;
`arch::apic::tsc_hz` + `arch::now_cycles` on x86_64). `clock` exposes whole `uptime_secs()` and raw
`mono_ticks()` but no millisecond reading outside the `logts` feature, and adding one is an edit to a
shared kernel-core file outside this arc's lane. **It is duplication, it is flagged as duplication in
the source, and it should be folded into `clock` as a `pub fn mono_ms()` when a session owns that
file.** Where no counter is readable the repair is granted *and counted as `unclocked`*: declining
would reinstate CURSOR-6's unbounded absence on a machine whose only fault is an uncalibrated
counter, and granting silently would let the rollup read as paced when it is not.

`wm.rs`'s diff is one call — the new rollup, beside `[cursor6]`'s. `composite`'s tail is unchanged:
it still asks `take_present_dirty()` and still appends the repair after `Adopt` rather than
downgrading it. The whole mechanism sits in the consumer, which is what kept the shared-surface diff
to a single line ahead of the CLICK-ROUTE arc.

#### What the counters mean now

`[cursor8] repair rate scope=… requests=N repairs=N suppressed_stale=N suppressed_rate=N unclocked=N floor_ms=8 flush_kb=N -> V`

(`flush_kb` is CURSOR-10's; see below. It is a cost counter and does not enter the verdict.)

* **`requests`** — flag CONSUMPTIONS, not armings. The flag coalesces, so `requests <= present_over`
  by construction, and the ratio between them is how much the coalescing already absorbs.
* **`repairs`** — GRANTED, and the same counter as `[cursor6] repaired=`. Two lines that disagreed
  about how often the arrow was rebuilt would be worse than one.
* **`suppressed_stale`** — a full sprite cycle beat us to it. Load, not damage.
* **`suppressed_rate`** — inside the floor, DEFERRED (re-armed), never lost. This is the number that
  says the storm was actually caught.
* **`unclocked`** — granted with no floor applied. Must be 0 on both bench platforms (CNTFRQ is
  architectural on the Pi; x86 is calibrated long before the GUI); non-zero means the limiter is not
  RUNNING, which reads differently from "not needed".
* **Verdicts.** `UNCLOCKED` (checked first — any unclocked grant makes the rest unreadable);
  `UNWITNESSED` (`requests == 0`, which on QEMU raspi4b is every boot); `LIMITED`
  (`suppressed_rate > 0`, the limiter biting); `PACED` otherwise.

`[cursor6] repaired=` narrows in the same breath: it now counts GRANTED repairs, so
`repaired / present_over` is the coalescing AND the limiter together, and `[cursor8]` splits the two.
`present_over > 0 && repaired == 0` therefore acquires a second, benign reading it did not have under
CURSOR-7 — every request so far fell inside the floor — and the `OVERWRITTEN` verdict is kept, with
the instruction to read the next line rather than to go looking in `wm::composite`.

#### CURSOR-8 gate results (2026-07-28, QEMU raspi4b)

Both gates RUN and green in this session. The container's missing `cc`/`qemu-system-aarch64` were
bridged with `/run/host` wrappers; the host toolchain additionally needed `--sysroot=/run/host` to
reach glibc's link objects (the container ships a runtime libc but no crt objects or `.so` link
scripts), which is what defeated the CURSOR-7 session.

```
./arroyo check          ✅ x86_64 OK   ✅ aarch64 OK
UNAOS_QMP_PORT=4487 ./arroyo kernel8-test 210
  ✅ MBENCH PASS — 86/86 required witnesses, 0 forbidden hit(s), 15593 lines scanned
[cursor8] repair rate scope=fixture requests=0 repairs=0 suppressed_stale=0 suppressed_rate=0 unclocked=0 floor_ms=8 -> UNWITNESSED
```

`UNWITNESSED` is the only honest QEMU verdict, and the reason the gate proves wiring and
no-regression only: raspi4b delivers no HID pointer report, so the sprite is never drawn,
`live_box_relaxed()` is `None` for the whole boot, and nothing can arm. **CURSOR-7's own gates were
never run; this run is the first to cover both arcs**, and it clears CURSOR-7's outstanding debt.

#### Metal watch-list (CURSOR-8)

* The arc lands if the mouse is usable over a presenting vug with `suppressed_rate` climbing and
  `repairs` sitting near the motion rate. That is the storm caught and the repair still working.
* `repairs` still tracking present rate with `suppressed_rate == 0` means 8 ms is too low a floor for
  the observed present rate — but check `unclocked` first, because an unreadable counter produces the
  same shape for a completely different reason.
* `suppressed_stale` large relative to `requests` is good news and a hint: most requests are already
  being settled by somebody else's full cycle, and the arming test in `draw_window` could then be
  narrowed further (the next candidate: do not arm at all while another core holds an overlay
  session).
* Flashes in the vug surviving a green `[cursor8]` retires the "same mechanism family" finding above
  and makes the console present its own seed. `[cursor6] desktop_over` is the first counter to read
  in that case; it must still be 0.
* `[cursor3] repaint=` beside `[cursor8] repairs=` remains the honest ratio to watch. CURSOR-7's
  tripwire is still armed — this arc bounded the RATE, it did not remove the whole-sprite duty cycle,
  and the structural fix (a plan snapshot that sees a sprite arriving mid-pass) is still owed.

### CURSOR-10 — one pointer report, one cache sweep

CURSOR-9's root-cause named a second cost on the same path and left it standing: `refresh_locked` is
`undraw_locked` + `draw_locked` under one `SPRITE` acquisition, and **each phase ended in its own
`flush_box`, which cleans WHOLE PANEL SCANLINES over the sprite's height**. On the bench panel
(1920x1200, 4 B/px) that is ~36 x 1920 x 4 B ≈ 276 KB per phase, ~553 KB per HID report, ~69 MB/s of
`DC CVAC` at the 125 Hz report rate — for a sprite that can have dirtied at most ~36 x 36 px.

Two separable defects, both closed here, both entirely inside `video/cursor.rs`:

* **Two sweeps, and an arrow-less panel between them.** The undraw's flush PUBLISHES the restored
  rows to the HVS before the draw has put the arrow back. Both phases now defer their clean into one
  `FlushUnion` (the bounding box of what they dirtied), flushed once after both, with the lock still
  held. The intermediate state is never published, and the sweep is halved before any other change.
* **Whole scanlines for a 36 px sprite.** `flush_range` takes a byte range, so a column-bounded clean
  is one call per row — `h` calls where `h` is the SPRITE's height, not the panel's. `flush_rect` does
  that, falling back to the contiguous whole-span form once the columns cover ≥ 3/4 of the row, where
  `h` `dsb sy` barriers would cost more than the bytes they save.

**Cache-line alignment is not a correctness condition**, and the code says so: `cache::clean_range`
rounds the start down to the 64 B D-cache line and iterates to the end, so a rect off a line boundary
cleans a few neighbouring pixels too. `DC CVAC` writes a dirty line back — it never invalidates and
never discards — so cleaning a byte we did not write publishes that byte's newest value, including
another core's own data, which that core will clean again regardless. Misalignment costs bytes, never
correctness.

Estimated per report on the bench panel: **~553 KB → ~9 KB** (union column span ≈ sprite width plus
the inter-report motion delta, ~40 px ≈ 160 B, plus line rounding, over ~36 rows). The bound in the
degenerate case — a teleporting pointer whose union spans the panel — is one full-scanline sweep,
i.e. **still exactly half** of what this path did before, never more.

Untouched by construction: the saved-under logic (nothing here reads or writes `saved`, `off`, or the
epoch), the `[wc-d]` argument (every byte either phase wrote is still cleaned, once, before the lock
is released — including on the paths where the draw declines or fails, where the restore still owes
RAM its clean), and the standalone `undraw` / `ensure_drawn` / masked entry points, which keep their
own `flush_box` for their other callers.

**QEMU cannot witness this.** raspi4b delivers no HID pointer report, so `refresh_locked` never runs
from the router and the coalesced path is never taken; the gate proves no-regression and nothing more.
`[cursor8] flush_kb=` is the bench instrument: against an unchanged motion profile it should fall by
roughly the panel-width-to-sprite-width ratio.

### WEDGE-1 — P66's mechanism is UNKNOWN; this arc hardens and instruments

**Verdict first: this arc did not root-cause P66.** It landed two safe-direction changes and two
instruments so that the next wedge of this shape names itself. Nothing below should be read as a
diagnosis, and P66 must not be retired on it.

#### What P66 actually showed

Six vugs (PROCS-6), the operator click-pausing them one at a time and tab-cycling between them, and
then the wire went silent: no panic, no exception, no watchdog line. The last two lines were

```
[wc-fv] focus raise asid=0x6 windows=1 top_win=6 z=131 shell_z=120
:: SCHED: load c0=-- c1=-- c2=54% c3=-- ::
```

Three of four cores reporting `--` — `tracked()` false, i.e. they had stopped folding load spans
altogether — with c2 still turning over at 54 %. In the four raises before it, `[sched6] mean` climbed
351 188 → 2 253 430 → 6 233 952 → 8 502 171 cyc/pass, so the composite path was being strangled before
it stopped. **Three cores left the dispatch loop and did not come back. Why, is not known.**

#### The hypothesis this arc proposed, and why it is retracted

The first cut of this section argued the wedge was `DrainBarrier::drain` (which does spin IRQ-masked
and unpreemptible, reached from `sched::exit` → `boot::teardown_user_slot` →
`syscall::clear_handle_row` → `close_owner`) waiting on a composite that was itself blocked on the
`WRITER.lock()` taken inside the `BlitGuard` window, with `SPRITE` dragged into the wait set behind it
and SPREAD-2's band placement amplifying one dead core into three.

**Review refuted it on four independent points, all of them checkable and all of them decisive:**

1. All 27 `WRITER.lock()` sites in the tree are single-statement `Copy` reads (`*WRITER.lock()`); the
   guard dies at the semicolon. No holder ever blocks on a second lock, so `WRITER` cannot convoy.
2. `SPRITE` is the **outer** lock of the `SPRITE → WRITER` pair. The claimed back-door runs backwards:
   a wait set containing `WRITER` does not thereby contain `SPRITE`.
3. `JoinHandle::join` blocks the **task**, not the core — it is a semaphore park. A flusher stuck on a
   band does not stop its core from dispatching, so it cannot produce a `--` reading.
4. `BlitGuard::enter` has exactly **one** call site (`wm.rs`, inside `composite_inner`) and is never
   reached from `flush_parallel`. The proposed composite-versus-band interaction does not exist.

The retracted argument is kept here, and in the code comments at both sites, so it is not re-derived
by the next reader.

#### What landed, and on what grounds

Both changes are justified on their own merits as safe-direction hardening. Neither is claimed to fix
anything observed.

* **The framebuffer handle is taken before `BlitGuard::enter()`** (`video/wm.rs`). A pure ordering
  change — the handle is `Copy`, same value, same early return. Independent of any wedge theory it is
  strictly better: the `is_ready()` early return now precedes registration, so a pass that cannot draw
  no longer registers as in-flight at all, and the barrier waits on a slightly narrower window.
* **Band eligibility is freshness, not `tracked`** (`video/screen.rs`, `arch/aarch64/sched.rs`). This
  closes a genuine latent defect that is *not* the wedge: bands spawn pinned (`steal_ok: false`) with
  an untimed `join()`, so a band handed to a core that has stopped dispatching parks its flusher
  permanently. `tracked`'s ~2-window (~500 ms) slack exists to keep a live core off the `--` *display*
  during a slow rollover and is far too loose to gate a scheduling decision. `CoreLoad` now carries
  `fold_age_cyc` and `sched::dispatch_fresh_cyc()` (~30 ms) is the bound. A live core folds a span
  every dispatch pass and keeps its full fan-out; when no helper qualifies, `flush_parallel` returns
  `false` and the byte-identical serial path presents the frame. **Scope, stated: a parked flusher
  costs a vug its render task. It does not stall a core and cannot explain P66.**

#### The guard-window invariant, stated honestly

It is **not** "no blocking lock is acquired inside the `BlitGuard` window" — that is false, and was
false before this arc. On witness builds the window contains:

| Site | Lock | Why it is a bounded hold |
|---|---|---|
| `[wc-a]` / `[wc-c]` pass-tail prints, every print in `verify_window` | `SERIAL_PORT` | TX FIFO waits are explicitly spin-bounded (`arch/aarch64/serial.rs`, `break` past 1 000 000 spins) |
| `wcg::begin` / `wcg::end` / `wcg::stage_flush` | `SERIAL_PORT`, `FBCON` | same bound; `FBCON` is `try_lock` on every print path |
| `stage_window` buffer growth | global allocator | `allocate_first_fit` is O(free list) |

The invariant the barrier actually needs is weaker and true:

> **No lock acquired inside the window has a holder that can block unboundedly.**

The three exceptions above are audited against exactly that and pass. They are recorded rather than
removed: stripping instrumentation was not this arc's brief.

#### The spinlock hypothesis, checked and closed

The one account that would explain three cores leaving the dispatch loop at once is "something blocks
unboundedly with a spinlock held". A focused pass over every `spin::Mutex` reachable from the storm
path — `WRITER`, `TABLE`, `DEFER`, `STAGE`, `COMPAT_CREATE`, `SPRITE`, `OVERLAY`, `SERIAL_PORT`,
`FBCON`, the global allocator, `sched::RUN_QUEUES` — asking only whether a holder can spin or block
without bound while holding, and whether it is taken IRQ-masked: **nothing qualifies.** The two
genuinely unbounded constructs on the path (`DrainBarrier::drain`'s spin, `flush_parallel`'s untimed
join) are both reached with no lock held. `SPRITE`/`OVERLAY` were also checked for an ABBA inversion
and are clean — `adopt_overlay` is the only site holding both, always `SPRITE → OVERLAY`, and nothing
holds `OVERLAY` while blocking on `SPRITE`.

That closes the hypothesis without producing a suspect. **The tripwires below are P67's answer.**

#### The instruments

* `DrainBarrier::drain` counts its spins and, once per boot past a threshold no bounded blit can reach
  (~10⁸ spin hints), prints
  `:: [wedge1] DRAIN STALLED core=N blit_active=M pending=K spins=... == tripwire ::`
  and goes back to spinning. The spin is unchanged — returning early would hand a teardown's
  about-to-be-unmapped surface to an in-flight blit. Honest about its own risk: the core is IRQ-masked
  and `serial_println!` takes a blocking lock, so if a future wedge is *in* the serial path the
  tripwire blocks — but that path was spinning forever anyway, so it is strictly no worse.
* `[spread2] ... stale N` counts band placements declined for staleness.

#### WEDGE-1 gate results (2026-07-26, QEMU raspi4b)

`./arroyo check` green both arches; `kernel8` clean; `kernel8-test 150` — **MBENCH PASS, 86/86
required witnesses, 0 forbidden**.

**What QEMU did not prove.** The gate emits no `[spread2]` and no `[vugfps] bands=` line at all, before
this change or after: the parallel flush path is not reached by the regression boot (it needs `vugpar`
+ `baremetal`, live vugs, and `PAR_MIN_ROWS` of damage). The gate proves compilation, no-regression,
and an intact serial fallback. It does not exercise either change.

#### Metal watch-list (WEDGE-1)

* `[spread2] ... stale N` non-zero means cores are dropping out of the dispatch loop long enough to be
  caught — a fact currently unmeasured and worth having. It is **not** confirmation of any account of
  P66. Deliberately not a gate FORBID: a decline is the safe outcome, not a failure.
* `[wedge1] DRAIN STALLED` naming a core would locate the next wedge at this spin. Read `blit_active`
  with it: a small non-zero count is a composite that cannot finish, and the next question is which
  lock it is on.
* **If the machine wedges again with `stale 0` and no tripwire line, neither instrument is at the
  site** — which is the likely outcome given the refutation above, and is itself information. The
  three cores that stopped are the thing to chase; the next arc should get a per-core heartbeat that
  survives an IRQ-masked spin (the tripwire only fires from inside one specific loop) before it
  theorises further.

> **Superseded by §WEDGE-1r2.** The third bullet's "no tripwire line ⇒ this instrument is not at the
> site" is not a sound inference, and §WEDGE-2 below turned it into a settled premise. Read
> §WEDGE-1r2 before drawing anything from a quiet `[wedge1]`.

### WEDGE-1r2 — the drain tripwire's silence was not evidence

**This is a finding about an INSTRUMENT, not about a mechanism. No behaviour changes, no fix, no new
lock; the spin is byte-for-byte what WEDGE-1 left.**

§WEDGE-2 opens by treating one inference as settled: *"The drain barrier is exonerated. WEDGE-1's
`[wedge1] DRAIN STALLED` tripwire fires from inside the drain-barrier spin, and it stayed silent all
three times."* Reading a silence as a refutation is only sound where the instrument could have spoken
in the states it is being cleared of. This one could not, in two of them:

1. **It speaks through `serial_println!`, which takes a blocking lock.** WEDGE-1's own note concedes
   this ("if the wedge is IN the serial path the tripwire blocks") and answers it with "strictly no
   worse" — a claim about the *machine*, true, that was then used as a claim about the *evidence*,
   where it is false. s44's capture stopped **mid-word**, i.e. a core died holding the UART. On that
   shape the tripwire's silence is structurally guaranteed and carries no information at all. A
   blocked print does not merely fail to report the stall; it erases the fact that the threshold was
   ever crossed.
2. **It exists only INSIDE the spin.** Every teardown reaches the barrier through its own `TABLE`
   critical section — `close`/`close_owner` clear rows under the lock, `move_to` rewrites geometry
   under it — and a core that dies waiting on `TABLE` there never reaches `drain()`. The teardown
   path can therefore *be* the wedge site with the tripwire silent by construction, because the
   instrument sits one lock downstream of the death. `close_owner` is the sharp case: it is reached
   from `sched::exit` → `clear_handle_row`, which has already masked interrupts.

Neither is answered by moving the threshold. Both are answered by a voice that acquires nothing, and
the tree already has one — WEDGE-2's `wedge2_raw_byte`.

#### The teardown tokens (knob-gated with the rest of WEDGE-2)

| Token | Emitted at | Read as |
| --- | --- | --- |
| `<D1>` / `<d1>` | teardown entry, **before** the `TABLE`/`WRITER` section (`close`, `close_owner`, `move_to`) | uppercase = the core owning the open focus chain, lowercase = a teardown on some OTHER core while that chain is open. `<D1>` with no `<D2>` puts the death upstream of the barrier — blindness 2 |
| `<D2>` | that section is behind us; the barrier is going up and this core is about to spin | `<D2>` with no `<D3>` puts the death in the spin |
| `<D3>` | the spin returned; the barrier is held | pairs with `<D2>`; `<D3>` with no `<D4>` puts the death in the erase/reclaim run |
| `<D4>` | the erase/reclaim half is behind us and the barrier is down; the cursor bracket (`cursor::repaint`, and with it `SPRITE`) is next. `close_owner` only | `<D4>` as the LAST token on a torn wire puts the death on `SPRITE` — the **F4** site, not F1's `TABLE` |
| `<D!>` | the stall tripwire fired, **before** its `serial_println!` | `<D!>` with no `DRAIN STALLED` line after it is blindness 1 caught in the act |

**`<D4>` exists because the F4 death was otherwise misattributed to F1.** Before it, a `close_owner`
that died in `cursor::repaint` left `<D3>` as the last token on the wire — and `<D3>` is emitted from
inside `DrainBarrier::drain`, which *every* teardown reaches, so a `SPRITE` wedge and a reclaim wedge
produced the same trace and the audited F1 site (`TABLE`, closed by WEDGE-7's `fn table()`) was the
natural place to pin it. `SPRITE` is reached on **every** EL0 exit through this path, and a core that
blocks on it there is IRQ-masked by `sched::exit`, so the outcome is a silent total freeze of panel,
cursor and input. The token does not fix that — it makes the capture name the lock. It is
unconditional (`wedge2::mark`), on `<D2>`/`<D3>`'s terms rather than `<D1>`'s: it sits past the
`n == 0` early return, so its population is teardowns that genuinely freed a row and raised a
barrier, and a wedge that eats the panel is worth naming whether or not a TAB is in flight.

`<D1>` is chain-gated (`wedge2::mark_composite`) and the budget is measured, not preferred: the first
cut emitted it unconditionally and the wedge2 gate run priced it at **96 tokens against the focus
chain's 17**, because `close_owner` runs on every EL0 task exit and 65 of those 96 found no window
and raised no barrier. One interleaved into another core's line mid-word
(`:<D1: B>GRUN-ST: slot reclaim PASS`) and took a required witness off the wire — 87/88. The gate is
also the right aim: all four recorded lockups happened during TAB cycling with a focus change in
flight, which is exactly `CHAIN_CORE != 0`.

#### `[wedge1] dwell` — the range the tripwire deliberately declined to measure

`DRAIN_STALL_SPINS` sits past ~10⁸ spin hints on purpose (a threshold a merely slow drain could reach
would put a serial write on a hot IRQ-masked path). The consequence was left standing: the wire had
exactly two readings, *nothing* and *wedged*, with the whole interesting range between them
unmeasured. A counter costs no print, so it can measure it.

```
[wedge1] dwell drains=N spun=M spin_max=K note=65536 in_spin=J tripwire=silent|fired span=Tms -> VERDICT
```

* `STRADDLE` — spin evidence with zero completed drains in this window: the drain that produced it
  was counted in a neighbouring window's `drains` (it straddled the rollup boundary). Read beside
  those neighbours. A straddle whose `spin_max` clears `note` prints `DWELL` — severity wins the
  precedence. (A window with no evidence at all does not print — there is no `NONE` verdict.)
* `SPUN` — drains completed and at least one of them spun, but `spin_max` stayed under `note`.
  Contention was observed and was short. **Not a fault**; it is the reading that used to be lost.

  **Why it exists (WEDGE-1r3, PA6 metal 2026-08-01).** The ladder's only gate above `QUIET` was
  `spin_max >= DRAIN_DWELL_NOTE`, so any spin below the note was banked as `QUIET` — whose contract
  is "the healthy steady state". PA6 printed `drains=20 spun=1 spin_max=6890 ... -> QUIET`, the
  first non-zero spin this track has ever recorded, filed as healthy. Note that `DRAIN_DWELL_NOTE`
  (`1<<16`) is **not calibrated against any measurement** — its stated justification is purely
  relative ("four orders under `DRAIN_STALL_SPINS`"), and `DRAIN_STALL_SPINS` is itself only
  "comfortably past a second of real time". So a genuine dwell can sit at 65535 indefinitely and
  every window still reads `QUIET`. Lowering the constant would move an arbitrary line rather than
  fix the lens; the fix is that a window which measured a spin no longer claims health, and the
  raw `spin_max` is left to speak. The gate REQUIREs the line, never the verdict, so `SPUN` is
  gate-neutral by construction.

  **What `spun`/`spin_max` actually measure — do not read them as lock contention.** They count the
  `DrainBarrier`'s wait on `BLIT_ACTIVE`, an `AtomicUsize` refcount of in-flight composites — a
  *phase barrier*, not a lock. A non-zero `spin_max` means a teardown on one core waited for a
  composite in flight on another. `wm::TABLE` spin time is **entirely uninstrumented**: `spin::Mutex`
  has no counter anywhere in the tree. PA6's `spun=1` is therefore evidence of cross-core concurrency
  inside `wm` — the population every masked-spinner interleaving requires — and evidence of nothing
  about `TABLE` itself.

  **`in_spin=0` beside `spun>0` is not a contradiction.** `in_spin` is a gauge sampled at rollup
  time by a presenting core; a short spin completes long before the sample. It exists to catch a
  drain that never returns, not to sample contention.

  **What this line's silence is worth — the honest boundary (s1u lens must-fix).** The dwell emit
  rides `wcn_emit`, whose drivers are `present()` and the fixture/pointer forced rollups, and
  `wcn_emit` holds the `TABLE` lock before the emit — the WEDGE-1r2 change bypassed only the
  dirty-paced guard, not the cadence or the lock. So in a scene where nothing presents, or where a
  core dies holding `TABLE`, this line CANNOT print, and per the reachability rule its silence there
  is evidence of nothing. The instruments for those regimes are the `<D…>` tokens (raw-UART,
  chain-gated, emitted before the locks) and the tripwire itself. Bank dwell-line silence as
  "no teardown activity" only on a wire that shows presents continuing (`[wc-c]`/`[pstrip]` alive).
* `QUIET` — drains ran and none spun past `note`. The only verdict here that is a statement about the
  barrier rather than about the scene.
* `DWELL` — a drain spun far enough to be a stalled core, four orders of magnitude under the
  tripwire. IRQ-masked time on a teardown's core.
* `INFLIGHT` — a drain was inside the spin when *another* core took the line. One sample at operator
  teardown rates against a 5 s window is a coincidence; the same reading twice is a core that is not
  coming out.

`spin_max` is published **from inside the spin**, not after it. A ledger written after the loop
reports only drains that finished, so the one that never does contributes nothing and the rollup
reads clean over a held core — WEDGE-4's W4-A defect, refused here for the same reason. `in_spin` is
a gauge (loaded, never drained) and stays raised for as long as that core is in there. The line is
emitted **ahead of** `wcn_emit`'s dirty-paced guard, because a wedged teardown is not presenting and
gating it on present traffic would silence it under exactly the condition it reports on.

#### WEDGE-1r2 gate results (2026-07-30, QEMU raspi4b)

`./arroyo check` green both arches, feature-off and with `witness,wedge2` armed; `kernel8` builds;
`kernel8-test 150` **MBENCH PASS 89/89, 0 forbidden**, and again **89/89 with `UNAOS_WEDGE2=1`**
(originally mis-recorded here as 88/88; the landing commit and an s1u tip re-run both read 89/89).
Wire: `[wedge1] dwell drains=21 spun=0 spin_max=0 ... -> QUIET` — the barrier ran 21 times in a
3-second window and never once had to wait for anybody, which was previously unmeasured in both
directions.

**What QEMU did not prove.** `<D1>`/`<d1>` do not appear at all, and that zero is a SCENE fact rather
than a wiring one: the chain is open only for the body of `focus_changed`, and nothing in the fixture
battery tears a window down from inside it (`clickplain_leg` closes before its `focus_changed(0)`;
`closebox_leg`'s router close runs after `chain_exit`). The scene the token is aimed at — one core in
a TAB while a vug exits on another — is the bench's, and is P66's. **`<D1>` absent on QEMU must not be
read as the mechanism being quiet.** What the gate does prove is the `<D2>`/`<D3>` pairing (31 pairs)
and the dwell line's wiring. One `<D2>` reads as `<activD2>` on the wire — another core's line
interleaved with the token's bytes, WEDGE-2's stated and accepted cost for taking no lock.

**`<D4>` gate reading (210 s full-knob window, MBENCH PASS 90/90, 26398 lines).** `<D2>`=31,
`<D3>`=32, `<D4>`=3, `<D1>`=0, `<D!>`=0. The `<D2>`/`<D3>` gap is one more interleave of the same
kind and not a missing token — the split `<D2>` is legible on the wire as
`run /fat/<DVUG.ELF2> <D3>— loaded 12568 bytes`, its two halves wrapped around another core's
`EXEC-UVUG` line, so the true pairing is 32/32. `<D4>`=3 against `<D3>`=32 is the expected ratio and
is itself the token's design: `<D4>` sits past `close_owner`'s `n == 0` early return, so it counts
only the teardowns that actually freed a row, while `<D3>` counts every barrier `close`, `move_to`
and `close_owner` raise between them. All three `<D4>`s are followed by further output (two by the
focus chain's own `<F4><F5>`), i.e. no wedge — which is what this gate can show and the limit of it:
`timer_preempt` never runs on raspi4b, so F4 cannot occur there and a clean QEMU run is a wiring
proof, never a refutation of the mechanism.

#### Metal watch-list (WEDGE-1r2)

* `-> DWELL` or a repeated `-> INFLIGHT` locates a real stall well below the tripwire, and is the
  first reading this instrument has ever been able to produce.
* `<D!>` with no `[wedge1] DRAIN STALLED` line after it: the stall is real **and** the serial path is
  where the core went to die. That combination is the one WEDGE-1 could not distinguish from health.
* `<D1>`/`<d1>` as the LAST thing on a torn wire: the death is in the teardown's own `TABLE`/`WRITER`
  section, upstream of the barrier — the region WEDGE-1 never covered, and the reason its silence
  could not exonerate the teardown path.
* `<D4>` as the LAST thing on a torn wire: the death is in `close_owner`'s cursor bracket —
  F4, downstream of everything F1 and the barrier cover. Expect it to come with a dead panel rather
  than a merely stalled teardown: the sprite gates the cursor bracket every compositor path takes, so
  panel, cursor and input stop together. Until this token existed that death read as
  `<D3>`-then-silence and was attributed to the reclaim run or to F1's `TABLE`.

  **Updated by §WEDGE-9 (2026-08-02).** `SPRITE` is no longer a lock that can be waited on: it is a
  claim/loan, and a masked `close_owner` that cannot claim it takes an immediate `Busy` and defers the
  repaint. So `<D4>`-then-silence no longer means *a masked spin on the sprite lock*. It still names
  the phase, and the phase is still worth naming — but read it now as the framebuffer work inside
  `refresh_locked` (or `WRITER`), not as the family wedge.
* A quiet `[wedge1]` still means **nothing about the barrier** unless `drains > 0`. That is the whole
  point of the verdict naming the scene.

### WEDGE-2 — breadcrumbs, so the next wedge names its dying step

WEDGE-1 ended with a request written into its own watch-list: *get a per-core heartbeat that survives
an IRQ-masked spin before theorising further.* This is that instrument. **It is instrumentation only
— no fix, no behaviour change, no new lock, and no existing lock taken on the breadcrumb path.** It
adds nothing to a default image.

#### The evidence it was built against

Four lockups, three of them silicon, all under a six-vug storm during TAB cycling:

| Run | Machine | Last thing on the wire |
| --- | --- | --- |
| P66 | Pi 4 bare metal | pointer/click traffic, then `[wc-fv] focus raise asid=0x…`, then total silence |
| P67v2 | Pi 4 bare metal | the same shape |
| P68 | Pi 4 bare metal | the same shape |
| s44 | x86 | the same shape, with the serial capture truncated **mid-word** |

Two facts fall straight out of that table and are treated as settled here rather than re-litigated:

* ~~**The drain barrier is exonerated.** WEDGE-1's `[wedge1] DRAIN STALLED` tripwire fires from inside
  the drain-barrier spin, and it stayed silent all three times. Whatever stops the machine, it is not
  that loop.~~ **RETRACTED by §WEDGE-1r2.** The tripwire speaks through a blocking serial lock — and
  s44 stopped mid-word, i.e. a core died holding the UART, the one shape on which its silence is
  structurally guaranteed — and it exists only *inside* the spin, so a teardown that dies on the
  `TABLE` acquisition upstream of `drain()` never reaches it. Its silence was therefore compatible
  with the drain path being the site, and this bullet should never have been settled from it.
  WEDGE-2's own reasoning below is unaffected (the breadcrumbs are worth having on their own terms),
  but the drain barrier is **not** ruled out and the `<D1>`/`<D2>`/`<D3>`/`<D!>` tokens exist to
  settle it properly on the next bench wedge.
* **The mechanism is ARCH-NEUTRAL.** s44 reproduces the identical death shape on x86, so it lives in
  the shared wm/compositor/input-routing/sched interplay, not in anything BCM2711-specific.

The mid-word truncation is the design constraint. A core that stops mid-print with IRQs masked (or
every core stopping at once) takes any buffered or deferred output down with it, and no panic handler
runs. So the only instrument that can speak is one that has already spoken: **a token written BEFORE
each phase, straight at the UART, unbuffered.**

#### The chain, and the token table

Read the wire's LAST token as the phase that died — or, equivalently, the phase whose successor never
started. Tokens are four bytes, angle-bracketed, and no other line in the tree emits that shape.

| Token | Emitted at | The phase it opens |
| --- | --- | --- |
| `<F1>` | `arch/aarch64/syscall.rs` — `wc_focus_key`, after the TAB edge is recognised | `wm::focus_ring` — reads the ring under the window `TABLE` lock. Both entry points (the in-ring router seam and `wc_shell_focus_key`) funnel through this one body. |
| `<F2>` | `wc_focus_key`, destination chosen | `user_input_set_active` — clears the target's ring and drains up to 64 events off `pal::EVENT_QUEUE` against a live producer |
| `<F3>` | `wc_focus_key`, focus routing moved | `wm::focus_changed` — the visible half. Every recorded wedge got at least this far. |
| `<F4>` | `video/wm.rs` — `focus_changed` entry (also claims the chain) | the `TABLE` critical section that does the z-bump / shell raise |
| `<F5>` | `focus_changed`, table guard dropped | `erase` of the vacated boxes + `screen::request_full_present` |
| `<F6>` | `focus_changed`, after the `[wc-fv]` line | `composite()` — **the highest-value token**: `[wc-fv] focus raise` is the last line every wedge printed, so `<F6>` present with nothing after it says the chain survived that print and died in the composite pass, while `<F5>` with no `<F6>` says the print itself was the last step |
| `<F7>` / `<f7>` | `composite_inner`, drain + cursor bracket done | the `WRITER` read, the `TABLE` snapshot and the `BlitGuard` registration — the region WEDGE-1 hardened |
| `<F8>` / `<f8>` | `composite_inner`, guard held and damage set closed | the back-to-front blit loop — `draw_window`, the WC-G/WC-D witnesses, the sprite overlay |
| `<F9>` | `wc_focus_key`, `focus_changed` returned | return to the input pump. A wire that reaches `<F9>` has exonerated the whole focus path for that press. |

**Uppercase vs lowercase.** `<F7>`/`<F8>` are the core that owns the focus change; `<f7>`/`<f8>` are
some OTHER core entering the same two phases while the chain is open. The six-vug storm is in the
evidence precisely because several cores are inside the composite pass whenever a TAB lands, and a
wire ending `<F7><f7><f7>` reads very differently from one ending `<F7>` alone. A composite pass
running with no focus change in flight stays silent — otherwise the steady-state present rate would
bury the chain in tokens. The claim is taken at `<F4>` and released at the end of `focus_changed`
(not at `<F9>`), so every path out of that function — including the FOCUS-VIS selftest's three calls —
releases it. Note the tokens do **not** encode a core id; `<f7>` says "not the chain's core", nothing
finer.

#### The write primitive, and its lock analysis

`crate::wedge2::mark` calls `arch::serial::wedge2_raw_byte`, which is a **lock-free, bounded** poll of
the UART's TX-ready flag followed by one volatile store to its data register — on aarch64 the PL011
`FR`/`DR` pair (reusing the existing `SerialPort::write_byte`, which is a method on a unit struct and
therefore needs no `SERIAL_PORT` guard); on x86_64 the bare 16550 `LSR`/`THR` sequence at `0x3F8`,
deliberately *not* the `SERIAL1` mutex.

It acquires **nothing**: not `SERIAL_PORT`/`SERIAL1`, not `FBCON`, not `WRITER`, not `TABLE`, not
`SPRITE`, not the allocator. That is the whole property, and it is not a nicety — every one of those
locks is reachable from the focus chain being instrumented, so a breadcrumb that could block on one
would be missing in exactly the runs it exists for. It is also why the tokens do not go through
`serial_println!`, which masks interrupts and takes the serial lock. Compare WEDGE-1's tripwire, which
does take that lock and says so: that was acceptable there because the alternative on the serial-path
wedge was spinning forever anyway; here it would defeat the instrument outright.

**The cost, stated plainly.** Taking no lock means a token CAN land in the middle of another core's
`serial_println!` line and split it. For a last-words instrument that is the right trade: an
interleaved `<F6>` is still perfectly legible inside any other text, whereas a token serialised behind
a lock is a token the wedge eats. The knob gate is what keeps anyone else from paying for it.

#### Knob and arch seam

`UNAOS_WEDGE2=1` arms the `wedge2` feature. Default OFF: `mark` becomes an empty
`#[inline(always)]` function, its argument is a dead constant, and the image contains no `<F` token at
all (verified by `strings`, both directions — see the gate results). Everything except
`wedge2_raw_byte` is arch-neutral, so the x86 tree inherits this by porting one function —
**instrumentation, not theories.**

#### WEDGE-2 gate results (2026-07-28)

* `./arroyo check` — **`✅ x86_64 OK` / `✅ aarch64 OK`**.
* `UNAOS_QMP_PORT=4491 ./arroyo kernel8-test 150` (knob **OFF**) — **`✅ MBENCH PASS — 86/86 required
  witnesses, 0 forbidden hit(s), 12417 lines scanned`**.
* `strings -a target/pi_baremetal/kernel8.img | grep -oE '<[Ff][1-9]>'` on the knob-OFF image — **0
  hits**.
* `UNAOS_WEDGE2=1 ./arroyo kernel8`, then the same census on the armed image — **all eleven tokens
  present exactly once**: `<F1> <F2> <F3> <F4> <F5> <F6> <F7> <F8> <F9> <f7> <f8>`.

The QEMU gate proves compilation, no-regression, and byte-inert knob-off. It does **not** exercise the
instrument: raspi4b delivers no HID pointer, there is no operator to press TAB, and there is no
six-vug storm. Positive verification is a wedge on the bench.

#### Metal watch-list (WEDGE-2)

* Read the **last** token, not the last line. The `[wc-c]`/`[wc-fv]` lines are buffered behind the
  serial lock; the tokens are not, so on a torn wire they are the more recent evidence.
* `<F6>` as the terminus locates the wedge inside `composite`, which is where the WEDGE-1 refutation
  already pointed and is the outcome to expect. `<F7>` vs `<F8>` then splits it: guard registration
  versus the blit loop.
* `<F5>` as the terminus is the surprise result and would move the whole investigation — it would mean
  the `[wc-fv]` print itself (i.e. the serial lock) is the last step, not the compositor.
* Any `<f7>`/`<f8>` trailing the owner's last token names a second core that was inside the same
  region — the first direct evidence about whether this is a one-core death or a pile-up.
* A wire that reaches `<F9>` and wedges later has exonerated the focus chain for that press, and the
  next instrument belongs downstream in the pump.

### WC-N — "predetermined fps" becomes wire data

Six vugs on the panel run at visibly different rates, and until this arc nothing on the wire could
say **why**. The two numbers that existed answer adjacent questions:

* `[vugfps]` (`user-vug`, commit `ff6ec88f`) is the **app's own** count of frames it *issued*, drawn
  in its window corner. It is measured at EL0 from `SYS_GETINFO`'s tick and cannot know whether a
  frame reached glass — a vug hidden below the shell draws a healthy `60` while presenting into a
  pass that is suppressed before it touches a pixel.
* `[sched6]` (`main.rs`) is the render task's passes and presented composites per second. Fleet-wide,
  with no window in it: it says the compositor is busy, never which window the work was for.

So the question Peter's phrase names — *is a given vug's effective present rate a CONSEQUENCE of load,
or a CEILING somewhere?* — had no reading. `[wcn]` is that reading, taken on the compositor's own side
of the seam, per window.

#### What it counts

Four counts per window, each a delta over the rollup window:

| field  | meaning |
| ------ | ------- |
| `att`  | `wm::present` calls naming this row — the owner's **attempt**, after `SYS_WIN_PRESENT`'s ownership check |
| `comp` | times the row was actually blitted by `composite_inner`'s loop — **pixels on glass** |
| `hid`  | presents suppressed by the VUGMIN-B arm: every window this owner holds was below `SHELL_Z` |
| `bel`  | passes in which the row was in the dirty set and then declined by `above_shell` |

`comp` can legitimately **exceed** `att`: a neighbour's present grows the dirty set upwards over
occlusion and repaints this row inside the neighbour's pass. `comp - att` is therefore a reading about
overlap — compositor work this window's owner never asked for and cannot see — not an error.

`hid` and `bel` are the same fact from the two ends. `hid` is the owner's own present being dropped in
`present`; `bel` is somebody *else's* pass declining to repaint the row. **There is no third skip
class, because there is no occlusion cull:** a window wholly covered by another is still blitted (the
dirty-set closure exists to repaint what is on top, not to drop what is underneath), and this witness
does not invent a category the compositor does not have.

#### The park, and why the rate is not `att / span`

VUGPAUSE-2 makes an idle vug **leave the run queues** — it blocks in the input wait and presents
nothing at all until the operator touches it. Divided by wall-clock span that vug reads as `0.2/s`,
which is indistinguishable from a vug being *starved of cores* at 0.2 fps. That is exactly the
confusion this witness exists to remove, so the per-window denominator is the window's own **active**
time:

* consecutive presents closer together than `WCN_PARK_GAP_MS` (250 ms) accumulate into `active`;
* any longer gap accumulates into `parked` instead, and is charged to neither numerator nor
  denominator.

250 ms is chosen against VUGPAUSE-2's own ~256 ms backstop period: anything past a quarter second is
provably not a render loop pacing itself. A parked vug therefore reports the rate it ran at *while it
was running*, with its park time stated beside it. (A window's first present after a park opens a new
active span rather than closing one, so `att` overstates the active interval by at most one present
per park — visible only at very low counts, and always slightly optimistic.)

The **aggregate** line's denominator is wall-clock, deliberately: "what did the fleet cost the panel
over these five seconds" is a wall-clock question, and a fleet with one vug parked and five running
genuinely did present less per second of panel time. Conflating the two denominators on one line would
make the aggregate un-addable from its own rows.

#### `gap` is the ceiling test

`gap=min..max` is the shortest and longest **active** inter-present gap in the window, in ms. This
pair, not the rate, is what makes "predetermined fps" checkable without a second run:

* a rate that is a **consequence of load** scatters — a contended vug's gaps run from a few ms to
  tens, so `min` and `max` are far apart;
* a rate that is a **fixed ceiling** does not — `min` and `max` collapse onto one value, because
  something is pacing the loop regardless of what else is happening.

A window that recorded no active gap at all (one present in the whole rollup) reports `0..0`, which is
what it honestly has: no interval to measure.

#### Cadence

Dirty-paced, following `[pstrip]`/`[sched6]`: a fixed `WCN_ROLLUP_MS` (5000 ms) period, one claim per
period taken by compare-exchange from the presenting core, and the tick itself driven from the tail of
`wm::present` — including the VUGMIN-suppressed path, so a fleet that has just been hidden does not
lose the very interval whose `hid=` count is the point.

Two consequences, both deliberate:

* line volume is bounded by construction — at most one block (live windows + one aggregate) per
  period, however many cores are compositing;
* a fleet that has **wholly parked goes silent** rather than printing a wall of zeros. The witness is
  driven by the traffic it measures, so "no `[wcn]` lines" reads as "nobody is presenting".

Because the period's final partial window is never emitted on its own account, a **forced** emit fires
from `vugmin_rollup`, on the same scoped rollup (`fixture` / `desktop`) every other window witness
reports on. That is what guarantees the gate a block. A slot with traffic but no live row still gets
its line, marked `live=no`: dropping it would silently delete the last frames of every window that
ever exits.

#### Wire format

```
[wcn] win=<id> asid=<owner> live=<yes|no> above=<yes|no> att=<n> comp=<n> hid=<n> bel=<n>
      rate=<x.y>/s comp_rate=<x.y>/s active=<n>ms parked=<n>ms gap=<min>..<max>ms
[wcn] rollup scope=<live|fixture|desktop> wins=<n> att=<n> comp=<n> hid=<n> bel=<n> stale=<n>
      passes=<n> aborted=<n> att_rate=<x.y>/s comp_rate=<x.y>/s span=<n>ms -> <IDLE|STARVED|LIVE>
```

(one physical line each; wrapped here). Rates are fixed-point tenths, for `[pstrip]`'s reason — an
integer `/s` truncates every honest sub-1 Hz rate to `0`. Aggregate-only fields: `stale` (presents
naming no live row — a window closed under its owner, with no slot to charge them to), `passes` /
`aborted` (composite passes that reached the blit loop, and passes that returned before it under the
F4 drain barrier or an unready framebuffer — an aborted pass cost its owner a syscall and produced no
pixels for *any* window). The verdict is `IDLE` when nothing was attempted, `STARVED` when presents
were attempted and none reached glass, `LIVE` otherwise.

#### WC-N gate results (2026-07-29, QEMU raspi4b)

* `./arroyo check` — **`✅ x86_64 OK` / `✅ aarch64 OK`**.
* `./arroyo kernel8-test` — **`✅ MBENCH PASS — 86/86 required witnesses, 0 forbidden hit(s), 5150
  lines scanned`**. No gate entry was added; the arc's claim is the reading, not a new REQUIRE.

Actual lines from `target/serial-pi.log`:

```
[wcn] win=1 asid=0x1 live=yes above=yes att=83 comp=85 hid=0 bel=0 rate=285.2/s comp_rate=292.0/s active=291ms parked=0ms gap=1..7ms
[wcn] rollup scope=live wins=1 att=83 comp=85 hid=0 bel=0 stale=0 passes=86 aborted=0 att_rate=16.6/s comp_rate=17.0/s span=5000ms -> LIVE
[wcn] win=1 asid=0x0 live=no above=no att=225 comp=244 hid=0 bel=1 rate=403.2/s comp_rate=437.2/s active=558ms parked=0ms gap=1..17ms
[wcn] win=2 asid=0x0 live=no above=no att=4 comp=16 hid=0 bel=1 rate=6.6/s comp_rate=26.7/s active=0ms parked=0ms gap=0..0ms
[wcn] rollup scope=fixture wins=0 att=229 comp=260 hid=0 bel=2 stale=0 passes=263 aborted=0 att_rate=382.9/s comp_rate=434.7/s span=598ms -> LIVE
[wcn] win=1 asid=0x1 live=yes above=yes att=1118 comp=1126 hid=0 bel=3 rate=560.4/s comp_rate=564.4/s active=1995ms parked=0ms gap=1..7ms
```

The first block is the whole mechanism in one line: the fixture's window attempted 83 presents inside
**291 ms of active time** out of a 5000 ms wall span — `rate=285.2/s` (what it does when it runs)
against `att_rate=16.6/s` (what it cost the panel). Pre-WC-N only the second number was obtainable,
and it would have been read as a 16 fps window. `gap=1..7ms` is the scatter signature: this window is
**not** ceiling-limited in QEMU. `comp=85 > att=83` is the overlap reading — two repaints the owner
did not ask for.

#### Honest scope on the gate

QEMU raspi4b has no HID, so nothing ever TABs to the shell and no owner is ever hidden: `hid=0` there
is structural, and the `bel=` counts that do appear come from the fixture's own z manipulation rather
than from an operator. The gate proves the counters are wired, the cadence is bounded, the park split
does not divide by zero, and the parked/active denominators behave. The numbers that carry the arc —
a hidden vug's `hid` climbing while its `[vugfps]` corner still reads 60, and a six-vug storm's `gap`
spread — are bench readings.

#### Metal watch-list (WC-N)

* **`gapmin == gapmax` on a busy vug is the finding.** It would mean the rate is paced, not earned,
  and the next question is by what — the present syscall, the drain barrier, or the app's own loop.
* **`comp` far above `att` across the fleet** means the tiling has the windows overlapping enough that
  every present costs several blits. That is a placement problem wearing a performance costume.
* **`aborted` climbing** is the F4 drain barrier eating passes under load — presents that cost a
  syscall and produced nothing, which is the P66 neighbourhood.
* **`STARVED`** (`att > 0`, `comp == 0`) with `hid == 0` and `bel == 0` should be impossible; it would
  mean presents are reaching `composite` and the blit loop is never drawing them.
* **`parked` large while the operator says the vug looks frozen** is the good outcome — it says the
  vug is in VUGPAUSE-2's idle and is waiting for input, not starved.

---

### CURSOR-11 — the arrow stops leaving the glass over a presenting window

**P73 (Peter, bench Pi 4).** With the pointer parked over a PRESENTING vug, the cursor and the vug's
own fps overlay text blink together, at the vug's present rate. Over a quiet window neither blinks —
that half is CURSOR-9, and it holds.

#### The mechanism, and why it is a duty cycle rather than a race

`note_present_over_sprite` arms `TOUCHED_SINCE_DRAW` on every present unconditionally, so over a live
window the compositor's repair rect is armed every pass and the window is re-blitted every pass. That
re-blit is not itself the blink. The blink is what the pass does *around* it.

A composite that owns the overlay session used to run:

```
undraw_within(paint set)   <- arrow comes off the glass, panel published arrow-less
paint_window -> layer      <- a whole off-screen compose
compose_into(layer)        <- arrow painted into the staged rows
blit rows -> front         <- arrow returns to the glass
adopt_overlay              <- bookkeeping
```

Between line 1 and line 4 sits the entire compose plus the row copies — milliseconds, once per
present. That is exactly the shape CURSOR-3 diagnosed in WC-I's bracket, surviving one level down in
the *mask* that replaced the bracket. No care inside the interval shortens it.

#### The fix: the handback is deferred, not taken

`undraw_within` is replaced on the session path by `cursor::defer_within`, which writes **no**
framebuffer pixels, takes no `WRITER` lock, bumps no generation and requests no `repair`. It sets a
third per-pixel class, `Sprite::pend`:

| class | on the panel? | `saved[i]` | who settles it |
|---|---|---|---|
| `off` (CURSOR-4) | no — handed back | stale | `redraw_off_locked`, from the front |
| `pend` (CURSOR-11) | **yes — never left** | still true, pending a verdict | `adopt_overlay` |
| neither | yes | true | nobody; nothing can reach it |

`off` and `pend` are disjoint by construction. The arrow simply stays on glass for the whole pass and
is overwritten by rows that already contain it.

#### The save-under coherence argument — the [wc-d] argument, stated as an ordering

The undraw's *only* justification was: a pixel a painter is about to overwrite must be handed back
before the overwrite, or `saved[i]` describes content the panel no longer holds and the next restore
stamps stale pixels into a live window's rect — which is a `[wc-d] -> FAIL`, which the Pi spec FORBIDs.
That obligation does not go away; it moves to the tail, where the front buffer is **final** and the
question is answerable exactly instead of conservatively. `adopt_overlay` settles every `pend` bit, in
this order, and the order is load-bearing:

1. **The coverage install runs FIRST.** For every pixel a staged present carried
   (`Overlay::covered`), `saved[i]` takes `ov.saved[i]` — the BACK LAYER's pixel, which is precisely
   the freshly composed window content now sitting beneath the arrow on the panel — and the `pend`
   bit is retired there.
2. **`settle_pending_locked` runs SECOND**, over what is left. It reads the finished front and uses
   `undraw_locked`'s colour guard in the opposite direction: `now == color` means nobody painted
   there, so `saved[i]` was never invalidated and nothing is written; `now != color` means a painter
   took the pixel, so `saved[i]` is re-taken from the front (provably that painter's content — we can
   see it is not our own `FILL`) and the arrow is put back over it.

**Reversing those two steps is the defect.** After a compose-through the front holds our own `FILL` at
the covered pixels. A front read there answers "untouched" — true and useless, because the pixel
*under* the arrow changed: the window presented new content beneath it. `saved[i]` would keep the
pre-present pixel and the next real undraw would restore last frame's window content into a live
window. The layer, and only the layer, can supply that save-under, which is CURSOR-3's original
load-bearing detail applied to the new class. So the install must retire the bit before the front read
ever sees it, and after it the two sets are disjoint.

The colour guard's residual is unchanged and closed the same way: a painter whose content happens to
equal `FILL` or `SHADOW` reads as "untouched" and keeps a stale save. Every present over the sprite box
armed `TOUCHED_SINCE_DRAW`, so the next full undraw's `repair` damages the windows involved.
**CURSOR-9's machinery is untouched and is exactly what covers this.**

#### What still brackets, deliberately

* **The WC-F reserved-box arm.** The probe paints the FRONT after the pass, outside every window box,
  so no staged present can carry the sprite through it. Full `undraw`, as since CURSOR-3.
* **The sessionless arm** (`overlay_open` refused — the VUGPAR steady state). This one *could* defer,
  and must not: without a session there is no coverage to install, so nothing would settle the
  deferred pixels. CURSOR-5's generation bump is the signal the session owner needs, and only an
  actual handback produces it.
* **An `adopt_overlay` whose session came back incoherent**, which falls to `refresh_locked`. That
  path answers the pending class correctly without knowing about it — the colour guard asks each pixel
  the same question, and `draw_locked` re-saves every one of them from the finished front. Both reset
  `pend`.

`disturbed` in `composite_inner` therefore widens from "the sprite is off the panel" to **"this pass's
tail owes the sprite its pixels"**. Both consumers stay right under the wider reading: `tail_of` still
needs `Adopt`, and `draw_window`'s `bracketed` argument asks exactly that question, so a deferring
pass correctly does **not** arm `PRESENT_DIRTY`.

#### Witness

```
[cursor11] compose-through scope=<s> passes= bracketed= px_deferred= px_installed= px_redrawn= -> <verdict>
```

Printed from `cursor8_rollup`, at the same two scopes and in the same block.

* **`passes` / `bracketed`** — passes that left the arrow on glass, against those that took it off.
  Before this arc every pass over a window was `bracketed` by construction.
* **`px_installed`** — deferred pixels a staged present delivered with the arrow already in the rows.
  **The arrow never left the panel here.** This is the number that carries the fix.
* **`px_redrawn`** — deferred pixels a painter took anyway (direct path, instrument exclusion,
  straddle): re-saved and redrawn by the tail. These blinked, exactly as before the arc.
* The remainder (`px_deferred - px_installed - px_redrawn`) is pixels nothing in the pass touched:
  one front read, no write.
* Verdict `THROUGH` when `px_installed >= px_redrawn`, `BRACKETED` otherwise, `UNWITNESSED` when no
  pass ever ran the mechanism.

#### CURSOR-11 gate results (2026-07-29, QEMU raspi4b)

* `./arroyo check` — **`✅ x86_64 OK` / `✅ aarch64 OK`**.
* `./arroyo kernel8-test` — **`✅ MBENCH PASS — 86/86 required witnesses, 0 forbidden hit(s), 6069
  lines scanned`**, first attempt, exit 0.

```
[wc-i] rollup scope=fixture windowed_flushes=3 intrusions=0 cursor_passes=349 cursor_brackets=1 -> CLEAN
[cursor3] rollup scope=fixture planned=0 offers=0 taken=0 adopt=0 repaint=1 ensure=348 ... -> UNWITNESSED
[cursor6] rollup scope=fixture present_over=0 masked=0 repaired=0 desktop_over=0 mismatch=0 uncover_lost=0/0 -> UNWITNESSED
[cursor8] repair rate scope=fixture requests=0 repairs=0 ... flush_kb=0 -> UNWITNESSED
[cursor11] compose-through scope=fixture passes=0 bracketed=0 px_deferred=0 px_installed=0 px_redrawn=0 -> UNWITNESSED
```

#### Honest scope on the gate

**QEMU raspi4b has no HID pointer.** No pointer report means the sprite is never drawn, `sprite_plan()`
is always `None`, no overlay session is ever opened, `defer_within` is never called and every counter
above is 0 — `UNWITNESSED` by construction, not by accident. The gate proves **no-regression only**:
every pre-existing cursor witness is unchanged and the whole 86-witness suite passes. **The blink
verdict is owed by an attended bench boot.**

#### Metal watch-list (CURSOR-11)

* **`px_installed` dominating `px_redrawn` with the pointer parked on a presenting vug** is the arc
  working. `-> THROUGH` is the one-word form.
* **`px_redrawn` dominating** means the compose-through is not reaching the pixels the pointer is over.
  Take that reading to `[cursor3] rollup`'s decline breakdown — `straddle`, `budget`, `lock` and
  `stale` name which class is losing them.
* **`bracketed` climbing with `passes` flat** under VUGPAR is the sessionless arm being the steady
  state, i.e. two cores compositing at once. That is CURSOR-5's territory, not this arc's.
* **`[cursor5] selfsave` non-zero** would mean a save-under captured our own arrow. It must stay 0:
  this arc adds one new front read (`settle_pending_locked`), and that read is guarded — it writes
  `saved[i]` only when the pixel is provably NOT our colour.
* **A white arrow standing in a vug's rect** would mean the install/settle ordering above was violated.
  Nothing else in this arc can produce it.

---

### CURSOR-12 — compose-through was dormant, and the rollup that says so could not print on x86

**P74, both seats.** The pi seat's live mouse sitting reads `[cursor3] tail=repaint offers=0 taken=0
-> BRACKETED` on every sampled present, with every `[cursor11]` counter zero. The rmbp's s46 capture
shows a single `[cursor3] present tail=adopt offers=2 taken=2 -> COMPOSED` for a whole sitting and no
`[cursor…] rollup` line at all. CURSOR-3's compose-through — five arcs of design, from CURSOR-3 to
CURSOR-11 — has essentially never executed on either bench outside its own selftest.

Two separate defects produce that, and only one of them is in the cursor module.

#### 1. The rollup block has no x86 caller, and fires before the pointer exists on aarch64

`wm::wci_rollup()` has exactly one caller in the tree: `arch::aarch64::syscall`'s EL0 window-verb
fixture, a boot-time one-shot. So:

* **On x86 the entire block has never printed.** `[cursor3]`, `[cursor5]`, `[cursor6]` and `[cursor8]`
  rollups are absent from every rmbp capture ever taken. The one `[cursor3] present …` line in s46
  comes from `note_cursor_tail`, a different site with a different format. The x86 track has been
  reading a five-arc mechanism through an eight-sample keyhole.
* **On both arches it fires before any HID report has arrived**, so even where it prints, every cursor
  counter in it is structurally zero and `UNWITNESSED` is not a finding.

`wm::wci_rollup_live()` plus `pal::cursor::rollup_tick` fix the cadence: the block is emitted from the
pointer's own motion choke point, rate-limited to 5 s (matching `[sched6]`, so the two interleave and
can be read against each other). Witness builds only, and unreachable without a real pointer report,
so both QEMU suites print nothing and their line counts are unchanged.

#### 2. `offers=0` is an UPSTREAM death, and nothing counted the upstream

`offers` is bumped in `note_cursor_overlay`, called from `stage_window` only once it has actually
reached `compose_into`. The existing breakdown (`straddle`/`lock`/`budget`/`stale`) covers only what
happens *after* an offer is made or a session is opened. The whole chain before that — is the sprite
on the panel, does any window meet it, did the session open — was invisible, and `offers=0` is exactly
what an upstream death looks like from the rollup.

`[cursor12]` names that chain, one bump per composite pass, in the order the pass tests it:

```
[cursor12] offer scope=live passes=N nosprite=… nohit=… reserved=… nosession=… planned=… excl_probe=… excl_unverified=… -> why
```

The terms are mutually exclusive by construction, so they sum to `passes` and the dominant one is the
answer.

**The leading candidate is `nosprite`, and it is a call-graph fact rather than a race.**
`Screen::flush` ends in `wm::service_damage()` → `composite()`, and the render task brackets its own
flush with `cursor::undraw()` … `cursor::repaint()` — x86's console loop does it explicitly
(`main.rs`), and the Pi render task has done it since CURSOR-1. **Every composite reached through the
desktop's flush therefore runs between the undraw and the repaint, with `sp.drawn == false`, by the
caller's own design.** `sprite_plan()` returns `None`, no plan is taken, no window is offered one, and
the pass settles `Untouched`. Compose-through can only ever run on a composite reached from
`wm::present`, and on the rmbp bench those are suspended while the installer is up.

If that is what the counter says, the fix is not in `video::cursor` at all: it is that the desktop
composites from inside a bracket that has just taken the sprite down.

#### The witness-exclusion question, sized rather than argued

`may_overlay` is suppressed by two `#[cfg(feature = "witness")]` exclusions — the WC-G probe, and a
window whose `VERIFIED` bit is not yet set — and **every bench image either seat has ever booted is a
witness build**. If those dominate, the instrument has been disabling the mechanism under observation,
which is the worst shape a defect can have: broken only where it is watched.

Reading the code says both are self-clearing. The WC-G probe is budgeted per window id and returns
`None` once spent; `VERIFIED`'s bit is set immediately after `draw_window` in the same loop body, so
pass 1 excludes and pass 2 permits, and the only clear is on window CREATE (an id is a recycled slot
alias, so a fresh window deserves a fresh verdict). Neither should persist for a whole sitting.

But "reading the code says" is what produced the sittings this arc exists to stop repeating, so
`excl_probe` and `excl_unverified` count them instead, and only on passes that actually held a plan to
lose. **If either is non-trivial, the correct scoping is per-window-per-pass** — exclude the one window
the probe is bracketing on the one pass it is spent on, never the general case — and that fix belongs
in `composite_inner`, not in a `cfg`.

#### A second, independent source of `offers=0`

`[wc-h] win=1 staged=no reason=fixture -> DIRECT` in the rmbp wire is `stage_window`'s
`FALLBACK_FIXTURE` one-shot taking the direct path for an unverified window. `compose_into` is reached
only from the staged path, so **a window that takes DIRECT is un-composable for that pass by
construction** — no offer is made and none is counted as declined. `[cursor12] planned=` non-zero
beside `[cursor3] offers=0` is the reading that isolates this case: the session opened, so the death is
below it, in `stage_window`'s decline chain.

#### Gate results (CURSOR-12)

* `./arroyo check` — **`✅ x86_64 OK` / `✅ aarch64 OK`**.
* `UNAOS_WC=1 ./arroyo test` — 0 FAIL. The suite is pointer-free, so `rollup_tick` is unreachable and
  no line count moves.
* `./arroyo test-arm` — banner, 0 FAIL.

QEMU delivers no pointer on either suite, so `[cursor12]` cannot print there and the gates prove
wiring and no-regression only. The line carries its evidence on the bench.

#### Metal watch-list (CURSOR-12)

* **`nosprite` ≈ `passes`** confirms the render-bracket call graph. The desktop flush is compositing
  with the sprite deliberately off the panel, and compose-through is unreachable on that path.
* **`excl_probe` / `excl_unverified` non-trivial** means the witness build is suppressing the
  mechanism. Rescope per-window-per-pass; do not "fix" it by removing the instrument.
* **`nohit` ≈ `passes`** means the operator was pointing at the desktop. Check the sitting, not the
  code.
* **`planned` > 0 with `[cursor3] offers=0`** puts the death below the session — read
  `[wc-h] staged=no reason=` next.
* **`[cursor12]` appearing at all on an rmbp capture** is itself the first result: it means the x86
  track can finally see the cursor mechanism it has been tuning blind.

### CURSOR-13 — one owner for the sprite: the flush-path bracket dies, composite carries the arrow

CURSOR-12 asked which predicate killed the offer and the wire answered on the first sitting. GR7 s48
silicon, `[cursor12] … nosprite=N passes=N -> nosprite`, 42 samples out of 42, and the same reading
on the x86 seat. Compose-through was not rare, it was **unreachable** — and the reason is a call
graph, not a race.

#### The mechanism, in one paragraph

`Screen::flush` is two things bolted together: `present_background`, a raw desktop blit into the
front framebuffer, and then the window layer (`wm::service_damage` → `composite`, or `wm::repaint` on
the intrusion fallback). The render task wrapped **the pair** in `cursor::undraw()` …
`cursor::repaint()` — the CURSOR-1 contract, on both arches (aarch64: `main.rs::render_service`;
x86: the console loop). So every composite reached through the desktop's flush ran *between* the
undraw and the repaint, which is to say with `sp.drawn == false`, which is to say
`cursor::sprite_plan()` returned `None`, which is to say the pass could not take a plan, could not
open an overlay session, could not offer a window anything, and could not reach CURSOR-11's pend
class either. Two mechanisms owned the sprite on that path and the older one starved the newer. Our
own P74 `BRACKETED` lines are the same fact seen from the other end.

Note what this was *not*: not a lock, not a budget, not a contended `try_lock`, not the
witness-exclusion pair. Every counter CURSOR-3/4/5 added measures declines that happen *downstream*
of a plan being taken. There was never a plan.

#### The fix: narrow the bracket, and move its owner

The bracket is not deleted. It is scoped to the half that needs it, and it moves from the caller into
`Screen::flush` itself:

```
flush():
    cursor::undraw()          // CURSOR-13 — desktop bracket OPEN
    present_background()      // raw desktop blit; subtracts window boxes (WC-I), not the sprite
    cursor::repaint()         // CURSOR-13 — desktop bracket CLOSED
    wm::service_damage()      // composite runs with the arrow ON GLASS
```

and `render_service`'s `if dirty { … }` arm becomes a bare `pal.render()`.

**Why `present_background` keeps its bracket.** It is not a window composite and there is no
compose-through to engage there — the desktop is not a staged surface, so there is no layer into
which the arrow could be composed. It writes desktop pixels straight at the panel and it knows
nothing about the sprite (its WC-I subtraction covers the *window* layer only), so a live arrow
standing over desktop would be overwritten and its save-under left describing pixels that are no
longer on the panel. Bracketing it costs exactly one restore/save/draw per present — the same cost
the old bracket paid — and buys the coherence the sprite module's save-under depends on. Extending
CURSOR-11's pend to cover it was considered and rejected: pend defers a handback because a *staged*
surface will carry the pixels through, and settles against the finished front through
`adopt_overlay`. The desktop blit stages nothing and there is no session to install coverage into, so
a deferral there would have nothing to settle it — the exact reason the sessionless composite arm
(`undraw_within_nosession`) is not allowed to defer either.

**Why it lives in `flush` and not in the render task.** Same argument that split `present_background`
out in the first place: no flush site should have to remember the bracket. The boot present, the
`rast_demo` path and the `video::witness` fixtures all get it now without restating it, and there is
exactly one place where the desktop-vs-sprite ordering is written down.

#### What the composite half now does

With the arrow on glass, `sprite_plan()` returns a real plan and the flush-reached pass takes the
machinery that has been sitting there since CURSOR-3, unchanged:

* a staged window opens the overlay session, `defer_within` writes no pixels, `compose_into` paints
  the arrow into the staged rows, and `adopt_overlay` installs the covered pixels before the deferred
  ones are settled — CURSOR-11's coverage-install-first ordering, which is what keeps the save-under
  answerable against a *finished* front;
* a WC-F reserved box, or a pass that cannot get the session, takes its whole-sprite /
  `undraw_within_nosession` bracket exactly as it does on the `wm::present` path today. That
  distinction already existed; CURSOR-13 adds no new class.

#### Coherence consequences, stated

* **Save-under.** `cursor::repaint` is called *after* `present_background` and *before* the
  composite, so the arrow's save-under is taken against a front buffer whose desktop is already
  final. Window pixels under the arrow may still change during the composite that follows — and that
  is precisely the case the pend/adopt path exists to settle, per pixel, against the finished front.
* **Why the close is `repaint()` and MUST NOT be "simplified" to `ensure_drawn()`** (cross-seat
  finding, both implementations converged on this the hard way): after `flush`'s leading `undraw()`
  the sprite is down, so on the common path the two closes look interchangeable. The case that
  separates them is a pointer report landing on another core *between* the undraw and the close:
  `ensure_drawn` would then find `drawn == true` and return — leaving a save-under captured
  MID-BLIT, stale by construction, for a later undraw to restore into a live rect. `repaint`
  re-takes the save-under unconditionally. (Corollary: the "CURSOR-9 mends sooner" note below is
  the concurrent-redraw case specifically — on the quiet path `repaint`'s internal undraw returns
  early and its repair damages nothing.)
* **Why plan-before-blit (the rejected variant B) stays dead**: a plan captured before
  `present_background` describes a panel the desktop blit then partially destroys, and adopt/settle
  would settle against a "finished front" that no longer contains the sprite the plan promised —
  the WC-L/P64 interleave re-entering by a new route, invisible to the epoch check because the
  epoch is unchanged across a desktop blit. It trades a recoverable failure (blinky arrow) for an
  unrecoverable one (`[wc-d] FAIL`) to save nothing.
* **CURSOR-9 / `TOUCHED_SINCE_DRAW`.** Untouched, and now actually exercised on this path: a present
  landing under a live sprite arms the repair through `note_present_over_sprite`, and there is at
  last a live sprite for it to arm for. The colour-guard residual is repaired *sooner* than before,
  not later — `repaint`'s own `repair` tail damages every window the restore crossed, and the
  composite on the very next line services that damage inside the same flush.
* **CURSOR-6 / `note_desktop_over_sprite`.** Its meaning is unchanged: it is the hole detector for
  the desktop bracket, and the bracket still exists. Expected 0. A non-zero reading now means the
  narrowed bracket has a hole, which is a strictly easier thing to locate than before.
* **`[wc-d]`.** The FORBID set is the correctness backstop for exactly this change — it read the
  scan-out back inside a window's rect and must never find a sprite pixel there. It is the composite
  path's brackets and the compose-through/adopt settle, not the flush caller's bracket, that make
  that true; CURSOR-13 removes nothing `[wc-d]` was resting on.

#### Expected wire (bench)

Post-fix, with the pointer over a window:

```
[cursor12] offer scope=… passes=N nosprite=0 nohit=… reserved=0 nosession=… planned=P>0 … -> …
[cursor3]  rollup scope=… planned=P>0 offers=O>0 taken=T>0 adopt=… repaint=… ensure=… -> …
[cursor11] compose-through scope=… passes=P bracketed=B px_deferred=D>0 px_installed=I px_redrawn=R
```

`nosprite` must fall to the passes where the pointer is *genuinely* hidden — before the first report
of the boot, and after CURSOR-HIDE's ~1.5 s idle expiry. `nosprite ≈ passes` with a pointer moving on
the panel means the fix did not take. With the pointer over the desktop the honest answer is
`nohit ≈ passes`, not `nosprite`. `[cursor6] desktop_over=` must stay 0.

#### Gate results (CURSOR-13, 2026-07-29, QEMU raspi4b)

* `./arroyo check` — **`✅ x86_64 OK` / `✅ aarch64 OK`**.
* `./arroyo kernel8-test 210` — **`✅ MBENCH PASS — 86/86 required witnesses, 0 forbidden hit(s)`**,
  21270 lines scanned. `[wc-d] … -> PASS` throughout; `[wc-i] rollup … -> CLEAN`
  (`intrusions=0`, `cursor_passes=350`, `cursor_brackets=1`).
* `[cursor12] offer scope=fixture passes=350 nosprite=350 … -> nosprite` and
  `[cursor6] … desktop_over=0 … -> UNWITNESSED`.

**QEMU raspi4b has no pointer**, so the sprite is never drawn, `nosprite=passes` is the *correct*
reading there and cannot move. The gate proves no-regression only. The verdict is owed by the bench.

#### Metal watch-list (CURSOR-13)

* **`[cursor12] nosprite` still ≈ `passes` with the pointer live over a window** — the fix did not
  take; look for a second bracket around a flush site.
  **Superseded by CURSOR-14 — this watch item as written cost three sittings.** Before CURSOR-14 the
  first `[cursor12] scope=live` block of a boot fired on the operator's first pointer report and
  reported cumulative counters, so it covered only the era *before* the sprite existed and read
  `nosprite = passes` on any kernel whatsoever. Read this item only against a block whose
  `passes < cum`; see the CURSOR-14 section below.
* **`[cursor6] desktop_over` > 0** — the narrowed desktop bracket has a hole; the desktop is erasing
  the arrow at flush rate.
* **`[cursor11] px_deferred` > 0 with `px_installed + px_redrawn` lagging it persistently** — the
  settle is not keeping up and the save-under is going stale; that is CURSOR-11's territory, now
  reachable for the first time.
* **A spotty or trailing arrow over the desktop** — the desktop bracket, not the composite path.
  Over a *window*, it is the composite path.

---

## CLICK-X86 — what a pointer press does on x86 today (2026-07-29)

This section exists because a lift was attempted and correctly refused. The pi seat landed
CLICK-ROUTE, then CLICK-SWALLOW (`1ed1c725`), then CLICK-PLAIN (`475c51d3`) — three arcs of click
routing policy on aarch64 — and the x86 track was asked to lift the last of them. It cannot be
lifted, and the reason is not a merge conflict: **x86 has no pointer-press path for a routing policy
to be a policy about.** The audit is recorded here in full so it is not re-run.

### The five findings, each with its line

1. **The x86 event drain has no `Event::Button` arm.** `main.rs`'s x86 loop matches `Key`,
   `Mouse`, `MouseAbsolute` and then `_ => {}`. A button report is pushed onto `pal::EVENT_QUEUE` by
   the xHCI/EHCI HID paths, popped by that drain, and discarded. Nothing hit-tests it, nothing routes
   it, nothing consumes it. (The exception is a full-screen in-kernel demo — `vug` — which drains
   `pal::pump_and_poll` itself and reads `Event::Button` as exit/drag. That is the only x86 code that
   has ever seen a click.)
2. **`wm::focus_changed` has no x86 caller.** Its only callers outside `video/wm.rs` are in
   `arch/aarch64/syscall.rs`. Focus, as a live concept, does not exist on x86: nothing raises a
   window, nothing publishes a focus owner, and `FOCUS_ASID` is only ever written by `wm`'s own
   selftests.
3. **`wm::hit_test` has no x86 caller either.** The seam is present and arch-neutral; the address
   lookup has simply never been performed on this arch.
4. **There is no ring-3 input delivery on x86 at all.** `arch/x86_64/syscall.rs` contains no
   `user_input_*` equivalent, no per-address-space input ring, and no `SYS_INPUT_POLL`. The file's own
   syscall-numbering note says so directly: 27 (`INPUT_POLL`) is reserved for x86's "later arcs".
   CLICK-PLAIN's central claim — that a focus-changing press is *delivered whole* into the raised
   owner's ring — has no addressee on x86.
5. **Every window x86 puts on the panel is owned by ASID 0.** `video/wcx.rs` creates the demo and
   probe rows with owner 0, and `video/fbcon.rs`'s console window likewise; `hit_test` skips
   `owner_asid == 0` by design (a compat/kernel row names nobody as a focus target). So even if the
   press path existed and called `hit_test` today, it would resolve `None` for every persistent x86
   window. The only x86 rows with a real owner are the transient ones `SYS_WIN_CREATE` mints for the
   WINX-1 fixture.

The earlier x86 audit's summary — "no hit-test on the button path, mirroring the pre-CLICK-ROUTE
aarch64 state" — is true but understates it. Pre-CLICK-ROUTE aarch64 *had* a press path, a focus
holder, and a delivery ring, and merely addressed the press to the wrong one of them. x86 has none of
the three.

### Arch-neutral vs. arch policy, for whoever lifts next

CLICK-PLAIN splits cleanly, and the split is worth naming because it is the same split every one of
these arcs has:

* **Arch-neutral (`video/wm.rs`)** — the VUGMIN-C amendment (a raise publishes only the ARRIVING
  owner's unhide; the departing owner is no longer hidden, so a focus change starts things and never
  stops them), the deletion of `owner_live` with its only caller, and the selftest leg. These are the
  liftable half in principle. In practice **none of it applies to this tree yet**: it is an edit to
  VUGMIN-A/B/C machinery (`vugmin_publish`, `vugmin_scan`, the raise arm's `marks`/`nmarks` block)
  that x86's `wm.rs` does not contain, so every hunk's context is absent and `git cherry-pick` has
  nothing to anchor on. When the VUGMIN chain does arrive here, it must arrive **post-CLICK-PLAIN** —
  see the reversal note below.
* **Arch policy (`arch/<arch>/syscall.rs`)** — the router: the press-edge tracker, the hit/miss arms,
  the ordering of `set_active` → `focus_changed` → push, and the press/release pairing that keeps a
  click from being split across two apps. This half is reimplemented per arch by design and is never
  lifted. x86's is unwritten because its three prerequisites are unwritten.

### The CLICK-SWALLOW → CLICK-PLAIN reversal, and why the later position is right

CLICK-SWALLOW's rule was: a press that *changes focus* is consumed. Its case was concrete — the one
gesture that restores a backgrounded vug was also working that vug's click-to-pause toggle, so the
vug came back paused and the operator had to click twice. Swallowing the refocusing press made the
first click mean only "come here".

CLICK-PLAIN reverses it: the focus-changing press is delivered again, whole, press and release both.
The reversal is right, and not because the pause-toggle problem went away — it is right because
CLICK-SWALLOW made *what a click does* a function of state the operator cannot see. Whether a press
reached the app depended on which window happened to hold focus a moment earlier, which is invisible
by construction on a panel where an unfocused window looks like a focused one that has stopped
drawing. Two identical gestures produced two different outcomes and nothing on the glass explained
the difference. That is the accretion P75 named ("no reason to it"), and swallowing was a
load-bearing part of it.

The right fix was to make the click's *effect* legible instead of making its *delivery* conditional:
CLICK-PLAIN pairs the restored delivery with an ABSOLUTE run-state toggle (`paused = !(paused ||
hidden)` — a click on anything not running makes it run) and a visible acknowledgement. Under those
two, the original defect cannot recur — a click on a parked vug makes it run whether or not the press
was also delivered — so the swallow was buying nothing and costing predictability. The same judgement
retires VUGMIN-C's departing-owner hide in the same commit: a focus change that *stops* an unrelated
window is another effect with no visible cause. **A raise is purely additive.** Both halves of
CLICK-PLAIN are the same principle, and it is the correct one: prefer a rule that is true
unconditionally over a rule that is true given hidden state.

Nothing about that reasoning is aarch64-specific, so when x86 grows a press path it should be built
to the CLICK-PLAIN contract directly and should never pass through the CLICK-SWALLOW shape.

### What this arc landed

One in-lane change, in `arch/x86_64/syscall.rs`: `wm::hittest_selftest()` now runs on x86, chained
off `winx_launcher` after the WINX-1 verdict (after the fixture's window is confirmed retired, so the
selftest's two rows cannot burn a one-shot per-window latch — the ordering rule aarch64 states at its
own call site). The battery has always been arch-neutral and has only ever been driven from
`arch/aarch64/syscall.rs`; every claim it makes about shared code was, until now, an aarch64-only
claim. It is table-driven rather than pixel-driven, so it needs no pointer and runs headless.

x86 wire line, both gate runs, 1280x800 panel:

```
[clickroute] hit-test at (428,215) inside=true topmost=true raise=true outside=true hidden=true -> PASS
```

All five legs hold on x86's real z-order with the console window live in the table: the lookup
resolves *something*, resolves the FRONTMOST something, follows a raise, returns `None` on a genuine
miss, and returns `None` for a row pushed below `SHELL_Z`. The address lookup is sound; only its
callers are missing. When the press path arrives, this line is what distinguishes "resolved the wrong
window" from "never resolved one" — which is the discrimination the operator's two standing
complaints ("clicks eaten" vs. "out-of-focus clicks stop the focused app") have never had on x86.

### The prerequisite chain — CLOSED (CLICK-X86 r2, 2026-07-29)

The four items the audit above listed as unwritten are written. Recorded in the audit's own order,
each with what actually landed:

1. **A press dispatch site — done.** `main.rs`'s x86 drain has an `Event::Button` arm, and the routing
   decision runs one step earlier still: the drain calls
   `arch::x86_64::syscall::wc_click_route(raw)` *before* `user_input_route`, because `user_input_route`
   routes by FOCUS and a click belongs to the window under the cursor. The arm is `#[cfg]`-gated to
   x86 (the loop is shared with aarch64-UEFI, which routes clicks from its own drains).
2. **Focusable windows — done, via a reserved kernel-owner band.** See the next subsection; this was
   the item that needed an argument rather than an edit.
3. **`SYS_INPUT_POLL` (27) and per-address-space input rings — already done** by WINX-7, which landed
   while the audit was being written. "The press is delivered" now names something.
4. **The router — done**, in `arch/x86_64/syscall.rs`, built to CLICK-PLAIN directly. x86 has never
   passed through the CLICK-SWALLOW shape and will not.

### Window ownership: `KERNEL_OWNER_*`, a band that is hittable but not focusable

The audit's finding 5 stated the blocker: every persistent x86 window carries owner ASID 0, and
`hit_test` skips owner 0, so a wired hit-test resolves `None` for the console and the desktop demo —
the two largest objects on the panel and the ones the operator will actually click.

`fbcon.rs`'s owner-0 choice was deliberate and documented, so changing it needed an argument. The
argument is that the comment justifies owner 0 by exactly **two** consequences, and neither is the one
being changed:

* the row is outside `focus_ring` — no keyboard-focus cycle can hand the keyboard to the kernel's
  console; and
* the row is outside `close_owner`'s reach — no EL0 task can move, present or close it.

Being **unhittable** was never argued for anywhere. It is a side effect of encoding two different
facts — "the kernel owns this" and "nobody owns this" — in one value. So the fix splits them rather
than overturning the decision:

* `wm::KERNEL_OWNER_BASE` (`0xFFFF_FF00`) names a reserved band of owner ASIDs meaning *the kernel
  owns this window*. `KERNEL_OWNER_CONSOLE` and `KERNEL_OWNER_DESKTOP` are distinct values in it, so a
  click raises exactly the window under the hand rather than all of the furniture together.
* `focus_ring` skips the band explicitly — property one, preserved.
* `close_owner` refuses the band outright — property two, preserved, and *strengthened*: owner 0 was
  safe only because no teardown seam happens to pass 0.
* `hit_test` needs no change: the band is non-zero, so kernel rows became hittable by construction.
* Owner 0 keeps its remaining meaning — *nobody owns this row* — held by the compat shim's rows and by
  the transient witness probes, which must stay unclickable.

**A second, separate ownership fix.** `SYS_WIN_CREATE` was handing the compositor an *unbiased* slot
number while `USER_INPUT_ACTIVE` carries `slot + 1`. Because x86 slot 0 is a real address space, the
first program to launch got a window with `owner_asid == 0` — skipped by both `hit_test` and
`focus_ring`, i.e. neither clickable nor tabbable, silently and only for slot 0. The owner is now
`slot + 1`, so the value `hit_test` returns is the value the router compares against the input focus,
with no conversion at the one seam that decides who receives a keystroke.

### The router: `wc_click_route`, and the one arm that is x86-specific

On a PRESS edge the point is hit-tested and lands in one of four arms:

| hit | disposition | focus | press |
| --- | --- | --- | --- |
| a window owned by another address space | `raise+deliver` | `set_active(owner)` then `focus_changed(owner)` | **delivered** into the raised owner's ring |
| the already-focused window | `deliver` | unchanged | delivered |
| a KERNEL-owned row (console, demo) | `consume` | `set_active(0)` — keyboard to the shell | consumed |
| nothing (bare desktop) | `consume` | `set_active(0)` | consumed |

with the two CLICK-SHELL r2 limits on the last arm intact: with focus already at the shell nothing is
consumed, and a full-screen app presenting through the compat row is exempt (`wm::compat_live`, added
here) because a compat row covers the panel but carries owner 0 and can never be hit.

The RELEASE edge is never re-hit-tested. `CLICK_PRESS_TARGET` records where the press went and the
release either follows it or is dropped — a press/release pair is never split across two apps, and no
release is ever delivered to an app that did not see the press.

**The one arm that differs from aarch64, and why.** On the kernel-row and desktop arms this router
calls `user_input_set_active(0)` but does **not** call `wm::focus_changed(0)`. On aarch64 the shell is
the desktop layer *beneath* the window layer, so raising `SHELL_Z` reveals the console. On x86 the
console **is a window row** (`fbcon::panel_console_window_open`), so raising `SHELL_Z` above every
window would push the console below the shell, stop it compositing and erase it to the desktop colour
— it would blank the console the operator just clicked. The kernel row's own z-bump
(`focus_changed(owner)`, which touches only that owner's rows) is the correct raise on this arch, and
`SHELL_Z` is left where it is. The selftest asserts that `SHELL_Z` does not move.

### The witness: one line per press, and it separates the two complaints

`[clickroute]` gains a per-press row beside the existing hit-test verdict — the same vocabulary, not a
second one:

```
[clickroute] press at (x,y) win=N owner=0xA was=F -> <disposition> deliver=D
```

Human-rate by construction (a hand cannot click faster than serial can print), so it carries no
throttle. It resolves the operator's two standing complaints in one sitting:

* **"the click was eaten"** — press the pad and **no `[clickroute] press` line appears at all**. The
  press never reached the router; the defect is upstream in HID or the queue and no routing change can
  fix it. Before this arc *every* x86 press was in this state, silently.
* **"the click was mis-routed"** — a line appears and its `win=`/`owner=` name something other than the
  window the hand was over, or `deliver=` names an asid other than that window's owner.
* **"an out-of-focus click stopped my app"** — a line whose `was=` names the focused app while
  `deliver=` is 0 or another asid is positive proof the rule held: the press went where the hand
  pointed, not to whoever held the keyboard.

`deliver=0` means the press entered no app's ring. `click_stats()` carries the `(presses, delivered)`
rollup for a coarser read.

### Coverage: `clickroute_selftest`, headless-drivable

QEMU delivers no pointer, so coverage extends `hittest_selftest`'s idiom one layer up: the press
POSITION is a parameter (`wc_click_route_at`) rather than a read of the cursor, and every claim is
made against the window table and the input rings rather than against the panel. Where
`hittest_selftest` asserts *which window owns a pixel*, this asserts *who receives the press*.

Six legs, each a distinct failure direction: `hit` (three disjoint probe rows resolve to their own
owners), `deliver` (a press over an unfocused window is **not** consumed and moves focus — the
direction CLICK-SWALLOW would fail), `depth` (the press then actually lands in that ring, and its
release follows it — 1 then 2), `kernel` (a press on kernel furniture is consumed, hands the keyboard
to the shell, and does not move `SHELL_Z`), `desktop` (a press on an unowned point is consumed and
hands the keyboard to the shell; reported as `skip` rather than a false verdict if the live panel
leaves no unowned point), and `nofab` (the release after a consumed press is dropped, never
delivered).

The probe rows are deliberately three DISJOINT boxes, not `hittest_selftest`'s stacked pair: this
witness raises windows as part of the decisions it is testing, and overlapping boxes would let a raise
silently turn the "different window" leg into the "already focused" one.

### Interaction with the full-screen `vug` demo: none

`vug.rs`'s `drain_input` reads `crate::pal::pump_and_poll()` directly and treats `Event::Button(_)` as
its exit gesture. That is a *different drain*: while the full-screen demo is presenting, `handle_key`
has taken over and `kernel_main`'s inner drain — the only place `wc_click_route` is called from — is
not running. The router is never interposed on the demo's events, its click-to-exit is unchanged, and
this arc adds no call into `pump_and_poll`. (The compat-row exemption in the desktop arm is the
belt-and-braces half of the same statement, for a *ring-3* full-screen app presenting through
`SYS_FB_PRESENT` while the main drain does run.)

## HITTEST-GEOM — the hit-test witness stops assuming an empty panel (2026-07-29)

`hittest_selftest` passed on every QEMU gate and FAILED on the bench. s50, rMBP, 2880x1800:

```
[clickroute] hit-test at (962,465) inside=true topmost=true raise=true outside=false hidden=true -> FAIL
[clickroute] route hit=true deliver=true depth=1/2 kernel=true desktop=true nofab=true -> PASS
```

Four legs held on silicon; only **outside** — "a point clear of both probe windows hits nothing" —
failed, and only at bench geometry.

### The cause is the FIXTURE, and `wm::hit_test` is exonerated

The fixture derived its miss point arithmetically: probe origin `(pw/3, ph/4 + TITLE_H + BORDER)`,
plus `8 * scale + BORDER + 4` on both axes. At 2880x1800 that is roughly `(1029, 532)` — which is
inside the console WINDOW, 1314x750 centred at `(783, 444)` on that panel. `hit_test` answered
correctly: that pixel belongs to the console row, so it is not a miss. The leg asserted a fact about
the panel that stopped being true in CLICK-X86.

Why it held before: `hit_test` skips owner-0 rows, and before CLICK-X86 every piece of kernel
furniture carried owner 0. Everything but an app window read as a miss, so a point offset from the
fixture's own origin was unowned **by construction** and no search was needed. CLICK-X86 gave the
console and the desktop demo owners in the reserved band and made them hittable — deliberately, so
the operator can click them — and that invariant died with the change. The sibling
`clickroute_selftest` was written in that same arc and already FINDS its desktop point on the live
table, reporting `skip` when the panel leaves none; the hit-test witness predates it and kept its
constant. That is the whole of the difference between the two.

Why QEMU never saw it: `wcx::activate` runs only from the Kepler display takeover, so on the QEMU x86
gate neither the console window nor the desktop demo row exists at all. The gate panel is bare, every
derived point is unowned, and the leg passes for a reason that has nothing to do with the leg. (The
CLICK-X86 r2 note above says the x86 gate line was taken "with the console window live in the table";
that was wrong — it is live on the bench, never on the gate.)

`hit_test` itself needs no change. It converts a non-negative `i32` to `usize`, mirrors `outer_box`'s
saturation on both edge sums, and has no stride, `i32` boundary or wrap-around exposure at any panel
size — 2880x1800 is nowhere near a `usize` edge. It is arch-neutral shared code and is untouched by
this arc, so the pi seat inherits nothing from it.

### The fix

The miss point is now FOUND, not computed: the historical diagonal point first (so a bare panel probes
the pixel it always did), then the four panel corners inset 2 px, and the leg reports `outside=skip`
with the chosen point printed as `miss=(x,y)` if the live panel leaves nothing unowned. Two properties
keep the search from making the leg circular:

* it runs **before** the probe rows are created, so the only thing it can reject is a point some REAL
  window owns — a probe row cannot influence the choice;
* candidates inside the probe box are rejected **arithmetically**, from `spawn_geometry`, not by
  asking `hit_test`.

What the leg then asserts — still a miss with both probe rows up and A raised — is a claim about the
probe rows, which is what it was always meant to be. Raising A cannot lower anything, so a point
unowned at search time stays unowned unless a probe claims it.

### Proof of geometry-independence

The x86 panel is drivable from the harness with no source change:
`UNAOS_QEMU_EXTRA="-vga none -device VGA,xres=W,yres=H,vgamem_mb=N"`. A bench-shaped panel was
simulated by opening one pinned kernel-owned window at 7/8 of the panel, centred — the console
window's own shape — in a throwaway build.

| panel | furniture | before | after |
|---|---|---|---|
| 1280x800 | none | `outside=true` PASS | `outside=true miss=(463,250)` PASS |
| 1920x1200 | none | `outside=true` PASS | `outside=true miss=(677,350)` PASS |
| 2880x1800 | none | `outside=true` PASS | `outside=true miss=(1029,532)` PASS |
| 1280x800 | console-sized | `outside=false` **FAIL** | `outside=true miss=(2,2)` PASS |
| 2880x1800 | console-sized | `outside=false` **FAIL** | `outside=true miss=(2,2)` PASS |

The 2880x1800 occupied row reproduces the bench line exactly, probe point and all:
`[clickroute] hit-test at (962,465) inside=true topmost=true raise=true outside=false hidden=true -> FAIL`.
The verdict is now the same at every geometry, occupied or bare, which is what a trustworthy fixture
owes.

### Gate results (HITTEST-GEOM, 2026-07-29, QEMU)

* `./arroyo check` — **`✅ x86_64 OK` / `✅ aarch64 OK`**.
* `UNAOS_WC=1 ./arroyo test 90` — **0 FAIL**, `[clickroute] hit-test ... -> PASS` and
  `[clickroute] route ... -> PASS`; measured baseline at `693e097f` was the same set with the
  pre-fix `hit-test` line.
* `./arroyo test-arm 22` — banner `:: AARCH64 build: witness=on ... ::` present, **0 FAIL**; the
  arm gate emits no `[clickroute]` line at all (the battery's aarch64 driver does not reach the
  hit-test witness inside the gate window), so that gate is byte-inert across this change.

**Shared-code note for the pi seat.** `hittest_selftest` IS driven on aarch64
(`arch/aarch64/syscall.rs`, after `wci_rollup`), so this is a shared-fixture change, not an x86-only
one: on a panel where the aarch64 side has kernel furniture in the table, the witness now finds its
miss point instead of assuming one, and its wire line gains a `miss=(x,y)` field. `wm::hit_test`
itself is untouched — there is no routing behaviour change on either arch.

### Gate results (CLICK-X86 r2, 2026-07-29, QEMU)

* `./arroyo check` — **`✅ x86_64 OK` / `✅ aarch64 OK`**.
* `UNAOS_WC=1 ./arroyo test 90` — **42 PASS lines / 0 FAIL**; measured baseline at `aed91dc0` was
  **41 / 0**. The +1 is the new `[clickroute] route ... -> PASS` verdict and nothing else moved.
* `./arroyo test-arm 22` — boots to `gui:handoff`, banner present, **0 FAIL**, identical to the
  measured baseline. aarch64 is behaviour-inert: the `main.rs` Button arm is `#[cfg]`-gated to x86,
  the `wm.rs` additions are new items plus one `focus_ring` predicate and one `close_owner` guard that
  are both unreachable on a target where no row ever carries a band owner, and `fbcon.rs`'s console
  window is itself x86-only.

x86 wire lines, this arc:

```
[clickroute] press at (482,215) win=2 owner=0x2 was=1 -> raise+deliver deliver=2
[clickroute] press at (536,215) win=3 owner=0xffffff7f was=1 -> consume deliver=0
[clickroute] press at (2,2) win=0 owner=0x0 was=1 -> consume deliver=0
[clickroute] route hit=true deliver=true depth=1/2 kernel=true desktop=true nofab=true -> PASS
```

### Gate results (CLICK-X86 r1 — the audit arc, 2026-07-29, QEMU)

* `./arroyo check` — **`✅ x86_64 OK` / `✅ aarch64 OK`**.
* `UNAOS_WC=1 ./arroyo test 90`, two consecutive runs — **39 PASS lines / 0 FAIL** each
  (37 verdicts; two of the 39 are `[wc-d] verify` lines that carry the word incidentally). Baseline
  at `9c2c6b94` was 38/36; the +1 is the new `[clickroute]` line and nothing else moved.
  `MISSION SUCCESS` present.
* `./arroyo test-arm 22` — **`MISSION SUCCESS`**, 0 FAIL. aarch64 is byte-inert: the code diff is one
  file under `arch/x86_64/`.

---

## CURSOR-14 — the instrument was the defect: `nosprite` measured the era before the sprite existed

### What was asked, and why the question was wrong

CURSOR-13 landed on both arches. The next attended x86 boot (s50, kernel with CURSOR-13 aboard) put
this on the wire:

```
[cursor] armed x=1433 y=900
[cursor12] offer scope=live passes=69 nosprite=69 nohit=0 reserved=0 nosession=0 planned=0 excl_probe=0 excl_unverified=0 -> nosprite
[cursor6] rollup scope=live present_over=0 masked=0 repaired=0 desktop_over=0 mismatch=0 uncover_lost=0/0 -> UNWITNESSED
```

Read against GR7 s48 (42/42) and s49 (47/47) this says "CURSOR-13 changed nothing measurable", and
the arc was opened as a refutation on silicon: find the second bracket that is still starving the
composite.

**There is no second bracket on the hot path. The three readings are the same measurement artefact,
and it is the only thing those numbers could ever have said.**

### The proof, from the capture itself

Two facts about the instrument, both in the tree before this arc:

1. **`pal::cursor::rollup_tick` fired on the FIRST pointer report of the boot.** `ROLLUP_LAST_MS`
   starts at 0 and the limiter read `if last != 0 && now - last < ROLLUP_EVERY_MS { return }`, so a
   zero fell straight through to the print.
2. **Every `[cursor12]` term was a running total since boot**, loaded and printed raw, never
   snapshotted.

Compose them. `move_rel` runs `touch()` → `set_clamped` → `repaint_on_move()`, and `repaint_on_move`
is `cursor::repaint()` *then* `rollup_tick()`. On the first report of a boot the `repaint` draws the
sprite for the first time — emitting `[cursor] armed` — and the very next call prints a block
covering every composite that ran **before that draw**. During all of them `sp.drawn == false`, so
`cursor::sprite_plan()` returns `None` by construction. `nosprite == passes` is not a finding; it is
arithmetic.

The tell is visible in the capture without reading any code: **`[cursor] armed` sits immediately
above the block, because both lines come from the same pointer report.** 42, 47 and 69 are not a rate
that failed to move — they are three boots' worth of pre-pointer composite totals, and they differ
from each other exactly as much as three boots differ.

The debt did not clear on the next block either. With ~40–70 boot-era passes permanently in the
numerator and a sitting adding ~14 passes/s, `nosprite * 2 >= passes` — the verdict's own predicate —
stays true for the first ten-odd seconds of continuous motion **whatever the mechanism is doing**.

### Consequence for the cross-seat record

The previous executor's call-graph analysis — *"after CURSOR-13 our x86 path has exactly one
caller-side bracket left, `pal::TargetPal::render`, and its `undraw` … `ensure_drawn` is an
idempotent close that cannot re-starve the composite"* — **was correct.** What was wrong was the
inference drawn from the wire against it. The failure was not in the reasoning about the code; it was
in treating a structurally-pinned counter as evidence.

The same applies backwards. **P74's original `nosprite ≈ passes` reading on GR7 s48 was a first-block
reading too**, so the evidence that motivated CURSOR-13 was the identical artefact. CURSOR-13's
*argument* stands on its own — a bracket that spans a composite does starve `sprite_plan()`, and the
call graph says so without any counter — but its *measurement* never demonstrated the effect it
claimed. Neither seat has yet seen a valid `nosprite` sample. This arc is what makes one possible.

The operator's own words on the same s50 boot are the independent corroboration:
**"mouse makes it over the top of everything now."** The arrow is on glass above window content. The
instrument was simply unable to say so.

### The composite call-site table (x86), with the sprite's state at entry

`wm::composite()` is private-by-design behind `composite_inner`; every path below reaches it.

| # | Call site | Reached on x86 from | Sprite at entry | Why |
|---|---|---|---|---|
| 1 | `Screen::flush` → `wm::service_damage` → `composite` | `pal::TargetPal::render`, every console-loop pass | **UP** | CURSOR-13: `flush`'s `repaint()` runs before the composite. Early-returns on an undamaged table. |
| 2 | `Screen::flush` → `wm::repaint` → `composite` | same, intrusion fallback only | **UP** | Same bracket, same close. `[wc-i] intrusions=0` says this arm is not taken. |
| 3 | `fbcon::route_present_rows` → `wm::present_rows` → `present_banded` → `composite` | **every routed console line** (FBCON-DMG) | **UP**, and this is the volume path when the console is not suspended | The print path takes no cursor bracket of its own. Suspended entirely while an INSTGUI dialog is open. |
| 4 | `arch::x86_64::syscall` window-present verb → `wm::present` | EL0 `SYS_WIN_PRESENT` | **UP** | Syscall context; no bracket anywhere above it. |
| 5 | `instgui::repaint` → `wm::present` | dialog state change / keypress only | **UP** | Ordinary `wm::present`. **No special path** — see below. |
| 6 | `wcx::activate` → `wm::present` / `create_at` | once, from PCI enumeration | DOWN, harmlessly | `wcx.rs:193` undraws before its one-shot `fill_screen`. Runs before the sprite has ever existed, so the undraw is a no-op. |
| 7 | `wm::create_at` / `create_inner` → `composite` | window creation | **UP** | No caller-side bracket. |
| 8 | `wm::focus_changed` → `composite` | focus raise | **UP** | No caller-side bracket. |
| 9 | `wm::move_to` → `erase` → `composite` | window move | **was DOWN** | `erase` undraws; the composite ran inside that bracket. **Fixed this arc.** |
| 10 | `wm::close` → `erase`/`reclaim` → `composite` | window close | **was DOWN** | Same. **Fixed this arc.** |
| 11 | `wm::close_owner` → `erase`/`reclaim` → `composite` | ASID teardown | **was DOWN** | Same. **Fixed this arc.** |
| 12 | `pal::TargetPal::render`'s own bracket | every present | spanned #1/#2 harmlessly, but held the sprite down across the whole desktop blit | **Removed this arc.** |

Rows 9–11 are genuine starving paths of exactly CURSOR-13's shape, and they were the only ones. They
are also *rare* — a window move or teardown, not a frame — so they cannot account for a 100% reading
and never could have.

### What changed

**1. `pal::TargetPal::render` — the last caller-side bracket goes (`pal.rs`, was x86-gated).**

```rust
fn render(&mut self) {
    self.surface.flush();          // CURSOR-13's body owns the sprite end to end
}
```

Two reasons, and the second is the one that matters. It cost two wasted `SPRITE` acquisitions per
present (an earlier arc flagged this and could not act on it). And from its `undraw` to `flush`'s
`repaint` the arrow was off the panel **for the length of the entire desktop blit** — the longest
front-buffer write in the system — so any composite reached during that window from another core, or
from an IRQ-context printer on this one (`fbcon::route_present_rows`, row 3 above), entered with
`sprite_plan() == None`. That window is now `present_background` alone, the smallest it can be while
a raw desktop blit exists. `render` is arch-neutral in shape again.

**2. `wm::move_to` / `close` / `close_owner` — close the erase bracket before the composite.**

`super::cursor::repaint();` immediately before each `composite()`, after the last `erase`/`reclaim`
and after the drain barrier is dropped. Same shape as `Screen::flush`: the save-under is taken
against a front buffer whose desktop is already final, and the composite then owns the sprite. Costs
one restore/save/draw per window move or close.

**3. `pal::cursor::rollup_tick` — the first report ARMS the window, it does not print one.**

**4. `[cursor12]` — every term is a delta since the previous block; `cum=` carries the boot total.**

Together, the first `[cursor12] scope=live` block to reach the wire now covers one full 5 s window of
real pointer motion with the sprite alive throughout. A block whose `passes == cum` is the whole-boot
baseline and carries no verdict.

```
[cursor12] offer scope=live passes=N nosprite=… … cum=M -> why
```

### What proves the fix on the next boot, and what refutes it

**Proves.** A `[cursor12] scope=live` block with **`passes` strictly less than `cum`** (i.e. a real
window, not the baseline) in which, with the pointer moving **over a window**:

* `nosprite` is small — ideally 0, legitimately non-zero only for passes inside CURSOR-HIDE's ~1.5 s
  idle expiry;
* `planned > 0`, and `[cursor3] offers > 0` with `taken > 0`;
* `[cursor6] desktop_over` stays **0** (the retained desktop bracket, confirmed clean on s50).

With the pointer over **bare desktop**, the correct reading is `nohit ≈ passes`, not `nosprite`.

**Refutes.** A windowed block (`passes < cum`) that *still* reads `nosprite ≈ passes` with the
pointer demonstrably moving over a window. That would mean a bracket this table missed, and the next
thing to instrument is the composite's *caller*, not its predicate — tag each `nosprite` pass with
its entry point.

**Does not refute, and must not be read as refutation:** the first block of a boot, or any block
where `passes == cum`. Also `nosprite = passes` on either QEMU gate — neither has a pointer, so the
sprite is never drawn and the reading is *correct* there and cannot move.

### The INSTGUI appearance report — a separate defect, and the discriminating question

Same s50 boot, operator at the machine: **"mouse makes it over the top of everything now but it
doesn't look right over the top of the installer window."**

Row 5 of the table answers "does instgui reach the compositor by a path the others do not": **it does
not.** `instgui::repaint` → `wm::present` is the ordinary verb. What *is* unique to instgui is on the
other side of the seam — **`instgui::open` calls `fbcon::console_present_suspend(true)`**, so while
the dialog is up the routed console (row 3, the highest-volume compositor path on this arch) stops
presenting entirely. The repaint *cadence* under the arrow changes there and nowhere else.

Candidates at pixel level, from the mechanism:

* **(A) Trailing / smear while moving.** The save-under is captured from the front buffer at draw
  time; if it was taken against non-final dialog pixels, the restore stamps them back. CURSOR-9's
  `TOUCHED_SINCE_DRAW` repair mends it at the next composite — within one desktop frame — so this
  predicts a *brief* smear at the pointer's rate, visible only in motion.
* **(B) A hard block of stale content inside the dialog.** Same mechanism, but persisting, which
  requires the mend not to arrive. With console presents suspended the mend comes only from
  `Screen::flush` → `service_damage`; that still runs every console-loop pass, so the code does
  **not** predict a persistent block.
* **(C) Flicker at the dialog's own repaint rate.** `instgui::service` repaints only when the disk
  list signature changes, so the dialog presents almost never. The code **positively predicts this
  does not happen.**
* **(D) The arrow reading wrong rather than behaving wrong.** The sprite is a white `FILL` with a
  dark drop shadow offset by `(s, s)`. Every other surface on this panel — console, corner demo,
  desktop — is dark; the installer dialog is the one piece of **light** chrome in the system. A white
  arrow plus a shadow overhang on light grey is low-contrast and the shadow reads as a smudge. This
  is not a defect in any mechanism.

**The code predicts (A) or (D), and rules out (C).** One question separates them, and it is the only
question the operator needs to be asked:

> **Hold the pointer perfectly still over the installer dialog. Does it look wrong then too, or only
> while you are moving it?**

* *Only while moving* → (A): save-under / mend latency. Compose-through is the fix, and making it
  reachable is exactly what this arc does — **expect it to improve**.
* *Wrong when still as well* → (D) (or (B)): appearance over light chrome, or a static stale restore.
  **Compose-through will not touch it.** (D)'s remedy is the sprite's own colours — an outline, or a
  contrast-aware fill — which is a different arc.

**Do not assume the two are the same bug.** They share a symptom surface, not a mechanism: the
`nosprite` finding is an instrumentation artefact, and the instgui appearance is either a save-under
latency or a palette choice. Nothing in the code makes the second a consequence of the first.

### Gate results (CURSOR-14, 2026-07-29, QEMU)

* `./arroyo check` — **`✅ x86_64 OK` / `✅ aarch64 OK`**.
* `UNAOS_WC=1 ./arroyo test 90` — **33 PASS lines / 0 FAIL**, 594 log lines. Baseline measured this
  session at `693e097f`: **33 PASS / 0 FAIL, 594 lines**. No regression, and no line moved.
* `./arroyo test-arm 22` — banner reached (`:: AARCH64 Core Hardware Init ::`,
  `AARCH64 boot diag: EL=2 CNTFRQ=62500000 Hz MMU=on`), 341 lines, 0 FAIL / 0 PANIC.

**Neither QEMU gate has a pointer**, so `rollup_tick` never fires and no `[cursor12] scope=live` block
is emitted on either. The gates are no-regression only; the verdict is owed by the next attended
boot.

### Out of lane / flagged for the pi seat

Three of the four changes are in shared files and the pi seat should lift them:

* **`wm.rs` rows 9–11** (`repaint()` before `composite()` in `move_to`/`close`/`close_owner`) — three
  one-line insertions, arch-neutral, behavioural on both arches.
* **`wm.rs` `[cursor12]` delta reporting** — witness-only, arch-neutral, changes the wire format
  (adds `cum=`). Any log-scraper on either bench needs the new field.
* **`pal.rs` `rollup_tick`** — witness-only and behaviour-inert, but `rollup_tick` is *not*
  arch-gated (only the `repaint` above it is), so **this changes aarch64's witness output too**: the
  pi bench will lose the first `[cursor12] scope=live` block of each boot. That is the intended
  effect — it is the block that could never be true — and the pi seat should expect it.

`pal::TargetPal::render` (change 1) is x86-only in effect: the bracket it removes was
`cfg(target_arch = "x86_64")`, so aarch64 is byte-inert there.

## CLOSE-BOX — the close button, and the single action-click exception (2026-07-29)

P79, bench sitting, Peter's direct request: *"put a close button in the upper right of the windows
to exit."* Landed on the pi track as `video/wm: CLOSE-BOX`.

### Geometry

Every decorated (non-compat) app window's title strip now ends in a **close box**: a
`TITLE_H`-sided square (12 px at 1x) flush against the inner edge of the border at the strip's
RIGHT end — `x = bx + bw - BORDER - side`, `y = by + BORDER` in outer-box coordinates. The box is
filled in a red-tinted chrome colour (`CHROME_CLOSE_BG`, brightening to `CHROME_CLOSE_BG_FOCUS`
with the rest of the focused window's chrome) with a 2-px X glyph in the title foreground colour.
The title's width budget excludes the box, so a long title truncates beside it rather than running
under it.

One function, `wm::close_box(r)`, is the single source of the rect: `paint_window` draws it and
`wm::close_box_hit(id, x, y)` (the router's test) checks it, so the drawn box and the clickable box
are the same rect by construction. Rows that decline the box: **compat** (no chrome at all — the
full-screen shim's own click-to-exit owns its clicks), **owner-0** (kernel furniture, the same
CLICK-SHELL distinction that keeps `hit_test` from ever naming the console/desktop — a close box on
shell furniture would be an invitation to kill the shell), and a strip too narrow to hold the box
plus one title glyph.

### Routing precedence — close beats select

In `wc_click_route`'s PRESS arm the close-box guard is tested FIRST, on the window `hit_test` has
already named for the point: a press in that window's close box takes the CLOSE arm, and only a
press elsewhere in the window falls through to the ordinary select/deliver arms. The press is
CONSUMED (`CLICK_TARGET_DROP`, so the release is dropped with it) — it is not delivered to the app,
because the app it would address is being torn down.

### Why this is the ONE action click, and why it does not breach CLICK-SELECT

CLICK-SELECT (P77) is Peter's grammar and it is FINAL: a click on a window only SELECTS (focus +
the cyan `clicks=N` ack); SPACE stops/starts; focus never stops or starts anything. The close box
does not amend that rule, because the box is not the app's surface — it is **window furniture**,
kernel chrome drawn and hit-tested by the window system itself. A click there is an instruction TO
THE WINDOW SYSTEM ("this window goes away"), not input to the app, which is why it takes effect at
the router and never enters the app's ring. Every other pixel of the window, title strip included,
keeps the select-only grammar unchanged.

### Close semantics

The CLOSE arm (`wc_close_click`), in order:

1. **Windows first** — `wm::close_owner(owner)` removes every row the owner holds (a vug parent
   and its workers are one owner), so the click is answered on the panel immediately.
2. **Focus** — if the closed owner held either half of focus, it is handed to the SHELL through the
   one focus primitive (`user_input_set_active(0)` then `focus_changed(0)`), the same state a
   TAB-to-shell leaves.
3. **Kill** — the SKILL-1 primitive, ASID-scoped (`sched::kill(pid, owner)`), so every sibling
   thread dies with the parent; `kill` evicts targets parked in kernel waits (`futex_wake_killed`
   scans all buckets), which is what reaches a fleet idled in `SYS_INPUT_WAIT`. On a CONFIRMED kill
   the Proc row is **settled, never reaped** — a status store, a CAS
   `PRUNNING -> PEXITED`, one `done` post, exactly the fault-kill shape — because this path races
   whichever launcher owns the row (`bg`'s poll or a foreground `run_user_image` blocked in its
   wait loop), and reaping here would re-open the double-reap hazard `bg_kill`'s LENS note
   documents. **CLOSE-CLEAN (P80) amends the settle status**: the row settles with the fault-kill
   SHAPE but the status is `EXEC_CLOSED_STATUS`, not `EXEC_KILLED_STATUS` — see below.
   Both launchers converge on the settled row through their existing exit paths. An
   UNCONFIRMED kill detaches (stays armed; the target dies at its next boundary) and leaves the row
   alone. Owners outside the slot range (witness fixtures) skip the kill — the row close is the
   whole effect.

### CLOSE-CLEAN — a close-box exit reads *closed*, not *faulted* (2026-07-29)

P80, bench sitting, Peter's verdict: *"i closed all the vugs with the x but jobs says faulted and
reaped."* Root cause: the CLOSE-BOX settle reused the fault-kill sentinel (`EXEC_KILLED_STATUS`,
`i32::MIN`) as the Proc row's exit status, and every classifier downstream — `bg_poll` for the
`jobs` verb, the `run_user_image` wait for a foreground `run` — maps that sentinel to *Faulted*.
An operator-requested close therefore printed as `FAULTED (contained; reaped)`, indistinguishable
from a genuine contained fault.

The fix is a second, distinct sentinel: `EXEC_CLOSED_STATUS` (`i32::MIN + 1`, equally
non-colliding with real exit codes). `wc_close_click`'s confirmed-kill settle stores it instead of
the kill sentinel; the classifiers grew a matching clean variant end to end:

* `bg_poll` → `BgPoll::Closed`; the `jobs` verb prints `pid N  closed (reaped)  <name>` (serial:
  `:: BGRUN: jobs — pid=N exit=CLOSED reaped ::`) — the shape of a normal completed job.
* `run_user_image` → `RunOutcome::Closed`; a foreground `run` whose window is closed prints
  `run: <path>: closed (window close box)` (serial: `exit=CLOSED`).

Genuine fault-kills are untouched — the fault-kill net still stores `EXEC_KILLED_STATUS` and still
reads `FAULTED`; the two classifications never blur. The one race (a fault-kill and the close
settle hitting the same task) is decided by the existing single-shot CAS: whichever settle wins
the `PRUNNING -> PEXITED` flip has already published its own status, honestly.

### Witness

* `[clickroute] close=win<id> asid=<owner> at (x,y) settle=<tag>` — one line per close click,
  rate-limited `[spread4] rewake`-style (first 16 named, then quiet) so a stuck button over a
  re-spawning owner cannot flood the wire. CLOSE-CLEAN moved the emit to after the settle and
  added the `settle=` field: `closed` (kill confirmed, row settled clean), `noproc` (window
  furniture only), `dead` (target already exited), `armed` (unconfirmed; request stays armed),
  `exhausted` (no kill slot).
* `[skill] close-box ...` — the kill's disposition (confirmed / stays-armed / nothing-to-kill).
* **Leg 9** of `hittest_selftest` (`close=` field in the `[clickroute] hit-test` line): a probe
  window is placed so its CLOSE BOX contains the real cursor (the CLICK-PLAIN fixture discipline —
  move the window, never the pointer), one press is driven through the shipped router, and the leg
  asserts the press was CONSUMED, the row is GONE (`info(w)` empty), and focus fell to the shell.
  The owner is synthetic and outside the slot range, deliberately: the kill arm provably no-ops, so
  the leg is a window-layer witness and the kill half stays gated where SKILL-1 already gates it.
  Pinned in `scripts/specs/pi4-regression.spec` (`REQUIRE ... close=true -> PASS`), which also pins
  legs 1–8 via the line's tail verdict.

### Gate results (CLOSE-BOX, 2026-07-29, QEMU raspi4b)

* `./arroyo check` — both arches green.
* `./arroyo kernel8-test` — MBENCH PASS 87/87 (was 86; the new directive is the leg-9 pin), with
  `[clickroute] close=win3 asid=3085 at (320,240)` and
  `[clickroute] hit-test ... close=true -> PASS` on the wire.
* `./arroyo test-arm` — MISSION SUCCESS (leg 9 DORMANT there: no arch router on the hosted build,
  `close=skip`).

Hardware verification (the bench click on a storm vug's close box: window gone, `[skill]
close-box killed ... confirmed=1`, fleet count down by one) is owed at the next attended sitting.

## CURSOR-15 — the sessionless present composes through: the hover stutter dies (2026-07-29)

### The P82 mechanism

P82 (bench, attended, CURSOR-14 aboard): with the pointer parked over the presenting vug fleet the
arrow stutters, and the wire names the shape — `[flick2] sess_undraws=290+` and climbing at present
cadence (~123/s), `down_slow` accumulating. The same mechanism was measured independently on x86 the
same night. The compose-through machinery itself is healthy (`[cursor12] planned`/`offers`/`taken`
all fire), so the erase is not the session path: it is the passes that LOSE the session.

Under a presenting fleet several cores composite at once and exactly one owns the overlay session
per pass. Every loser took `undraw_within_nosession` — a masked handback of the sprite pixels inside
its paint set — followed by a `Repaint` tail, i.e. a whole-sprite `refresh_locked`
(restore→save→draw). One erase-and-rebuild of the arrow per overlapping present, at PRESENT cadence,
while the pointer's own repaint runs at EVENT cadence. `sess_undraws` is those tail refreshes
landing while some other pass's session is open (the full undraw finds a coherent session and
restores from the layer save — FLICKER-2's fixed path, running healthily but far too often), and
`down_slow` is the intervals they cost. The sprite spends its life mid-restore: the stutter.

### The fix: COMPOSE-THROUGH for the sessionless arm

The handback's one justification — a pixel a painter in this pass is about to overwrite must be
handed back first, or its save-under goes stale — is answered the way CURSOR-11 answered it for the
session owner: at the tail, per pixel, against the finished front. The sessionless arm now:

* **defers instead of undrawing** (`cursor::defer_nosession`, the shared `defer_common` body with
  `defer_within`): `pend` bits are marked inside the paint set, no framebuffer pixel is written, no
  generation is bumped. The arrow stays on glass; the pass's blits composite over it where they
  reach it.
* **settles at a new tail** (`CursorTail::Settle` → `cursor::settle_nosession`): for each pending
  pixel the colour guard asks the finished front whether a painter took it. Untouched → no read, no
  write (the common case). Taken → `saved[i]` is re-taken from the freshly-composited content
  BEFORE the arrow pixel is painted back over it — the FLICKER-2/3 session-fresh save-under
  discipline, extended to compose-through. This is `settle_pending_locked`, unchanged; only the
  second caller is new.
* **gates the settle on session quiescence**: `settle_nosession` probes `OVERLAY` under the sprite
  lock (`SPRITE` → `OVERLAY`, the documented order, `try_lock` never a wait). An open or contended
  session leaves the bits standing (`ct_owner`) for the owner's own tail — `settle_pending_locked`
  is bit-driven, not owner-driven, so `adopt_overlay` answers them against ITS finished front, and
  its incoherent fallback resets `pend`, which is settlement by bracket. The gate is load-bearing:
  a settle that read the front while the owner's rows were still in flight would install a stale
  save. Because `adopt_overlay` closes the session and settles under one `SPRITE` acquisition, and
  `settle_nosession` holds `SPRITE` across probe and settle, "no session open" really does mean the
  previous owner's tail fully retired.

### Coherence consequences, stated

* **CURSOR-5's generation bump is not needed on this arm any more — its stale-stamp interleave has
  no first move.** The bump existed because a sessionless handback WROTE `sp.saved` (pre-pass
  content) into the owner's freshly-presented rows; the deferral writes nothing.
* **The bump's second duty moves to a coverage clear.** If the sessionless pass's blit overwrites a
  pixel the owner's layer COVERED, the owner's install would claim a panel pixel that is now the
  loser's, with a layer save the panel no longer holds. `cursor::overlay_uncover_any` — called from
  `draw_window` for plan-less windows whose box meets the advisory sprite box — clears the open
  session's coverage inside the painted box, so the owner's tail settles those pixels against the
  finished front instead. This is CURSOR-4's back-to-front verdict rule applied across passes; the
  lens's rejected alternative on `undraw_within_nosession` is exactly right HERE because nothing
  stale was written. A contended clear invalidates the session wholesale via the existing
  `note_uncover_lost` → refuse-install → `refresh_locked` fallback, already priced.
* **What still brackets, deliberately:** the pointer-move `repaint` (sprite RELOCATION keeps its
  undraw/redraw — compose-through is for OTHER passes crossing a stationary sprite), `wm::erase`,
  the WC-L deferred-erase drain (its fills paint the front directly, so `undraw_within_nosession`
  survives with it as sole caller, generation bump and all), the WC-F reserved arm, an incoherent
  adopt tail, and `Screen::flush`'s CURSOR-13 desktop bracket (unchanged — so `[cursor6]
  desktop_over`'s meaning of "over" is untouched and its =0 verdict still catches a sprite-region
  desync on the desktop path).
* **Cross-pass settle residual, named:** a third concurrent pass can take a pixel between one
  settle's front read and its arrow write. Same class as every settle, absorbed the same way:
  `note_present_over_sprite` arms `TOUCHED_SINCE_DRAW`, the next full undraw's `repair` damages the
  window, and the taker's own tail re-settles — the settle is idempotent and the last tail wins.
* **`tail_of` precedence:** `disturbed` outranks `deferred` — a pass that both drained (masked
  handback, `off` pixels) and deferred takes `Repaint`, whose `refresh_locked` resets `pend` as it
  goes; the deferral is settled by the bracket rather than dropped.

### Witness (reading key)

* `[flick2] ... compose_through= ct_owner=` — sessionless compose-through passes, and settles left
  to an open session's tail. Expected on metal: `compose_through` tracks the present rate while
  `sess_undraws` and `mask_sess` COLLAPSE toward pointer-move-only counts and `down_slow` → 0 under
  hover. That collapse is the arc's verdict.
* `[cursor3] rollup ... settle=` (and sampled `tail=settle -> THROUGH`) — the new tail. On a hovered
  fleet `settle` should absorb what `repaint` used to count.
* `[cursor5] masked_nosession=` — must read 0 from this arc on: the mechanism it counts no longer
  runs. Non-zero means someone reintroduced the sessionless handback.
* `[cursor11] passes=` now includes sessionless deferrals (`defer_nosession` feeds the same
  counters), so `passes`/`bracketed` remains the honest through-vs-bracket ratio.
* `[cursor6]` is unchanged in meaning: a deferring pass reports `bracketed=true` to
  `note_present_over_sprite` (its `Settle` tail owes the pixels a verdict), so its presents count as
  `masked` — the denominator — exactly as CURSOR-11's session deferrals do, and `desktop_over` must
  still be 0.
* All of it `UNWITNESSED` on QEMU by construction (no HID pointer, sprite never drawn); the gate
  proves no-regression only.

### Gate results (CURSOR-15, 2026-07-29, QEMU raspi4b)

* `./arroyo check` — both arches green, no new warnings in the arc files.
* `./arroyo kernel8-test` — MBENCH PASS 87/87 required witnesses, 0 forbidden. New line shapes on
  the wire: `[cursor3] rollup ... settle=0 ...`, `[flick2] ... compose_through=0 ct_owner=0 ...`,
  `[cursor5] ... masked_nosession=0` — all UNWITNESSED/0, as the gate must read them.
* `./arroyo test-arm` — MISSION SUCCESS.

Hardware verification is owed at the next attended sitting: pointer parked over the presenting
fleet, read `[flick2]` for `sess_undraws`/`mask_sess` collapsing and `down_slow` flat while
`compose_through` climbs at present cadence, and the stutter gone from the chair.
### CLOSE-FIX — the wire discriminator, the real-path close leg, and the teardown guard (2026-07-29)

P82, bench: `[clickroute] close=win3 asid=3085 at (961,599) settle=noproc` on a live boot —
asid 3085 is `0xC0D`, the hit-test battery's leg-9 synthetic owner. The line was read as a REAL
operator close carrying the selftest's fake ASID (kill finds no process, the vug survives its
window, `jobs` accumulates undead rows). Reproducing the gate at bench geometry
(`UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test`) shows the deeper defect: **leg 9's own
wire line is byte-for-byte the shape of that failure** — `close=win3 asid=3085 at (960,600)
settle=noproc`, at the boot cursor's panel centre — so the wire could not distinguish the
battery's benign no-op from a click that killed nobody, and the REQUIRE passed either way. That
is a gate-honesty defect, and the class it slept through is real: any synthetic row that
outlives the battery sits in the table with a high z, hit-tests FIRST over a real window's close
box, and starves the real owner's kill for the whole boot. Three fixes, one arc:

1. **The discriminator.** `wc_close_click`'s out-of-slot-range arm now settles
   `noproc-selftest`, a distinct tag; plain `noproc` is reserved for a REAL slot ASID with no
   live process — P82's exact kill-finds-nobody shape. The spec REQUIREs the selftest tag and
   FORBIDs plain `noproc` on the headless gate outright (no operator clicks there, so nothing
   may legitimately print it).
2. **Leg 10 (`closereal=`).** The close arm re-proven against a row the battery created through
   the ordinary path (`wa`), through the shipped router, asserting: the NAMED row is the reaped
   row, the settle read-back (`wc_close_last_settle`, a witness-only code the wire line cannot
   provide to a leg) is `noproc-selftest` and nothing else, press consumed, focus to the shell.
   A router that threads a constant, resolves the wrong row, or regresses the discriminator
   fails this leg; leg 9 alone cannot catch any of the three.
3. **The teardown witness guard + close fall-through.** `hittest_selftest`'s tail now sweeps the
   table for rows still owned by any battery ASID and reaps them, printing
   `[clickroute] hit-test teardown LEAK — N synthetic row(s) reaped -> FAIL` (spec-FORBIDden;
   also caught by the default `-> FAIL` scan) — a fixture leak can no longer be silent or
   permanent. And the router's close arm no longer stops at witness furniture: a resolution that
   settles `noproc-selftest` re-runs the hit-test at the same point and closes the next row
   whose close box contains it (bounded by the table size, one rate-limited wire line per hop),
   so even a leaked fixture cannot starve the kill of the real owner the operator was aiming at.
   Real settles (`closed`, `noproc`, `dead`, `armed`, `exhausted`) never retry.

Undead rows (window gone, process alive) accumulated by the defect remain reachable the honest
way: they read `running` in `jobs` and die by `kill <pid>`; no sweep invents an exit for them.

### Gate results (CLOSE-FIX, 2026-07-29, QEMU raspi4b)

* `./arroyo check` — both arches green.
* `./arroyo kernel8-test` — MBENCH PASS 88/88 (was 87; the new REQUIRE is the
  `settle=noproc-selftest` pin, the leg-10 pin folds into the existing hit-test REQUIRE), with
  `close=true closereal=true -> PASS` and two `settle=noproc-selftest` lines on the wire, no
  LEAK line, no plain `noproc`. Green at both default and bench geometry
  (`UNAOS_FBW=1920 UNAOS_FBH=1200`).
* `./arroyo test-arm` — MISSION SUCCESS (legs 9–10 DORMANT on the hosted build: `close=skip
  closereal=skip`).
## COMPOSITE-2 — measure the pass, then multiply it (2026-07-29)

The compositor's aggregate throughput wall — ~123 composite passes/s, ~8 ms per pass at 1920x1200,
measured identically across P79/P80/P82 — is the fps ceiling for the whole vug fleet while V3D
stays walled. This arc first put the pass's cost breakdown on the wire, then rebuilt the two terms
the measurement convicted. No protocol, window-API, CURSOR-13-bracket or FLICKER-3 semantics moved;
the WC-D/WC-G/HT oracles keep their verdicts (87/87).

### The instrument: `[comp2]` (aarch64 + witness, `video/wm.rs`)

One rollup line rides the `[wcn]` cadence and partitions every pass's wall time:

    [comp2] rollup passes=N pass_us= max_us= sprite_us= wait_us= blit_us= cache_us=
            bytes_pp= dmg_px_pp= rate=/s span=ms

* `sprite_us` — deferred-erase drain + cursor bracket + tail (`adopt`/`repaint`/`ensure`).
* `wait_us` — WRITER read, TABLE lock, damage close, ordering, guard registration.
* `blit_us` — the blit loop (compose + present), minus the cache term.
* `cache_us` — `draw_window`'s trailing clean for the non-coherent HVS.
* `bytes_pp` / `dmg_px_pp` — panel bytes written and damage-clipped box area, per pass.

### The measurement (QEMU raspi4b, `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test`)

| term                          | before   | after   | change |
|-------------------------------|----------|---------|--------|
| `pass_us` (per-pass wall)     | 14912    | 2821    | 5.3x   |
| `max_us`                      | 60213    | 13852   | 4.3x   |
| `blit_us`                     | 12952    | 1720    | 7.5x   |
| `cache_us`                    | 1007     | 267     | 3.8x   |
| `sprite_us` / `wait_us`       | 22 / 10  | 3 / 4   | noise  |
| `[wc-h]` 514x526 `compose_us` | 11034    | 850     | 13x    |
| `[wc-h]` 514x526 `present_us` | 896      | 706     | ~1x    |

The verdict was unambiguous: ~87% of the pass was the per-pixel compose — every destination pixel
paid a `put_pixel` call (four bounds checks, a format match, three byte stores), and the dense
chrome fill wrote the whole outer box only for the content loop to rewrite 97% of it. The present
copy was already bulk (`copy_nonoverlapping` rows) and barely moved. Sprite and wait terms were
never the wall. (The residual `pass_us - blit - cache` ≈ 0.8 ms is the WC-F ground-truth probe's
per-pass repaint — witness+baremetal instrumentation, absent from production media.)

### The rebuild (expected-value order, as the measurement justified)

1. **Word blits** (`framebuffer.rs`): `encode4` un-gated from x86; new `fill_span4` (per-span
   bounds + swizzle hoist, 64-bit paired stores — the build is `+strict-align`, so spans check
   word alignment and fall back to `put_pixel` rather than fault); `fill_rect`/`fill_rows` take
   the span path on aarch64 (x86 keeps its loops so `videobench`'s poke counters keep meaning).
   The only byte that differs is the pad, written 0 where `put_pixel` skipped it — no reader
   decodes it (scan-out ignores it, `read_pixel` reads 3 bytes, the staged layer's pads are 0).
2. **Flattened row compose** (`wm.rs paint_window`): the staged single-line case (every `dup` row,
   every `scale==1` row) does its clipping once per row and degenerates to "encode, store `scale`
   words, advance" over one contiguous run. Clip bounds identical to the span writer's.
3. **Damage honesty — write each pixel once**: the chrome fill is now the outer box MINUS the
   content extent (title band, bottom strip, side borders). The subtracted region is only what the
   content loop provably writes in the same call — both painters clip with the same bounds at the
   same coordinates — so WC-H's "every pixel the present copies was written by this pass"
   invariant holds exactly; short content (`cols==0`/`rows==0`) keeps the full dense fill.
4. **Cache honesty** (`flush_rect` + `arch/aarch64/cache.rs::clean_rows`): the post-blit clean
   covers the box's own columns per row with ONE trailing `DSB`, instead of full-width scanlines —
   3.7x fewer bytes cleaned for the bench's 514-wide box. Same swap on the two staged-erase flush
   sites. WC-D's bare-`IVAC` discipline untouched.

**Not taken, and why**: cross-window occlusion coalescing (the fleet tiles; the dirty set is
per-window and honest, so covered-pixel double-blits are not in the measured population) and
band-parallel composite (`wait_us` ≈ 4 µs — no serialisation to spread; the surviving cost is
byte throughput on compose+present, which more cores do not multiply and whose sched coupling the
constraints priced as not worth a QEMU-only guess).

### Projection to metal, stated honestly

QEMU measures instruction count, not DRAM/write-combining bandwidth, so the 5.3x pass_us is an
upper-bound shape, not a metal number. What transfers: the eliminated work is real (per-pixel call
overhead ~10 instructions/px -> 2 stores per 8 bytes; ~2x panel bytes written per pass -> ~1x;
3.7x cleaned bytes -> 1x), and on the A72 the compose (cached RAM) was CPU-bound in exactly the
population QEMU models. If the metal pass is bandwidth-bound after the rebuild, the floor is the
box's ~2.1 MB of traffic (compose write + present read/write + clean) against the Pi 4's ~4 GB/s
practical DRAM bandwidth ≈ 0.5–1 ms/pass; if it stays instruction-bound the QEMU ratio applies to
the measured 8 ms ≈ 1.5 ms/pass. Either way the honest projection is **~4–6x the pass rate — from
~123/s aggregate to the 500–800/s band** — with the metal verdict owed to Peter's boot at the next
attended sitting.

### Gate results (COMPOSITE-2, 2026-07-29, QEMU raspi4b)

* `./arroyo check` — both arches green.
* `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test` — MBENCH PASS 87/87, 12 consecutive runs
  (one early 86/87 run during development did not reproduce across those 12 and left no captured
  witness identity; recorded here so a recurrence is read as a flake candidate, not a surprise).
* `./arroyo kernel8-test` (default geometry) — MBENCH PASS 87/87.
* `./arroyo test-arm` — MISSION SUCCESS.

## FLUID-3 — price the wait that motion buys (2026-07-29)

Two live P83 bench observations (Peter, at the panel): **pointer motion increases a fleet core's
idle** ("when I move the mouse it makes more reserve" — one core sits ~47% busy under a six-vug
storm while the other three run 99%, and heavy motion windows deepen the reserve), and **each vug
settles to a characteristic fps below available capacity** ("vug still wants to fall to its
predetermined fps even though it could run faster"). The SCHED load meter counts service time as
busy (`CoreAccount::busy_pct`; only placement uses the EL0-only figure), so the reserve is genuine
idle — fleet tasks leaving the run queues with headroom measurably present.

### What the P83 wire says (`pi4-r23s1r`, six-vug storm stretch)

Correlating `[prio] el0=` per-window (motion proxy: EL0 dispatches spike under pointer events)
against `SCHED: load`, `[wcn]` rates, `[comp2]` and `[spread9]` over the same windows:

* The aggregate present rate is **conserved at ~258 passes/s** across the storm — quiet windows
  (`el0` ≈ 0) and heavy-motion windows (`el0` ≈ 1.1 M) alike, `att_rate` 248–262/s, `aborted=0`.
  One live window alone gets the whole figure (win5 solo: 255/s); six share it.
* The shares are wildly unequal and stable over minutes: win1/2/4 ≈ 19–21/s, win3 ≈ 50/s,
  win5/6 ≈ 71–80/s. Same binary, same class — the settled "predetermined fps" Peter feels.
* One core holds a ~44–48% busy reserve in every storm window; under sustained motion the busy
  cores sag from 99% to ~84% and the reserve core dips as low as 23% busy (motion window
  `el0=589k`: c2=23%) — motion buys REAL idle while `att_rate` holds. Quiet mean total busy
  336/400, motion mean 327/400 across the stretch, with the deep sags exclusively in motion
  windows.
* `[spread9] kick` deltas: ~4/window quiet vs ~13/window motion, both trivial against ~1290
  passes/window — **H2 (preemption storm) refuted**.
* `[comp2] sprite_us=1` per pass throughout motion windows — **H3 (compose-through cost)
  refuted**.

### What the code says (H1, corrected)

`SYS_WIN_PRESENT` → `wm::present` → `composite()` runs the full pass **inline on the presenting
task's own core** and returns when it finishes. There is no present queue, no single consumer, and
no ack rendezvous: the "ack" IS the synchronous return, so (a) a vug's next frame is gated on its
own composite completing — ~2.5 ms of service time per frame at fleet damage sizes — and nothing
else. (b) The `[sched6]` "dirty-paced strip@250ms" cadence belongs to the shell's `render_service`
strip repaint only; nothing batches or quantizes per-window presents. (c) The symmetry break
between a 19 fps and an 80 fps window is therefore NOT in the present path (which is symmetric and
first-come-first-served): the remaining pace-setter is where a live vug is allowed to leave the run
queue at all — its futex parks (frame barrier `DONE` behind its two workers, workers on `PHASE`),
crossed with placement packing. A parent whose workers sit deep in a saturated core's run queue
parks for milliseconds per frame; that park is the reserve the load meter shows, and its duration —
not any fps target (the vug has none) — is the settled rate.

### The instrument: `[fluid3]` (aarch64 + witness; `video/wm.rs` + `arch/aarch64/sched.rs`)

One line per `[wcn]`/`[comp2]` window:

    [fluid3] parks=N park_us mean= max= p50<= p90<= p99<= depth_max= overlap= span=..ms

* `parks` / `park_us` / percentiles — completed futex parks and their duration distribution
  (log2-µs buckets, park-to-first-dispatch, stamped around `switch_context` in `futex_wait`).
  Percentile figures are bucket upper bounds. Expected modes: tens-to-hundreds of µs = barrier
  behind healthy workers; **milliseconds = a parent starved behind a packed worker — the P83
  mechanism if confirmed**; >32 ms (top bucket) = idle vugs parked on input rings.
* `depth_max` / `overlap` — high-water of concurrently in-flight composites (`BLIT_ACTIVE` at
  guard registration) and passes that entered with another in flight. `depth_max>1` under storm
  proves presents overlap rather than serialize behind one consumer.

### The two fix directions (priced, not chosen — the frame-pacing contract is Peter's)

* **Async present** — enqueue damage and return; a dedicated service performs passes. Decouples the
  vug's frame loop from the pass cost entirely (each vug pays raster only; the ~2.5 ms moves to a
  service core). Cost: breaks the WC-G/tearing invariant that the owner is parked inside
  `SYS_WIN_PRESENT` while its surface is read — the one moment the surface is provably quiescent —
  so it needs double-buffered surfaces or a copy, and it changes the barrier semantics VUG-PACE
  deliberately preserved.
* **Per-window fair compositor service** — keep the synchronous shape but make pass inclusion fair
  across windows (e.g. round-robin damage service) so no window's share collapses to 19/s while a
  sibling holds 80/s. Cost: adds a scheduler-like policy to the compositor; does not return the
  parked milliseconds (the barrier park remains), only redistributes them.

If `[fluid3]` shows the millisecond mode riding motion windows on the parents of the slow windows,
the shares are a placement/park story and the second option is the wrong lever entirely — the fix
is worker co-placement (keep a vug's workers adjacent to its parent), which is scheduler-side and
in reach without touching the present contract.

### Gate results (FLUID-3, 2026-07-29, QEMU raspi4b)

* `./arroyo check` — both arches green.
* `./arroyo kernel8-test` — MBENCH PASS 88/88.
* `./arroyo test-arm` — MISSION SUCCESS.

## WEDGE-9 — `cursor::SPRITE` becomes a claim/loan; F4 is closed (2026-08-02)

The last unfixed member of the F1–F4 family, and the worst user-visible one. §WEDGE-2 built the
`<D4>` token to stop this death being misattributed to F1's `TABLE`, audited the nine `SPRITE`
acquisitions, and stated plainly that WEDGE-7's masked micro-guard could not be applied here. This is
the fix that audit called for: WEDGE-8's claim/loan (`drivers/xhci::claim`), by way of MBOX-1's
transposition of it in `arch/aarch64/mailbox.rs`.

### The family shape, and why F4 is the expensive one

A masked span blocks on a lock a **preemptible** holder holds; the holder is preempted; the masked
spinner's core can take no timer interrupt, so the holder never runs again and the core spins
forever. No ABBA cycle is involved and none is needed.

`SPRITE` was that lock. Every one of its nine acquisitions held it across the whole operation, and
seven of those were unbounded — two ≤`MAX_PIX` framebuffer read/write passes against non-coherent
scan-out, a `flush_box` that cleans **whole panel scanlines** (~276 KB, ~4300 cache lines on the
bench's 1920×1200), and in `adopt_overlay` a nested blocking `OVERLAY.lock()`.

### The acquirer side, which §WEDGE-2's audit did not enumerate

This is what makes F4 the family's worst case rather than merely another instance. Three chains reach
`video/cursor.rs` with interrupts already masked:

| Chain | Where the mask comes from | Reaches |
| --- | --- | --- |
| EL0 task exit | `sched::exit` → `mask_irq()` → `boot::teardown_user_slot` → `syscall::clear_handle_row` → `wm::close_owner` | `undraw` (via `erase`), `repaint` (the `<D4>` site), then a whole `composite()` pass — i.e. every entry point in the module |
| `SYS_WIN_PRESENT` | `syscall::sys_win_present` → `IrqGuard::mask_save()` + `WINDOWS` → `present_surface_common` → `wm::present` → `composite()` | the same whole-pass set — **one masked pass per window present, several windows, several cores** |
| `SYS_FB_PRESENT` | `syscall::sys_fb_present`, same guard, → `wm::compat_present` → `composite()` | the same |

The scheduler's off-CPU reap arm (`retire_killed` → `teardown_user_slot`) is a fourth entrance to the
first chain. Unmasked acquirers — the render task's `Screen::flush` bracket, the HID router's motion
repaint, `wm::move_to`/`wm::close` (both deliberately outside their verb's `IrqGuard`), and
`wc_close_click` from the input pump — are preemptible and were never the hazard.

So the symptom of an F4 death is not a stalled teardown. The sprite gates the cursor bracket every
compositor path takes, so panel, cursor and input stop together, with nothing on the wire.

### The shape: the discipline goes on the LOCK, not the WORK

* `SPRITE_STATE` is a private `static mut`, reachable only through `SpriteLoan`'s `Deref`/`DerefMut`.
* `SPRITE_FREE: Mutex<bool>` is the only lock, held for a masked O(1) take/put and nothing else. Mask
  before the acquisition, guard released before the mask is restored (WEDGE-7's field order, as local
  drop order).
* The long work runs on the **loan**, with no lock held. A contender's `claim()` takes `SPRITE_FREE`
  for a few dozen cycles, reads `false`, and answers `Busy` — it never waits on a preemptible holder.
* Grep-checkable, the F1/WEDGE-8 idiom: `SPRITE_FREE.lock()` appears only in `claim` and
  `SpriteLoan::drop`; `SPRITE_STATE` is named only by the two loan accessors.

### Per-caller Busy policy

`Busy` is never a shrug. Eight of the ten sites route a refusal into `owe_repaint()`, which arms a
whole-sprite repaint for the next composite tail and arms CURSOR-9's `TOUCHED_SINCE_DRAW` alongside.

| Site | Policy | Why |
| --- | --- | --- |
| `repaint` (**the F4 site**) | bounded unmasked retry (`CLAIM_RETRY_MS` = 2 ms), else owe | The pointer's own motion path; a lost one is a stale cursor position. Masked callers skip the retry entirely (`arch::irqs_masked`, the WEDGE-8 rule) — that refusal *is* the fix |
| `undraw`, `undraw_within`, `undraw_within_nosession` | owe | The caller is about to paint over pixels it could not hand back. The owed refresh re-establishes arrow and save-under from the finished front |
| `defer_within` / `defer_nosession` | return 0, owe | An unrecorded deferral leaves the tail's settle nothing to verdict and `saved` stale. The 0 return is safe: neither caller reads it, and the session arm still reports `Adopt`, so no session leaks |
| `settle_nosession` | owe | The `pend` bits stand; the owed refresh is settlement-by-bracket in its strongest form |
| `ensure_drawn` | owe | Its whole job is "is the arrow up?"; a refusal means it could not check |
| `adopt_overlay` | **close the session anyway**, then owe | It is the ONLY closer of the overlay session; a leaked session locks the mechanism out for the boot. Closed with no loan held — not an order inversion but its degenerate case |
| `sprite_plan` | `None` + `TOUCHED_SINCE_DRAW`, **no owe** | The composite bracket decision, once per pass on every core. `None` is the arm the pass already takes when the sprite is down, and every window blit's `note_present_over_sprite` (lock-free) arms the repair anyway. Owing here would rebuild the P69 loop |
| `sprite_box` | answer from `live_box_relaxed()` | Split from `sprite_plan` for this: "could this operation reach the sprite?" is answerable from the CURSOR-6 mirror with no lock, and answering `None` would be the one degradation its contract rules out — a MISSED bracket. `wm::drain_deferred` is the caller that makes it matter |

### The handoff, and its bound

`owe_repaint()` sets `REPAINT_OWED`; `take_present_dirty()` — already called by `wm::composite` before
the tail is chosen — is the consumer, and all four tails end in a `repaint` when it answers `true`.
The owed request is **exempt from CURSOR-8's `stale` test** (a context that could not take the sprite
never observed an epoch worth comparing, and the generation has almost certainly moved *because* the
holder was mid-cycle) but **subject to its `REPAIR_MIN_MS` rate floor**, re-armed rather than dropped.
That is deliberate in both directions: without the exemption the repaints this arc exists to preserve
would be suppressed; without the floor, refusals arriving from tails that run on every pass would
reinstate CURSOR-7's ungated storm.

Latency is one composite pass plus at most one 8 ms floor. On the path that matters most it is
microseconds — `wm::close_owner` calls `composite()` on the line after its refused `repaint()`.

**The honest bound.** `wm::repaint`/`wm::service_damage` both early-return when nothing is damaged, so
on a panel with no windows and no damage the owed repaint waits for the next pointer report's own
`repaint`. That is not silence: on such a panel nothing is painting over the arrow either.

### `adopt_overlay`'s nested `OVERLAY.lock()`

The audit's first disqualifier, and it is answered by construction rather than by restructuring.
Holding the **loan** across a blocking `OVERLAY.lock()` is sound precisely because the loan is not a
spinnable lock: `SPRITE_FREE` was released the instant `claim` returned, so nothing can be waiting on
this context. The documented `SPRITE` → `OVERLAY` order survives in the only sense that still applies
— this is the one place that holds the sprite while taking the overlay, and nothing takes them the
other way round.

`OVERLAY` itself is unchanged and is *not* claimed to be safe by this arc. Its blocking acquirers are
`overlay_open` (O(1) field writes) and `adopt_overlay` (a bounded ≤`MAX_PIX` index walk, no I/O, no
lock taken inside the hold); every other site `try_lock`s. Both holds satisfy WEDGE-7's precondition,
and WEDGE-9 strictly improves the picture by removing the spinnable lock the nested one used to sit
inside. **Flagged, not done: giving `OVERLAY` the masked micro-guard is a separate arc.**

### Witness

```
[wedge9] sprite-claim scope=… refused=N masked=N retried=N owed=0|1 serviced=N -> QUIET|ABSORBED|DEFERRED|LOST
```

Chained off `[cursor11]`'s rollup. `masked` is the F4 population: each of those was, before this arc,
an unpreemptible spin on a preemptible holder. **A non-zero `masked` is the mechanism being caught,
not a fault.** `serviced` is expected far below `refused` — the flag coalesces and the floor defers;
`LOST` (refusals with nothing ever cashed and nothing pending) is the only reading that is a defect.

### Gate results (WEDGE-9, 2026-08-02, QEMU raspi4b)

* `./arroyo check` — both arches green.
* Full-knob `kernel8` (`witness,wedge2,vugpar,smp8,usbdebug,sched_demo,bootlog`) — builds.
* `./arroyo kernel8-test 210` — **MBENCH PASS 90/90 required witnesses, 0 forbidden, 28824 lines.**
  Cursor fixtures unchanged and green: `[wc-i] rollup … cursor_passes=350 cursor_brackets=0 -> CLEAN`,
  `[wc-i] reopen … -> PASS`, `[wc-g] rollup win=1/2/3 … -> CLEAN`, `[cursor6] … -> UNWITNESSED`.
  `[wedge9] sprite-claim scope=fixture refused=0 masked=0 retried=0 owed=0 serviced=0 -> QUIET`.
* `UNAOS_WEDGE2=1 ./arroyo kernel8-test 210` — **MBENCH PASS 90/90, 0 forbidden, 31356 lines.**
  `<D4>`=2 against exactly two `[wc-a] close_owner … closed=1` lines, i.e. the token still fires on
  every teardown that frees a row; `<D2>`=32, `<D3>`=32 (three of them split by another core's line,
  WEDGE-2's stated cost), `<D1>`=0, `<D!>`=0. Both `<D4>`s are followed by further output — no wedge.

**What the gate cannot prove, for the same reason WEDGE-7's could not.** `timer_preempt` never runs on
raspi4b, so no holder can be preempted and F4 cannot occur there. A clean QEMU run proves the wiring
and the absence of regression; it is not a refutation of the mechanism, and `[wedge9] -> QUIET` on the
gate means the population was never sampled rather than that contention is absent on metal.

### Metal watch-list (WEDGE-9)

* `[wedge9] masked=` climbing on a bench boot: the F4 interleave is real and is now being absorbed.
  Read it beside `[cursor8] suppressed_rate` — if both climb together the floor is doing its job.
* `[wedge9] -> LOST`: refusals with no grant and none pending. Means composite tails have stopped
  running, which is a different fault; check `[wc-i] windowed_flushes` and `[comp2] passes`.
* `<D4>` as the LAST token on a torn wire no longer implicates the sprite lock's *wait*. It still
  names `close_owner`'s cursor bracket as the phase, but a death there is now downstream of the claim
  — look at `refresh_locked`'s framebuffer work and at `WRITER`, not at a masked spin on `SPRITE`.

## FBCON-DMG — the console window presents the rows that changed (x86 re-land M4, 2026-08-03)

The console-as-a-window (`win=1`, 1314x750 on the rMBP bench) cost **3.9 MB and ~24 ms of present per
printed line** — 1.5 frame budgets — with `[wc-h] torn=yes` on 3 of 4 samples. The console knew which
rows it had dirtied the whole time; the information was thrown away twice before it could reach the
compositor, and `wm` had nowhere to put it if it had arrived:

* `FbCon::flush_dirty` reset the band and returned nothing on the routed path;
* `PanelSink::flush` took the band, ignored it, and presented the whole box — once per 16-byte
  `CHUNK`, so an 80-column line paid ~5 full-box presents;
* a window row carried `damaged: bool` and nothing else, and `draw_window` recomputed `outer_box`
  from scratch on every pass.

### The band, and the invariant that makes every old path inert

`Window` carries `dmg_y0`/`dmg_y1` — a **SOURCE-ROW** band, the coordinate the *owner* of the surface
knows — beside `damaged`. **An EMPTY band (`dmg_y1 <= dmg_y0`) means THE WHOLE OUTER BOX.**
`Window::empty` leaves one, and `Window::damage_all` clears the band on every call. Every damage path
that predates this arc — `compat_present`, `present`, `move_to`, both `focus_changed` arms, the raise,
`repaint`, `damage_intersecting`, `create_inner`, the tiler in `place` — goes through `damage_all`, so
they all declare exactly what they declared before and unbanded presents are byte-for-byte unchanged.

Source rows and not panel rows: the console tracks a dirty band in its own glyph grid and has no
business re-deriving the compositor's placement. `damaged_box(r, band)` does the conversion once,
through the same `r.y`/`r.scale` the content blit uses, and intersects with `outer_box`, so a band can
neither disagree with the pixels it describes nor extend a window's damage past its own chrome.

### The two present verbs

`present(id)` and `present_rows(id, sy0, sy1)` both delegate to one private `present_banded(id, band)`.
There is no second present path to keep in step: the pass, the occlusion closure, the staged-present
discipline, the cursor bracket, VUGMIN-B's hidden-owner suppression, WC-N's accounting and every
witness are the same code on the same path. A band the row does not contain (`y1 > r.h`) is **not**
narrowed to the part that fits — that means the caller and the row disagree about the geometry, and
the only answer that cannot leave a stale pixel is the whole box.

### Band propagation through `composite_inner`

The band is snapshotted **in the same critical section as the `dirty[i]` snapshot** and cleared with
it, so a `present_rows` landing after the snapshot re-damages the row rather than having its rows
absorbed by a pass that is no longer going to draw them. Nothing else in the pass moves — in
particular **WC-L's deferred-erase drain still runs BEFORE the snapshot and outside the F4 `BlitGuard`
window**, which is what its placement argument (and CURSOR-5's P64 fix) depends on.

The occlusion closure then reads the *damaged region* of `i` — its band-clipped box — rather than its
whole box, and **promotes every window it drags in to a whole-box repaint**:

```rust
let bi = damaged_box(&rows[i], bands[i]);
for j in 0..MAX_WINDOWS {
    if !rows[j].used || rows[j].z <= rows[i].z { continue; }
    if dirty[j] && bands[j].is_none() { continue; }   // already whole-box: nothing to widen
    if boxes_overlap(bi, outer_box(&rows[j])) { dirty[j] = true; bands[j] = None; grew = true; }
}
```

Both halves are the conservative direction: a narrower `bi` can only reach *fewer* windows, and every
window it does reach repaints at least the rows `i` is about to overwrite. A `j` that was itself banded
must re-enter the fixed point (`bands[j].is_some()`), or a banded window could stay banded while a
lower window repainted rows outside that band. Termination is unchanged: each `j` moves at most once
into the `dirty && band.is_none()` state, which is the state the `continue` skips.

### `draw_window` and `stage_window` — the band is WC-M's band

`draw_window` converts the source band to box-relative rows (`damaged_box`, minus `by`) and hands
`dy0`/`dy1` to `stage_window`. The box geometry passed to `paint_window` is still the WHOLE box, so
the chrome keeps its true position across the seam exactly as a WC-M-banded present does — this reuses
that machinery rather than adding a second kind of band. `stage_window` then:

* clamps the range to the box (an out-of-box band gets the whole box — over-painting is free of
  consequence, under-painting is not) and derives `span = dy1 - dy0`;
* sizes `chunk_rows` off `span`, not `bh`, so a banded present of a box that would have needed several
  WC-M bands takes **one**;
* starts its banding loop at `dy0` and stops at `dy1`. `dy0 == 0 && dy1 == bh` is the pre-FBCON-DMG
  loop verbatim;
* keeps CURSOR-3's offer contract untouched — `chunk_rows >= span` is the same "single band" test
  restated against the rows this present owes.

Two extents follow whichever path actually ran, not the band: the cache clean at `draw_window`'s tail
is derived from `staged` (the **direct fallback ignores the band and paints the whole box**, which can
only over-paint), and `[wc-h]`'s `bytes=` reports `row_bytes * span` — what this present put on the
glass — so a damage-limited console line reports its true cost instead of the cost of the box it lives
in. (On x86 that last line is currently mute; see the ⚠ note under the gate results.)

### The producer side, and the ledger that makes a declined present safe

`fbcon` accumulates the band across a line and presents **ONCE**, at `PanelSink::finish`, instead of
once per 16-byte `CHUNK`. A band that cannot be presented — the `ROUTE_BUSY` re-entry guard, the
INSTGUI suspension, a `wm` that reports the row gone — is merged into the `PEND` ledger **before** any
decline is tested, rather than dropped, so no glyph can be left stale by a declined present. Every
degradation in that ledger repaints MORE, never less: `PEND` is always `try_lock`ed (this is reached
from print context with interrupts enabled), and contention is answered with `PEND_FULL`, i.e. a
whole-box present — exactly the behaviour this arc replaces, for one line.

**Scroll.** The console wraps rather than scrolls, so only the wrap line unions the last cell row with
the first and costs a full box — one line in `rows` (23 on the bench console), and the bounded worst
case.

### x86-scoped by construction — and this is checkable, not asserted

The band is produced by exactly one call site, `fbcon::route_present_banded`, which is
`#[cfg(all(target_arch = "x86_64", feature = "wc"))]`; it is the **only** caller of `wm::present_rows`
in the tree, and `present_rows` is the only caller of `Window::damage_rows`. `FbCon::flush_dirty`
returns `None` on every path that survives `cfg` on aarch64. So on aarch64 `dmg_y1 > dmg_y0` is
unreachable, `bands[i]` is always `None`, `damaged_box` returns `outer_box`, `draw_window` passes
`(0, bh)`, and `span == bh` — every aarch64 present is byte-for-byte the pre-arc present, including
`[wc-h]`'s and COMPOSITE-2's numbers.

### Gate results (FBCON-DMG, 2026-08-03)

* `./arroyo check` — both arches green. `⚡ kernel features: ehcihid,kbdwit,smolnet`;
  x86_64 16 warnings, aarch64 14 warnings.
* `UNAOS_WITNESS=1 UNAOS_WC=1 UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1
  UNAOS_SMC=1 ./arroyo check` — both arches green.
  `⚡ kernel features: witness,ehcihid,kbdwit,smc,smolnet,nvidia-kepler,nvidia-kepler-takeover,nvidia-kepler-fifo,wc,intel-ivb,unaos_ivb`;
  x86_64 36 warnings, aarch64 14 warnings.
* **Warning-set delta: zero on both arches under both gates** — the sorted warning lines are
  byte-identical to the pre-change baseline. That also proves the two FBCON-DMG-substrate
  `#[expect(dead_code)]` markers came off correctly: `damage_rows` and `damaged_box` both gained
  callers, and neither an unfulfilled-expectation nor a dead-code warning appears.
* **No QEMU verdict, and none is possible.** `wcx::activate()` — the compositor's only x86 ignition —
  has exactly one caller, `drivers/gpu/kepler_display.rs`, inside the Kepler takeover, for which QEMU
  has no part. A QEMU run of this path would be vacuous. **Metal is the only verdict.**

### ⚠ `[wc-h]` cannot fire on x86 in this tree — the arc's own evidence line is missing

The 3.9 MB / ~24 ms / `torn=yes` numbers that motivate this arc came from `[wc-h] win=1`, and on the
merged trunk **that instrument is unreachable on x86**. The R23 merge took the Pi's compositor as the
`wm.rs` baseline, which arch-gated every WC-H producer: `wcg::stage_note`, `wcg::stage_flush` and the
`decline!` macro's `wcg::stage_decline` are all `#[cfg(all(target_arch = "aarch64", feature =
"witness"))]` here, where the pre-merge x86 tip had them at plain `#[cfg(feature = "witness")]`. The
`stage_note(…, span, row_bytes * span, …)` change this arc makes is therefore **correct and, on x86,
currently mute**.

Re-widening those gates is a separate change to `video/wcg.rs`'s call sites and was deliberately not
taken here — it is instrument scope, not FBCON-DMG scope, and it belongs with whoever re-lands the
rest of the x86 witness surface. Until then the x86 bench cannot read the arc's cost directly; the
lines below are what it CAN read.

### Metal watch-list (FBCON-DMG)

* `[wc-x] console-route first-paint win=N (glyphs -> window surface, damage-limited)` — the routed
  first paint. **This line is NOT a discriminator for this arc.** It came back with the merge's
  producer side and its `, damage-limited` wording was already on the wire while the consumer was
  presenting whole boxes; it says the route exists, nothing about the band. Treat it as a
  prerequisite, not as evidence.
* **There is no x86 wire line that distinguishes a banded present from a whole-box one.** That is a
  gap this arc inherits rather than creates (see the ⚠ note), and it should be closed by re-widening
  the WC-H gates before anyone reads a bench boot as confirming or refuting FBCON-DMG on the wire.
* **Wall-clock, since `[wc-h]` is mute here**: the boot-log print rate through the routed console.
  A line that cost ~24 ms of present should now cost a band of a few rows; the arc lands as a visible
  drop in the time the console spends painting, not as a witness line.
* `[wc-a] composite windows=N drawn=M` and the WC-N per-window `comp=` accounting are `#[cfg(feature =
  "witness")]` and DO reach x86 — they say a pass ran and which ids it drew, not how many rows.
* A console line that leaves a stale glyph is the ledger failing, not the band: read `PEND_FULL`'s
  effect (a whole-box present) as the fallback that should have covered it.
* On aarch64 the whole watch-list is "nothing moved" — `[wc-h] box=`/`bytes=` must be byte-identical
  to the pre-arc boot, since no band is ever produced there.
