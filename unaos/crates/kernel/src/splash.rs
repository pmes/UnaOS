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

//! SPLASH-2 — the crystal-cluster boot splash (x86 GUI builds only).
//!
//! v1 of the boot splash: v0's lone equilateral prism read as too literal a Dark Side of the
//! Moon quote and its fan was faint. This version is a *crystal cluster* — three irregular
//! convex shards at varied tilts, like quartz points grown at angles — and the RAYS are the
//! star: a white beam enters the main shard and disperses; the exit fans are wide, haloed,
//! and brightened toward white, and the fan crosses the flanking shards for secondary
//! refractions. Still actual physics: every wavelength sample marches as a ray and bends at
//! every facet crossing by Snell's law with its own refractive index (the only external
//! standard bound here). Q16.16 fixed point in `i64`, no float, no allocation — called
//! pre-heap from `kernel_main`, drawing straight onto the front framebuffer, before the slow
//! bring-up (ACPI/SMP/xHCI) so the panel shows it while boot works. Drawn ONCE; fbcon's
//! QUIET-PANEL milestone lines paint over it (they stay the boot witness surface) and the
//! GUI's own background paint replaces it at handoff. main.rs gates the call off
//! usbdebug/bootlog/witness builds, so test/bench media stay byte-identical.
//!
//! Cost model: one background fill plus the ray march. Rays plot thin strips (a core strip
//! plus a 3x-wide halo strip per 1-px step), so total pixel traffic stays within a few
//! megapixels — the same order as v0 — and boot is not measurably slowed.

#![cfg(target_arch = "x86_64")]

use crate::video::framebuffer::FrameBuffer;
use core::sync::atomic::{AtomicBool, Ordering};
use unaos_boot_info::FrameBufferInfo;

/// SPLASH-SEAMLESS: set once the splash has rendered. While up, `fbcon::milestone` goes
/// serial/ring-only — the QUIET-PANEL milestone lines used to text-paint straight over the
/// crystal (the "jolting flash between splash and midden" metal observation: the VUG-POLISH-2
/// handoff reorder un-garbled the prompt but re-homed the milestone burst onto the splash
/// window). The bootlog ring still records every milestone (the `bootlog` shell verb reads it
/// on-panel) and serial still carries them; only the on-splash text paint is suppressed. A
/// panic is NOT gated on this — `fbcon::panic_screen` repaints its own red backdrop first.
static SPLASH_UP: AtomicBool = AtomicBool::new(false);

/// Whether the splash currently owns the pre-GUI panel (never true on
/// usbdebug/bootlog/witness builds — main.rs gates the paint off them).
pub fn active() -> bool {
    SPLASH_UP.load(Ordering::Relaxed)
}

/// Splash backdrop — near-black, so the beam and spectrum carry the frame.
const SPLASH_BG: u32 = 0x0006_0608;
/// Facet edge line — faint cool grey, drawn last so the crystal reads over the rays.
const SPLASH_EDGE: u32 = 0x004A_4658;
/// Inner facet line — dimmer still.
const SPLASH_FACET: u32 = 0x002C_2936;
/// The white beam (pre-entry).
const SPLASH_BEAM: u32 = 0x00F2_F2EE;

/// Spectrum sample count and colours, red → violet.
const NRAYS: usize = 9;
const SPECTRUM: [u32; NRAYS] = [
    0x00E8_1414, // deep red
    0x00F0_5810, // red-orange
    0x00F8_9008, // orange
    0x00F0_D010, // yellow
    0x0060_D818, // yellow-green
    0x0018_C860, // green
    0x0018_B8C8, // cyan
    0x002E_58E8, // blue
    0x0090_28D8, // violet
];
/// Per-sample refractive index, Q16.16. Physically red bends least; the spread is exaggerated
/// (1.30 → ~1.79) so the fans read at panel size across a room.
const IOR_BASE: i64 = 85197; // 1.30
const IOR_STEP: i64 = 3932; // ~0.06 per sample

/// Q16.16 in i64 (positions are pixels · 65536; 2880-px panels overflow i32 products).
const ONE64: i64 = 1 << 16;

#[inline]
fn fmul64(a: i64, b: i64) -> i64 {
    (a * b) >> 16
}

/// Integer square root of a non-negative i64 (Newton's method).
fn isqrt(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// sqrt of a Q16.16 value, in Q16.16.
#[inline]
fn sqrt_fx(a: i64) -> i64 {
    isqrt(a << 16)
}

/// Normalize a Q16.16 vector to unit length (Q16.16). Returns (0,0) untouched.
fn norm2(x: i64, y: i64) -> (i64, i64) {
    // isqrt(Q16*Q16>>16 = Q16) yields Q8; <<8 gives Q16 length.
    let len = isqrt(fmul64(x, x) + fmul64(y, y)).max(1) << 8;
    ((x << 16) / len, (y << 16) / len)
}

/// Scale an 0x00RRGGBB colour by `num`/256.
#[inline]
fn dim(c: u32, num: u32) -> u32 {
    let r = ((c >> 16) & 0xFF) * num / 256;
    let g = ((c >> 8) & 0xFF) * num / 256;
    let b = (c & 0xFF) * num / 256;
    (r << 16) | (g << 8) | b
}

/// Push an 0x00RRGGBB colour toward white by `num`/256 — the exit-ray "hot core" pop.
#[inline]
fn glow(c: u32, num: u32) -> u32 {
    let r = (c >> 16) & 0xFF;
    let g = (c >> 8) & 0xFF;
    let b = c & 0xFF;
    let r = r + (255 - r) * num / 256;
    let g = g + (255 - g) * num / 256;
    let b = b + (255 - b) * num / 256;
    (r << 16) | (g << 8) | b
}

// ---------------------------------------------------------------------------------------------
// The cluster: three irregular convex shards, like quartz points grown at varied angles.
// Vertex offsets are per-mille of the frame's short side; centres are per-mille of (w, h).
// Each shard must stay CONVEX (the inside test is a per-edge sign test) and the shards must
// not overlap (the march tracks one glass body at a time).
// ---------------------------------------------------------------------------------------------

const MAX_VERTS: usize = 5;

struct Shard {
    /// Centre, per-mille of (width, height).
    c: (i64, i64),
    /// Vertex offsets around the centre, per-mille of min(w, h). y grows downward.
    v: [(i64, i64); MAX_VERTS],
    n: usize,
    /// Inner facet lines, as vertex-index pairs (cosmetic only).
    facets: [(usize, usize); 2],
    nfacets: usize,
}

/// The main shard: a tall five-facet point tilted up-right — the beam's target.
/// The flankers: a slim point to the right (catches the exit fan → secondary fan) and a
/// small stub low-left (catches the downward spread).
const SHARDS: [Shard; 3] = [
    Shard {
        c: (540, 490),
        v: [(90, -430), (235, -160), (170, 320), (-150, 330), (-215, -120)],
        n: 5,
        facets: [(0, 2), (0, 3)],
        nfacets: 2,
    },
    Shard {
        c: (820, 515),
        v: [(-20, -270), (105, -140), (65, 190), (-95, 130), (0, 0)],
        n: 4,
        facets: [(0, 2), (0, 0)],
        nfacets: 1,
    },
    Shard {
        c: (330, 760),
        v: [(-105, -140), (45, -195), (95, 85), (-55, 140), (0, 0)],
        n: 4,
        facets: [(1, 3), (0, 0)],
        nfacets: 1,
    },
];

/// A shard resolved to Q16.16 pixel space.
struct Poly {
    v: [(i64, i64); MAX_VERTS],
    n: usize,
    /// Per-edge centroid sign (+1/-1): a point is inside iff every edge function matches.
    sign: [i64; MAX_VERTS],
}

impl Poly {
    /// Signed edge function for edge i (v[i] → v[(i+1)%n]) at point p, Q16.16·px scale.
    #[inline]
    fn edge_fn(&self, i: usize, px: i64, py: i64) -> i64 {
        let p1 = self.v[i];
        let p2 = self.v[(i + 1) % self.n];
        fmul64(p2.0 - p1.0, py - p1.1) - fmul64(p2.1 - p1.1, px - p1.0)
    }

    /// Per-edge inside flags at p (all true = inside the shard).
    fn flags(&self, px: i64, py: i64) -> [bool; MAX_VERTS] {
        let mut f = [true; MAX_VERTS];
        for i in 0..self.n {
            f[i] = self.edge_fn(i, px, py) * self.sign[i] >= 0;
        }
        f
    }

    #[inline]
    fn inside(f: &[bool; MAX_VERTS]) -> bool {
        f.iter().all(|&b| b)
    }
}

/// SPLASH-2 — paint the crystal-cluster splash once onto the front framebuffer.
pub fn boot_splash(base: usize, len: usize, info: FrameBufferInfo) {
    let mut fb = FrameBuffer::new();
    fb.init(base, len, info);
    if !fb.is_ready() {
        return;
    }
    let w = fb.width() as i64;
    let h = fb.height() as i64;
    let s = w.min(h);
    fb.fill_screen(SPLASH_BG);

    // --- resolve the cluster to Q16.16 pixel space -------------------------------------------
    let mut polys: [Poly; 3] = [
        Poly { v: [(0, 0); MAX_VERTS], n: 0, sign: [1; MAX_VERTS] },
        Poly { v: [(0, 0); MAX_VERTS], n: 0, sign: [1; MAX_VERTS] },
        Poly { v: [(0, 0); MAX_VERTS], n: 0, sign: [1; MAX_VERTS] },
    ];
    for (pi, sh) in SHARDS.iter().enumerate() {
        let cx = (w * sh.c.0 / 1000) << 16;
        let cy = (h * sh.c.1 / 1000) << 16;
        let p = &mut polys[pi];
        p.n = sh.n;
        for i in 0..sh.n {
            p.v[i] = (cx + ((s * sh.v[i].0 / 1000) << 16), cy + ((s * sh.v[i].1 / 1000) << 16));
        }
        // Centroid fixes each edge's inside sign.
        let (mut gx, mut gy) = (0i64, 0i64);
        for i in 0..sh.n {
            gx += p.v[i].0;
            gy += p.v[i].1;
        }
        gx /= sh.n as i64;
        gy /= sh.n as i64;
        for i in 0..sh.n {
            p.sign[i] = if p.edge_fn(i, gx, gy) >= 0 { 1 } else { -1 };
        }
    }

    // --- the beam: from the left edge, rising toward the main shard's heart -----------------
    let start = (0i64, ((h * 78) / 100) << 16);
    let aim = (
        polys[0].v[3].0 + (polys[0].v[0].0 - polys[0].v[3].0) * 55 / 100,
        polys[0].v[3].1 + (polys[0].v[0].1 - polys[0].v[3].1) * 55 / 100,
    );
    let (dx0, dy0) = norm2(aim.0 - start.0, aim.1 - start.1);

    let th = ((h / 200).max(3)) as usize; // ray core thickness in px
    let halo = 3 * th; // soft glow width
    let max_steps = (3 * (w + h)) as usize;

    // Per-step strip plot: consecutive steps advance ~1 px, so a 1-px-thick strip laid across
    // the travel direction tiles into a solid band `wd` wide with no per-step overdraw.
    let strip = |fb: &FrameBuffer, px: i64, py: i64, dx: i64, dy: i64, wd: usize, c: u32| {
        let x = px >> 16;
        let y = py >> 16;
        if x < -64 || y < -64 {
            return;
        }
        let half = (wd / 2) as i64;
        if dx.abs() >= dy.abs() {
            fb.fill_rect(x.max(0) as usize, (y - half).max(0) as usize, 1, wd, c);
        } else {
            fb.fill_rect((x - half).max(0) as usize, y.max(0) as usize, wd, 1, c);
        }
    };

    // Two passes: pass 0 lays every ray's wide dim halo, pass 1 lays every bright core over
    // the halos — an additive-glow read without framebuffer read-back (VRAM reads are slow on
    // metal). The march is retraced per pass; it is cheap.
    let mut first_entry: Option<(i64, i64, i64, i64)> = None; // (px, py, rdx, rdy) reflection
    for pass in 0..2 {
        for k in 0..NRAYS {
            let ior = IOR_BASE + IOR_STEP * (k as i64);
            let (mut px, mut py) = start;
            let (mut dx, mut dy) = (dx0, dy0);
            let mut glass: Option<usize> = None; // which shard the ray is inside
            let mut entered = false;
            let mut was = [[true; MAX_VERTS]; 3];
            for (pi, p) in polys.iter().enumerate() {
                was[pi] = p.flags(px, py);
            }

            for _ in 0..max_steps {
                px += dx;
                py += dy;
                if px < -(64 << 16)
                    || px > (w + 64) << 16
                    || py < -(64 << 16)
                    || py > (h + 64) << 16
                {
                    break;
                }
                for (pi, p) in polys.iter().enumerate() {
                    let now = p.flags(px, py);
                    let inside_now = Poly::inside(&now);
                    let was_inside = glass == Some(pi);
                    if inside_now != was_inside {
                        // Crossed a facet of shard pi: which edge flipped?
                        let mut ei = 0;
                        for i in 0..p.n {
                            if now[i] != was[pi][i] {
                                ei = i;
                                break;
                            }
                        }
                        // Facet normal (unit, Q16.16), oriented against the ray (n·d < 0).
                        let p1 = p.v[ei];
                        let p2 = p.v[(ei + 1) % p.n];
                        let (mut nx, mut ny) = norm2(p2.1 - p1.1, -(p2.0 - p1.0));
                        if fmul64(nx, dx) + fmul64(ny, dy) > 0 {
                            nx = -nx;
                            ny = -ny;
                        }
                        // Snell: eta = n1/n2 for this crossing.
                        let eta = if inside_now { (ONE64 << 16) / ior } else { ior };
                        let cosi = -(fmul64(nx, dx) + fmul64(ny, dy));
                        let kk =
                            ONE64 - fmul64(fmul64(eta, eta), ONE64 - fmul64(cosi, cosi));
                        if kk < 0 {
                            // Total internal reflection: bounce, stay inside.
                            dx += 2 * fmul64(cosi, nx);
                            dy += 2 * fmul64(cosi, ny);
                            let (ndx, ndy) = norm2(dx, dy);
                            dx = ndx;
                            dy = ndy;
                        } else {
                            if inside_now && !entered && first_entry.is_none() {
                                // Remember the partial-reflection sparkle off the entry facet.
                                let rdx = dx + 2 * fmul64(cosi, nx);
                                let rdy = dy + 2 * fmul64(cosi, ny);
                                first_entry = Some((px, py, rdx, rdy));
                            }
                            let t = fmul64(eta, cosi) - sqrt_fx(kk);
                            dx = fmul64(eta, dx) + fmul64(t, nx);
                            dy = fmul64(eta, dy) + fmul64(t, ny);
                            let (ndx, ndy) = norm2(dx, dy);
                            dx = ndx;
                            dy = ndy;
                            glass = if inside_now { Some(pi) } else { None };
                            if glass.is_some() {
                                entered = true;
                            }
                        }
                    }
                    was[pi] = now;
                }

                // Plot. Pre-entry: the shared white beam once (k == 0). Inside glass: the
                // sample's colour, dimmed — the fan is already diverging. After exit: full
                // colour with a white-hot core over a wide halo — the star of the frame.
                if entered {
                    let ing = glass.is_some();
                    if pass == 0 {
                        let hcol = dim(SPECTRUM[k], if ing { 40 } else { 90 });
                        strip(&fb, px, py, dx, dy, halo, hcol);
                    } else {
                        let ccol =
                            if ing { dim(SPECTRUM[k], 150) } else { glow(SPECTRUM[k], 70) };
                        strip(&fb, px, py, dx, dy, th, ccol);
                    }
                } else if k == 0 {
                    if pass == 0 {
                        strip(&fb, px, py, dx, dy, halo, dim(SPLASH_BEAM, 60));
                    } else {
                        strip(&fb, px, py, dx, dy, th, SPLASH_BEAM);
                    }
                }
            }
        }
    }

    // Partial-reflection sparkle off the entry facet: one faint white streak.
    if let Some((ex, ey, rdx, rdy)) = first_entry {
        let (rdx, rdy) = norm2(rdx, rdy);
        let far = 3 * (w + h);
        fb.draw_line(
            (ex >> 16) as i32,
            (ey >> 16) as i32,
            ((ex >> 16) + fmul64(rdx, far << 16) / 65536) as i32,
            ((ey >> 16) + fmul64(rdy, far << 16) / 65536) as i32,
            dim(SPLASH_BEAM, 70),
        );
    }

    // The crystal itself, last: faint outer facet edges + dimmer inner facet lines, so the
    // glass reads over the rays without hiding them.
    for (pi, p) in polys.iter().enumerate() {
        for i in 0..p.n {
            let a = p.v[i];
            let b = p.v[(i + 1) % p.n];
            fb.draw_line(
                (a.0 >> 16) as i32,
                (a.1 >> 16) as i32,
                (b.0 >> 16) as i32,
                (b.1 >> 16) as i32,
                SPLASH_EDGE,
            );
        }
        let sh = &SHARDS[pi];
        for f in 0..sh.nfacets {
            let (i, j) = sh.facets[f];
            fb.draw_line(
                (p.v[i].0 >> 16) as i32,
                (p.v[i].1 >> 16) as i32,
                (p.v[j].0 >> 16) as i32,
                (p.v[j].1 >> 16) as i32,
                SPLASH_FACET,
            );
        }
    }

    // SPLASH-SEAMLESS: from here until the GUI's first frame, nothing text-paints over the
    // crystal (fbcon::milestone checks this flag; see its doc for the metal defect).
    SPLASH_UP.store(true, Ordering::Relaxed);

    serial_println!(":: SPLASH: crystal cluster traced — 3 shards, {} spectrum rays ::", NRAYS);
}
