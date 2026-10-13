# MOTIONKIT (rmbp-ledger B648) — the glass's motion primitive, and Settings' three motions

Branch `exec-rmbp-motionkit`, cut from `e57d1b0e`. Design written before the code (R106); one compile (R107); the
QEMU and bench proof is the seat's at the fold.

## The spec

DESIGN-LANGUAGE.md §2.6 (docs/dev/OS/05_USER_EXPERIENCE/DESIGN-LANGUAGE.md:322-351):

* durations by role — `motion.fast` 100, `motion.base` 160 (toggle, checkbox, segmented thumb, tab switch),
  `motion.enter` 240, `motion.exit` 160;
* easings as cubic Béziers — `ease.standard` (0.2, 0, 0, 1), `ease.decelerate` (0, 0, 0, 1),
  `ease.accelerate` (0.3, 0, 1, 1); "all curves are evaluated from a 64-entry Q16 lookup table (integer only)";
* the no-motion fallback — "Settings → Accessibility → Reduce motion": toggles change in one frame, a spatial
  slide becomes a crossfade or a cut. The pane is named; no store key is, and `prefs::key` (prefs.rs:75-104)
  has no accessibility namespace.

The clock: ui_guidelines.md §1 (the 16 ms frame) and the desktop service pass, which already calls
`settings::hover_poll` every pass (quarry/live.rs:2902).

## The design

**`draw::motion` — video/motion.rs**, `CHARTER: Kernel — wm`, declared `#[path = "motion.rs"] pub mod motion;` from
draw.rs's TAIL. draw.rs is under the desktop gate (video/mod.rs:1257: `wc` on x86, `desktop_firmware` on
aarch64), so the new module rides that gate; declaring it from draw.rs rather than video/mod.rs keeps the module
root — the file every image lexes, where a new line moves every `panic::Location` below it (LEDGER P7) — untouched.

* `Curve::{Standard, Decelerate, Accelerate}` — each a `static [u32; 65]` built at compile time: `build()` is a
  const fn that, for x = i/64, bisects s in Q16 (twenty halvings, one unit of resolution) until the Bézier's
  x(s) meets x, then reads y(s); `bez()` is the cubic in i64 Q16 with the products shifted as they go. `at(t)`
  reads the entry at `t >> 10` and interpolates linearly to the next. No float (draw.rs has none: `grep -c
  "f32\|f64"` = 0).
* `Tween { start, end, t0, dur, curve }` with `progress(now)`, `value(now)`, `done(now)`; `lerp(a, b, t)`.
* `Glides` — sixteen keyed slots (no heap: draw.rs's paint-path contract). `to(key, target, now, dur, curve)`
  returns the value to draw THIS frame: a key's first sight records the target with no motion (a window's first
  paint never animates); a moved target starts a tween from the value drawn now (so a flip mid-slide reverses from
  where the knob is); a finished tween is cleared on the paint that drew its end. `running()` says whether any
  slot still carries one.
* `Frame` — `due(now, running)`: true at most once per `FRAME_MS` (16) and only while running; `count` is the
  repaints it granted.
* `reduced()` over the documented constant `REDUCE_MOTION = false`. The Accessibility pane's arc adds
  `accessibility.reduce_motion` to the schema and swaps that one body for a `prefs_client::sys_flag` read. Under
  reduce, every glide is a cut (toggles: one frame, as the spec asks; the pill and the thumb: the cut the spec
  allows — the crossfade is owed with the key).

**draw.rs**: `toggle(…, on, …)` is now `toggle_at(…, knob: Q16, …)` at 0 / `ONE` — the knob x is
`lerp(r.x + i, r.right() - i - k, knob)`, the track colour `blend(control_track, accent, knob)` and the off hairline
fades with it, so one thing moves. `segmented_w(…, sel, …)` is now `segmented_at(…, sel, thumb_x, …)` at segment
`sel`'s left edge — the thumb is one rect anywhere along the well; the hairlines it covers are not drawn (at rest:
the same two the old code skipped, k = sel and k = sel + 1). At rest both draw the pixels they drew before, so
`tests settings`' `craft=` probe is unchanged.

**settings.rs**: `State` gains `glides`, `frame`, and this paint's `now` and `tab` (read once at the top of
`paint()`; a helper never locks `CUR`, which `open()` holds across its first paint). Keys are `mkey(kind, tab, row,
x)`. `tog()` asks the glide for the knob (target 0 / `ONE`) and draws `toggle_at`; `seg()` asks for the thumb's
device x and draws `segmented_at`; `paint_column()` draws the washes for every row, then the ONE pill at its glided
y, then the icons and labels, a label taking the light ink while the pill covers its row's centre. All three are
`motion::BASE` / `Curve::Standard`. `hover_poll()` (every desktop service pass) asks `frame.due(now,
glides.running())` and repaints when a frame is owed — one per 16 ms while a tween runs, none idle; the change's
own repaint starts the tween, the gate drives the rest, the paint that draws `done()` clears it.

Not touched: theme.rs / dpi (DESIGNAPPLY), winmenu/crystal (MENUSCRAFT), quarry/ (QUARRYHOVER), the dock, the menu
bar, video/mod.rs, quarry/live.rs. Colour literals added: none (`appearance-check.py`'s rule is six- or
eight-digit hex; the Q16 constants are decimal and the key masks are `&` operands).

## `tests motion`

Registered in tests.rs's settings block (line 191, a LINE-NEUTRAL append before the closing brace).

1. Samples: each curve at 0/¼/½/¾/1 against references solved offline in double precision
   (standard 0 39795 57530 63929 65536 · decelerate 0 45256 58319 63992 65536 · accelerate 0 8422 24381 43750
   65536), tolerance 16 Q16 units (the integer table is within 4 of each, measured with a Python mirror of the
   const fn before the compile); monotone over 257 steps; pinned at both ends.
2. Repaints: a glide driven by a simulated pass clock stepping 4 ms (passes faster than frames). 400 ms idle → the
   gate grants 0. One BASE tween → the gate grants ≥ 1 and ≤ 160 / 16 + 1 = 11, the values never go backwards,
   the value settles at the end and nothing runs after.
3. Cost: 1024 `value()` calls timed on draw.rs's `now_ns` clock → `budget_us` (what a thousand frames of one tween
   cost; a real frame evaluates three or four).

Wire: `:: MOTION: curves=3 samples=ok idle_repaints=0 run_repaints=<n> budget_us=<n> -> PASS ::` with
1 ≤ n ≤ 11 (10 with a 16 ms-aligned clock).

## The compile (the arc's one x86 line)

```
cd unaos/crates/kernel && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave33/target cargo +nightly check -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target ../../x86_64-unknown-none.json --features "$(cat /root/unaos-bench/scratch/cloud/wave33/kfeats24.txt),stria,gen7r9,wifi9,usbnet"
```

Run 1: rc=101, one error — `toggle_at`'s parameter `knob: u32` shadowed `fn knob` (draw.rs E0618); renamed
`pos`. Run 2 (the fix and nothing else): rc=0 —
`Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 40s`, 99 warnings in the crate, none in
motion.rs, draw.rs or the lines this arc added to settings.rs / tests.rs (settings.rs's `row_of` / `sel`
dead-code warnings predate the arc: B630's evidence names them).

Not run (R107, the seat's at the fold): the aarch64 leg of `./arroyo check`, `tests motion` / `tests settings`
on QEMU, `appearance-check.py`, `charter-check.sh`, knoboff.
