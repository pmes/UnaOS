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

//! CERAMIC — brushed anodised aluminium for the window chrome, integer Q16.
//!
//! # Provenance — DERIVED, not lifted
//!
//! **This material is not in the kit.** `kits/crispy/theme.json` @ `us-crispy-modern`
//! `0787ba9f` carries `content_surface.Paper` and nothing else; there is no `Ceramic` block, no
//! metal algo in `libs/quartzite/src/surface.rs`'s `PaperAlgo`, and no parameter here can be
//! traced to a kit literal. Every constant below is **derived**, and its provenance line says so
//! in as many words. Nothing in this file is dressed up as a kit citation.
//!
//! The authority is Peter's directive, 2026-08-09, verbatim:
//!
//! > *"the 'ceramic' aluminum acer has on this zen is really nice and would be incredible if you
//! > could emulate it then crispy would truly be an amalgamation of macos/apple"*
//!
//! — with the standing scope note from the same directive: *"the 'paper' is about having a subtle
//! texture for the human experience and would like to add texture to the window borders scrollbars
//! buttons etc and have paper for text surfaces."* So: **ceramic on the chrome, paper on the
//! text.** Peter is the taste gate and he named the material; the shared-source law still governs
//! HOW it is expressed, which is why the parameters are a named table with a provenance line each
//! and the one thing that cannot be derived — the seed — is derived by a *stated rule* rather than
//! picked out of the air.
//!
//! If a future kit revision adds a `Ceramic` block, this table is what it replaces, one constant
//! at a time, exactly as `theme.rs` describes for the palette.
//!
//! # The physical model, and why it is a PER-ROW material
//!
//! A brushed-aluminium laptop lid has two independent things going on:
//!
//! 1. a dense field of **fine parallel striations** left by the brush, which runs in ONE direction
//!    — across the lid, i.e. along `x` for a screen-aligned surface. Cross-grain the surface
//!    varies fast; along the grain it barely varies at all. That is the anisotropy Peter asked for.
//! 2. a **broad luminance undulation** from the lid's curvature: the sheet is not flat, so the
//!    specular response drifts slowly as the surface normal turns. It is what makes metal read as
//!    an object rather than as noise on a plane.
//!
//! Both of those are, to first order, functions of `y` ALONE. So ceramic is modelled as a
//! **one-dimensional field over rows**, and that is not a shortcut taken for speed — it is the
//! *ideal limit* of the anisotropy the material is defined by, and it happens to be free.
//!
//! **DISCLOSED: the anisotropy here is perfect, not merely strong.** The real lid does vary a
//! little along the brush direction (the striations are not infinitely long, and the curvature is
//! not perfectly cylindrical). This model's cross-grain variation is exactly zero: two chrome
//! pixels on the same row of the same window get byte-identical shades. That is a *choice*, it is
//! the one deviation in this file that is not an arithmetic consequence, and it is the same class
//! of disclosure as `paper.rs`'s deviation 1. The visible cost is that a very wide title bar has
//! no along-grain drift; the visible benefit is that a chrome span stays a FLAT SPAN, which is the
//! entire performance argument below.
//!
//! # Why this costs no pixels
//!
//! `FrameBuffer::fill_rect` already walks its rectangle **one row at a time**, calling
//! `fill_span4` per row. A per-row material therefore adds *no per-pixel work whatsoever*: the
//! chrome painter splits its `fill_rect` calls into one per row, and each of those does the
//! identical `fill_span4` the single call would have done for that row anyway. What is added, per
//! chrome ROW (not per pixel), is one table lookup, three channel multiplies and one `encode4`.
//!
//! This is exactly the shape Peter's third standing priority asks for — *"if chrome texturing
//! measurably costs frames, make it a cheap lookup (precomputed per-row modulation) rather than
//! per-pixel math at paint time"* — so it is a precomputed per-row modulation from the start,
//! rather than a per-pixel texture retrofitted into one. [`selftest`] times [`shade`] and puts the
//! number on the wire.
//!
//! The table is **512 bytes** of `.bss` (128 `u32`), generated once behind a `spin::Once`.
//! `paper.rs`'s tile is 88 KiB by comparison, because paper is genuinely two-dimensional.
//!
//! # It MODULATES; it never replaces
//!
//! [`shade`] multiplies a role colour by a Q16 factor within `1 ± 0.02`. Every chrome pixel is
//! still the Crispy role it always was, moved by at most two percent of a channel — the same
//! contrast budget `paper.rs` honours, and the reason the material cannot fight the palette. A
//! role that changes in `theme.rs` changes here for free, because no colour is stored in this file.
//!
//! Two consequences worth stating because they are load-bearing:
//!
//! * **black stays black.** The modulation is multiplicative, so `shade(0x000000, _)` is exactly
//!   `0x000000`; ceramic can never invent ink on a glyph or lift a shadow. [`selftest`] asserts it.
//! * **the mean is not the role.** Unlike paper — whose field is a zero-mean sine pair, so the flat
//!   fill it replaced is exactly the texture's mean — ceramic's grain is a value-noise fBm whose
//!   mean is only approximately centred, and the curve is sampled over a whole turn. The chrome's
//!   average brightness therefore moves by a fraction of one `u8` step. That is a real (if
//!   invisible) change to the composited image and it is recorded rather than implied away.
//!
//! # Fixed point
//!
//! Q16 (`value * 65536`) in `i64`, no `f32` and no libm, on `paper.rs`'s precedent. The
//! primitives — the lattice hash, the Hermite fade, the wrapping value noise, the fBm and the
//! four-term sine — are `paper.rs`'s, reused rather than copied, so there is exactly one
//! implementation of each in the kernel and `paper`'s pinned fixture guards both users. Nothing in
//! this file can perturb paper: it only calls those functions, and paper's own tile hash is
//! asserted unchanged in its fixture.
//!
//! # The tile
//!
//! [`TILE_H`] = 128 rows. Seam-freedom pins the choices exactly, as it did for paper:
//!
//! * the grain lattice has period [`GRAIN_PITCH`] = 2 px, so the row count must be a multiple of 2
//!   (and the wrap is `TILE_H / GRAIN_PITCH` = 64 cells, doubling with each octave);
//! * the curve is exactly ONE turn of the sine over the tile, so it closes on itself by
//!   construction;
//! * `65536 % TILE_H == 0`, so the curve's phase step is an exact integer and the closure is not a
//!   rounding accident.
//!
//! 128 was chosen over paper's 64 so the curve's period is broad relative to a 34-px title bar —
//! a strip then shows a fraction of an undulation (a gradient) rather than a whole one (a band).
//! All three facts are const-asserted below.
//!
//! # Anchoring
//!
//! The chrome painter indexes this table by the row's offset **inside the window box**, not by its
//! panel row. The grain therefore belongs to the window and travels with it under a drag, which is
//! what makes the window read as a machined object rather than as a hole cut in a textured screen.

use super::paper::{fbm_q16, fnv1a, hash_q16, sin_turn_q16, value_noise_q16};

// ---------------------------------------------------------------------------
// The parameter table. EVERY line is derived — see the module header. The
// provenance column says what it was derived FROM, never a kit key.
// ---------------------------------------------------------------------------

/// The grain's lattice seed.
///
/// DERIVED by a stated rule rather than picked: it is **FNV-1a 32 of the ASCII bytes `ceramic`**,
/// using the same FNV constants [`fnv1a`] already uses at 64 bits (`0x811C_9DC5` offset basis,
/// `0x0100_0193` prime). The rule is what makes the number re-derivable by anyone who reads this
/// line, which is the property "a seed I chose" would not have. There is no kit seed to lift.
pub const SEED: u32 = 0x75AE_10B7;

/// Device pixels per grain lattice cell, i.e. the coarsest striation pitch.
///
/// DERIVED, Peter's directive 2026-08-09 ("brushed aluminium"): a brush leaves striations at the
/// finest scale the surface can carry, so the pitch is set to the smallest value that still admits
/// a smoothed octave above the pixel grid — 2 px. With [`GRAIN_OCTAVES`] the finer octave lands at
/// exactly 1 px, which is the pixel-pitch limit and the reason no third octave exists.
pub const GRAIN_PITCH: i64 = 2;

/// Grain octaves.
///
/// DERIVED, as above: octave 0 is the 2-px smoothed component and octave 1 the 1-px component. A
/// third octave would sample below the pixel grid and only alias, so the count is a consequence of
/// [`GRAIN_PITCH`] rather than a free parameter. Persistence and lacunarity are `fbm_q16`'s (1/2
/// and 2), shared with paper.
pub const GRAIN_OCTAVES: u32 = 2;

/// Grain amplitude — 0.012 of a channel, at Q16.
///
/// DERIVED, Peter's directive 2026-08-09 ("very low amplitude", and the standing instruction that
/// the materials stay "in the same 2%-of-a-channel league as paper's 0.02"): the total budget is
/// paper's 0.02, split so the striations carry the larger share (they are the thing being looked
/// at) and the curve the smaller. Produced by the kit's own pinned Q16 rounding rule,
/// `0.012 * 65536 + 0.5` truncated, which is the rule `theme.rs` uses for the gloss scalars — the
/// VALUE is derived, but the way it becomes an integer is not invented.
pub const GRAIN_AMP_Q16: i64 = 786;

/// Curve amplitude — 0.008 of a channel, at Q16.
///
/// DERIVED, same directive and same rounding rule: the remainder of the 0.02 budget after
/// [`GRAIN_AMP_Q16`]. It is deliberately the smaller share — a broad shade that out-shouted the
/// grain would read as a gradient with speckle on it, not as metal.
pub const CURVE_AMP_Q16: i64 = 524;

/// The two amplitudes are a BUDGET, not two independent knobs: their sum is the worst-case
/// deviation of any row, and it must stay inside the 2 % league paper set. `1310 / 65536` =
/// 0.01999. If a future taste pass moves one, it must move the other.
const _: () = assert!(GRAIN_AMP_Q16 + CURVE_AMP_Q16 <= 1311);

/// The modulation gain applied to the three circular title-bar controls, at Q16 — half.
///
/// DERIVED, Peter's directive 2026-08-09 (the controls take the material "at a lower amplitude so
/// the discs stay legible"). The discs are the smallest chrome objects on the glass and the only
/// saturated ones; at full gain the grain competes with their 12-px silhouette. Half is the
/// coarsest reduction that is still visibly the same material.
pub const CONTROL_GAIN_Q16: i64 = 32768;

/// Full gain — the chrome face, the frame and the title strip take the material as authored.
pub const FULL_GAIN_Q16: i64 = 65536;

// ---------------------------------------------------------------------------
// The tile
// ---------------------------------------------------------------------------

/// Rows in the material's repeat. See the module header for why 128 and not paper's 64.
pub const TILE_H: usize = 128;

/// Grain lattice cells across the tile at the COARSEST octave; each finer octave doubles it,
/// exactly as in `paper.rs`, which is what closes every octave on the same boundary.
const GRAIN_CELLS: i64 = TILE_H as i64 / GRAIN_PITCH;

/// Seam-freedom, and the exactness of the curve's phase step, are divisibility facts — so they are
/// asserted rather than described.
const _: () = {
    assert!(TILE_H as i64 % GRAIN_PITCH == 0);
    assert!(GRAIN_CELLS > 1);
    // The curve's phase step is `65536 / TILE_H` turns per row and must be an exact integer, or the
    // wrap closes only approximately.
    assert!(65536 % TILE_H == 0);
};

/// The generated row table: one Q16 modulation factor per row, all near `65536`.
///
/// SAFETY: written only inside `GEN.call_once`, which `spin::Once` runs exactly once and publishes
/// with release ordering; every reader goes through [`rows`], which acquires that publication
/// before handing out the shared reference. No `&mut` ever escapes. Mirrors `paper::TILE` exactly.
static mut ROWS: [u32; TILE_H] = [0; TILE_H];

static GEN: spin::Once<()> = spin::Once::new();

/// The row table, generating it on the first call.
pub fn rows() -> &'static [u32; TILE_H] {
    GEN.call_once(|| {
        // SAFETY: see `ROWS`. `call_once` guarantees this body runs on exactly one core, with no
        // reader able to observe the buffer until it returns.
        let r = unsafe { &mut *core::ptr::addr_of_mut!(ROWS) };
        generate(r);
        witness(r);
    });
    // SAFETY: see `ROWS`. Shared, immutable, after the one-shot initialisation above.
    unsafe { &*core::ptr::addr_of!(ROWS) }
}

/// The Q16 modulation factor for a row, indexed modulo the tile. The one hot lookup.
#[inline]
pub fn row_mod(row: usize) -> i64 {
    rows()[row % TILE_H] as i64
}

/// Modulate `color` (packed `0x00RRGGBB`) by the material at `row`, at `gain_q16` of full
/// amplitude. Three multiplies and three shifts; no division, no branch per channel beyond the
/// clamp.
///
/// `gain_q16 == 0` returns `color` unchanged, bit for bit — the property that makes "is the
/// material on?" a question with a provable answer.
#[inline]
pub fn shade_gain(color: u32, row: usize, gain_q16: i64) -> u32 {
    let m = 65536 + (((row_mod(row) - 65536) * gain_q16) >> 16);
    let mut out = 0u32;
    for shift in [16u32, 8, 0] {
        let c = ((color >> shift) & 0xFF) as i64;
        out = (out << 8) | ((c * m) >> 16).clamp(0, 255) as u32;
    }
    out
}

/// Modulate `color` by the material at `row`, at full amplitude — the chrome face, the frame and
/// the title strip.
#[inline]
pub fn shade(color: u32, row: usize) -> u32 {
    shade_gain(color, row, FULL_GAIN_Q16)
}

// ---------------------------------------------------------------------------
// The generator — integer Q16, on paper.rs's primitives
// ---------------------------------------------------------------------------

/// The grain at a row: `2 * fbm - 1`, i.e. the fBm re-centred to the signed unit range
/// `[-65536, 65536]`. Sampled at the row INDEX, not the row centre: for a per-row material the row
/// *is* the sample, there is no sub-row position to centre within, and sampling on the integer
/// makes both of [`selftest`]'s hand-derivable identities exact rather than approximate.
///
/// `x` is fixed at 0 with a wrap of 1 cell, which degenerates the shared 2-D noise to pure 1-D:
/// the x fade is zero at every octave, so the bilinear blend collapses to the y lerp.
#[inline]
fn grain_q16(y: i64) -> i64 {
    2 * fbm_q16(0, (y << 16) / GRAIN_PITCH, SEED, GRAIN_OCTAVES, 1, GRAIN_CELLS) - 65536
}

/// The broad curvature term at a row: exactly one turn of the shared sine over the tile, so it
/// closes on itself. Signed, `[-65536, 65536]`.
#[inline]
fn curve_q16(y: i64) -> i64 {
    sin_turn_q16((y << 16) / TILE_H as i64)
}

/// One row of the material: `1 + GRAIN_AMP * grain + CURVE_AMP * curve`, at Q16.
fn row_q16(y: usize) -> u32 {
    let y = y as i64;
    let m = 65536
        + ((GRAIN_AMP_Q16 * grain_q16(y)) >> 16)
        + ((CURVE_AMP_Q16 * curve_q16(y)) >> 16);
    m as u32
}

/// Fill `r` with one whole tile.
fn generate(r: &mut [u32; TILE_H]) {
    for (y, slot) in r.iter_mut().enumerate() {
        *slot = row_q16(y);
    }
}

// ---------------------------------------------------------------------------
// Witness + fixture
// ---------------------------------------------------------------------------

/// The row table's FNV-1a 64, as generated by this file's constants. Named here so a divergence is
/// a FAILED ASSERTION with a number rather than a chrome nobody looks at closely enough.
pub const ROWS_FNV: u64 = 0x2c52_5bfd_b49d_f67d;

/// **The wire says which material the chrome is machined from.** One line at first generation,
/// naming every parameter and the checksum of the row table actually produced — so a QEMU capture
/// and a metal capture can be compared to each other and to [`ROWS_FNV`] without a photograph.
///
/// Deliberately NOT `witness`-gated, on `paper::witness`'s precedent (itself on
/// `wm::crispy_witness`'s): the metal image is built without the `witness` feature, so a gated line
/// is absent from the only artefact that matters. Cost is one bounded serial line per boot.
fn witness(r: &[u32]) {
    serial_println!(
        "[ceramic] derived=peter-2026-08-09 algo=brushed-1d grain_oct={} pitch={} grain_amp_q16={} curve_amp_q16={} ctrl_gain_q16={} seed={:#010x} rows={} hash={:#018x}",
        GRAIN_OCTAVES,
        GRAIN_PITCH,
        GRAIN_AMP_Q16,
        CURVE_AMP_Q16,
        CONTROL_GAIN_Q16,
        SEED,
        TILE_H,
        fnv1a(r),
    );
}

/// CERAMIC fixture — the material is deterministic, it produces THESE rows, and it costs THIS much.
///
/// Six legs, cheapest first:
///
/// 1. **Hand-checkable identities.** Two, and both are exact by the construction described above:
///    at an EVEN row the grain's coarse octave sits exactly on a lattice point (the fade is zero
///    there), so the wrapping value noise IS the lattice hash of the wrapped cell; and at row
///    `TILE_H / 4` the curve's phase is exactly a quarter turn, so the shared sine returns exactly
///    `65536` and the curve contributes exactly `+CURVE_AMP_Q16`. Both can be re-derived on paper
///    from the formulas in this file, so a coefficient typo cannot hide behind a checksum.
/// 2. **The amplitude budget, on every row.** No row may deviate from `65536` by more than
///    `GRAIN_AMP_Q16 + CURVE_AMP_Q16`. This is the assertion that ceramic cannot fight the palette
///    — it is the 2 %-of-a-channel promise, checked rather than asserted in prose.
/// 3. **Modulation, not replacement.** `shade` at zero gain is the identity on an arbitrary
///    colour, and `shade` of black is black at every row and every gain. A material that failed
///    either of these would be painting, not modulating.
/// 4. **The 8-row reference.** `CHROME_FACE` shaded at rows 0..8, byte for byte.
/// 5. **Determinism + the pinned checksum.** Every row is recomputed from scratch and hashed; the
///    result must equal the hash of the stored table AND [`ROWS_FNV`].
/// 6. **The cost, measured.** `shade` is the only thing this material adds to a composite, and it
///    runs once per chrome ROW (see the module header: the per-pixel span work is unchanged). The
///    leg times `1 << 14` calls on the monotonic counter and puts ticks-per-1024-calls on the wire,
///    so "what did the chrome texture cost" is a number in the capture rather than an estimate.
///    The accumulator is printed, so the loop cannot be optimised away.
///
/// Emits one `:: CERAMIC: ... ::` verdict line.
pub fn selftest() {
    let mut fails = 0usize;

    // Leg 1 — the two hand-derivable identities.
    //
    // (a) row 2 is even, so its coarse-octave lattice coordinate is `(2 << 16) / 2` = `65536` — an
    //     exact lattice point, cell 1, where the Hermite fade is zero.
    if value_noise_q16(0, (2i64 << 16) / GRAIN_PITCH, SEED, 1, GRAIN_CELLS)
        != hash_q16(0, 1, SEED)
    {
        fails += 1;
    }
    // (b) `TILE_H / 4` rows in, the curve has turned exactly a quarter.
    if curve_q16(TILE_H as i64 / 4) != 65536 {
        fails += 1;
    }

    // Leg 2 — the amplitude budget, on every row.
    let r = rows();
    let budget = GRAIN_AMP_Q16 + CURVE_AMP_Q16;
    for &m in r.iter() {
        if (m as i64 - 65536).abs() > budget {
            fails += 1;
        }
    }

    // Leg 3 — modulation, not replacement.
    if shade_gain(0x0012_3456, 7, 0) != 0x0012_3456 {
        fails += 1;
    }
    for y in 0..TILE_H {
        if shade(0x0000_0000, y) != 0 || shade_gain(0x0000_0000, y, CONTROL_GAIN_Q16) != 0 {
            fails += 1;
        }
    }

    // Leg 4 — the reference rows: the kit's window-body role under this material.
    const REF8: [u32; 8] = [
        0x00EC_ECEE, 0x00EB_EBED, 0x00EC_ECEE, 0x00EB_EBED, //
        0x00EC_ECEE, 0x00EA_EAEC, 0x00EA_EAEC, 0x00EA_EAEC,
    ];
    for (y, &want) in REF8.iter().enumerate() {
        if shade(super::theme::CHROME_FACE, y) != want {
            fails += 1;
        }
    }

    // Leg 5 — determinism, then the pinned checksum.
    let mut regen = [0u32; TILE_H];
    generate(&mut regen);
    let stored = fnv1a(r);
    if fnv1a(&regen) != stored {
        fails += 1;
    }
    if stored != ROWS_FNV {
        fails += 1;
    }

    // Leg 6 — the cost of the ONE thing this material adds per chrome row.
    const ITERS: usize = 1 << 14;
    let t0 = crate::clock::mono_ticks();
    let mut acc = 0u32;
    for i in 0..ITERS {
        acc = acc.wrapping_add(shade(super::theme::CHROME_FACE, i));
    }
    let t1 = crate::clock::mono_ticks();
    let per_1k = match (t0, t1) {
        (Some(a), Some(b)) if b >= a => ((b - a) * 1024) / ITERS as u64,
        _ => u64::MAX,
    };

    if fails == 0 {
        serial_println!(
            ":: CERAMIC: brushed material — {} rows/{} oct, regen hash={:#018x} == pinned, budget<={} q16, shade tk/1k={} acc={:#010x} :: PASS ::",
            TILE_H, GRAIN_OCTAVES, stored, budget, per_1k, acc
        );
    } else {
        serial_println!(
            ":: CERAMIC: brushed material — {} leg(s) failed, hash={:#018x} want={:#018x} :: FAIL ::",
            fails, stored, ROWS_FNV
        );
    }
}
