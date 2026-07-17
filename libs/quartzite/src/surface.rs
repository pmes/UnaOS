// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Surface — the material layer beneath quartzite content (SURFACE-1).
//!
//! Peter's seed (`future/unaos-texture-and-retro-kits.md`): humans need visual
//! *texture*, and a subtle paper-like texture under text "would really slay".
//! The acceptance bar is his, verbatim — **"a person wishing they could touch
//! it"**: texture as tactility, never decoration.
//!
//! # What this module is
//!
//! The **platform-agnostic** heart of the Surface primitive: procedural,
//! parametric paper-texture generators plus the contrast budget that keeps them
//! honest. It contains no windowing code and no `unsafe`; a backend (macOS
//! first — see [`crate::platforms`]) asks this module for a device-pixel field
//! or an RGBA8 raster and composites content *on top*. Text never composites
//! over the texture — the paper sits under the glyphs.
//!
//! # The design of record (Peter + Maestro, R15 — do not re-litigate)
//!
//!  1. A Surface is a **material layer** owned by a region, not a per-widget
//!     style. One sheet of paper; ink laid on top.
//!  2. Paper is **light behaving correctly**: (a) ink into the fibers, (b)
//!     micro-relief, (c) no tiling artifacts — hash-based procedural noise,
//!     seeded per region, felt more than seen at 100 %.
//!  3. **CPU-path honesty:** a backend that cannot render a surface well renders
//!     it as *nothing* (clean flat) — never a bad approximation.
//!
//! Ingredient (a), ink-bleed at glyph boundaries, perturbs glyph edges and so
//! belongs to the GPU path (a euclase shader that has the glyph coverage). On
//! the CPU/AppKit sample-board path the texture is strictly *under* the text, so
//! this module ships ingredients (b) micro-relief and (c) tone; glyph
//! antialiasing is left entirely to the native text stack and is unharmed by
//! construction. See `docs/dev/USERLAND/SURFACE.md`.
//!
//! # The contrast budget
//!
//! Subtlety is the whole game; the Gemini failure was *execution* — an
//! over-strong texture reads as kitsch instantly. Every generated field is
//! bounded, by construction, to a caller-declared maximum **relative luminance
//! deviation** ([`PaperParams::amplitude`]). Because the field modulates the
//! base colour multiplicatively (`out = base * (1 + delta)`), the relative
//! luminance deviation of every pixel equals its `delta`, which lies in
//! `[-amplitude, +amplitude]`. [`measure_max_deviation`] recovers that number
//! from a rendered raster so a test can assert it *measured*, not by eye.

/// Which procedural paper algorithm generates the field.
///
/// Three genuinely different characters, so the taste-gate spans a real space
/// rather than one look at three strengths:
///
///  * [`PaperAlgo::Grain`] — isotropic micro-relief: the emboss (directional
///    gradient) of fine hash value-noise, lit from the top-left. Paper *tooth*.
///  * [`PaperAlgo::Laid`] — the directional structure of laid/mould-made stock:
///    closely spaced wire (laid) lines crossed by sparse chain lines, their
///    amplitude modulated by low-frequency noise so they read as irregular
///    fibre, never a mechanical grating.
///  * [`PaperAlgo::Blotch`] — low-frequency luminance cloudiness (a few octaves
///    of fBm), the gentle unevenness of real stock held to the light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaperAlgo {
    Grain,
    Laid,
    Blotch,
}

impl PaperAlgo {
    /// A short stable identifier for labels and doc records.
    pub const fn label(self) -> &'static str {
        match self {
            PaperAlgo::Grain => "grain",
            PaperAlgo::Laid => "laid",
            PaperAlgo::Blotch => "blotch",
        }
    }

    /// All algorithms, in board order.
    pub const ALL: [PaperAlgo; 3] = [PaperAlgo::Grain, PaperAlgo::Laid, PaperAlgo::Blotch];
}

/// The full parameter set for one paper surface — the thing the taste-gate
/// picks and a future theming/kit layer addresses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaperParams {
    /// Which procedural algorithm.
    pub algo: PaperAlgo,
    /// The contrast budget: maximum relative luminance deviation, as a fraction
    /// (e.g. `0.02` = ±2 %). The generated field is bounded to this by
    /// construction; it is both the strength dial and the hard ceiling.
    pub amplitude: f32,
    /// Feature size, in **device pixels per lattice unit** — larger = coarser.
    /// For [`PaperAlgo::Laid`] this sets the laid-line pitch.
    pub scale: f32,
    /// Octaves of fBm where the algorithm layers noise (`Grain`, `Blotch`).
    /// Clamped to `1..=6`.
    pub octaves: u32,
    /// Per-region seed — decorrelates neighbouring regions so no tiling seam is
    /// ever visible across a layout.
    pub seed: u32,
}

impl PaperParams {
    /// A human-readable one-line parameter record for a cell label / the doc.
    pub fn describe(&self) -> String {
        format!(
            "{}  amp {:.1}%  scale {:.0}px  oct {}  seed {}",
            self.algo.label(),
            self.amplitude * 100.0,
            self.scale,
            self.octaves.clamp(1, 6),
            self.seed,
        )
    }
}

/// A paper surface: a base colour with a procedural field laid over it.
///
/// This is the shape [`Surface::Paper`] wraps: a region declares it, content
/// composites on top. Adoption stays OFF everywhere until a view-owner opts in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paper {
    /// Linear-ish sRGB base stock colour, 0..1 per channel. A warm off-white is
    /// the canonical paper; the field only *modulates* this.
    pub base_rgb: [f32; 3],
    pub params: PaperParams,
}

/// The canonical warm paper-stock base colour (sRGB, 0..1).
pub const PAPER_STOCK: [f32; 3] = [0.960, 0.949, 0.918];

// ---------------------------------------------------------------------------
// The taste-gated default (M2)
// ---------------------------------------------------------------------------

/// The parameters Peter picked at the attended taste-gate (2026-07-17): the
/// **laid** algorithm at the board's **medium** cell — amp 2.0 %, scale 4 px,
/// the board's laid octaves and that cell's exact seed. His words: "lets
/// default to this for now and we'll put in sliders to play with later so
/// people can dial it in or turn it off."
///
/// This is what `Surface::Paper` (via `Paper::default()` /
/// `PaperParams::default()`) means with no explicit parameters. It stays fully
/// parametric — a future settings layer (SURFACE-2 candidate: per-user dial-in
/// + off switch) addresses exactly this one place.
pub const GATED_PAPER: PaperParams = PaperParams {
    algo: PaperAlgo::Laid,
    amplitude: 0.020,
    scale: 4.0,
    octaves: 3,
    // The sample board's laid/medium cell seed — SEED_BASE (0x51A7_0000) +
    // cell_seed(row 1, col 2) — so the default IS the gated pixels, bit for bit.
    seed: 0xFBB6_0E9F,
};

impl Default for PaperParams {
    /// The taste-gated pick — see [`GATED_PAPER`].
    fn default() -> Self {
        GATED_PAPER
    }
}

impl Default for Paper {
    /// The gated paper on the canonical stock.
    fn default() -> Self {
        Paper { base_rgb: PAPER_STOCK, params: GATED_PAPER }
    }
}

/// The Surface a quartzite region declares — the M2 capability.
///
/// A region (panel / window / text area) opts in **explicitly**; content
/// composites on top; widgets inherit the material they sit on. The default
/// everywhere remains [`Surface::None`] — adopting paper in tabula/midden/etc.
/// is a future arc per view-owner. `Surface::Paper(Paper::default())` (or
/// [`Surface::paper()`]) is the taste-gated look; future materials (brushed
/// metal, glass, sci-fi panel) are further variants when their arcs come.
///
/// A backend that cannot render a surface well MUST render it as
/// [`Surface::None`] (clean flat) — never a bad approximation.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Surface {
    /// No material — clean flat (the honest zero, and the universal default).
    #[default]
    None,
    /// Procedural paper under the content. `Paper::default()` is the
    /// taste-gated pick ([`GATED_PAPER`] on [`PAPER_STOCK`]).
    Paper(Paper),
}

impl Surface {
    /// The taste-gated paper — what a view gets when it declares paper with no
    /// explicit parameters.
    pub fn paper() -> Self {
        Surface::Paper(Paper::default())
    }

    /// The RGBA8 raster of this surface across `w × h` device pixels, or `None`
    /// for [`Surface::None`] (the backend paints its clean flat and moves on).
    pub fn render_rgba8(&self, w: u32, h: u32) -> Option<Vec<u8>> {
        match self {
            Surface::None => None,
            Surface::Paper(p) => Some(render_rgba8(p, w, h)),
        }
    }
}

/// Rec.709 luminance weights (the perceptual channel the budget is measured in).
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

// ---------------------------------------------------------------------------
// Hash value noise — deterministic, tile-free
// ---------------------------------------------------------------------------

/// Integer hash of a lattice point + seed → `u32`. A small, fast, well-mixed
/// integer hash (PCG-style final mix); the sole entropy source, so every render
/// is bit-for-bit reproducible from its parameters.
#[inline]
fn hash2(xi: i32, yi: i32, seed: u32) -> u32 {
    let mut h = seed
        .wrapping_add((xi as u32).wrapping_mul(0x9E37_79B1))
        .wrapping_add((yi as u32).wrapping_mul(0x85EB_CA77));
    // PCG-style output permutation.
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h
}

/// Hash → float in `[0, 1)`.
#[inline]
fn hash_unit(xi: i32, yi: i32, seed: u32) -> f32 {
    (hash2(xi, yi, seed) >> 8) as f32 / (1u32 << 24) as f32
}

/// Smoothstep (Hermite) fade for value-noise interpolation.
#[inline]
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Bilinear-interpolated value noise at continuous lattice coords, in `[0, 1]`.
#[inline]
fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let xi = x0 as i32;
    let yi = y0 as i32;
    let fx = smooth(x - x0);
    let fy = smooth(y - y0);

    let v00 = hash_unit(xi, yi, seed);
    let v10 = hash_unit(xi + 1, yi, seed);
    let v01 = hash_unit(xi, yi + 1, seed);
    let v11 = hash_unit(xi + 1, yi + 1, seed);

    let a = v00 + (v10 - v00) * fx;
    let b = v01 + (v11 - v01) * fx;
    a + (b - a) * fy
}

/// Fractal Brownian motion (summed octaves) of [`value_noise`], normalised to
/// `[0, 1]`.
#[inline]
fn fbm(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let octaves = octaves.clamp(1, 6);
    let mut sum = 0.0f32;
    let mut amp = 1.0f32;
    let mut norm = 0.0f32;
    let mut fx = x;
    let mut fy = y;
    for o in 0..octaves {
        // A distinct seed per octave keeps the layers uncorrelated.
        sum += amp * value_noise(fx, fy, seed.wrapping_add(o.wrapping_mul(0x68E3_1DA4)));
        norm += amp;
        amp *= 0.5;
        fx *= 2.0;
        fy *= 2.0;
    }
    sum / norm
}

// ---------------------------------------------------------------------------
// The signed field — [-1, 1], before the amplitude scaling
// ---------------------------------------------------------------------------

/// The signed, unit-amplitude paper field at device pixel `(px, py)`.
///
/// Returns a value in `[-1, 1]`; the caller multiplies by
/// [`PaperParams::amplitude`] to get the bounded luminance delta. Pure and
/// side-effect-free — the determinism guarantee lives here.
pub fn field_at(params: &PaperParams, px: f32, py: f32) -> f32 {
    let scale = params.scale.max(1.0);
    let s = params.seed;
    let raw = match params.algo {
        PaperAlgo::Grain => {
            // Emboss: directional gradient of fine value-noise, lit top-left.
            // This is the micro-relief ingredient — tooth you could rub a
            // thumb across, not flat speckle.
            let u = px / scale;
            let v = py / scale;
            let d = 1.0; // one lattice-unit finite difference
            let h = |ax: f32, ay: f32| fbm(ax, ay, s, params.octaves);
            let gx = h(u + d, v) - h(u - d, v);
            let gy = h(u, v + d) - h(u, v - d);
            // Light from the top-left: brighten slopes facing it, darken away.
            // 6.0 maps the small gradients into a useful signed range; the
            // clamp below is the hard ceiling.
            (gx + gy) * 6.0
        }
        PaperAlgo::Laid => {
            // Laid + chain lines, amplitude-modulated by low-freq noise so they
            // read as irregular fibre. Laid pitch = scale; chain lines ~11x
            // coarser and weaker.
            use std::f32::consts::PI;
            let laid = (2.0 * PI * py / scale).sin();
            let chain = (2.0 * PI * px / (scale * 11.0)).sin();
            // Slow noise envelope (in [~0.35, 1]) breaks mechanical regularity.
            let env = 0.35 + 0.65 * fbm(px / (scale * 8.0), py / (scale * 8.0), s, 3);
            (laid * 0.82 + chain * 0.30) * env
        }
        PaperAlgo::Blotch => {
            // Low-frequency cloud: the gentle stock unevenness. Coarse fBm,
            // mean-centred.
            let u = px / (scale * 4.0);
            let v = py / (scale * 4.0);
            (fbm(u, v, s, params.octaves) - 0.5) * 2.0
        }
    };
    raw.clamp(-1.0, 1.0)
}

// ---------------------------------------------------------------------------
// Rasterisation
// ---------------------------------------------------------------------------

/// Render the signed field (`[-1, 1]`) across a `w × h` device-pixel grid,
/// row-major, top row first. Deterministic in the parameters alone.
pub fn render_field(params: &PaperParams, w: u32, h: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; w as usize * h as usize];
    for y in 0..h {
        for x in 0..w {
            out[(y as usize) * w as usize + x as usize] =
                field_at(params, x as f32 + 0.5, y as f32 + 0.5);
        }
    }
    out
}

/// Render an opaque 8-bit sRGB **RGBA** raster of the paper: `base_rgb`
/// modulated by the bounded field, `w × h` device pixels, row-major, top row
/// first (the layout [`crate::platforms::macos`] hands to an `NSBitmapImageRep`).
///
/// `out = base * (1 + amplitude * field)`, per channel, so the relative
/// luminance deviation of every pixel is `amplitude * field ∈ [-amplitude,
/// amplitude]` — the contrast budget, honoured by construction.
pub fn render_rgba8(paper: &Paper, w: u32, h: u32) -> Vec<u8> {
    let amp = paper.params.amplitude.max(0.0);
    let base = paper.base_rgb;
    let mut out = vec![0u8; w as usize * h as usize * 4];
    for y in 0..h {
        for x in 0..w {
            let f = field_at(&paper.params, x as f32 + 0.5, y as f32 + 0.5);
            let m = 1.0 + amp * f;
            let idx = ((y as usize) * w as usize + x as usize) * 4;
            out[idx] = to_u8(base[0] * m);
            out[idx + 1] = to_u8(base[1] * m);
            out[idx + 2] = to_u8(base[2] * m);
            out[idx + 3] = 255;
        }
    }
    out
}

#[inline]
fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Measure the maximum **relative luminance deviation** actually present in a
/// rendered RGBA8 raster versus the base colour — the contrast budget, recovered
/// empirically so a test asserts it rather than trusting the math.
///
/// Returns the max over all pixels of `|L(pixel) - L(base)| / L(base)`.
pub fn measure_max_deviation(base_rgb: [f32; 3], rgba: &[u8], w: u32, h: u32) -> f32 {
    let l0 = luma(base_rgb).max(1e-6);
    let mut max_dev = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            let idx = ((y as usize) * w as usize + x as usize) * 4;
            if idx + 2 >= rgba.len() {
                break;
            }
            let px = [
                rgba[idx] as f32 / 255.0,
                rgba[idx + 1] as f32 / 255.0,
                rgba[idx + 2] as f32 / 255.0,
            ];
            let dev = ((luma(px) - l0) / l0).abs();
            if dev > max_dev {
                max_dev = dev;
            }
        }
    }
    max_dev
}

#[inline]
fn luma(rgb: [f32; 3]) -> f32 {
    rgb[0] * LUMA[0] + rgb[1] * LUMA[1] + rgb[2] * LUMA[2]
}

// ===========================================================================
// TESTS — determinism + the contrast budget, measured (no eye involved)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn params(algo: PaperAlgo, amp: f32) -> PaperParams {
        PaperParams { algo, amplitude: amp, scale: 3.0, octaves: 3, seed: 0x51A7 }
    }

    /// Same parameters ⇒ identical pixels, byte for byte, every algorithm.
    #[test]
    fn deterministic_rgba() {
        for algo in PaperAlgo::ALL {
            let paper = Paper { base_rgb: PAPER_STOCK, params: params(algo, 0.02) };
            let a = render_rgba8(&paper, 128, 96);
            let b = render_rgba8(&paper, 128, 96);
            assert_eq!(a, b, "{} raster not reproducible", algo.label());
        }
    }

    /// The signed field is deterministic per-pixel too (the pure seam).
    #[test]
    fn deterministic_field() {
        for algo in PaperAlgo::ALL {
            let p = params(algo, 0.02);
            let a = render_field(&p, 64, 64);
            let b = render_field(&p, 64, 64);
            assert_eq!(a, b, "{} field not reproducible", algo.label());
        }
    }

    /// Different seeds ⇒ different rasters (no accidental constant / tiling).
    #[test]
    fn seed_decorrelates() {
        for algo in PaperAlgo::ALL {
            let p0 = Paper {
                base_rgb: PAPER_STOCK,
                params: PaperParams { seed: 1, ..params(algo, 0.03) },
            };
            let p1 = Paper {
                base_rgb: PAPER_STOCK,
                params: PaperParams { seed: 2, ..params(algo, 0.03) },
            };
            assert_ne!(
                render_rgba8(&p0, 96, 96),
                render_rgba8(&p1, 96, 96),
                "{} ignores seed",
                algo.label()
            );
        }
    }

    /// The field never escapes `[-1, 1]` (the ceiling the budget rides on).
    #[test]
    fn field_bounded() {
        for algo in PaperAlgo::ALL {
            let p = params(algo, 0.02);
            for f in render_field(&p, 200, 200) {
                assert!((-1.0..=1.0).contains(&f), "{} field out of range: {f}", algo.label());
            }
        }
    }

    /// The MEASURED max relative luminance deviation of a rendered raster never
    /// exceeds the declared amplitude (plus one 8-bit quantisation step). This
    /// is the contrast budget enforced, not asserted by eye — across every
    /// algorithm and every subtlety level the board ships.
    #[test]
    fn contrast_budget_respected() {
        // One 8-bit step relative to paper luminance, as tolerance.
        let l0 = luma(PAPER_STOCK);
        let quant_tol = (1.0 / 255.0) / l0;
        for algo in PaperAlgo::ALL {
            for &amp in &[0.010f32, 0.020, 0.035] {
                let paper = Paper { base_rgb: PAPER_STOCK, params: params(algo, amp) };
                let rgba = render_rgba8(&paper, 256, 256);
                let measured = measure_max_deviation(PAPER_STOCK, &rgba, 256, 256);
                assert!(
                    measured <= amp + quant_tol,
                    "{} @ amp {amp}: measured deviation {measured} exceeds budget (+{quant_tol} tol)",
                    algo.label()
                );
            }
        }
    }

    /// The `Surface::Paper` default IS the taste-gated pick, exactly — the
    /// laid algorithm at the board's medium cell (amp 2.0 %, scale 4 px, oct 3,
    /// the cell's seed), on the canonical stock. Peter, 2026-07-17.
    #[test]
    fn default_params_are_the_gated_pick() {
        let expected = PaperParams {
            algo: PaperAlgo::Laid,
            amplitude: 0.020,
            scale: 4.0,
            octaves: 3,
            // The board's laid/medium cell: SEED_BASE + cell_seed(row 1, col 2)
            // = 0x51A7_0000 + (1·0x9E37_79B1 + 2·0x85EB_CA77) mod 2^32.
            seed: 0x51A7_0000u32
                .wrapping_add(1u32.wrapping_mul(0x9E37_79B1))
                .wrapping_add(2u32.wrapping_mul(0x85EB_CA77)),
        };
        assert_eq!(expected.seed, 0xFBB6_0E9F, "gated seed formula drifted");
        assert_eq!(GATED_PAPER, expected);
        assert_eq!(PaperParams::default(), expected);
        let Surface::Paper(p) = Surface::paper() else {
            panic!("Surface::paper() is not Paper");
        };
        assert_eq!(p.params, expected);
        assert_eq!(p.base_rgb, PAPER_STOCK);
        // And the universal default stays honest zero.
        assert_eq!(Surface::default(), Surface::None);
        assert!(Surface::None.render_rgba8(8, 8).is_none());
    }

    /// A textured raster must actually deviate from flat — the board's cells are
    /// not silently blank (honest zero is the *control* cell's job, not a bug).
    #[test]
    fn texture_is_present() {
        for algo in PaperAlgo::ALL {
            let paper = Paper { base_rgb: PAPER_STOCK, params: params(algo, 0.03) };
            let rgba = render_rgba8(&paper, 256, 256);
            let measured = measure_max_deviation(PAPER_STOCK, &rgba, 256, 256);
            assert!(measured > 0.003, "{} produced no visible texture", algo.label());
        }
    }
}
