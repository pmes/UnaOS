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

// SPLASHX86 (R74): arch-neutral ray tracer; only the animation tail (ANIM/advance/retire) stays x86.

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
    SPLASH_UP.load(Ordering::Relaxed) || HOLD_GLASS.load(Ordering::Relaxed) // SPLASH2: the takeover blit keeps the glass owned until hold_release
}
/// SPLASH2: true from the takeover blit (the splash IS the glass) until `hold_release`.
static HOLD_GLASS: AtomicBool = AtomicBool::new(false);

/// Splash backdrop — near-black, so the beam and spectrum carry the frame.
const SPLASH_BG: u32 = crate::video::theme::painter::SPLASH_BG;
/// Facet edge line — faint cool grey, drawn last so the crystal reads over the rays.
const SPLASH_EDGE: u32 = crate::video::theme::painter::SPLASH_EDGE;
/// Inner facet line — dimmer still.
const SPLASH_FACET: u32 = crate::video::theme::painter::SPLASH_FACET;
/// The white beam (pre-entry).
const SPLASH_BEAM: u32 = crate::video::theme::painter::SPLASH_BEAM;

/// Spectrum sample count and colours, red → violet.
const NRAYS: usize = 9;
const SPECTRUM: [u32; NRAYS] = [
    crate::video::theme::painter::SPLASH_SPECTRUM[0], // deep red
    crate::video::theme::painter::SPLASH_SPECTRUM[1], // red-orange
    crate::video::theme::painter::SPLASH_SPECTRUM[2], // orange
    crate::video::theme::painter::SPLASH_SPECTRUM[3], // yellow
    crate::video::theme::painter::SPLASH_SPECTRUM[4], // yellow-green
    crate::video::theme::painter::SPLASH_SPECTRUM[5], // green
    crate::video::theme::painter::SPLASH_SPECTRUM[6], // cyan
    crate::video::theme::painter::SPLASH_SPECTRUM[7], // blue
    crate::video::theme::painter::SPLASH_SPECTRUM[8], // violet
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
pub fn boot_splash(base: usize, len: usize, info: FrameBufferInfo) { paint(base, len, info, true); } fn paint(base: usize, len: usize, info: FrameBufferInfo, arm: bool) {
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
    if arm { SPLASH_UP.store(true, Ordering::Relaxed); }

    if arm { serial_println!(":: SPLASH: crystal cluster traced — 3 shards, {} spectrum rays ::", NRAYS); }

    // SPLASH-ALIVE: publish the framebuffer handle and arm the animation, as the LAST act of the
    // paint so no baseline statement above it shifts line. From here each boot milestone
    // (`bootpace::record`) drives one `advance()` frame until the `gui` stamp latches it off just
    // before the desktop's first paint. Gated off usbdebug/bootlog/witness — see the SPLASH-ALIVE
    // block at the foot of this file for why every addition is placed and gated the way it is.
    #[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "bootlog", feature = "witness"))))]
    {
        if arm { *SPLASH_FB.lock() = Some(fb); }
        if arm { ANIM.store(true, Ordering::Relaxed); }
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
#[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "bootlog", feature = "witness"))))]
static SPLASH_FB: crate::sync::Mutex<Option<FrameBuffer>> = crate::sync::Mutex::new(None);

/// Set once the base frame is up; cleared at the `gui` handoff stamp. While true, milestone stamps
/// drive one animation frame each. Never armed on usbdebug/bootlog/witness (`boot_splash` is gated
/// off there, so this stays false and `advance()` is a single-load no-op).
#[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "bootlog", feature = "witness"))))]
static ANIM: AtomicBool = AtomicBool::new(false);

/// Monotonic frame counter — the animation's whole time base. Deterministic (no TSC read needed):
/// the light angle is `PHASE` LUT steps and each glint's crawl offset is a function of `PHASE`.
#[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "bootlog", feature = "witness"))))]
static PHASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// cos(2π·i/32) · 256, i in 0..32 — the fixed-point light-direction table (no float, pre-heap).
/// `sin(i) = COS_Q8[(i + 24) & 31]`.
#[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "bootlog", feature = "witness"))))]
const COS_Q8: [i32; 32] = [
    256, 251, 237, 213, 181, 142, 98, 50, 0, -50, -98, -142, -181, -213, -237, -251, -256, -251,
    -237, -213, -181, -142, -98, -50, 0, 50, 98, 142, 181, 213, 237, 251,
];

/// Pixel length of a facet edge (Q16.16 endpoints → whole pixels).
#[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "bootlog", feature = "witness"))))]
#[inline]
fn edge_steps(a: (i64, i64), b: (i64, i64)) -> i64 {
    ((b.0 - a.0).abs().max((b.1 - a.1).abs())) >> 16
}

/// Paint the sub-run `[i0, i1]` (in whole-pixel parameter, clamped to `[0, steps]`) of the facet
/// edge `a → b`, in `color`. A DETERMINISTIC parametric sampler: the pixel at parameter `i` is the
/// exact lerp of the two Q16.16 endpoints, so a sub-run traces a strict subset of the full edge's
/// pixels. That is the seamlessness guarantee — repainting the WHOLE edge in `SPLASH_EDGE` erases
/// any previous glint exactly, because both used this same locus. `put_pixel` clips to the panel.
#[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "bootlog", feature = "witness"))))]
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
#[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "bootlog", feature = "witness"))))]
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
#[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "bootlog", feature = "witness"))))]
pub fn advance(tag: &str) { #[cfg(feature = "wc")] if ANIM.load(Ordering::Relaxed) && crate::video::fbcon::panel_console_live() { retire("video/fbcon.rs panel_console_resume (the Kepler takeover's full-panel clear; under wc that clear is the compositor's ignition)"); return; } // SPLASHRETIRE (B137) ⚠ SAME-LINE fold. THE BROKEN CRYSTAL, and this is the line that ends it. `ANIM` used to latch off ONLY at the `gui` stamp (below), which `main.rs` records ~500 lines and one whole boot phase AFTER the panel has already been taken: `pci::init` -> `kepler::takeover_display` -> `fbcon::panel_console_resume` clears the WHOLE panel and starts mirroring console glyphs onto it, and under `wc` `desktop_uefi::activate_on` then fills it with `DESKTOP_BG` and composites the console window and the menu bar. The very next `bootpace::record` after that seam is `pci-usb` — flight 11 measured it at t=28036 ms against `[wc-x] desktop-clear` at 27428 ms and `gui` at 28059 ms — and the frame it drove painted, through the pre-heap front-framebuffer handle captured at `SPLASH_FB`, NOTHING BUT FACET EDGES: `edge_run(… SPLASH_EDGE)` per edge plus a glint, with no background fill, no fans and no spectrum. A crystal's wireframe laid over a live desktop with none of the body that made it read as a crystal is exactly "a broken crystal", and it is the LAST thing painted before the desktop's first paint. Asking the seam here, before any pixel moves, is the whole fix; nothing about the crystal's LOOK is touched.
    if !ANIM.load(Ordering::Relaxed) {
        return;
    }
    if tag == "gui" {
        // The GUI is taking the panel (recorded before its first paint on both handoff paths).
        #[cfg(feature = "wc")] retire("bootpace gui stamp (main.rs, before the desktop's first paint)"); ANIM.store(false, Ordering::Relaxed); // SPLASHRETIRE (B137) ⚠ SAME-LINE fold. THE BACKSTOP, not the fix: on every boot that reaches the Kepler seam the fold at `advance`'s entry has already retired the splash and this arm never runs. It is kept, and now witnessed, for the boot that reaches `gui` WITHOUT that seam — so that `SPLASH_UP` is cleared on every path that exists rather than on the one this arc happened to measure, and so the retirement always names the site that did it. `retire` is idempotent: the second caller prints nothing.
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
            edge_run(&fb, a, b, g - gl / 2, g + gl / 2, dim(crate::video::theme::painter::SPLASH_GLINT, inten as u32));
        }
    }
}

// =================================================================================================
// SPLASHRETIRE (B137) — the splash is HANDED OVER, not merely stopped.
//
// Before this arc `SPLASH_UP` had exactly one writer in the whole tree — `store(true)` at the foot
// of `boot_splash` — and no reader ever saw it go false. Two things followed, and both are defects
// rather than curiosities:
//
//   1. `fbcon::milestone` consults `splash::active()` and returns early while it is true. Since it
//      never became false, the QUIET-PANEL on-panel milestone leg was DEAD from the splash paint
//      onward on every x86 GUI boot — including `gui:handoff`, the last milestone there is. The
//      suppression was written as "while the crystal owns the panel"; it silently became "forever".
//   2. Nothing marked the instant the crystal STOPPED owning the panel, so `advance` kept painting
//      past it. That is the broken crystal (see the fold at `advance`'s entry).
//
// `retire` gives the flag its missing second writer and makes the handover an EVENT with a time and
// an author, which is what `:: SPLASH: retired at <ms> ms by <site> ::` is. The `<site>` field is
// load-bearing and is the whole reason this is a function rather than two stores: the mutation test
// for this arc is to re-introduce a paint after the seam, and a witness that only said "retired"
// would not name which site took the glass back.
//
// IDEMPOTENT, and it has to be. Two callers exist (the seam fold and the `gui` backstop) and on a
// Kepler boot both are reached; the second must not print a second, later, wrong retirement time.
// The `swap` is the latch — only the caller that observed `true` prints.
//
// BYTE-IDENTITY. `wc`-gated, so on a knob-off build (`default = []`) this function and both of its
// call sites are erased and the image cannot move — `./arroyo knoboff bt` / `btc` would otherwise
// convict a splash change of being a Bluetooth change. Appended at the FOOT of the file, after
// `advance`, so no existing `panic::Location` in `splash.rs` shifts; both call sites are SAME-LINE
// folds for the same reason (B94).
//
// WHAT IS NOT PROVEN HERE, said plainly rather than left for a reader to assume. `./arroyo test`
// force-arms `UNAOS_WITNESS=1` (`arroyo`, the `case` at the head of the file), and `witness`
// compiles the `boot_splash` CALL out of `main.rs` altogether — so no battery run has ever had a
// splash on its panel, and QEMU has no Kepler, so `panel_console_resume` is never reached there
// either. The gate for this line is the artifact grep plus a metal sitting. That gap is B137's
// queue row, not something this file can close.

/// SPLASHRETIRE — hand the panel over: latch the animation off, clear `SPLASH_UP`, and witness the
/// instant and the site that took the glass. Idempotent; only the first caller prints.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn retire(site: &str) {
    if !SPLASH_UP.swap(false, Ordering::Relaxed) {
        return;
    }
    #[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "bootlog", feature = "witness"))))]
    ANIM.store(false, Ordering::Relaxed);
    serial_println!(":: SPLASH: retired at {} ms by {} ::", crate::arch::ms(), site);
}

// =================================================================================================
// SPLASHX86 (R74 + FIRSTBOOT) — the splash HOLDS the glass from the compositor takeover until the
// installer stage is known.
//
// Metal boot 17: `activate_on` mints the bar and the console ~430 ms BEFORE the users store loads, so a
// fresh card flashed desktop furniture and then swept it (`[login] installer: furniture swept n=2`). The
// pre-GUI splash above is retired by the takeover's own panel clear; this block re-renders the SAME ray
// tracer into a panel-sized RAM surface and parks it as a chromeless, input-less compat row
// (`wm::splash_open`), pinned topmost with `wm::set_modal_top` (the LOGINZ ceiling), until
// `users::stage_publish` (from `stage_resolve` / `stage_no_store` / `desktop_allowed`) releases it —
// or `hold_service` does, 5 s after it opened. Furniture may mint beneath it; it only has to be ABOVE.
// Same code path on both arches (x86 `desktop_uefi::activate_on`, aarch64 `desktop_firmware::activate`).

/// The compat row holding the glass (0 = none).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
static HOLD_WIN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// `arch::ms()` when the row opened.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
static HOLD_T0: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The rendered surface (xRGB, panel-sized, never grows) and its geometry, from `hold_prepare` until `hold_release`.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
static HOLD_SURF: crate::sync::Mutex<Option<(alloc::vec::Vec<u8>, usize, usize)>> = crate::sync::Mutex::new(None);

#[cfg(target_arch = "x86_64")]
const ARCH_NAME: &str = "x86_64";
#[cfg(target_arch = "aarch64")]
const ARCH_NAME: &str = "aarch64";

/// Render the splash into a panel-sized RAM surface. Called BEFORE the takeover's desktop-clear so the
/// glass is bare for as short a time as possible. Silent no-op once the stage is already known.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn hold_prepare() {
    if HOLD_SURF.lock().is_some() || HOLD_WIN.load(Ordering::Relaxed) != 0 {
        return;
    }
    #[cfg(feature = "login")]
    if crate::fs::users::stage_resolved() {
        serial_println!("[splash] hold SKIP reason=stage-known");
        return;
    }
    let (w, h) = {
        let fb = *crate::video::WRITER.lock();
        if !fb.is_ready() {
            return;
        }
        let i = fb.info();
        (i.width, i.height)
    };
    let len = w * h * 4;
    let mut store: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if w == 0 || h == 0 || store.try_reserve_exact(len).is_err() {
        serial_println!("[splash] hold DECLINE reason=alloc len={}", len);
        return;
    }
    store.resize(len, 0);
    let t0 = crate::arch::ms();
    // Same BGR/4-byte store the console window uses: the little-endian word 0x00RRGGBB wm reads.
    paint(
        store.as_mut_ptr() as usize,
        len,
        FrameBufferInfo { width: w, height: h, stride: w, bytes_per_pixel: 4, pixel_format: unaos_boot_info::PixelFormat::Bgr },
        false,
    );
    let ms = crate::arch::ms().saturating_sub(t0);
    *HOLD_SURF.lock() = Some((store, w, h));
    serial_println!(":: SPLASH: arch={} WxH={}x{} ms={} -> painted ::", ARCH_NAME, w, h, ms);
}

/// Park the prepared surface as the topmost chromeless row. Called AFTER the desktop-clear and BEFORE the
/// console window is minted. The furniture minted later lands beneath it (the modal pin re-claims the top
/// on every create / raise).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn hold_open() {
    let (addr, len, w, h) = match HOLD_SURF.lock().as_ref() {
        Some((v, w, h)) => (v.as_ptr() as usize, v.len(), *w, *h),
        None => return,
    };
    let id = crate::video::wm::splash_open(addr, len, w, h);
    if id == crate::video::wm::WIN_NONE {
        *HOLD_SURF.lock() = None;
        serial_println!("[splash] hold DECLINE reason=window-table");
        return;
    }
    crate::video::wm::set_modal_top(id);
    HOLD_T0.store(crate::arch::ms(), Ordering::Relaxed);
    HOLD_WIN.store(id, Ordering::Release);
    serial_println!("[splash] hold OPEN win={} {}x{} (until users::stage_resolve; 5000 ms bound)", id, w, h);
}

/// Release the glass: unpin, close the row (the compositor repaints what is beneath), free the surface.
/// Idempotent. The clear is immediate (fade_ms=0, inside the 300 ms bound). `by` is `store-loaded` or `timeout`.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn hold_release(by: &str) {
    let id = HOLD_WIN.swap(0, Ordering::AcqRel);
    if id == 0 {
        return;
    }
    HOLD_GLASS.store(false, Ordering::Relaxed);
    let t_rel = crate::arch::ms();
    crate::video::wm::clear_modal_top(id);
    crate::video::wm::close(id);
    *HOLD_SURF.lock() = None; // after the row is gone: nothing reads the surface any more
    OVER_UP.store(false, Ordering::Relaxed); crate::fs::bootstep::splash_released(by); // SPLASHSTALL (B510): the splash's waits (`store-wait`, `first-screen`) end with the glass
    crate::bootlog_println!(
        "[splash] held_ms={} released_by={} fade_ms={}",
        t_rel.saturating_sub(HOLD_T0.load(Ordering::Relaxed)),
        by,
        crate::arch::ms().saturating_sub(t_rel)
    );
}

/// The 5 s bound: a store that never answers must not strand the splash. Polled from the device-service pass.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn hold_service() {
    if HOLD_WIN.load(Ordering::Relaxed) != 0 && crate::arch::ms().saturating_sub(HOLD_T0.load(Ordering::Relaxed)) >= 5000 {
        hold_release("timeout");
    }
}

// =================================================================================================
// SPLASH2 — the splash owns the glass from the FIRST frame the panel can show (the UEFI GOP framebuffer,
// painted at boot entry) through the Kepler takeover (the compositor's first frame IS the splash — the
// same RAM surface `hold_open` parks) until the first real screen is PAINTED (`login::open_as`, or the
// desktop's furniture composite in `users::stage_publish`) — not until `store-loaded`.

/// M1 — paint the splash on the GOP framebuffer at boot entry (allocation-free; pre-heap). On builds
/// where `boot_splash` is armed (non-usbdebug/bootlog/witness) that is the animated paint; on the
/// excluded builds (the metal bench carries `bootlog`) it is the same static frame, unarmed, with
/// `SPLASH_UP` set so the console's milestone lines stay off the glass.
#[cfg(target_arch = "x86_64")]
pub fn gop_stage(base: usize, len: usize, info: FrameBufferInfo) {
    let t0 = crate::arch::ms();
    #[cfg(not(any(feature = "usbdebug", feature = "bootlog", feature = "witness")))]
    boot_splash(base, len, info);
    #[cfg(any(feature = "usbdebug", feature = "bootlog", feature = "witness"))]
    {
        paint(base, len, info, false);
        SPLASH_UP.store(true, Ordering::Relaxed);
    }
    serial_println!(":: SPLASH: stage=gop at_ms={} paint_ms={} WxH={}x{} ::", t0, crate::arch::ms().saturating_sub(t0), info.width, info.height);
}

/// M2 — at the takeover's full-panel clear: render the hold surface (heap exists by now) and BLIT it to
/// the live panel instead of clearing, so the first frame after the takeover is the splash (no black
/// frame, no bar/console flash). Returns true when the splash is on the glass. `fb` is the panel handle.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn takeover_blit(fb: &crate::video::FrameBuffer) -> bool {
    hold_prepare();
    let g = HOLD_SURF.lock();
    let Some((surf, w, h)) = g.as_ref() else { return false };
    let i = fb.info();
    if i.bytes_per_pixel != 4 || *w != i.width || *h != i.height {
        return false;
    }
    let row = *w * 4;
    let pitch = i.stride * 4;
    for y in 0..*h {
        fb.blit(y * pitch, &surf[y * row..(y + 1) * row]);
    }
    fb.flush_all();
    HOLD_GLASS.store(true, Ordering::Relaxed);
    serial_println!(":: SPLASH: stage=takeover at_ms={} ::", crate::arch::ms());
    true
}

/// Whether the takeover blit has put the splash on the glass (activate_on skips its DESKTOP_BG clear).
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn glass_held() -> bool {
    HOLD_GLASS.load(Ordering::Relaxed)
}

/// SPLASH2: builds without a compositor hold nothing; `login::open_as` calls this unconditionally.
#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
pub fn hold_release(_by: &str) {}

// =================================================================================================
// BOOT80 (rmbp-ledger B350) M4 — the held splash NAMES the step the boot is waiting on. Boot 21 held the
// glass for ~60 s with nothing on it ("machine seemed locked up"). `fs::bootstep::begin` calls this with
// the step's words ("Writing file types"); they are painted in a dark band on the held surface and the
// band is composited at once (the step runs inside the device-service pass, so no later pass would show
// them). A no-op when nothing holds the glass.

/// Paint `text` as the held splash's step line (one band, replaced on every call) and composite it.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn step_label(text: &str) {
    use crate::video::font::{self, Face};
    if HOLD_WIN.load(Ordering::Acquire) == 0 {
        return;
    }
    let face = Face::Chrome;
    let (cw, ch) = (face.cell_w(), face.cell_h());
    let band = {
        let mut g = HOLD_SURF.lock();
        let Some((surf, w, h)) = g.as_mut() else { return };
        let (w, h) = (*w, *h);
        let band_h = ch * 2;
        if w < cw || h < band_h * 4 || surf.len() < w * h * 4 {
            return;
        }
        let y0 = h - band_h * 3;
        if OVER_UP.swap(false, Ordering::Relaxed) { over_band_clear(surf, w, y0 + band_h, band_h); } // SPLASHSTALL (B510): a new step's word takes the old step's OVER line off the glass
        let put = |s: &mut alloc::vec::Vec<u8>, o: usize, px: u32| s[o..o + 4].copy_from_slice(&px.to_le_bytes());
        for y in y0..y0 + band_h {
            for x in 0..w {
                put(surf, (y * w + x) * 4, crate::video::theme::painter::SPLASH_CAPTION_BG);
            }
        }
        let bytes = text.as_bytes();
        let n = crate::video::text::fit(bytes, true, crate::video::text::Face::Chrome, w); // KERNELFONT (B359): through `video::text`
        let x0 = (w - crate::video::text::advance(&bytes[..n], true, crate::video::text::Face::Chrome).min(w)) / 2;
        let ty = y0 + (band_h - ch) / 2;
        let _ = crate::video::text::draw_with(&bytes[..n], true, crate::video::text::Face::Chrome, x0, ty as isize, w - x0, crate::video::theme::painter::SPLASH_CAPTION_INK, &mut |px, py, a| {
            if px < 0 || py < 0 || px as usize >= w || py as usize >= h {
                return;
            }
            let o = (py as usize * w + px as usize) * 4;
            let bg = u32::from_le_bytes([surf[o], surf[o + 1], surf[o + 2], surf[o + 3]]);
            put(surf, o, font::blend(bg, crate::video::theme::painter::SPLASH_CAPTION_INK, a));
        });
        let _ = face;
        //
        //
        //
        (y0, band_h, w)
    };
    crate::video::wm::damage_intersecting(0, band.0, band.2, band.1 * 2); // SPLASHSTALL (B510): the word's band and the OVER band under it
    crate::video::wm::composite();
}

// =================================================================================================
// SPLASHSTALL (rmbp-ledger B510) — the watchdog's line on the panel, UNDER the step's word. Flight 26's two
// stalled boots had no wire (the FTDI had not enumerated or had died), so the panel is the only surface a
// stall can be read from. `fs::bootstep::poll` calls this from the watchdog task, NOT the stepping task, so
// it may not wait on anything the stepping task can hold: the surface by `try_lock`, the panel handle by
// `try_lock`, and NO `wm` call (`damage_intersecting` / `composite` lock the window table). The band is
// written into the held surface (a compositor that is alive re-composites it unchanged) AND straight onto
// the panel (a compositor that is wedged never would). `""` clears the band.

/// Whether the OVER band is painted (the next step's word clears it).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
static OVER_UP: AtomicBool = AtomicBool::new(false);

/// Whether the splash holds the glass (a held row is open) — the steps that are the splash's own waits
/// (`store-wait`, `first-screen`) exist only while this is true.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn holding() -> bool {
    HOLD_WIN.load(Ordering::Acquire) != 0
}

/// Fill the OVER band (rows `y0..y0+band_h`) with the splash's backdrop.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn over_band_clear(surf: &mut alloc::vec::Vec<u8>, w: usize, y0: usize, band_h: usize) {
    let px = SPLASH_BG.to_le_bytes();
    for y in y0..y0 + band_h {
        for x in 0..w {
            let o = (y * w + x) * 4;
            if o + 4 <= surf.len() {
                surf[o..o + 4].copy_from_slice(&px);
            }
        }
    }
}

/// Paint `text` as the OVER line under the step's word (or clear it with `""`), on the surface and the panel.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn over_label(text: &str) {
    if HOLD_WIN.load(Ordering::Acquire) == 0 {
        return;
    }
    let ch = crate::video::font::Face::Chrome.cell_h();
    let Some(mut g) = HOLD_SURF.try_lock() else { return };
    let Some((surf, w, h)) = g.as_mut() else { return };
    let (w, h) = (*w, *h);
    let band_h = ch * 2;
    if w == 0 || h < band_h * 4 || surf.len() < w * h * 4 {
        return;
    }
    let y0 = h - band_h * 2;
    over_band_clear(surf, w, y0, band_h);
    if !text.is_empty() {
        let bytes = text.as_bytes();
        let n = crate::video::text::fit(bytes, false, crate::video::text::Face::Chrome, w);
        let x0 = (w - crate::video::text::advance(&bytes[..n], false, crate::video::text::Face::Chrome).min(w)) / 2;
        let ty = y0 + (band_h - ch) / 2;
        let put = |s: &mut alloc::vec::Vec<u8>, o: usize, px: u32| s[o..o + 4].copy_from_slice(&px.to_le_bytes());
        let _ = crate::video::text::draw_with(&bytes[..n], false, crate::video::text::Face::Chrome, x0, ty as isize, w - x0, crate::video::theme::painter::SPLASH_CAPTION_INK, &mut |px, py, a| {
            if px < 0 || py < 0 || px as usize >= w || py as usize >= h {
                return;
            }
            let o = (py as usize * w + px as usize) * 4;
            let bg = u32::from_le_bytes([surf[o], surf[o + 1], surf[o + 2], surf[o + 3]]);
            put(surf, o, crate::video::font::blend(bg, crate::video::theme::painter::SPLASH_CAPTION_INK, a));
        });
    }
    OVER_UP.store(!text.is_empty(), Ordering::Relaxed);
    // Straight onto the panel: the same handle and geometry test `takeover_blit` uses. A busy handle skips
    // this look (the next one, 5 s on, tries again); a compositor that is alive carries the surface anyway.
    #[cfg(target_arch = "x86_64")]
    if let Some(fbg) = crate::video::WRITER.try_lock() {
        let fb = *fbg;
        drop(fbg);
        let i = fb.info();
        if fb.is_ready() && i.bytes_per_pixel == 4 && i.width == w && i.height == h {
            let (row, pitch) = (w * 4, i.stride * 4);
            for y in y0..y0 + band_h {
                fb.blit(y * pitch, &surf[y * row..(y + 1) * row]);
            }
            fb.flush_rect(0, y0, w, band_h);
        }
    }
}

/// SPLASHSTALL: builds without a compositor hold no glass.
#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
pub fn holding() -> bool {
    false
}

/// SPLASHSTALL: builds without a compositor have no panel line to paint.
#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
pub fn over_label(_text: &str) {}

/// BOOT80: builds without a compositor hold no glass to label.
#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
pub fn step_label(_text: &str) {}
