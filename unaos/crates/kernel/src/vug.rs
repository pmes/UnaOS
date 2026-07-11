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

//! Vug — the sculptor's demo, and the graphics engine's living testbed.
//!
//! Canon (docs/CODEX.md §5): Vug is the SCULPTOR, the future 3D CAD app; a *vug* is a
//! crystal-lined cavity in rock. So the demo shows what the name promises: a real-time,
//! software-rendered rotating quartz crystal on the panel, drawn through the Gneiss PAL.
//!
//! Everything here is arch-neutral (compiled on x86_64 and aarch64, reachable from the Orin
//! panel shell) and float-free: geometry and the rotation/projection maths run in Q16.16
//! fixed point. The renderer is the engine's proving ground — each engine primitive
//! (`draw_line`, `fill_triangle`, `pump_and_poll`) lands with a visible artifact here.

use unaos_boot_info::FrameBufferInfo;

use crate::pal::{Event, GneissPal, TargetPal};
use crate::video::FrameBuffer;

// ---------------------------------------------------------------------------------------------
// Background painter (called once at boot from main.rs — signature is load-bearing, out of lane).
// ---------------------------------------------------------------------------------------------

pub fn init(base: usize, len: usize, info: FrameBufferInfo) {
    serial_println!(":: VUG Init ::");
    serial_println!(":: FB Size: {}x{} (stride {}) ::", info.width, info.height, info.stride);
    serial_println!(":: FB Format: {:?} ::", info.pixel_format);

    // Paint the background through the shared surface (format/bounds handled in one place).
    // Can-Am dark grey: #1E1E1E.
    let mut surface = FrameBuffer::new();
    surface.init(base, len, info);
    surface.fill_screen(BG);

    serial_println!(":: Framebuffer painted #1E1E1E ::");
}

// ---------------------------------------------------------------------------------------------
// Fixed-point maths (Q16.16). No float in the kernel.
// ---------------------------------------------------------------------------------------------

/// A Q16.16 signed fixed-point scalar: the integer value is `real * 65536`.
type Fx = i32;
const ONE: Fx = 1 << 16;

/// sin(theta) in Q16.16, theta measured in *brads* (256 brads = one full turn).
const SIN: [Fx; 256] = [
    0, 1608, 3216, 4821, 6424, 8022, 9616, 11204,
    12785, 14359, 15924, 17479, 19024, 20557, 22078, 23586,
    25080, 26558, 28020, 29466, 30893, 32303, 33692, 35062,
    36410, 37736, 39040, 40320, 41576, 42806, 44011, 45190,
    46341, 47464, 48559, 49624, 50660, 51665, 52639, 53581,
    54491, 55368, 56212, 57022, 57798, 58538, 59244, 59914,
    60547, 61145, 61705, 62228, 62714, 63162, 63572, 63944,
    64277, 64571, 64827, 65043, 65220, 65358, 65457, 65516,
    65536, 65516, 65457, 65358, 65220, 65043, 64827, 64571,
    64277, 63944, 63572, 63162, 62714, 62228, 61705, 61145,
    60547, 59914, 59244, 58538, 57798, 57022, 56212, 55368,
    54491, 53581, 52639, 51665, 50660, 49624, 48559, 47464,
    46341, 45190, 44011, 42806, 41576, 40320, 39040, 37736,
    36410, 35062, 33692, 32303, 30893, 29466, 28020, 26558,
    25080, 23586, 22078, 20557, 19024, 17479, 15924, 14359,
    12785, 11204, 9616, 8022, 6424, 4821, 3216, 1608,
    0, -1608, -3216, -4821, -6424, -8022, -9616, -11204,
    -12785, -14359, -15924, -17479, -19024, -20557, -22078, -23586,
    -25080, -26558, -28020, -29466, -30893, -32303, -33692, -35062,
    -36410, -37736, -39040, -40320, -41576, -42806, -44011, -45190,
    -46341, -47464, -48559, -49624, -50660, -51665, -52639, -53581,
    -54491, -55368, -56212, -57022, -57798, -58538, -59244, -59914,
    -60547, -61145, -61705, -62228, -62714, -63162, -63572, -63944,
    -64277, -64571, -64827, -65043, -65220, -65358, -65457, -65516,
    -65536, -65516, -65457, -65358, -65220, -65043, -64827, -64571,
    -64277, -63944, -63572, -63162, -62714, -62228, -61705, -61145,
    -60547, -59914, -59244, -58538, -57798, -57022, -56212, -55368,
    -54491, -53581, -52639, -51665, -50660, -49624, -48559, -47464,
    -46341, -45190, -44011, -42806, -41576, -40320, -39040, -37736,
    -36410, -35062, -33692, -32303, -30893, -29466, -28020, -26558,
    -25080, -23586, -22078, -20557, -19024, -17479, -15924, -14359,
    -12785, -11204, -9616, -8022, -6424, -4821, -3216, -1608,
];

#[inline]
fn fsin(brad: i32) -> Fx {
    SIN[(brad & 0xFF) as usize]
}
#[inline]
fn fcos(brad: i32) -> Fx {
    SIN[((brad + 64) & 0xFF) as usize]
}
#[inline]
fn fmul(a: Fx, b: Fx) -> Fx {
    (((a as i64) * (b as i64)) >> 16) as Fx
}

/// Integer square root of a non-negative i64 (Newton's method). Used to normalise face normals.
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

#[derive(Clone, Copy)]
struct Vec3 {
    x: Fx,
    y: Fx,
    z: Fx,
}

impl Vec3 {
    /// Rotate around Y then X by the two brad angles (two axes at different rates read as a
    /// deliberate tumble, not a flat spin).
    fn rotate(self, ay: i32, ax: i32) -> Vec3 {
        let (sy, cy) = (fsin(ay), fcos(ay));
        let x1 = fmul(self.x, cy) - fmul(self.z, sy);
        let z1 = fmul(self.x, sy) + fmul(self.z, cy);
        let y1 = self.y;
        let (sx, cx) = (fsin(ax), fcos(ax));
        let y2 = fmul(y1, cx) - fmul(z1, sx);
        let z2 = fmul(y1, sx) + fmul(z1, cx);
        Vec3 { x: x1, y: y2, z: z2 }
    }
}

// ---------------------------------------------------------------------------------------------
// The crystal: an elongated hexagonal bipyramid (a quartz point) — 14 vertices, 24 triangles.
// ---------------------------------------------------------------------------------------------

const APEX: Fx = 88474; // 1.35
const TY: Fx = 32768; //   0.50 — half prism height
// Hex ring (radius 0.8) in the xz plane, one vertex every 60 degrees.
const RING: [(Fx, Fx); 6] = [
    (52429, 0),
    (26214, 45405),
    (-26214, 45405),
    (-52429, 0),
    (-26214, -45405),
    (26214, -45405),
];

fn crystal_vertices() -> [Vec3; 14] {
    let mut v = [Vec3 { x: 0, y: 0, z: 0 }; 14];
    v[0] = Vec3 { x: 0, y: APEX, z: 0 }; // top apex
    for i in 0..6 {
        v[1 + i] = Vec3 { x: RING[i].0, y: TY, z: RING[i].1 }; // top ring
        v[7 + i] = Vec3 { x: RING[i].0, y: -TY, z: RING[i].1 }; // bottom ring
    }
    v[13] = Vec3 { x: 0, y: -APEX, z: 0 }; // bottom apex
    v
}

/// The 24 triangles, each an outward-wound (CCW-from-outside) vertex triple. Backface culling
/// keys off this winding.
const TRIS: [[usize; 3]; 24] = [
    [0, 2, 1], [0, 3, 2], [0, 4, 3], [0, 5, 4], [0, 6, 5], [0, 1, 6], // top cap
    [13, 7, 8], [13, 8, 9], [13, 9, 10], [13, 10, 11], [13, 11, 12], [13, 12, 7], // bottom cap
    [1, 8, 7], [1, 2, 8], [2, 9, 8], [2, 3, 9], [3, 10, 9], [3, 4, 10], // prism sides
    [4, 11, 10], [4, 5, 11], [5, 12, 11], [5, 6, 12], [6, 7, 12], [6, 1, 7],
];

// ---------------------------------------------------------------------------------------------
// Palette.
// ---------------------------------------------------------------------------------------------

const BG: u32 = 0x001E_1E1E; // Can-Am dark grey
/// Deep amethyst base facet colour (155, 89, 182); shading scales it toward black.
const AMETHYST: (u32, u32, u32) = (155, 89, 182);
/// Facet edge highlight — a paler lilac line drawn on the seams for definition.
const EDGE: u32 = 0x00C9_A6E8;

/// A fixed key light, roughly upper-left-front, as a Q16.16 direction (need not be unit — the
/// shade divides by |L|). Points from the surface toward the light.
const LIGHT: Vec3 = Vec3 { x: -30000, y: 55000, z: -45000 };

/// Scale an (r,g,b) triple by `num/den` (clamped) and pack it. The Lambert shade.
fn shade(base: (u32, u32, u32), num: i64, den: i64) -> u32 {
    let s = |c: u32| -> u32 {
        let v = (c as i64 * num / den).clamp(0, 255);
        v as u32
    };
    (s(base.0) << 16) | (s(base.1) << 8) | s(base.2)
}

// ---------------------------------------------------------------------------------------------
// The demo.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Solid,
    Wire,
}

/// Upper bound on the CPU-pulse meter's per-core bars (fixed-size scratch; actual count is
/// `sched::meter_cpu_count()` capped here).
const MAX_METER_CPUS: usize = 16;

/// M3b render-load readout, refreshed ~5x/sec. `fps_x10`/`ms_x10` are fixed-point (value*10) to
/// print one decimal without float. `load` is the render busy-fraction percent; `tris`/`px` are the
/// current frame's drawn-triangle count and filled-pixel estimate.
#[derive(Default, Clone, Copy)]
struct RenderStats {
    fps_x10: u64,
    ms_x10: u64,
    load: u32,
    tris: u32,
    px: u64,
}

/// Run the rotating crystal until any key is pressed. `mode` selects solid facets or wireframe.
/// The loop owns the pump: it drives input itself (via `pal::pump_and_poll`) so a keystroke exits
/// cleanly, presents exactly one frame per iteration, and `yield_now`s between frames (never
/// sleeps — the post-drop aarch64 rule: no timer to wake a sleeper).
pub fn run_crystal(pal: &mut TargetPal, mode: Mode) {
    let w = pal.width() as i32;
    let h = pal.height() as i32;
    let cx = w / 2;
    let cy = h / 2;
    let focal = (w.min(h) * 40) / 100; // pixels-per-unit at the crystal's centre depth
    let dist: Fx = 4 * ONE; // camera distance along -z

    let base = crystal_vertices();
    let solid = mode == Mode::Solid;
    serial_println!(
        ":: VUG: crystal live — {} faces, {}, exit clean ::",
        TRIS.len(),
        if solid { "solid" } else { "wire" }
    );

    let mut ay: i32 = 0; // yaw
    let mut ax: i32 = 0; // pitch — advances slower, so the tumble reads as two axes
    let mut frame: u64 = 0;

    // Rotated + projected scratch, refreshed each frame.
    let mut px = [0i32; 14];
    let mut py = [0i32; 14];
    let mut rot = [Vec3 { x: 0, y: 0, z: 0 }; 14];

    // --- M3b: the two corner load meters --------------------------------------------------
    // Render-load meter (the honest "GPU monitor" — we render in software): each frame we clock
    // the render span (`now_cycles`) against the whole frame span to get a busy fraction, and time
    // the window with `ms()` to get frame time / FPS. `stats` holds the last window's readout.
    // CPU-pulse meter: per-core busy/idle counts sampled from the scheduler (see `sched::meter_*`).
    // SEAM: both meters read introspection counters; a real GPU/PMU feed would replace the sources.
    let mut m = RenderStats::default();
    let ncpu = MAX_METER_CPUS.min(crate::arch::sched::meter_cpu_count());
    let mut cpu_prev = [(0u64, 0u64); MAX_METER_CPUS];
    let mut cpu_load = [0u32; MAX_METER_CPUS];
    for c in 0..ncpu {
        cpu_prev[c] = crate::arch::sched::meter_cpu_ticks(c);
    }
    let mut demo_core_logged = false;
    let mut prev_top = crate::arch::now_cycles();
    let mut work_acc: u64 = 0;
    let mut total_acc: u64 = 0;
    let mut win_frames: u32 = 0;
    let mut win_ms = crate::arch::ms();

    loop {
        let top = crate::arch::now_cycles();
        // --- input: exit on any key ------------------------------------------------------
        if let Some(Event::Key(_)) = crate::pal::pump_and_poll() {
            break;
        }

        // --- transform: rotate every vertex, then project to pixels ----------------------
        for i in 0..14 {
            let v = base[i].rotate(ay, ax);
            rot[i] = v;
            let zc = v.z + dist; // always > 0 (|z| < 1.4, dist = 4)
            let ppu = ((focal as i64) * (dist as i64) / (zc as i64)) as i64;
            px[i] = cx + (((v.x as i64) * ppu) >> 16) as i32;
            py[i] = cy - (((v.y as i64) * ppu) >> 16) as i32;
        }

        // --- clear to the dark-grey backdrop ---------------------------------------------
        pal.clear_screen(BG);

        // --- draw the front-facing faces, painter-sorted back-to-front -------------------
        // Collect visible faces with their average camera-depth, then insertion-sort farthest
        // first (a convex solid needs only depth order; 24 faces => a tiny sort).
        let mut order: [(i64, usize); 24] = [(0, 0); 24];
        let mut n = 0usize;
        let mut est_px: u64 = 0; // filled-pixel estimate this frame (sum of front-face areas)
        for (fi, tri) in TRIS.iter().enumerate() {
            let (a, b, c) = (tri[0], tri[1], tri[2]);
            // Screen-space signed area picks front faces (this projection flips y, so a
            // front-facing, outward-CCW triangle has a negative area).
            let area = (px[b] - px[a]) as i64 * (py[c] - py[a]) as i64
                - (py[b] - py[a]) as i64 * (px[c] - px[a]) as i64;
            if area >= 0 {
                continue; // back face
            }
            est_px += (-area / 2) as u64; // triangle pixel area
            let depth = rot[a].z as i64 + rot[b].z as i64 + rot[c].z as i64;
            order[n] = (depth, fi);
            n += 1;
        }
        // Insertion sort: larger depth (farther) first.
        for i in 1..n {
            let cur = order[i];
            let mut j = i;
            while j > 0 && order[j - 1].0 < cur.0 {
                order[j] = order[j - 1];
                j -= 1;
            }
            order[j] = cur;
        }

        for &(_, fi) in order.iter().take(n) {
            let tri = &TRIS[fi];
            let (a, b, c) = (tri[0], tri[1], tri[2]);
            if solid {
                // Lambert: intensity = max(0, N.L) / (|N||L|), then ambient + diffuse.
                let e1 = Vec3 {
                    x: rot[b].x - rot[a].x,
                    y: rot[b].y - rot[a].y,
                    z: rot[b].z - rot[a].z,
                };
                let e2 = Vec3 {
                    x: rot[c].x - rot[a].x,
                    y: rot[c].y - rot[a].y,
                    z: rot[c].z - rot[a].z,
                };
                // Face normal (i64; magnitude arbitrary).
                let nx = (e1.y as i64 * e2.z as i64 - e1.z as i64 * e2.y as i64) >> 16;
                let ny = (e1.z as i64 * e2.x as i64 - e1.x as i64 * e2.z as i64) >> 16;
                let nz = (e1.x as i64 * e2.y as i64 - e1.y as i64 * e2.x as i64) >> 16;
                let dot = nx * LIGHT.x as i64 + ny * LIGHT.y as i64 + nz * LIGHT.z as i64;
                let nlen = isqrt(nx * nx + ny * ny + nz * nz).max(1);
                let llen = isqrt(
                    (LIGHT.x as i64).pow(2) + (LIGHT.y as i64).pow(2) + (LIGHT.z as i64).pow(2),
                )
                .max(1);
                // diffuse in [0,255]; ambient floor keeps back-lit facets from going pure black.
                let diffuse = (dot.max(0) * 255 / (nlen * llen)).clamp(0, 255);
                let intensity = 64 + diffuse * 191 / 255; // ambient 64 .. full 255
                let color = shade(AMETHYST, intensity, 255);
                pal.fill_triangle((px[a], py[a]), (px[b], py[b]), (px[c], py[c]), color);
            } else {
                pal.draw_line(px[a], py[a], px[b], py[b], EDGE);
                pal.draw_line(px[b], py[b], px[c], py[c], EDGE);
                pal.draw_line(px[c], py[c], px[a], py[a], EDGE);
            }
        }

        // Solid mode: trace the frontmost cap seams for extra facet definition.
        if solid {
            for i in 0..6 {
                let a = 1 + i;
                let b = 1 + (i + 1) % 6;
                if rot[a].z < 0 || rot[b].z < 0 {
                    pal.draw_line(px[a], py[a], px[b], py[b], EDGE);
                }
            }
        }

        m.tris = n as u32;
        m.px = est_px;
        draw_stats(pal, frame, n as u32, solid, w, h);
        draw_meters(pal, &m, &cpu_load, ncpu, h);

        pal.render(); // present ONCE per frame

        // --- M3b render-load accounting: work span vs whole-frame span -------------------
        let end = crate::arch::now_cycles();
        work_acc += end.wrapping_sub(top); // render + present cycles this frame
        total_acc += top.wrapping_sub(prev_top); // whole prior frame incl. input poll + yield
        prev_top = top;
        win_frames += 1;

        // Refresh the meter readout roughly every 200 ms (a steady display, not per-frame jitter).
        let now_ms = crate::arch::ms();
        let dt = now_ms.wrapping_sub(win_ms);
        if dt >= 200 && win_frames > 0 {
            m.fps_x10 = (win_frames as u64 * 10_000) / dt.max(1);
            m.ms_x10 = (dt * 10) / win_frames as u64;
            m.load = if total_acc > 0 {
                ((work_acc * 100) / total_acc).min(100) as u32
            } else {
                0
            };
            // Per-core CPU-pulse load. Two honest sources, picked per core by whether the scheduler
            // is accounting that core this window:
            //   * db+di > 0  → the core is inside `sched::run()` (dispatching tasks or spinning idle,
            //     both of which bump the counters). Use the scheduler's busy fraction — this is the
            //     Orin scheduled-pump path, and per-core APs on x86; do NOT regress it.
            //   * db+di == 0 → the core is executing OUTSIDE `sched::run()` and the scheduler never
            //     sees it (the x86 GUI runs this crystal demo in the inline BSP loop). Its counters
            //     are frozen, so the sched fraction would read a false ~0 even though the core is
            //     pegged rendering. Credit it instead from the render loop's OWN measured busy-vs-
            //     yield fraction (`m.load`, work cycles / whole-frame cycles) — the same honest
            //     number the RENDER meter shows for the core actually doing the work. That core IS
            //     the demo core (the current CPU); log it once so the label is truthful.
            for c in 0..ncpu {
                let (b, i) = crate::arch::sched::meter_cpu_ticks(c);
                let db = b.wrapping_sub(cpu_prev[c].0);
                let di = i.wrapping_sub(cpu_prev[c].1);
                if db + di > 0 {
                    cpu_load[c] = ((db * 100) / (db + di)) as u32;
                } else {
                    // Unscheduled executing core → this is the demo core; show its real render load.
                    cpu_load[c] = m.load;
                    if !demo_core_logged {
                        serial_println!(":: VUG: CPU meter — core {} is the demo core (unscheduled render loop, load from render busy%) ::", c);
                        demo_core_logged = true;
                    }
                }
                cpu_prev[c] = (b, i);
            }
            win_ms = now_ms;
            win_frames = 0;
            work_acc = 0;
            total_acc = 0;
        }

        ay = (ay + 3) & 0xFF; // yaw ~3 brad/frame
        ax = (ax + 1) & 0xFF; // pitch ~1 brad/frame — different rate => a tumble
        frame += 1;
        crate::arch::sched::yield_now();
    }

    serial_println!(":: VUG: crystal exit clean — {} frames ::", frame);
}

/// The title HUD (the existing Vug aesthetic), plus a live frame counter.
///
/// POLISH-1: the legacy right-edge VU-meter segment stack was removed. It predated the M3b corner
/// meters and, sitting at the right edge in purple/grey, read as the CPU pulse meter's per-core bars
/// gone astray (the CPU meter actually lives bottom-left under RENDER, drawn by `draw_meters`).
/// POLISH-2: the blinking red heartbeat square was removed too (Peter's call — not needed; any real
/// problem will be obvious from the frozen crystal or a dead frame counter). Title + stat line stay.
fn draw_stats(pal: &mut TargetPal, frame: u64, faces: u32, solid: bool, _w: i32, _h: i32) {
    // Title + live stat line. UI-1: all positions derive from the panel metrics.
    let m = pal.metrics();
    pal.draw_text(m.margin, m.margin, "VUG // quartz", 0x00FFFFFF);
    let mode = if solid { "solid" } else { "wire " };
    let line = alloc::format!("mode {}  faces {:>2}  frame {}", mode, faces, frame);
    pal.draw_text(m.margin, m.margin + m.line_h, &line, 0x00A0A0A0);
    pal.draw_text(m.margin, m.margin + 2 * m.line_h, "press any key to exit", 0x00707070);
}

/// M3b — the two corner load meters (kept small; the crystal stays the star). Bottom-left:
/// a RENDER meter (the honest software "GPU monitor" — frame time, FPS, render busy-fraction bar,
/// triangles + estimated filled pixels this frame) above a CPU pulse meter (per-core scheduler
/// busy fraction, BeOS-Pulse style). Both draw through the same damage-tracked back buffer, so the
/// one-present-per-frame contract still holds.
fn draw_meters(pal: &mut TargetPal, m: &RenderStats, cpu_load: &[u32], ncpu: usize, h: i32) {
    const DIM: u32 = 0x00_2A2432;
    const LILAC: u32 = 0x00_B36BFF;
    const PURPLE: u32 = 0x00_9B59B6;
    const LABEL: u32 = 0x00_8A8296;

    let x0 = 20usize;
    let base = (h as usize).saturating_sub(96);

    // --- RENDER meter --------------------------------------------------------------------
    pal.draw_text(x0, base, "RENDER", LABEL);
    let bar_w = 132usize;
    let bar_h = 8usize;
    pal.draw_rect(x0, base + 12, bar_w, bar_h, DIM);
    let fill = (m.load as usize * bar_w) / 100;
    pal.draw_rect(x0, base + 12, fill, bar_h, LILAC);
    // Numeric readout: frame time / FPS (one decimal, fixed-point) + triangles + filled pixels.
    let (px_val, px_unit) = if m.px >= 1000 { (m.px / 1000, "Kpx") } else { (m.px, "px ") };
    let line = alloc::format!(
        "{}.{}ms {}.{}fps  t{} {}{}",
        m.ms_x10 / 10,
        m.ms_x10 % 10,
        m.fps_x10 / 10,
        m.fps_x10 % 10,
        m.tris,
        px_val,
        px_unit,
    );
    pal.draw_text(x0, base + 24, &line, LABEL);

    // --- CPU pulse meter -----------------------------------------------------------------
    pal.draw_text(x0, base + 44, "CPU", LABEL);
    let cw = 8usize; // bar width
    let gap = 4usize;
    let ch = 26usize; // bar height
    let top = base + 56;
    for c in 0..ncpu {
        let bx = x0 + c * (cw + gap);
        pal.draw_rect(bx, top, cw, ch, DIM);
        let fh = (cpu_load[c] as usize * ch) / 100;
        if fh > 0 {
            pal.draw_rect(bx, top + ch - fh, cw, fh, PURPLE);
        }
    }
}

/// The BeBox tribute screen — a static homage (kept from the original demo, dressed up a little).
/// UI-1: laid out from the panel metrics (no absolute pixel sizes).
pub fn run_bebox_mode(pal: &mut TargetPal) {
    let m = pal.metrics();
    let x0 = 5 * m.margin;
    let y0 = 5 * m.margin;
    pal.clear_screen(0x00101018);
    pal.draw_text(x0, y0, "BeBox // GeekPort tribute", 0x0066CCFF);
    pal.draw_text(x0, y0 + 2 * m.line_h, "dual-CPU dreams, one framebuffer", 0x00889AAA);
    // Two "CPU" LED columns, a nod to the twin BeBox meters.
    let led_w = 2 * m.cell_w + m.cell_w / 2;
    let led_h = m.cell_h + m.cell_h / 2;
    let pitch_x = led_w + m.cell_w;
    let pitch_y = 2 * m.cell_h;
    let leds_y = y0 + 5 * m.line_h;
    for col in 0..2 {
        for i in 0..8 {
            let on = (i + col) % 3 != 0;
            let color = if on { 0x0000FF66 } else { 0x00223322 };
            pal.draw_rect(x0 + col * pitch_x, leds_y + i * pitch_y, led_w, led_h, color);
        }
    }
    pal.render();
}
