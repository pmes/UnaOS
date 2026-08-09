// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! PAPER — the Crispy kit's procedural paper texture, ported to integer Q16.
//!
//! # Source of record
//!
//! `kits/crispy/theme.json` @ branch `us-crispy-modern`, commit `0787ba9f`, block
//! `content_surface.Paper`. The generator this file ports is quartzite's
//! `libs/quartzite/src/surface.rs` — the host-side rasteriser the same kit data drives.
//!
//! `video/theme.rs` §"What is NOT lifted" and `docs/dev/OS/08_VIDEO/engine.md` §9 both
//! deliberately left this block unlifted, with the note that *"lifting it means porting a
//! multi-octave noise generator, a rasterizer concern"*. This module is that port, and it
//! sits beside the rasteriser exactly where those notes said it belongs.
//!
//! **Peter's ruling (white board, 2026-08-08, A1 follow-up):** *"we must have the paper
//! texture but i don't want it as the desktop background"*. Paper is a **CONTENT** surface —
//! what a content region composites UNDER its content. The desktop keeps [`super::wm::DESKTOP_BG`]
//! (and, later, the lake scene). Nothing in this module may be reached from a desktop fill.
//!
//! # The kit parameters, and the constant each one became
//!
//! ```text
//!   json key                        value            kernel constant
//!   ------------------------------  ---------------  --------------------------------
//!   content_surface.Paper.base_rgb  [0.96,0.949,      via theme::CONTENT_FILL (see below)
//!                                    0.918]
//!   …params.algo                    "Laid"           the `field_q16` body (laid + chain + env)
//!   …params.amplitude               0.02             AMP_Q16      = 1311
//!   …params.scale                   4.0              SCALE        = 4
//!   …params.octaves                 3                ENV_OCTAVES  = 3
//!   …params.seed                    4223012511       SEED         = 0xFBB6_0E9F
//! ```
//!
//! Constants that are **not** in the json but ARE in the kit's generator (`surface.rs`'s
//! `field_at`, `PaperAlgo::Laid` arm) and are therefore lifted from it, not invented:
//!
//! ```text
//!   surface.rs literal              meaning                       kernel constant
//!   ------------------------------  ----------------------------  -------------------
//!   `laid * 0.82`                   laid-line weight              LAID_W_Q16  = 53740
//!   `chain * 0.30`                  chain-line weight             CHAIN_W_Q16 = 19661
//!   `scale * 11.0`                  chain pitch = 11x laid pitch  CHAIN_MULT  = 11
//!   `0.35 + 0.65 * fbm(...)`        envelope floor / span         ENV_A_Q16 = 22938,
//!                                                                 ENV_B_Q16 = 42598
//!   `px / (scale * 8.0)`            envelope lattice scale        ENV_SCALE_MULT = 8
//!   `fbm(..., s, 3)`                envelope octaves — a LITERAL  ENV_OCTAVES = 3
//!                                   3 in the Laid arm, not
//!                                   `params.octaves` (see note)
//!   `amp *= 0.5` / `f *= 2.0`       persistence 1/2, lacunarity 2 the `fbm_q16` loop
//!   `0x9E37_79B1`/`0x85EB_CA77`/    the lattice hash              `hash2`, verbatim
//!   `0x2C1B_3C6D`/`0x297A_2D39`
//!   `seed + o * 0x68E3_1DA4`        per-octave seed               `fbm_q16`, verbatim
//! ```
//!
//! Every Q16 literal above was produced by the kit's own pinned rounding rule at Q16
//! (`value * 65536 + 0.5`, truncated) — the same rule `theme.rs` uses for its gloss scalars.
//!
//! **`octaves` is not ambiguous here even though the Laid arm ignores it.** `surface.rs`'s
//! `PaperAlgo::Laid` arm hard-codes `3` for its envelope fBm and never reads
//! `params.octaves`; the json's `octaves: 3` happens to be the same number. Both readings
//! give 3, so there is nothing to ask. If a future kit changes `octaves` away from 3, this
//! module must follow the GENERATOR (which would still use 3) and not the json field — that
//! is a kit bug to report, not a kernel decision.
//!
//! # The base colour
//!
//! `base_rgb` is `[0.96, 0.949, 0.918]`, which is the same triple as `palette.content_fill`.
//! `theme.rs` and engine.md §9 both record that the paper "reads `base_rgb` from
//! `CONTENT_FILL` — the two agree by construction", so this module takes its base from
//! [`super::theme::CONTENT_FILL`] rather than lifting a second copy of the triple. The
//! const-assert below is the tripwire if a future kit ever separates them.
//!
//! The channel base is re-expanded to Q16 from the `u8` role (`c * 65536 / 255`, round-half-up),
//! which is at most 1/510 of a step away from the json's f32 — three orders of magnitude below
//! the 2 % amplitude this texture is allowed to move a pixel by.
//!
//! # Fixed point, and how far the fidelity claim reaches
//!
//! Everything is **Q16** (`value * 65536`) in `i64`, on `theme.rs`'s `blend_q16` precedent.
//! There is no `f32` and no libm anywhere in this file, by construction — the kernel has neither.
//!
//! Measured against a bit-exact `f64` model of `surface.rs`'s `field_at` (same lattice, wrapping
//! disabled), the integer field's **maximum absolute error is 6.9e-5** of full scale, over the
//! whole tile. At `amplitude = 0.02` that is 1.4e-6 of the base colour — 4e-4 of one `u8` step,
//! i.e. the ported texture and the host's are the same 8-bit image wherever the deviations below
//! do not apply.
//!
//! # DISCLOSED DEVIATIONS from the kit's generator
//!
//! 1. **The lattice WRAPS; the host's does not.** `surface.rs` generates the field across a whole
//!    region and decorrelates neighbours with a per-region `seed`; it never tiles, so it never had
//!    to. A kernel that must not regenerate a texture per region, per window, per frame has to
//!    tile — so `value_noise_q16` reduces its lattice indices modulo the tile's cell count (and
//!    the count doubles with each octave, exactly as the coordinates do). The result is a
//!    different *instance* of the same generator — same algorithm, same hash, same octave
//!    structure, seamless by construction — not an approximation of the host's instance. Sampled
//!    against the unwrapped host field the difference reaches 0.26 of full scale (i.e. 0.5 % of a
//!    base channel at 2 % amplitude); it is a different draw of the same noise, and it is the one
//!    deviation here that is a *choice* rather than an arithmetic consequence.
//!
//! 2. **`sin` is a 4-term odd polynomial, not libm's.** The Laid arm needs two sines and the
//!    kernel has no libm. [`sin_turn_q16`] evaluates the odd Taylor series of `sin(pi*x/2)` on the
//!    quarter wave in Q16, with the `x^7` coefficient nudged from the Taylor `-307` to `-297` so
//!    the quarter-wave endpoint is EXACTLY `1.0` (Q16 `65536`) rather than `65526`. Maximum
//!    absolute error over the full turn is **3 Q16 units** (4.6e-5), verified against `f64` `sin`
//!    at all 65536 phases. The endpoints are exact: `0`, `+65536`, `0`, `-65536` at 0/¼/½/¾ turn.
//!
//! 3. **`hash_unit` is truncated to Q16.** The host's `(hash2(..) >> 8) as f32 / 2^24` is exact in
//!    `f32` (24-bit mantissa); the Q16 form is `hash2(..) >> 16`, the same value truncated to 16
//!    fractional bits. Deviation < 1.6e-5 per lattice value. The **hash itself is verbatim** — the
//!    kit's `hash2` is already pure integer (a PCG-style final mix), so the entropy source needed
//!    no substitute and there is no JS/float dependence anywhere in it.
//!
//! 4. **One seed, one tile.** The host's `seed` is per-REGION, so no two regions share a field.
//!    The kernel generates ONE tile from the kit's single `seed` and every content surface shows
//!    it. Adjacent content regions therefore correlate where the host's would not — the price of
//!    "generate once, blit thereafter", and the reason the tile is 352 px wide rather than small.
//!
//! # The tile
//!
//! **The kit names no tile size** (it never tiles — deviation 1). The size here is chosen so the
//! wrap is seam-free *by construction*, which pins it exactly:
//!
//! * the laid lines have period `SCALE` = 4 px in y;
//! * the chain lines have period `SCALE * CHAIN_MULT` = 44 px in x;
//! * the envelope's coarsest lattice cell is `SCALE * ENV_SCALE_MULT` = 32 px on both axes, and a
//!   wrapped lattice can only close on a whole number of cells.
//!
//! So width must be a multiple of `lcm(44, 32) = 352` and height a multiple of `lcm(4, 32) = 32`.
//! [`TILE_W`] x [`TILE_H`] = **352 x 64** is the smallest such tile with more than one envelope
//! cell vertically: 88 KiB of `.bss`, comfortably L2-resident on both the Pi 4 and the rMBP, which
//! is what keeps the blit cheap. Generation is once per boot, on first use; the blit is a modular
//! row copy and nothing regenerates per frame.

use super::theme;

// ---------------------------------------------------------------------------
// Kit parameters
// ---------------------------------------------------------------------------

/// `content_surface.Paper.params.seed` = `4223012511` @ `0787ba9f`.
const SEED: u32 = 0xFBB6_0E9F;

/// `content_surface.Paper.params.scale` = `4.0` @ `0787ba9f` — device pixels per lattice unit,
/// and (for `Laid`) the laid-line pitch. Integer because the kit's value is integral; a
/// fractional kit scale would need a Q16 divisor here and is a kit change, not a silent one.
const SCALE: i64 = 4;

/// `content_surface.Paper.params.amplitude` = `0.02` @ `0787ba9f`, at Q16. The contrast budget:
/// the hard ceiling on relative luminance deviation, honoured by construction.
const AMP_Q16: i64 = 1311;

/// `surface.rs`'s `PaperAlgo::Laid` arm: `chain` pitch is `scale * 11.0`.
const CHAIN_MULT: i64 = 11;

/// `surface.rs`'s `PaperAlgo::Laid` arm: the envelope samples at `px / (scale * 8.0)`.
const ENV_SCALE_MULT: i64 = 8;

/// `surface.rs`'s `PaperAlgo::Laid` arm: `fbm(..., s, 3)` — a literal 3 in the generator, and
/// also the json's `octaves`. See the module note on why this is not ambiguous.
const ENV_OCTAVES: u32 = 3;

/// `surface.rs`: `laid * 0.82`, at Q16.
const LAID_W_Q16: i64 = 53740;

/// `surface.rs`: `chain * 0.30`, at Q16.
const CHAIN_W_Q16: i64 = 19661;

/// `surface.rs`: the `0.35` floor of `env = 0.35 + 0.65 * fbm(..)`, at Q16.
const ENV_A_Q16: i64 = 22938;

/// `surface.rs`: the `0.65` span of `env = 0.35 + 0.65 * fbm(..)`, at Q16.
const ENV_B_Q16: i64 = 42598;

/// The envelope floor and span must still reach exactly 1.0 at the top of the fBm range, or the
/// amplitude budget quietly changes meaning.
const _: () = assert!(ENV_A_Q16 + ENV_B_Q16 == 65536);

/// The kit's `base_rgb` `[0.96, 0.949, 0.918]` under the pinned `to_u8` rule is `0xF5F2EA`, which
/// is what `palette.content_fill` rounds to as well. `theme.rs` and engine.md §9 both state the
/// two agree by construction and that the paper reads its base from `CONTENT_FILL`; this is the
/// tripwire that says so out loud if a future kit ever separates them.
const _: () = assert!(theme::CONTENT_FILL == 0x00F5_F2EA);

// ---------------------------------------------------------------------------
// The tile
// ---------------------------------------------------------------------------

/// Tile width in pixels. `lcm(SCALE * CHAIN_MULT, SCALE * ENV_SCALE_MULT)` = `lcm(44, 32)`.
pub const TILE_W: usize = 352;

/// Tile height in pixels. A multiple of `lcm(SCALE, SCALE * ENV_SCALE_MULT)` = `lcm(4, 32)` = 32;
/// two envelope cells.
pub const TILE_H: usize = 64;

/// Seam-freedom is a divisibility fact, so it is asserted rather than described.
const _: () = {
    assert!(TILE_W as i64 % (SCALE * CHAIN_MULT) == 0);
    assert!(TILE_W as i64 % (SCALE * ENV_SCALE_MULT) == 0);
    assert!(TILE_H as i64 % SCALE == 0);
    assert!(TILE_H as i64 % (SCALE * ENV_SCALE_MULT) == 0);
};

/// Envelope lattice cells across the tile, at the COARSEST octave. Each finer octave doubles both
/// the coordinate and this period, which is what makes the wrap close on every octave at once.
const ENV_CELLS_X: i64 = TILE_W as i64 / (SCALE * ENV_SCALE_MULT);
const ENV_CELLS_Y: i64 = TILE_H as i64 / (SCALE * ENV_SCALE_MULT);

#[repr(align(64))]
struct Tile([u32; TILE_W * TILE_H]);

/// The generated tile. Zero-initialised `.bss` (88 KiB, no image cost); filled exactly once, by
/// [`tile`]'s `call_once`.
///
/// SAFETY: written only inside `GEN.call_once`, which `spin::Once` runs exactly once and
/// publishes with release ordering; every reader goes through [`tile`], which acquires that
/// publication before handing out the shared reference. No `&mut` ever escapes.
static mut TILE: Tile = Tile([0; TILE_W * TILE_H]);

static GEN: spin::Once<()> = spin::Once::new();

/// The generated tile, generating it on the first call. Row-major, top row first, `0x00RRGGBB`
/// per pixel — the same word layout `wm::draw_window` and every kernel-drawn surface use.
pub fn tile() -> &'static [u32; TILE_W * TILE_H] {
    GEN.call_once(|| {
        // SAFETY: see `TILE`. `call_once` guarantees this body runs on exactly one core, with no
        // reader able to observe the buffer until it returns.
        let px = unsafe { &mut (*core::ptr::addr_of_mut!(TILE)).0 };
        generate(px);
        witness(px);
    });
    // SAFETY: see `TILE`. Shared, immutable, after the one-shot initialisation above.
    unsafe { &(*core::ptr::addr_of!(TILE)).0 }
}

/// Paint the paper into `dst` — a `surf_w`-wide `0x00RRGGBB` surface — over the rectangle
/// `(x, y, w, h)`, clipped to the buffer.
///
/// The texture's phase is anchored at the SURFACE origin, not at the rectangle's, so a content
/// well that grows, shrinks or is repainted in pieces never shifts its grain. Cost is one modular
/// index per row plus a straight copy: no arithmetic per pixel, and no generation after the first.
pub fn fill_rect(dst: &mut [u32], surf_w: usize, x: usize, y: usize, w: usize, h: usize) {
    if surf_w == 0 || w == 0 || h == 0 {
        return;
    }
    let src = tile();
    let rows = dst.len() / surf_w;
    for dy in y..(y + h).min(rows) {
        let srow = (dy % TILE_H) * TILE_W;
        let drow = dy * surf_w;
        for dx in x..(x + w).min(surf_w) {
            dst[drow + dx] = src[srow + (dx % TILE_W)];
        }
    }
}

// ---------------------------------------------------------------------------
// The generator — integer Q16 throughout
// ---------------------------------------------------------------------------

/// `surface.rs`'s `hash2`, VERBATIM: the lattice hash, a PCG-style final mix. Already pure
/// integer in the kit, so the entropy source is shared exactly rather than approximated.
#[inline]
pub(super) fn hash2(xi: i64, yi: i64, seed: u32) -> u32 {
    let mut h = seed
        .wrapping_add((xi as u32).wrapping_mul(0x9E37_79B1))
        .wrapping_add((yi as u32).wrapping_mul(0x85EB_CA77));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h
}

/// The kit's `hash_unit` at Q16: `(h >> 8) / 2^24` in `f32` is `h >> 16` in Q16. Deviation 3.
#[inline]
pub(super) fn hash_q16(xi: i64, yi: i64, seed: u32) -> i64 {
    (hash2(xi, yi, seed) >> 16) as i64
}

/// The kit's `smooth(t) = t*t*(3 - 2t)` Hermite fade, at Q16. `t` in `[0, 65536)`.
#[inline]
pub(super) fn smooth_q16(t: i64) -> i64 {
    let t2 = (t * t) >> 16;
    (t2 * (3 * 65536 - 2 * t)) >> 16
}

/// The kit's bilinear value noise, at Q16, on a lattice that WRAPS every `wx` x `wy` cells
/// (deviation 1). `x`/`y` are non-negative Q16 lattice coordinates; the result is `[0, 65536)`.
///
/// At an integer lattice point the fade is zero and the result is exactly
/// `hash_q16(x % wx, y % wy, seed)` — the identity [`selftest`] checks by hand.
#[inline]
pub(super) fn value_noise_q16(x: i64, y: i64, seed: u32, wx: i64, wy: i64) -> i64 {
    let xi = x >> 16;
    let yi = y >> 16;
    let fx = smooth_q16(x & 0xFFFF);
    let fy = smooth_q16(y & 0xFFFF);
    let (x0, x1) = (xi % wx, (xi + 1) % wx);
    let (y0, y1) = (yi % wy, (yi + 1) % wy);
    let v00 = hash_q16(x0, y0, seed);
    let v10 = hash_q16(x1, y0, seed);
    let v01 = hash_q16(x0, y1, seed);
    let v11 = hash_q16(x1, y1, seed);
    let a = v00 + (((v10 - v00) * fx) >> 16);
    let b = v01 + (((v11 - v01) * fx) >> 16);
    a + (((b - a) * fy) >> 16)
}

/// The kit's `fbm`: summed octaves of [`value_noise_q16`], persistence 1/2, lacunarity 2,
/// normalised to `[0, 65536]`. The per-octave seed offset is the kit's `0x68E3_1DA4`.
///
/// The wrap period doubles with the coordinate at every octave, so all `ENV_OCTAVES` layers close
/// on the same tile boundary.
#[inline]
pub(super) fn fbm_q16(x: i64, y: i64, seed: u32, octaves: u32, wx: i64, wy: i64) -> i64 {
    let mut sum = 0i64;
    let mut amp = 65536i64;
    let mut norm = 0i64;
    let (mut fx, mut fy) = (x, y);
    let (mut px, mut py) = (wx, wy);
    for o in 0..octaves {
        let s = seed.wrapping_add(o.wrapping_mul(0x68E3_1DA4));
        sum += (amp * value_noise_q16(fx, fy, s, px, py)) >> 16;
        norm += amp;
        amp >>= 1;
        fx <<= 1;
        fy <<= 1;
        px <<= 1;
        py <<= 1;
    }
    (sum << 16) / norm
}

// The odd Taylor coefficients of `sin(pi*x/2)` at Q16, for `x` in `[0, 1]` = a quarter turn.
// `P7` is the ONE nudged value (Taylor gives `-307`): with `-297` the four terms sum to exactly
// `65536` at `x = 1`, so the quarter-wave endpoint is exact instead of 10 units short. Maximum
// absolute error over all 65536 phases is 3 Q16 units — see the module's deviation 2.
const SIN_P1: i64 = 102944; //  1.57079632679
const SIN_P3: i64 = -42334; // -0.64596409751
const SIN_P5: i64 = 5223; //  0.07969262624
const SIN_P7: i64 = -297; // -0.00468175413, nudged for an exact endpoint

/// `sin(2*pi*turn)` at Q16, where `turn` is a phase in TURNS at Q16 (only the fractional part is
/// read). Result in `[-65536, 65536]`.
#[inline]
pub(super) fn sin_turn_q16(turn: i64) -> i64 {
    let mut t = turn & 0xFFFF;
    let neg = t >= 32768;
    if neg {
        t -= 32768;
    }
    if t > 16384 {
        t = 32768 - t;
    }
    // `t` is now in `[0, 16384]` = the first quarter turn; rescale to `[0, 65536]` = `[0, 1]`.
    let x = t << 2;
    let x2 = (x * x) >> 16;
    let mut r = SIN_P7;
    r = SIN_P5 + ((r * x2) >> 16);
    r = SIN_P3 + ((r * x2) >> 16);
    r = SIN_P1 + ((r * x2) >> 16);
    r = (r * x) >> 16;
    if neg {
        -r
    } else {
        r
    }
}

/// The kit's `field_at` for `PaperAlgo::Laid`, at Q16: laid lines plus chain lines, amplitude-
/// modulated by a slow fBm envelope so they read as irregular fibre rather than a grid.
///
/// `px`/`py` are Q16 device-pixel coordinates. Result is the SIGNED unit field in
/// `[-65536, 65536]`; the caller multiplies by [`AMP_Q16`].
fn field_q16(px: i64, py: i64) -> i64 {
    let laid = sin_turn_q16(py / SCALE);
    let chain = sin_turn_q16(px / (SCALE * CHAIN_MULT));
    let ex = px / (SCALE * ENV_SCALE_MULT);
    let ey = py / (SCALE * ENV_SCALE_MULT);
    let env = ENV_A_Q16
        + ((ENV_B_Q16 * fbm_q16(ex, ey, SEED, ENV_OCTAVES, ENV_CELLS_X, ENV_CELLS_Y)) >> 16);
    let inner = ((LAID_W_Q16 * laid) >> 16) + ((CHAIN_W_Q16 * chain) >> 16);
    ((inner * env) >> 16).clamp(-65536, 65536)
}

/// The kit's `base_rgb` at Q16, taken from [`theme::CONTENT_FILL`] (see the module note): each
/// `u8` channel re-expanded as `c * 65536 / 255`, round-half-up.
#[inline]
fn base_q16(shift: u32) -> i64 {
    let c = ((theme::CONTENT_FILL >> shift) & 0xFF) as i64;
    (c * 65536 + 127) / 255
}

/// One pixel of the paper: the kit's `out = base * (1 + amplitude * field)` per channel, then the
/// kit's pinned `to_u8` rule (`v * 255 + 0.5`, truncated) — both in integers.
fn pixel(x: usize, y: usize) -> u32 {
    // Pixel CENTRES, as `render_rgba8` samples them (`x as f32 + 0.5`).
    let f = field_q16(((x as i64) << 16) + 32768, ((y as i64) << 16) + 32768);
    let m = 65536 + ((AMP_Q16 * f) >> 16);
    let mut out = 0u32;
    for shift in [16u32, 8, 0] {
        let v = (base_q16(shift) * m) >> 16;
        let u = (((v * 255) + 32768) >> 16).clamp(0, 255) as u32;
        out = (out << 8) | u;
    }
    out
}

/// Fill `px` with one whole tile.
fn generate(px: &mut [u32; TILE_W * TILE_H]) {
    for y in 0..TILE_H {
        let row = y * TILE_W;
        for x in 0..TILE_W {
            px[row + x] = pixel(x, y);
        }
    }
}

// ---------------------------------------------------------------------------
// Witness + fixture
// ---------------------------------------------------------------------------

/// FNV-1a 64, `wcg::checksum`'s constants and byte order, over the tile's words as they sit in
/// memory. Both arches are little-endian, so the same tile hashes the same on both — which is the
/// whole point of printing it.
pub(super) fn fnv1a(px: &[u32]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &w in px.iter() {
        for k in 0..4 {
            h ^= ((w >> (8 * k)) & 0xFF) as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// The tile's FNV-1a 64, as generated by this file's constants. Named here so a divergence is a
/// FAILED ASSERTION with a number rather than a texture nobody looks at closely enough.
pub const TILE_FNV: u64 = 0x0df2_b838_2510_69dc;

/// **The wire says which texture the glass shows.** One line at first generation, naming the kit
/// revision, every parameter, and the checksum of the pixels actually produced — so a QEMU capture
/// and a metal capture can be compared to each other and to [`TILE_FNV`] without a photograph.
///
/// Deliberately NOT `witness`-gated, on `wm::crispy_witness`'s precedent: the metal image is built
/// without the `witness` feature, so a gated line is absent from the only artefact that matters.
/// Cost is one bounded serial line per boot.
fn witness(px: &[u32]) {
    serial_println!(
        "[paper] kit=us-crispy-modern@0787ba9f algo=laid octaves={} scale={} amp_q16={} seed={:#010x} base={:#08x} tile={}x{} hash={:#018x}",
        ENV_OCTAVES,
        SCALE,
        AMP_Q16,
        SEED,
        theme::CONTENT_FILL,
        TILE_W,
        TILE_H,
        fnv1a(px),
    );
}

/// PAPER fixture — the generator is deterministic, and it produces THESE bytes.
///
/// Four legs, cheapest first:
///
/// 1. **Hand-checkable primitives.** `smooth(0.5) == 0.5`; the sine's four exact quadrant points;
///    and the identity that at an integer lattice point the value noise IS the lattice hash of the
///    wrapped cell (the fade is zero there). Each of these can be re-derived on paper from the
///    formulas in this file, so a coefficient typo cannot hide behind a checksum nobody can
///    reproduce.
/// 2. **The 4x4 reference corner.** The top-left 4x4 of the tile, byte for byte. It is also
///    STRUCTURALLY checkable: the laid line has period `SCALE` = 4 px in y and the sampled phases
///    are `(y + 0.5) / 4` turns, i.e. `+0.7071, +0.7071, -0.7071, -0.7071` — so rows 0 and 1 must
///    be LIGHTER than rows 2 and 3, which is exactly what the constants below say.
/// 3. **Determinism.** Every pixel is recomputed from scratch and hashed; the result must equal
///    the hash of the stored tile. Same inputs, same bytes, twice.
/// 4. **The pinned checksum.** That hash must equal [`TILE_FNV`], so a parameter that drifts from
///    the kit is caught even if it drifts consistently.
///
/// Emits one `:: PAPER: ... ::` verdict line.
pub fn selftest() {
    let mut fails = 0usize;

    // Leg 1 — primitives.
    if smooth_q16(32768) != 32768 {
        fails += 1;
    }
    if sin_turn_q16(0) != 0
        || sin_turn_q16(0x4000) != 65536
        || sin_turn_q16(0x8000) != 0
        || sin_turn_q16(0xC000) != -65536
    {
        fails += 1;
    }
    if value_noise_q16(2 << 16, 3 << 16, SEED, ENV_CELLS_X, ENV_CELLS_Y)
        != hash_q16(2 % ENV_CELLS_X, 3 % ENV_CELLS_Y, SEED)
    {
        fails += 1;
    }

    // Leg 2 — the reference corner, and the laid line's direction.
    const REF4: [u32; 16] = [
        0x00F7_F4EC, 0x00F7_F4EC, 0x00F7_F4EC, 0x00F7_F4EC, //
        0x00F7_F4EC, 0x00F7_F4EC, 0x00F7_F4EC, 0x00F7_F4EC, //
        0x00F3_F0E9, 0x00F4_F1E9, 0x00F4_F1E9, 0x00F4_F1E9, //
        0x00F3_F0E9, 0x00F4_F1E9, 0x00F4_F1E9, 0x00F4_F1E9,
    ];
    let px = tile();
    for y in 0..4 {
        for x in 0..4 {
            if px[y * TILE_W + x] != REF4[y * 4 + x] {
                fails += 1;
            }
        }
    }
    // The structural reading of the same corner: the laid crest is above the trough.
    if REF4[0] <= REF4[8] {
        fails += 1;
    }

    // Leg 3 — determinism: regenerate every pixel and hash it independently of the stored tile.
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for y in 0..TILE_H {
        for x in 0..TILE_W {
            let w = pixel(x, y);
            for k in 0..4 {
                h ^= ((w >> (8 * k)) & 0xFF) as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    let stored = fnv1a(px);
    if h != stored {
        fails += 1;
    }

    // Leg 4 — the pinned checksum.
    if stored != TILE_FNV {
        fails += 1;
    }

    if fails == 0 {
        serial_println!(
            ":: PAPER: kit texture — {}x{} laid/{} oct, regen hash={:#018x} == pinned, 4x4 reference exact :: PASS ::",
            TILE_W, TILE_H, ENV_OCTAVES, stored
        );
    } else {
        serial_println!(
            ":: PAPER: kit texture — {} leg(s) failed, hash={:#018x} regen={:#018x} want={:#018x} :: FAIL ::",
            fails, stored, h, TILE_FNV
        );
    }
}
