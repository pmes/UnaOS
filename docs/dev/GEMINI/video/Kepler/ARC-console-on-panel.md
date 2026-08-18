# ARC — CONSOLE-ON-PANEL

Goal: make the kernel console (`video::fbcon`) render text on the 2012 rMBP panel, which scans
out the GOP framebuffer, after the Kepler display takeover has finished with the surface.

## What the console path actually was (root cause)

fbcon's geometry was never the problem. `fbcon::current_info()` already reports stride 4096 px /
16384 B rows against the scanned GOP surface at phys `0x90020000`, confirmed matching hardware in
metal sittings #29–#32. No stride fix was needed and none was made.

The console simply never drew. Two independent gates, both part of the QUIET-PANEL policy:

1. `fbcon::_print` — the mirror leg fed by every `serial_println!` — begins with

   ```rust
   #[cfg(all(target_arch = "x86_64", not(feature = "bootlog")))]
   if !PANIC_MIRROR.load(Ordering::Relaxed) { return; }
   ```

   On any x86 build without the `bootlog` feature it returns before touching a pixel. Only a panic
   (which sets `PANIC_MIRROR`) re-armed it.

2. `fbcon::milestone` — the remaining on-panel leg, driven by `bootlog::record` — is
   `#[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "witness"))))]`.
   It compiles to a no-op stub on `usbdebug` builds.

The metal Kepler media is built `UNAOS_USBDEBUG=1 UNAOS_KEPLER=1 ...` (see `unaos/arroyo` lines
60/62/131–133), i.e. `usbdebug` on and `bootlog` off. Both legs are therefore dead: after
`fbcon::init`'s one-time black `fill_screen`, fbcon painted **zero** pixels for the entire boot.
There was no console text on the panel because there was no console text anywhere but serial.

Secondary factor, confirmed independently by metal sitting #30: the raw font8x8 cell is 8x8 px,
which on a 2880 px / ~286 mm panel is ~0.8 mm. Even with drawing enabled, that is not legible at
bench distance — the pull-20 `fbcon-probe` 8 px blocks were reported invisible.

## What was wired

`unaos/crates/kernel/src/video/fbcon.rs`

- New `PANEL_CONSOLE: AtomicBool` (x86 only) — a third override alongside `PANIC_MIRROR`. When
  set, `_print` mirrors to the panel on quiet builds. Default false; nothing else sets it.
- `FbCon` gained `cell_w` / `cell_h` / `scale`. All drawing (`glyph`, `newline`, `write_byte`) now
  uses the live cell instead of the `CELL_W`/`CELL_H` constants. `scale == 1` is the previous
  behaviour, instruction-for-instruction equivalent (one poke per set font bit); a higher scale
  paints a `scale`x`scale` block per bit, so no new font asset is involved.
- New `pub fn panel_console_resume() -> usize` (x86 only). It drops the cached-RAM shadow if one
  is attached (so no later full-surface blit can resurrect the calibration pattern), sets the cell
  to `PANEL_SCALE` (6 → 48x48 px, ~4.8 mm, 0.6 mm stroke, 60 cols x 37 rows on this panel),
  recomputes the grid, clears the whole real surface, homes the cursor, flips `PANEL_CONSOLE`, and
  replays the `bootlog` milestone ring. Returns rows painted (`1 + ring length`).

`unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`

- One seam, immediately **after** `:: kdisp: fb-draw done ::` at the end of `takeover_display`:
  calls `panel_console_resume()` and prints the repaint marker. The calibration draw, the 5 s
  hold, the register dumps, the head-stat scan and the pull-20 probe above it are untouched.

Gating: the only caller is inside `takeover_display`, which is `nvidia-kepler-takeover`. Non-kepler
boots never reach it, `PANEL_CONSOLE` stays false, and their behaviour is unchanged.

Not touched: `kepler.rs` (fence lane), the calibration draw/hold/probe sequence, the EVO latch.

## New serial markers

Emitted in this order at the tail of the takeover:

```
:: kdisp: fb-draw done ::
:: fbcon: glyphs-active base=<hex> pitch=<bytes> cell=<W>x<H> cols=<N> rows=<N> scale=6 ::
:: [<ms> ms] <milestone tag> ::            (one per bootlog ring entry, replayed)
:: kdisp: console-repaint rows=<N> ::
```

Failure form (console not ready / lock contended): `:: fbcon: glyphs-active ABORT console-not-ready ::`
followed by `:: kdisp: console-repaint rows=0 ::`.

Note that `glyphs-active` is printed *after* the flag is set, so it is itself the first line drawn
through the new path — seeing that exact text on glass is the proof.

## What metal must verify

1. `:: fbcon: glyphs-active ... ::` appears on serial, and `base`/`pitch` read `90020000` / `16384`
   (i.e. fbcon's handle is the scanned GOP surface). A mismatch here, not a blank panel, is the
   thing to report.
2. `:: kdisp: console-repaint rows=N ::` with N > 1.
3. On the panel: the calibration pattern is gone, replaced by black, and large grey text —
   starting with the `glyphs-active` line itself, then the `[ms] tag` milestone replay — is
   readable at bench distance. Photograph it.
4. Whether subsequent kernel serial output continues to appear on the panel (it should — the
   mirror stays on) and whether it wraps at the bottom without corrupting the surface.
5. If text is present but still too small or too faint, the single knob is `PANEL_SCALE` in
   `fbcon.rs`; report the observed apparent size so it can be set from evidence rather than from
   the ~0.1 mm/px arithmetic.

## Gate

- `UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo check` — x86_64
  OK, aarch64 OK.
- `./arroyo test` — complete, 0 failures in `target/serial.log`.
- `./arroyo test-arm` — complete, 0 failures in `target/serial-arm.log`.

QEMU-green is not evidence for this arc: the takeover path does not run under QEMU. The gate proves
only that nothing else regressed.
