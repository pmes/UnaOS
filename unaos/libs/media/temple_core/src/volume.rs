// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// TEMPLE (SR147): THE DARK-MATTER FLOW, a 3D field baked once into a 64^3 grid and sampled with integer trilinear
// interpolation (the same integers on every backend, so the flow is bit-identical in Fx and f32).
//
// THE FIELD. Two potentials `a`, `b` (three-octave value noise over a domain-warped space: the warp is what bends the
// bands into Van Gogh's swirls) and a smooth mass `c` (a low-frequency noise through a smoothstep: clumps and the
// filaments between them). The flow is v = grad(a) x grad(b) — divergence-free, a curl field (v = curl(a grad b)) —
// and its streamlines lie on the level sets of `a` and `b` alike. The renderer draws the level-set BANDS of `a` (the
// streamlines as they cross a sampling shell) broken into strokes by `b` (the dash along the stroke), bright where
// the mass is; `c`'s gradient is the lens.
//
// Per cell one u32: a | b << 8 | c << 16. Precompute is pure integer (Q16 noise), one pass, ~5M noise evaluations.

use alloc::vec::Vec;

pub const GRID: usize = 64;
/// The grid spans [-HALF, HALF]^3 world units (HALF in Q16 below), the outer shell at radius 17 inside it.
pub const HALF_Q16: i64 = 35 << 15; // 17.5
/// World units per cell (Q16): 2 * HALF / (GRID - 1).
pub const STEP_Q16: i64 = (2 * HALF_Q16) / (GRID as i64 - 1);

#[inline(always)]
pub fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = seed ^ (x as u32).wrapping_mul(0x8DA6_B343) ^ (y as u32).wrapping_mul(0xD816_3841) ^ (z as u32).wrapping_mul(0xCB1A_B31F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h
}

/// Value noise at a Q16 point: [0, 65535].
fn vnoise(p: [i64; 3], seed: u32) -> i64 {
    let c = [(p[0] >> 16) as i32, (p[1] >> 16) as i32, (p[2] >> 16) as i32];
    let mut s = [0i64; 3];
    for i in 0..3 {
        let f = p[i] & 0xFFFF;
        s[i] = (f * f * (3 * 65536 - 2 * f)) >> 32;
    }
    let v = |dx: i32, dy: i32, dz: i32| (hash3(c[0] + dx, c[1] + dy, c[2] + dz, seed) >> 16) as i64;
    let l = |a: i64, b: i64, t: i64| a + (((b - a) * t) >> 16);
    let x00 = l(v(0, 0, 0), v(1, 0, 0), s[0]);
    let x10 = l(v(0, 1, 0), v(1, 1, 0), s[0]);
    let x01 = l(v(0, 0, 1), v(1, 0, 1), s[0]);
    let x11 = l(v(0, 1, 1), v(1, 1, 1), s[0]);
    l(l(x00, x10, s[1]), l(x01, x11, s[1]), s[2])
}

/// Three octaves, weights 4:2:1, normalised back to [0, 65535].
fn fbm(p: [i64; 3], seed: u32) -> i64 {
    let a = vnoise(p, seed);
    let b = vnoise([p[0] * 2 + (17 << 16), p[1] * 2, p[2] * 2], seed ^ 0x55);
    let c = vnoise([p[0] * 4, p[1] * 4 + (31 << 16), p[2] * 4], seed ^ 0xAA);
    (a * 4 + b * 2 + c) / 7
}

#[inline(always)]
fn scale(p: [i64; 3], f_q16: i64) -> [i64; 3] {
    [(p[0] * f_q16) >> 16, (p[1] * f_q16) >> 16, (p[2] * f_q16) >> 16]
}

/// `(v - lo) / (hi - lo)` into a byte.
#[inline(always)]
fn stretch(v: i64, lo: i64, hi: i64) -> u32 {
    (((v - lo) * 255) / (hi - lo)).clamp(0, 255) as u32
}

/// One trilinear sample: the potentials and the mass in Q8-of-a-byte (byte * 256), and the mass gradient in
/// mass-units per cell, Q16 (`c` in [0, 1] = byte / 255 ~ byte / 256).
#[derive(Copy, Clone, Debug, Default)]
pub struct Sample {
    pub a: i32,
    pub b: i32,
    pub c: i32,
    pub g: [i32; 3],
}

pub struct Volume {
    pub cells: Vec<u32>,
}

impl Volume {
    /// Bake the field (one pass over the grid; ~0.1-0.3 s on one core).
    pub fn bake() -> Volume {
        let n = GRID * GRID * GRID;
        let mut cells = Vec::with_capacity(n);
        for k in 0..GRID {
            for j in 0..GRID {
                for i in 0..GRID {
                    let p = [-HALF_Q16 + i as i64 * STEP_Q16, -HALF_Q16 + j as i64 * STEP_Q16, -HALF_Q16 + k as i64 * STEP_Q16];
                    // The warp: a slow vector noise displaces the space by up to ~5 units (the swirl).
                    let pw = scale(p, 5900); // 0.09
                    let wx = fbm(pw, 0x1111) - 32768;
                    let wy = fbm([pw[0] + (40 << 16), pw[1], pw[2]], 0x2222) - 32768;
                    let wz = fbm([pw[0], pw[1] + (80 << 16), pw[2]], 0x3333) - 32768;
                    let q = [p[0] + wx * 10, p[1] + wy * 6, p[2] + wz * 10];
                    let a = fbm(scale(q, 10486), 0x4444); // 0.16
                    let b = fbm(scale(q, 29491), 0x5555); // 0.45
                    let m = fbm(scale(p, 4588), 0x6666); // 0.07
                    // The mass: smoothstep(0.40, 0.70) of the slow field — clumps and the filaments between them.
                    let lo = 26214i64;
                    let hi = 45875i64;
                    let t = ((m - lo) * 65536 / (hi - lo)).clamp(0, 65536);
                    let c = (t * t * (3 * 65536 - 2 * t)) >> 32;
                    let cell = stretch(a, 13107, 52428) | stretch(b, 13107, 52428) << 8 | ((((c * 255) >> 16).clamp(0, 255) as u32) << 16);
                    cells.push(cell);
                }
            }
        }
        Volume { cells }
    }

    #[inline(always)]
    fn at(&self, i: usize, j: usize, k: usize) -> u32 {
        self.cells[(k * GRID + j) * GRID + i]
    }

    /// The mass alone at a grid point (Q16 grid coordinates), trilinear — the tests' finite-difference reference.
    pub fn mass_at(&self, g: [i32; 3]) -> i32 {
        self.sample(g).map(|s| s.c).unwrap_or(0)
    }

    /// Sample at Q16 grid coordinates (0 ..= GRID-1 on each axis). `None` outside the grid.
    #[inline(always)]
    pub fn sample(&self, g: [i32; 3]) -> Option<Sample> {
        let (ci, cj, ck) = (g[0] >> 16, g[1] >> 16, g[2] >> 16);
        let top = GRID as i32 - 1;
        if ci < 0 || cj < 0 || ck < 0 || ci >= top || cj >= top || ck >= top {
            return None;
        }
        let (fx, fy, fz) = ((g[0] >> 8) & 255, (g[1] >> 8) & 255, (g[2] >> 8) & 255);
        let (i, j, kk) = (ci as usize, cj as usize, ck as usize);
        let c000 = self.at(i, j, kk);
        let c100 = self.at(i + 1, j, kk);
        let c010 = self.at(i, j + 1, kk);
        let c110 = self.at(i + 1, j + 1, kk);
        let c001 = self.at(i, j, kk + 1);
        let c101 = self.at(i + 1, j, kk + 1);
        let c011 = self.at(i, j + 1, kk + 1);
        let c111 = self.at(i + 1, j + 1, kk + 1);
        // Mass first: an empty neighbourhood has no strokes and no lens (the common case in the void).
        let m = |w: u32| ((w >> 16) & 255) as i32;
        let (m000, m100, m010, m110, m001, m101, m011, m111) = (m(c000), m(c100), m(c010), m(c110), m(c001), m(c101), m(c011), m(c111));
        if (m000 | m100 | m010 | m110 | m001 | m101 | m011 | m111) == 0 {
            return Some(Sample::default());
        }
        // x pass (Q8: byte * 256)
        let lx = |a: i32, b: i32| (a << 8) + (b - a) * fx;
        let l8 = |a: i32, b: i32, f: i32| a + (((b - a) * f) >> 8);
        let cx00 = lx(m000, m100);
        let cx10 = lx(m010, m110);
        let cx01 = lx(m001, m101);
        let cx11 = lx(m011, m111);
        let cxy0 = l8(cx00, cx10, fy);
        let cxy1 = l8(cx01, cx11, fy);
        let c = l8(cxy0, cxy1, fz);
        // The gradient (byte*256 per cell): d/dx from the four x-differences, d/dy and d/dz from the partial lerps.
        let dx0 = l8((m100 - m000) << 8, (m110 - m010) << 8, fy);
        let dx1 = l8((m101 - m001) << 8, (m111 - m011) << 8, fy);
        let gx = l8(dx0, dx1, fz);
        let gy = l8(cx10 - cx00, cx11 - cx01, fz);
        let gz = cxy1 - cxy0;
        let ch = |w: u32, s: u32| ((w >> s) & 255) as i32;
        let tri = |s: u32| {
            let x00 = lx(ch(c000, s), ch(c100, s));
            let x10 = lx(ch(c010, s), ch(c110, s));
            let x01 = lx(ch(c001, s), ch(c101, s));
            let x11 = lx(ch(c011, s), ch(c111, s));
            l8(l8(x00, x10, fy), l8(x01, x11, fy), fz)
        };
        Some(Sample { a: tri(0), b: tri(8), c, g: [gx, gy, gz] })
    }
}
