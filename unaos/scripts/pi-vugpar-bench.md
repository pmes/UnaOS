# pi-vugpar-bench.md — VUG-PAR attended Pi 4 sitting (band-parallel flush)

The **positive** verification of band-parallel `Screen::flush` — the deferred step 2 of VUG-FPS.
`flush` splits the damaged row-copy blit (and large clears) into disjoint scanline **bands** and
dispatches all-but-one to free secondary cores via `sched::spawn_joinable`, joining before it
returns. QEMU `raspi4b` boots it green (deterministic — the join is a correctness barrier), but the
real fps win over the VUG-FPS dirty-rect baseline is read on **metal**. This card is for an
**attended** sitting (LC drives, Peter physical). It is **not** part of the QEMU DONE gate — that
gate is zero-regression + clean boot only.

## What it does

- `flush` reads the currently SCHEDULED secondary cores (`sched::core_load(cpu).tracked`, minus this
  core) to pick a band count, partitions the damaged y-extent into that many disjoint contiguous
  bands, runs band 0 inline on the render core, spawns the rest to helper APs, and joins.
- Falls back to the single-core serial flush when no free AP is scheduled or the damage is under
  `PAR_MIN_ROWS` (64) scanlines — that fallback path is byte-identical to the VUG-FPS serial flush.
- Feature **OFF** (`vugpar` absent) => the whole parallel machinery + its `sched` dependency vanish;
  `flush` is byte-identical to the serial path.

## Build & stage

```
# From the hw-pi4 worktree's unaos/ dir. Unmount UNAOS before kernel8 builds.
UNAOS_VUGPAR=1 UNAOS_V3D=1 UNAOS_PIUSB=1 UNAOS_PI=1 ./arroyo kernel8
```

Then stage the flashable image to `~/unaos-bench/flash/pi4/` (stamp + sha256 + MANIFEST) — never
flash a `target/` path directly (unaos-flash-staging rule). Bench card = the **32 GB** card.

## Witness — the `[vugfps]` line

The vug render loop prints `~1x/s`:

```
:: [vugfps] <fps> fps  <N> bytes/frame flushed  bands=<K> (<frames> / <ms> ms) ::
```

- `bands=1` — serial / fallback (no free AP, or feature off). This is what a default (non-vugpar)
  boot prints, and what QEMU prints when only the render core is scheduled at flush time.
- `bands=2` (or more) — the parallel path ran: the render core plus that many helper APs each blitted
  a disjoint scanline band. On metal the Pi 4 has at least core 2 free, so expect `bands=2`.

## Read the win

Compare the fps against the VUG-FPS dirty-rect baseline (metal P38: 8–9 fps full-flush; dirty-rect
alone 2–3x). With `bands>=2` the panel-height rotating crystal's row copies fan across cores, so the
bandwidth-bound flush should climb further. `bytes/frame flushed` is banding-independent (the same
total is copied, just split), so the delta shows purely in **fps** at equal `bytes/frame`.
