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
Pulse homage — a full-screen monitor view (a host-native `apps/pulse` vessel is
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

## 5. Serial evidence

`run_crystal` emits, when invoked:

```
:: VUG: crystal live — 24 faces, solid/wire, exit clean ::
:: VUG: crystal exit clean — N frames ::
```

Headless regression gates never type `vug`, so these are GUI/panel-verified when
the demo is run (attended); the demo does not perturb headless boots.
