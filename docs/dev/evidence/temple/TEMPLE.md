# TEMPLE (LEDGER SR147, R128): THE BAR, measured on the host before the vessel

R128 (all three parts, plus Peter's clarification relayed by the seat, verbatim "i meant the both combined"): the game
renders ONE picture, with the zircon and its beam in the FOREGROUND over the lensed star sphere and the dark-matter flow
behind, the same composition STARFIELD (B661) reduces for the boot screen. The second amendment's condition is that the
game tier gets built only if its 3D is mind-blowing, and that the executor measures against a stated bar and stops if
the bar is out of reach.

**THE BAR (the arc's words):** on the rMBP's one core, a still frame at 960x600 converges by progressive accumulation
in ≤ 3 s, AND the orbit moves at ≥ 10 fps at 480x300 (upscaled, the accumulator reset every frame).

**RESULT: the bar is NOT reached by the arithmetic x86 ring 3 can run.** Per the arc's stop rule, the work stops here:
the renderer core and its measurement are committed. The vessel (crates/user-temple), the staging lines (arroyo, which
would also need the builder's file list) and the design record are NOT built.

## What x86 ring 3 can compute in (the arc asked: "say what you found")

- `unaos/x86_64-unknown-none.json`, the target every x86 user program builds for, has
  `"features": "-mmx,-sse,-sse2,…,+soft-float"` and `"rustc-abi": "softfloat"`. An `f32` in a ring-3 program
  compiles to a compiler_builtins call per operation. (user-lumen's and user-aether's `txt.rs` use f32 that way, for
  glyph coverage only.)
- The kernel clears CR4.OSFXSR whenever it dispatches a UnaOS ring-3 task
  (`crates/kernel/src/arch/x86_64/linuxabi/fpu.rs:20`, "cleared when a RING-3 non-Linux task is dispatched"). Only
  Linux-ABI tasks get an FXSAVE area and SSE. A UnaOS program that ran an SSE instruction would take #UD.
- aarch64 ring 3 is hard-float (`aarch64-base.json`: `+neon`, "Bare ARM64, hardfloat").
- A ring-3 window is capped at 288x288 on both arches (`FB_WIN_MAX_W/H`, `arch/x86_64/memory.rs:538`). The bar's
  960x600 and 480x300 frames are renderer throughput; on screen the game would get at most 288x180 at 16:10.

So the core is written over a scalar trait (`src/real.rs`) with two backends. **fx16** is Q16.16 integer, what the x86
program would run, and it is bit-exact on every machine. **f32** is the hardware-float reference: the host here,
aarch64 ring 3, or an x86 ring 3 that one day keeps an FPU context.

## The measurement (one thread, `cargo test -- --nocapture --test-threads=1`, `[profile.test] opt-level = 3`)

Host: Intel(R) Xeon(R) Processor @ 2.80GHz (cloud container, 4 logical cpus, one used). **This is not the rMBP.**
Expect the rMBP's single core to be in the same class or slower. Its integer `idiv r64` is slower than this core's,
and the Q16.16 divides count against it. No number below has been taken on the rMBP.

```
TEMPLE bar [fx16] build (tables + flow bake) 103 ms
TEMPLE bar [fx16] still 960x600: 1 spp 723 ms, 4 spp 2930 ms, 8 spp 5594 ms, 16 spp 10884 ms (1181 ns per sample incl. resolve)
TEMPLE bar [fx16] still convergence: mean |img4-img8| 0.955 LSB (2.26% ch > 8), mean |img8-img16| 0.297 LSB (0.09% ch > 8); converged (mean <= 0.5 LSB, < 0.5% > 8) at 8 spp = 5594 ms -> MISSES (bar <= 3000 ms)
TEMPLE bar [fx16] orbit 480x300 (reset every frame, 2x upscale to 960x600 timed): 5.87 fps, worst frame 225 ms -> MISSES (bar >= 10 fps)
TEMPLE bar [f32] build (tables + flow bake) 84 ms
TEMPLE bar [f32] still 960x600: 1 spp 314 ms, 4 spp 1066 ms, 8 spp 2127 ms, 16 spp 4283 ms (465 ns per sample incl. resolve)
TEMPLE bar [f32] still convergence: mean |img4-img8| 0.961 LSB (2.26% ch > 8), mean |img8-img16| 0.296 LSB (0.09% ch > 8); converged (mean <= 0.5 LSB, < 0.5% > 8) at 8 spp = 2127 ms -> MEETS (bar <= 3000 ms)
TEMPLE bar [f32] orbit 480x300 (reset every frame, 2x upscale to 960x600 timed): 14.51 fps, worst frame 88 ms -> MEETS (bar >= 10 fps)
```

(Copied from the run's output. `bar.txt` beside this file is the run's own copy.) "Converged" is defined in the test:
the mean display difference between N and 2N passes is ≤ 0.5 LSB, with < 0.5 % of channels differing by more than 8.
By that rule both scalars converge at 8 passes.

| | still, converged (8 spp, 960x600) | orbit (480x300 → 960x600) | bar |
|---|---|---|---|
| fx16 (x86 ring 3's arithmetic) | 5594 ms | 5.87 fps | misses both: 1.9x short on the still, 1.7x short on the orbit |
| f32 (hardware float) | 2127 ms | 14.51 fps | meets both, on this host |

The fx16 core is 2.4x slower than f32 per sample (1181 vs 465 ns). The cost is the Q16.16 divides (i64 `idiv`) and
`u64::isqrt` in every normalise, refraction and shell crossing. A soft-float f32 build (what the x86 profile would
actually compile `f32` to) was not measured. It costs a library call per operation, so it would be slower than fx16.

The other tests in the same run:

```
TEMPLE fx16 vs f32 64x64: mean |d| = 0.914 LSB, 1.77% of channels > 8 LSB
TEMPLE lens: ray at sweep (26,29) of 48x30, 6 shell crossings: deflection 0.01467 rad = 10.4 px (f32), 0.01465 rad = 10.4 px (fx16); reference 0.01493 rad, cos 0.9996
TEMPLE reference 64x64 fx16 4 passes: checksum=0xccb62dbc8bf87eed lit_px=4088/4096
TEMPLE refraction [f32] ch=0 n=1.920 sin_in=0.5000 sin_out=0.26041 n*sin_out=0.49998 … angle_out=15.094deg
TEMPLE refraction [f32] ch=1 n=1.955 … sin_out=0.25575 n*sin_out=0.50000 … angle_out=14.818deg
TEMPLE refraction [f32] ch=2 n=2.000 … sin_out=0.25000 n*sin_out=0.50000 … angle_out=14.478deg
TEMPLE refraction [f32] exit sin_in=0.34202 sin_out=0.66864 want 0.66865; 35deg -> TIR
TEMPLE refraction [fx16] (the same to 1e-4: 15.095 / 14.818 / 14.478 deg; exit 0.66862)
test result: ok. 5 passed; 0 failed
```

The checksum `0xccb62dbc8bf87eed` was pinned into `tests/bar.rs` (`REFERENCE_64`) AFTER this run, from the value it
printed. The pinned test has not been re-run; it is the fold's first check.

Runs: the first `cargo test` invocation stopped at a compile error (a `as u32 << 16` precedence error in volume.rs).
The second run, after that one-token fix, is the measurement above. No other host run was made.

## The frame (fx16, 16 passes, 960x600): `temple-still-960x600-16spp-fx16.png`; one orbit frame (1 pass, 480x300): `temple-orbit-480x300-fx16.png`

What the frame shows, read by the executor and not yet by Peter's eyes. The composition is the one asked for: a blue
zircon in the foreground (eight-sided prism, four-faced caps), the white beam striking its apex with a splash glow, two
dispersed red/green/blue fans leaving the lower facets, swirling cyan/violet flow strokes behind, and a dense lensed
star field. What it is not, at this budget: cinematic. The gem's facets read as flat blue panels (two bounces, with the
environment reduced to the beam), and the star layer is busier than "ultra black" wants. Tuning the look was left
undone, because the arithmetic ring 3 can run already misses the bar before any polish.

## What would move the bar (the seat's to weigh, none of it this arc's)

1. **Give UnaOS ring-3 tasks an FPU context on x86** (FXSAVE/FXRSTOR per task, as linuxabi/fpu.rs already does for
   Linux tasks; CR4.OSFXSR left set), plus an x86 user target with SSE2. The f32 numbers above then apply, and on this
   host they meet the bar with room to spare (2.1 s, 14.5 fps). This is a kernel ABI change, a ruling's to make.
2. **Run the game on more than one core**: ring 3 has SYS_THREAD_SPAWN (user-vug's workers). Rows split cleanly, so
   two workers would bring fx16 near the bar on this host. The bar as stated says one core.
3. **A cheaper fx16 integrator**: reciprocal tables instead of `idiv` and a table rsqrt. A guess of 1.3–1.6x, not
   measured, and not enough alone for the 1.9x the still is short.

## Palette (the rows `src/palette.rs` holds until the fold moves them into palette_core beside STARFIELD's)

The beam and the spectrum already exist as palette_core rows: `painter::SPLASH_BEAM` and `painter::SPLASH_SPECTRUM`
([0], [4], [7] for R, G, B). These new rows are proposed for `libs/sys/palette_core/src/lib.rs`, under names shared with
STARFIELD:

| token | value | job |
|---|---|---|
| `SCENE_ULTRA_BLACK` | 0x000000 | the void (zero light) |
| `FLOW_VIOLET` | 0x5A2DB4 | dark-matter strokes, thin mass |
| `FLOW_CYAN` | 0x32A8C8 | strokes where the mass gathers |
| `FLOW_HAZE` | 0x301C5A | the mass's diffuse glow |
| `STAR_WARM` / `STAR_COOL` | 0xFFD6A8 / 0xB4CCFF | star colour temperature ends |
| `ZIRCON_TRANSMIT` | 0x9CD8F6 | the gem's per-unit transmission (blue zircon) |
| `ZIRCON_FIRE` | 0x78B4FF | the trapped beam light inside the gem |
