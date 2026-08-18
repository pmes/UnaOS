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

    // SPLASH-ALIVE: publish the framebuffer handle and arm the animation, as the LAST act of the
    // paint so no baseline statement above it shifts line. From here each boot milestone
    // (`bootpace::record`) drives one `advance()` frame until the `gui` stamp latches it off just
    // before the desktop's first paint. Gated off usbdebug/bootlog/witness — see the SPLASH-ALIVE
    // block at the foot of this file for why every addition is placed and gated the way it is.
    #[cfg(not(any(feature = "usbdebug", feature = "bootlog", feature = "witness")))]
    {
        *SPLASH_FB.lock() = Some(fb);
        ANIM.store(true, Ordering::Relaxed);
    }
}

// =================================================================================================
// SPLASH-ALIVE — the crystal breathes during the boot wait.
//
// The base frame (fans + shards) is painted ONCE by `boot_splash`; from then on `advance()` is
// called from the boot-milestone seam (`bootpace::record`) and does a CHEAP per-frame partial
// redraw: a moving light source (`COS_Q8` LUT, one 11.25° step per milestone) sweeps specular
// GLINTS along the crystal's facet edges, and the beam-entry facet throbs. The milestone stamps are
// densest exactly where the boot waits (the M4 xHCI subdivision — a dozen stamps through
// `pci::init`), so the crystal is liveliest during the longest bring-up wait.
//
// FRAME-DRIVER CHOICE (the load-bearing question): milestone-driven, NOT a TSC frame loop or an
// APIC-timer callback. The pre-heap bring-up runs single-threaded on the BSP with no yield point, so
// a frame loop cannot let bring-up proceed, and a periodic timer callback would touch the
// interrupt/APIC path (outside this lane) and race the TSC calibration. Driving one cheap frame off
// each `bootpace::record` advances the crystal WITHOUT adding any wall clock of its own — the stamp
// already happened; we borrow it. Each frame touches only a few thousand facet-edge pixels
// (kilopixels), far below the one-time `fill_screen` the base paint already pays, so `gui=` on the
// BPACE total line does not move.
//
// SEAMLESSNESS: every moving highlight rides ON a facet-edge locus, and each frame's FIRST act per
// edge is to repaint that whole edge in `SPLASH_EDGE` (the eraser) — so last frame's glint is
// overwritten exactly, with no cached backbuffer (there is no heap yet) and no ghosting. Animation
// LATCHES OFF at the `gui` stamp, which both handoff paths record BEFORE the desktop's first paint,
// so no glint frame ever lands over the GUI and the SPLASH-SEAMLESS contract holds. A panic still
// repaints its own screen (it never consults this module).
//
// BYTE-IDENTITY: every item below is gated OFF for usbdebug/bootlog/witness, uses fully-qualified
// paths (no new `use` line), and lives at the FOOT of the file after `boot_splash`. So for those
// three builds this file's post-cfg token stream — and every baseline line number — is unchanged,
// and the kernel the test/bench media carries is byte-identical to baseline (verified: `.text`,
// `.rodata` and the stripped image all hash-match).

/// The initialised front-framebuffer handle captured by `boot_splash`, so `advance()` can repaint
/// without re-deriving it. `FrameBuffer` is `Copy`; the `Mutex` only guards the one-time publish and
/// gives `advance()` a `try_lock` bail against any (theoretical) re-entrant milestone.
#[cfg(not(any(feature = "usbdebug", feature = "bootlog", feature = "witness")))]
static SPLASH_FB: spin::Mutex<Option<FrameBuffer>> = spin::Mutex::new(None);

/// Set once the base frame is up; cleared at the `gui` handoff stamp. While true, milestone stamps
/// drive one animation frame each. Never armed on usbdebug/bootlog/witness (`boot_splash` is gated
/// off there, so this stays false and `advance()` is a single-load no-op).
#[cfg(not(any(feature = "usbdebug", feature = "bootlog", feature = "witness")))]
static ANIM: AtomicBool = AtomicBool::new(false);

/// Monotonic frame counter — the animation's whole time base. Deterministic (no TSC read needed):
/// the light angle is `PHASE` LUT steps and each glint's crawl offset is a function of `PHASE`.
#[cfg(not(any(feature = "usbdebug", feature = "bootlog", feature = "witness")))]
static PHASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// cos(2π·i/32) · 256, i in 0..32 — the fixed-point light-direction table (no float, pre-heap).
/// `sin(i) = COS_Q8[(i + 24) & 31]`.
#[cfg(not(any(feature = "usbdebug", feature = "bootlog", feature = "witness")))]
const COS_Q8: [i32; 32] = [
    256, 251, 237, 213, 181, 142, 98, 50, 0, -50, -98, -142, -181, -213, -237, -251, -256, -251,
    -237, -213, -181, -142, -98, -50, 0, 50, 98, 142, 181, 213, 237, 251,
];

/// Pixel length of a facet edge (Q16.16 endpoints → whole pixels).
#[cfg(not(any(feature = "usbdebug", feature = "bootlog", feature = "witness")))]
#[inline]
fn edge_steps(a: (i64, i64), b: (i64, i64)) -> i64 {
    ((b.0 - a.0).abs().max((b.1 - a.1).abs())) >> 16
}

/// Paint the sub-run `[i0, i1]` (in whole-pixel parameter, clamped to `[0, steps]`) of the facet
/// edge `a → b`, in `color`. A DETERMINISTIC parametric sampler: the pixel at parameter `i` is the
/// exact lerp of the two Q16.16 endpoints, so a sub-run traces a strict subset of the full edge's
/// pixels. That is the seamlessness guarantee — repainting the WHOLE edge in `SPLASH_EDGE` erases
/// any previous glint exactly, because both used this same locus. `put_pixel` clips to the panel.
#[cfg(not(any(feature = "usbdebug", feature = "bootlog", feature = "witness")))]
fn edge_run(fb: &FrameBuffer, a: (i64, i64), b: (i64, i64), i0: i64, i1: i64, color: u32) {
    let steps = edge_steps(a, b);
    if steps <= 0 {
        let (x, y) = (a.0 >> 16, a.1 >> 16);
        if x >= 0 && y >= 0 {
            fb.put_pixel(x as usize, y as usize, color);
        }
        return;
    }
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let hi = i1.clamp(0, steps);
    let mut i = i0.clamp(0, steps);
    while i <= hi {
        let x = (a.0 + dx * i / steps) >> 16;
        let y = (a.1 + dy * i / steps) >> 16;
        if x >= 0 && y >= 0 {
            fb.put_pixel(x as usize, y as usize, color);
        }
        i += 1;
    }
}

/// Resolve one shard's vertices to Q16.16 pixel space for the panel `(w, h, s)` — the same mapping
/// `boot_splash` uses, factored out so `advance()` can re-derive the facet edges each frame without
/// caching a `Poly` (the earliest frames predate the heap).
#[cfg(not(any(feature = "usbdebug", feature = "bootlog", feature = "witness")))]
fn resolve_verts(sh: &Shard, w: i64, h: i64, s: i64) -> [(i64, i64); MAX_VERTS] {
    let cx = (w * sh.c.0 / 1000) << 16;
    let cy = (h * sh.c.1 / 1000) << 16;
    let mut v = [(0i64, 0i64); MAX_VERTS];
    for i in 0..sh.n {
        v[i] = (cx + ((s * sh.v[i].0 / 1000) << 16), cy + ((s * sh.v[i].1 / 1000) << 16));
    }
    v
}

/// SPLASH-ALIVE — advance the living crystal by one frame, driven from a boot milestone.
///
/// Called from `bootpace::record` (gated OFF for usbdebug/bootlog/witness). A no-op unless the base
/// frame is up, and it LATCHES OFF permanently at the `gui` handoff stamp so nothing ever paints
/// over the desktop. Cheap by construction: per facet edge it repaints the edge (the eraser) and
/// lays one short bright glint whose position crawls with `PHASE` and whose brightness is the
/// specular alignment of that facet with the sweeping light — the beam-entry facet also throbs.
#[cfg(not(any(feature = "usbdebug", feature = "bootlog", feature = "witness")))]
pub fn advance(tag: &str) {
    if !ANIM.load(Ordering::Relaxed) {
        return;
    }
    if tag == "gui" {
        // The GUI is taking the panel (recorded before its first paint on both handoff paths).
        ANIM.store(false, Ordering::Relaxed);
        return;
    }
    let fb = match SPLASH_FB.try_lock() {
        Some(g) => match *g {
            Some(fb) => fb,
            None => return,
        },
        None => return, // a re-entrant milestone owns the frame; skip this one.
    };

    let p = PHASE.fetch_add(1, Ordering::Relaxed) as i64;
    // Light direction: one 11.25° LUT step per milestone — a slow sweep across the whole bring-up.
    let lx = COS_Q8[(p & 31) as usize] as i64;
    let ly = COS_Q8[((p + 24) & 31) as usize] as i64;

    let w = fb.width() as i64;
    let h = fb.height() as i64;
    let s = w.min(h);

    for (pi, sh) in SHARDS.iter().enumerate() {
        let v = resolve_verts(sh, w, h, s);
        // The beam enters the main shard on its left face — the edge with the leftmost midpoint.
        // That facet gets the entry pulse ("the beam pulsing as it enters", Peter's word).
        let entry_edge = if pi == 0 {
            let mut best = 0usize;
            let mut bx = i64::MAX;
            for i in 0..sh.n {
                let mx = (v[i].0 + v[(i + 1) % sh.n].0) / 2;
                if mx < bx {
                    bx = mx;
                    best = i;
                }
            }
            best
        } else {
            usize::MAX
        };

        for i in 0..sh.n {
            let a = v[i];
            let b = v[(i + 1) % sh.n];
            // Eraser + crystal line: repaint the whole facet edge first (overwrites last glint).
            edge_run(&fb, a, b, 0, i64::MAX, SPLASH_EDGE);

            // Specular alignment of this facet with the light (sharpened to a glint).
            let (nx, ny) = norm2(b.1 - a.1, -(b.0 - a.0));
            let dot = ((nx >> 8) * lx + (ny >> 8) * ly) >> 8; // Q8, ~[-256, 256]
            let d = dot.max(0);
            let mut inten = (d * d) >> 8; // 0..256
            inten = (inten * d) >> 8; // cubic → a tight, sparkly highlight
            if pi == 0 && i == entry_edge {
                // Triangle throb on the entry facet, independent of the sweep.
                let tri = p & 15;
                let pulse = if tri < 8 { tri } else { 15 - tri }; // 0..7
                inten = (inten + pulse * 28).min(255);
            }
            if inten <= 10 {
                continue; // this facet is edge-on to the light this frame — no glint.
            }
            let steps = edge_steps(a, b);
            if steps <= 2 {
                continue;
            }
            // The glint crawls along the facet as PHASE advances; each edge is offset so the
            // sparkles do not march in lockstep.
            let seed = (pi as i64 * 5 + i as i64) * 17;
            let g = (((p * 3 + seed) % steps) + steps) % steps;
            let gl = (steps / 6).max(4);
            edge_run(&fb, a, b, g - gl / 2, g + gl / 2, dim(0x00FF_FFFF, inten as u32));
        }
    }
}
