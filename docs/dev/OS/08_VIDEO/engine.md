# The graphics engine (Vug) — primitive ledger

`vug` (`unaos/crates/kernel/src/vug.rs`) is UnaOS's software 3D demo **and** the
graphics engine's testbed. Canon (`docs/CODEX.md` §5): Vug is the sculptor, the
future 3D CAD app; a *vug* is a crystal-lined cavity in rock. The demo shows a
real-time, software-rendered rotating quartz crystal, and every new engine
capability lands here with a visible artifact.

This file is the **running ledger of engine primitives** — what the renderer can
do, where each primitive lives, and its invariants. It grows as the engine does.

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

## 4. Instrumentation — the corner load meters (M3b)

Two small meters, kept unobtrusive (the crystal stays the star), both drawn
through the damage-tracked back buffer (one present per frame still holds):

- **RENDER meter** — the honest software "GPU monitor" (there is no GPU; we render
  in software). Per frame it clocks the render+present span (`arch::now_cycles`)
  against the whole-frame span → a **busy %** bar, and times a ~200 ms window with
  `arch::ms()` → frame time / FPS; it also shows drawn triangles and an estimated
  filled-pixel count (sum of front-face screen areas). **Seam:** a real GPU
  utilization feed would replace `now_cycles`-derived busy % and the pixel
  estimate.
- **CPU pulse meter** (BeOS-Pulse style) — per-core busy fraction over the same
  window. Source: additive, relaxed, lock-free per-CPU counters bumped at the
  dispatch point (`CPU_BUSY`) and the idle point (`CPU_IDLE`) of **both**
  schedulers (`arch/x86_64/sched.rs`, `arch/aarch64/sched.rs`), read via
  `sched::meter_cpu_count()` / `sched::meter_cpu_ticks(cpu)`. These counters are
  **introspection only** — never read on any scheduling path. **Seam:** a real
  per-core utilization / PMU feed would replace `meter_cpu_ticks`.
  - *Caveat:* on x86 the GUI/`vug` context runs in the BSP inline loop (outside
    `sched::run()`), so the demo core's own load shows in the RENDER meter while
    the CPU-pulse bars reflect the scheduler-managed cores. On the Orin the pump
    is a scheduled task, so its core lights up in the CPU-pulse meter directly.

## 5. Serial evidence

`run_crystal` emits, when invoked:

```
:: VUG: crystal live — 24 faces, solid/wire, exit clean ::
:: VUG: crystal exit clean — N frames ::
```

Headless regression gates never type `vug`, so these are GUI/panel-verified when
the demo is run (attended); the demo does not perturb headless boots.
