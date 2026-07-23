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

// SPLASH-2: the boot splash lives in `crate::splash` (its own module, own tracer).

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

/// Fixed segment count of every CPU pulse bar (UI1-M2, Peter's sketch): filled ∝ load, and the
/// EMPTY segments draw dim — an idle core reads alive-but-empty, never blank.
const PULSE_SEGS: usize = 10;

// Meter palette (shared by the vug corner meters and the `pulse` full-screen monitor).
const METER_DIM: u32 = 0x00_2A2432;
const METER_LILAC: u32 = 0x00_B36BFF;
const METER_PURPLE: u32 = 0x00_9B59B6;
const METER_LABEL: u32 = 0x00_8A8296;
/// PULSE-ALIVE breath colour — clearly brighter than `METER_DIM`, dimmer than a load fill: the one
/// sweeping segment an idle-but-scheduled core lights so "alive and idle" reads at a glance.
const METER_BREATH: u32 = 0x00_5F4E86;
/// Parked dash colour — cooler/dimmer than `METER_DIM` so a broken track reads as "not participating".
const METER_PARKED: u32 = 0x00_3A3550;

/// VUG-HONESTY parked-core marker (a load-array sentinel, disjoint from the 0..=100 percent range). A
/// core whose pulse counters are frozen this window AND that is NOT the demo core is parked /
/// never-scheduled: the display must not fabricate load for it. `load[c] == PARKED` selects a distinct
/// DASHED bar (see [`draw_pulse_bar`]) — visually separable from an idle 0% bar (a solid dim track) so
/// "idle" and "never woken" never read alike, and `run_pulse` prints `park` instead of a percent.
const PARKED: u32 = u32::MAX;

/// VUG-HONESTY — the pure per-core display decision. Given one core's per-window busy/idle tick deltas
/// (`db`/`di`), whether it is the *demo core* (the core executing this render loop), and that loop's own
/// measured render busy% (`own_load`), return the load to display (0..=100) or [`PARKED`]:
///   * `db + di > 0`  → the scheduler accounted this core this window: honest busy fraction.
///   * frozen + demo  → the demo core runs OUTSIDE the scheduler, so its own counters freeze; credit its
///                      measured render load — the honest number for the core doing the drawing.
///   * frozen + other → a core with no scheduling activity this window that is NOT drawing: parked /
///                      never-woken. NEVER fabricate load. (The pre-fix code credited `own_load` to
///                      EVERY frozen core, so a parked AP mirrored the busy demo core and read PINNED —
///                      the display-honesty defect this arc closes. The merged idle/busy-heartbeats made
///                      the *counters* honest and passed the one-shot boot witness, but the LIVE meter
///                      samples per-window deltas: a parked EL2 secondary gets no periodic wake, so its
///                      counters are frozen between windows and this fallback still fabricated.) Report
///                      PARKED instead.
fn classify_load(db: u64, di: u64, is_demo: bool, own_load: u32) -> u32 {
    if db + di > 0 {
        ((db * 100) / (db + di)) as u32
    } else if is_demo {
        own_load
    } else {
        PARKED
    }
}

/// VUG-HONESTY witness — deterministic, arch-neutral, framebuffer-free. Exercises [`classify_load`]
/// over the cases the honesty rule must separate and emits one PASS/FAIL serial line. Wired into the
/// `virt` CAPSTONE boot (`arch::sched::run_capstone_boot_core`), so `test-arm` and the GICv3 suite
/// witness a parked core reading PARKED rather than a fabricated pinned bar. Returns true on PASS.
pub fn parked_display_witness() -> bool {
    let busy = classify_load(8, 0, false, 99) == 100; // scheduled + fully busy
    let idle = classify_load(0, 2, false, 99) == 0; // scheduled + idle → honest 0%, NOT own_load
    let half = classify_load(1, 1, false, 0) == 50; // scheduled + half busy
    let demo = classify_load(0, 0, true, 42) == 42; // frozen demo core → its own render load
    let park = classify_load(0, 0, false, 99) == PARKED; // frozen non-demo core → PARKED, not fabricated
    let pass = busy && idle && half && demo && park;
    serial_println!(
        ":: VUG-HONESTY: parked-core display witness {} — a frozen non-demo core reads PARKED (never the demo core's load); scheduled cores read their busy fraction, the demo core its render load ::",
        if pass { "PASS" } else { "FAIL" }
    );
    pass
}

/// The per-core CPU load sampler — shared by the vug corner meter and the `pulse` full-screen
/// monitor (UI1-M2/M3). Holds the previous `(busy, idle)` tick snapshot per core and the last
/// window's load percents. THE HONEST TWO-SOURCE RULE (VUG-1 M3b) lives in [`CpuPulse::refresh`],
/// kept verbatim — do not regress the Orin scheduled path.
struct CpuPulse {
    ncpu: usize,
    prev: [(u64, u64); MAX_METER_CPUS],
    load: [u32; MAX_METER_CPUS],
    demo_core_logged: bool,
    /// PULSE-ALIVE evidence: whether the one-shot per-core tick-delta line has fired (first
    /// refresh window only), so a metal serial capture proves which cores' counters actually tick.
    ticks_logged: bool,
    /// Serial-evidence tag for the one-shot demo-core log line ("VUG" / "PULSE").
    tag: &'static str,
}

impl CpuPulse {
    fn new(tag: &'static str) -> Self {
        let ncpu = MAX_METER_CPUS.min(crate::arch::sched::meter_cpu_count());
        let mut prev = [(0u64, 0u64); MAX_METER_CPUS];
        for (c, p) in prev.iter_mut().enumerate().take(ncpu) {
            *p = crate::arch::sched::meter_cpu_ticks(c);
        }
        Self { ncpu, prev, load: [0u32; MAX_METER_CPUS], demo_core_logged: false, ticks_logged: false, tag }
    }

    /// Refresh the per-core loads for one display window. Two honest sources, picked per core by
    /// whether the scheduler is accounting that core this window:
    ///   * db+di > 0  → the core is inside `sched::run()` (dispatching tasks or spinning idle,
    ///     both of which bump the counters). Use the scheduler's busy fraction — this is the
    ///     Orin scheduled-pump path, and per-core APs on x86; do NOT regress it.
    ///   * db+di == 0 → the core is executing OUTSIDE `sched::run()` and the scheduler never
    ///     sees it (the x86 GUI runs the demo/monitor in the inline BSP loop). Its counters
    ///     are frozen, so the sched fraction would read a false ~0 even though the core is
    ///     pegged rendering. Credit it instead from the render loop's OWN measured busy-vs-
    ///     yield fraction (`own_load`, work cycles / whole-frame cycles) — the same honest
    ///     number the RENDER meter shows for the core actually doing the work. That core IS
    ///     the demo core (the current CPU); log it once so the label is truthful.
    fn refresh(&mut self, own_load: u32) {
        // VUG-HONESTY: the demo core is whichever core is running THIS render loop right now (the loop
        // is cooperative and single-core, but read it live so a future migration can't mislabel). ONLY
        // that core takes the render-load fallback when its counters are frozen; every OTHER frozen-
        // counter core is parked / never-scheduled and reads PARKED — no fabricated load.
        let demo = crate::arch::sched::meter_current_cpu();
        // PULSE-ALIVE evidence: one bounded line on the FIRST refresh window with the raw per-core
        // busy/idle tick deltas — the metal disambiguator between "APs idle-ticking (honest 0%)"
        // and "AP counters frozen (parked)" that the rendered bars alone couldn't prove.
        if !self.ticks_logged {
            self.ticks_logged = true;
            let mut line = alloc::string::String::new();
            for c in 0..self.ncpu {
                let (b, i) = crate::arch::sched::meter_cpu_ticks(c);
                let _ = core::fmt::Write::write_fmt(
                    &mut line,
                    format_args!("c{}:{}/{} ", c, b.wrapping_sub(self.prev[c].0), i.wrapping_sub(self.prev[c].1)),
                );
            }
            serial_println!(":: {}: first-window per-core busy/idle tick deltas — {}::", self.tag, line);
        }
        for c in 0..self.ncpu {
            let (b, i) = crate::arch::sched::meter_cpu_ticks(c);
            let db = b.wrapping_sub(self.prev[c].0);
            let di = i.wrapping_sub(self.prev[c].1);
            self.load[c] = classify_load(db, di, c == demo, own_load);
            if c == demo && db + di == 0 && !self.demo_core_logged {
                serial_println!(":: {}: CPU meter — core {} is the demo core (unscheduled render loop, load from render busy%) ::", self.tag, c);
                self.demo_core_logged = true;
            }
            self.prev[c] = (b, i);
        }
    }
}

/// UI1-M2 — one segmented pulse bar: `PULSE_SEGS` fixed segments at `(x, y)`, filled ∝ `load`
/// (rounded; any nonzero load lights at least one), the rest drawn `METER_DIM` so an idle bar
/// reads alive-but-empty. `seg_w`/`seg_h`/`gap` come from the caller's metrics (the corner meter
/// and the full-screen `pulse` reuse this at two sizes). Returns the x just past the bar.
fn draw_pulse_bar(
    pal: &mut TargetPal,
    x: usize,
    y: usize,
    seg_w: usize,
    seg_h: usize,
    gap: usize,
    load: u32,
    fill_color: u32,
) -> usize {
    // VUG-HONESTY: a parked / never-woken core draws a DASHED, cooler track (every other segment lit
    // METER_PARKED, the rest bare background) — visually distinct from an idle core's solid-dim track,
    // so "never woken" never reads like "idle 0%".
    if load == PARKED {
        let mut bx = x;
        for s in 0..PULSE_SEGS {
            if s % 2 == 0 {
                pal.draw_rect(bx, y, seg_w, seg_h, METER_PARKED);
            }
            bx += seg_w + gap;
        }
        return bx;
    }
    // PULSE-ALIVE (metal verdict, 2026-07-18): an idle-but-scheduled core (honest 0% — its
    // busy/idle counters ARE ticking) previously drew an all-dim track, which on the panel read
    // exactly like "unlit"/dead — Peter's "pulse shows 1 CPU" report with 7 enabled, idle APs.
    // Now an idle core breathes: one segment, swept slowly along the bar by wall-clock time, lights
    // in `METER_BREATH`. Not a load claim (the percent text still reads 0%) — a liveness indicator,
    // visually distinct from BOTH a loaded bar (solid fill) and a parked core (static dashes above).
    if load == 0 {
        let phase = ((crate::arch::ms() / 300) as usize) % PULSE_SEGS;
        let mut bx = x;
        for s in 0..PULSE_SEGS {
            let color = if s == phase { METER_BREATH } else { METER_DIM };
            pal.draw_rect(bx, y, seg_w, seg_h, color);
            bx += seg_w + gap;
        }
        return bx;
    }
    let filled = ((load as usize * PULSE_SEGS + 50) / 100).clamp(1, PULSE_SEGS);
    let mut bx = x;
    for s in 0..PULSE_SEGS {
        let color = if s < filled { fill_color } else { METER_DIM };
        pal.draw_rect(bx, y, seg_w, seg_h, color);
        bx += seg_w + gap;
    }
    bx
}

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

/// CURSOR-VIS — the full-screen demos' shared input pump: drain EVERY queued event this frame
/// (`pump_and_poll` until empty), returning `true` when a key was pressed (exit). Mouse motion
/// routes into the shared `pal::cursor` position, so the sprite the render loop draws each frame
/// tracks the trackpad. Before this, the demos popped ONE event per frame and matched only `Key` —
/// mouse reports backed up and the cursor was never drawn (the metal invisible-cursor defect).
fn drain_input(pal: &mut TargetPal) -> bool {
    let (w, h) = (pal.width() as i32, pal.height() as i32);
    while let Some(e) = crate::pal::pump_and_poll() {
        match e {
            Event::Key(_) => return true,
            // CLICK-1 (metal verdict): a trackpad/mouse click closes the full-screen demo through
            // the SAME exit path a keystroke takes — the on-metal click observable for the next
            // sitting (press the pad while vug runs; the demo exits to the console).
            Event::Button(_) => return true,
            Event::Mouse { x, y } => crate::pal::cursor::move_rel(x, y, w, h),
            Event::MouseAbsolute { x, y } => crate::pal::cursor::set_abs(x, y, w, h),
            _ => {}
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------
// GAME-MODE — held-key stepping + drag-rotate for the crystal ("UnaOS-native, like a game").
//
// WHAT THE INPUT STREAM ACTUALLY DELIVERS (xHCI HID boot protocol — see drivers/xhci/mod.rs):
//   * Keyboard: one `Event::Key(ascii)` on the PRESS of a mapped key. There is NO key-up event
//     (a release is an all-zero boot report that decodes to nothing) and NO SET_IDLE is issued,
//     so auto-repeat while a key stays down is NOT guaranteed. Arrow keys are absent from the
//     ASCII table (they decode to 0), so WASD/QE are the reliable controls.
//   * Pointer: `Event::Mouse{dx,dy}` on any relative motion (independent of buttons);
//     `Event::MouseAbsolute{x,y}` for tablet/absolute pointers; `Event::Button(mask)` ONCE on the
//     button-DOWN edge only. A button RELEASE is not delivered (the GUI-CLICK-2b gap).
//
// CONSEQUENCE / SAFE SUBSET: with no key-up and no button-release we cannot maintain a true
// press/release held-state. Instead each movement-key press arms a short HOLD window and the key
// reads "held" until it lapses (a resent report while down refreshes it → continuous stepping;
// otherwise a press nudges with a brief momentum). Drag-rotate is driven by pointer MOTION events
// (which flow regardless of button state) rather than a held button, since the button-down edge is
// already the click-to-exit observable and no release arrives to end a drag.
// ---------------------------------------------------------------------------------------------

const G_YAW_L: u8 = 1 << 0; // 'a' — yaw left
const G_YAW_R: u8 = 1 << 1; // 'd' — yaw right
const G_PIT_U: u8 = 1 << 2; // 'w' — pitch up
const G_PIT_D: u8 = 1 << 3; // 's' — pitch down
const G_ZOOM_IN: u8 = 1 << 4; // 'e' / '+' / '=' — zoom in (camera nearer)
const G_ZOOM_OUT: u8 = 1 << 5; // 'q' / '-' / '_' — zoom out (camera farther)
const G_BITS: usize = 6;

/// Map an ASCII key to its movement bit, or `None` if it is not a game-mode control. A `None` key
/// takes the shell exit path (any non-mapped key exits), so the exit contract is preserved.
fn game_key_bit(k: u8) -> Option<u8> {
    match k.to_ascii_lowercase() {
        b'a' => Some(G_YAW_L),
        b'd' => Some(G_YAW_R),
        b'w' => Some(G_PIT_U),
        b's' => Some(G_PIT_D),
        b'e' | b'+' | b'=' => Some(G_ZOOM_IN),
        b'q' | b'-' | b'_' => Some(G_ZOOM_OUT),
        _ => None,
    }
}

/// GAME-MODE per-frame input, accumulated over one full drain of the event queue.
#[derive(Default, Clone, Copy)]
struct GameInput {
    exit: bool,   // a non-mapped key or a button press-edge asked to exit
    pressed: u8,  // movement-key bits freshly seen this drain
    mdx: i32,     // summed relative pointer dx this drain (drag-rotate)
    mdy: i32,     // summed relative pointer dy this drain
    motion: bool, // any pointer motion this drain
}

/// GAME-MODE input drain — the crystal's frame-paced pump. Drains EVERY queued event this frame
/// (`pump_and_poll` until empty, so a burst never backs up one-per-frame AND `note_progress` fires
/// on each pass, keeping the app-input watchdog fed). Movement keys feed the held-step model; any
/// other key, or a button press-edge, requests exit (shell exit paths preserved). Pointer motion
/// both drives the cursor sprite (CURSOR-VIS) and accumulates a drag-rotate delta.
fn drain_game_input(pal: &mut TargetPal) -> GameInput {
    let (w, h) = (pal.width() as i32, pal.height() as i32);
    let mut gi = GameInput::default();
    while let Some(e) = crate::pal::pump_and_poll() {
        match e {
            Event::Key(k) => match game_key_bit(k) {
                Some(bit) => gi.pressed |= bit,
                None => gi.exit = true,
            },
            Event::Button(_) => gi.exit = true,
            Event::Mouse { x, y } => {
                crate::pal::cursor::move_rel(x, y, w, h);
                gi.mdx += x;
                gi.mdy += y;
                gi.motion = true;
            }
            Event::MouseAbsolute { x, y } => {
                crate::pal::cursor::set_abs(x, y, w, h);
                gi.motion = true;
            }
            _ => {}
        }
    }
    gi
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
    let mut dist: Fx = 4 * ONE; // camera distance along -z (GAME-MODE zoom adjusts it)

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

    // VUG-FPS dirty-rect state: the crystal's footprint last frame (so its vacated pixels get
    // repainted to background) and the cursor's last footprint (dirty-rect no longer clears the
    // whole panel, so the moving sprite must erase its own trail). Bboxes are `(x, y, w, h)`.
    let mut prev_crystal: Option<(usize, usize, usize, usize)> = None;
    let mut prev_cursor: Option<(usize, usize, usize, usize)> = None;

    // --- M3b: the two corner load meters --------------------------------------------------
    // Render-load meter (the honest "GPU monitor" — we render in software): each frame we clock
    // the render span (`now_cycles`) against the whole frame span to get a busy fraction, and time
    // the window with `ms()` to get frame time / FPS. `stats` holds the last window's readout.
    // CPU-pulse meter: per-core busy/idle counts sampled from the scheduler (see `sched::meter_*`),
    // via the shared `CpuPulse` sampler (the honest two-source rule lives in its `refresh`).
    // SEAM: both meters read introspection counters; a real GPU/PMU feed would replace the sources.
    let mut m = RenderStats::default();
    let mut cpu = CpuPulse::new("VUG");
    let mut prev_top = crate::arch::now_cycles();
    let mut work_acc: u64 = 0;
    let mut total_acc: u64 = 0;
    let mut win_frames: u32 = 0;
    let mut win_ms = crate::arch::ms();

    // VUG-FPS serial witness: frames and flushed bytes accumulated over ~1 s windows, printed as
    // `[vugfps]` so a metal serial capture reads the fps and the per-frame flush bandwidth the
    // dirty-rect path moves (the whole point of the arc).
    let mut wit_ms = crate::arch::ms();
    let mut wit_frames: u32 = 0;
    let mut wit_bytes: u64 = 0;
    // VUG-PAR: the max parallel band count seen in the window — reads the parallel flush win directly.
    let mut wit_bands: usize = 1;

    // GAME-MODE held-key model. The HID path delivers a Key event on PRESS only (no key-up, no
    // guaranteed auto-repeat — see the module notes above), so each press arms a hold window and a
    // key reads "held" until it lapses. `held_until[bit]` is the ms deadline; a fresh report while
    // the key is down refreshes it (continuous stepping), otherwise a press nudges with momentum.
    let mut held_until = [0u64; G_BITS];
    const HOLD_MS: u64 = 220; // how long a press keeps a key "held" absent a fresh report
    const DRAG_MS: u64 = 160; // how long pointer motion keeps the drag witness live
    let mut drag_until: u64 = 0;
    let mut game_wit_ms = crate::arch::ms(); // GAME-MODE ~1 Hz witness cadence

    loop {
        let top = crate::arch::now_cycles();
        // --- GAME-MODE input: held-key stepping + drag-rotate, frame-paced -----------------
        // Drain ALL pending events this frame (a burst never backs up one-per-frame; each drain
        // pass feeds the app-input watchdog via note_progress). A non-mapped key or a click exits.
        let gi = drain_game_input(pal);
        if gi.exit {
            break;
        }
        let now = crate::arch::ms();
        // Arm/refresh the hold window for each freshly-pressed movement key.
        for b in 0..G_BITS {
            if gi.pressed & (1 << b) != 0 {
                held_until[b] = now + HOLD_MS;
            }
        }
        // The currently-held mask: bits whose hold window has not lapsed.
        let mut held: u8 = 0;
        for b in 0..G_BITS {
            if now < held_until[b] {
                held |= 1 << b;
            }
        }
        if gi.motion && (gi.mdx != 0 || gi.mdy != 0) {
            drag_until = now + DRAG_MS;
        }
        let drag = now < drag_until;

        // Frame-paced stepping: fold held keys + pointer motion into per-frame yaw/pitch/zoom.
        const KEY_YAW: i32 = 4; // brad/frame while a yaw key is held
        const KEY_PITCH: i32 = 4; // brad/frame while a pitch key is held
        let mut yaw_step = 0i32;
        let mut pit_step = 0i32;
        if held & G_YAW_L != 0 {
            yaw_step -= KEY_YAW;
        }
        if held & G_YAW_R != 0 {
            yaw_step += KEY_YAW;
        }
        if held & G_PIT_U != 0 {
            pit_step -= KEY_PITCH;
        }
        if held & G_PIT_D != 0 {
            pit_step += KEY_PITCH;
        }
        // Zoom adjusts camera distance (nearer = larger crystal), clamped so |z| < dist keeps zc > 0.
        const ZOOM_STEP: Fx = ONE / 16;
        const DIST_MIN: Fx = 2 * ONE + ONE / 2;
        const DIST_MAX: Fx = 8 * ONE;
        if held & G_ZOOM_IN != 0 {
            dist = (dist - ZOOM_STEP).max(DIST_MIN);
        }
        if held & G_ZOOM_OUT != 0 {
            dist = (dist + ZOOM_STEP).min(DIST_MAX);
        }
        // Pointer motion → rotation (relative mice; absolute pointers drive only the cursor).
        yaw_step += gi.mdx;
        pit_step += gi.mdy;

        // Manual input this frame steps by the user; an idle frame keeps the auto-tumble.
        let manual = held != 0 || drag;
        if manual {
            ay = (ay + yaw_step) & 0xFF;
            ax = (ax + pit_step) & 0xFF;
        } else {
            ay = (ay + 3) & 0xFF; // idle yaw ~3 brad/frame
            ax = (ax + 1) & 0xFF; // idle pitch ~1 brad/frame — a tumble reads as two axes
        }

        // GAME-MODE witness ~1 Hz while active: the held mask + drag state the panel steps by.
        if manual && now.wrapping_sub(game_wit_ms) >= 1000 {
            serial_println!(":: [game] mode=step keys={:#04x} drag={} ::", held, drag);
            game_wit_ms = now;
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

        // --- VUG-FPS dirty-rect: erase only what changed, not the whole panel -------------
        // The flush is bandwidth-bound (a full clear reflushes the entire ~8 MB framebuffer every
        // frame — the metal 8–9 fps). Instead, background-fill only the crystal's footprint (union
        // of this frame's projected-vertex bbox with last frame's, so vacated pixels repaint) and
        // the cursor's old footprint. The HUD and corner meters clear their own small blocks in
        // `draw_stats`/`draw_meters`. `flush` then copies a few tight rectangles.
        if frame == 0 {
            pal.clear_screen(BG); // first frame only: establish a clean backdrop over the console
        }
        let mut bx0 = i32::MAX;
        let mut by0 = i32::MAX;
        let mut bx1 = i32::MIN;
        let mut by1 = i32::MIN;
        for i in 0..14 {
            bx0 = bx0.min(px[i]);
            by0 = by0.min(py[i]);
            bx1 = bx1.max(px[i]);
            by1 = by1.max(py[i]);
        }
        const PAD: i32 = 2; // cover the seam lines drawn a pixel outside the fill
        let cx0 = (bx0 - PAD).clamp(0, w) as usize;
        let cy0 = (by0 - PAD).clamp(0, h) as usize;
        let cx1 = (bx1 + PAD).clamp(0, w) as usize;
        let cy1 = (by1 + PAD).clamp(0, h) as usize;
        let cur_crystal = (cx0, cy0, cx1.saturating_sub(cx0), cy1.saturating_sub(cy0));
        // Erase the union of previous + current crystal footprints (frame 0 already cleared all).
        if frame != 0 {
            let (ex0, ey0, ex1, ey1) = match prev_crystal {
                Some((px0, py0, pw, ph)) => {
                    (cx0.min(px0), cy0.min(py0), cx1.max(px0 + pw), cy1.max(py0 + ph))
                }
                None => (cx0, cy0, cx1, cy1),
            };
            if ex1 > ex0 && ey1 > ey0 {
                pal.draw_rect(ex0, ey0, ex1 - ex0, ey1 - ey0, BG);
            }
            // Erase the cursor's previous footprint BEFORE redraw (so the crystal/HUD painted below
            // can cover it) — the dirty-rect path no longer wipes the whole panel each frame.
            if let Some((qx, qy, qw, qh)) = prev_cursor {
                pal.draw_rect(qx, qy, qw, qh, BG);
            }
        }
        prev_crystal = Some(cur_crystal);

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
        draw_meters(pal, &m, &cpu, h);
        // CURSOR-VIS: the cursor draws LAST, over everything, every frame. VUG-FPS: its previous
        // footprint was erased in the dirty-rect phase above; record the new one for next frame.
        crate::pal::cursor::draw(pal);
        prev_cursor = if crate::pal::cursor::visible() {
            let (curx, cury) = crate::pal::cursor::pos(w, h);
            let e = crate::pal::cursor::extent(pal);
            let pad = e / 8 + 1; // cover the one-block drop shadow offset
            Some((curx as usize, cury as usize, e + pad, e + pad))
        } else {
            None
        };

        pal.render(); // present ONCE per frame

        // VUG-FPS witness: accumulate this frame's flushed bytes and print a `[vugfps]` line ~1x/s.
        wit_bytes += pal.last_flush_bytes();
        wit_bands = wit_bands.max(pal.last_flush_bands());
        wit_frames += 1;
        {
            let wnow = crate::arch::ms();
            let wdt = wnow.wrapping_sub(wit_ms);
            if wdt >= 1000 && wit_frames > 0 {
                let fps_x10 = (wit_frames as u64 * 10_000) / wdt.max(1);
                let bpf = wit_bytes / wit_frames as u64;
                serial_println!(
                    ":: [vugfps] {}.{} fps  {} bytes/frame flushed  bands={} ({} frames / {} ms) ::",
                    fps_x10 / 10,
                    fps_x10 % 10,
                    bpf,
                    wit_bands,
                    wit_frames,
                    wdt
                );
                wit_ms = wnow;
                wit_frames = 0;
                wit_bytes = 0;
                wit_bands = 1;
            }
        }

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
            // Per-core CPU-pulse load — the honest two-source pick lives in `CpuPulse::refresh`.
            cpu.refresh(m.load);
            // BATMON-1 (x86, smc knob): refresh the cached battery snapshot on the same steady
            // cadence the meters redraw on (internally throttled to ~1 s of actual SMC port I/O).
            #[cfg(all(target_arch = "x86_64", feature = "smc"))]
            crate::drivers::smc::battery::refresh_if_due();
            win_ms = now_ms;
            win_frames = 0;
            work_acc = 0;
            total_acc = 0;
        }

        // (Rotation angles ay/ax are advanced at the top of the loop — held-key / drag stepping
        // when the user is driving, the idle auto-tumble otherwise.)
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
    // VUG-FPS: erase this HUD block's own footprint (the crystal-region clear does not reach the
    // top-left corner) so the changing frame counter doesn't ghost. Three text lines wide enough
    // for the longest ("mode solid  faces 24  frame NNNNN").
    pal.draw_rect(m.margin, m.margin, m.text_w(34), 3 * m.line_h, BG);
    pal.draw_text(m.margin, m.margin, "VUG // quartz", 0x00FFFFFF);
    let mode = if solid { "solid" } else { "wire " };
    let line = alloc::format!("mode {}  faces {:>2}  frame {}", mode, faces, frame);
    pal.draw_text(m.margin, m.margin + m.line_h, &line, 0x00A0A0A0);
    pal.draw_text(m.margin, m.margin + 2 * m.line_h, "press any key to exit", 0x00707070);
}

/// M3b/UI1-M2 — the two corner load meters (kept small; the crystal stays the star). Bottom-left:
/// a RENDER meter (the honest software "GPU monitor" — frame time, FPS, render busy-fraction bar,
/// triangles + estimated filled pixels this frame) above the CPU pulse row — Peter's sketch:
/// `CPU 1 ▮▮▮▮▯▯ 2 ▮▮▯▯▯▯ …` — per-core NUMBERED segment bars, one horizontal row, for however
/// many cores the scheduler reports. Filled ∝ load; empty segments draw dim, so an idle core reads
/// alive-but-empty, never blank. All geometry derives from the metrics (THE METRICS RULE). Both
/// meters draw through the same damage-tracked back buffer, so one-present-per-frame still holds.
fn draw_meters(pal: &mut TargetPal, m: &RenderStats, cpu: &CpuPulse, h: i32) {
    let mt = pal.metrics();
    let x0 = mt.margin;
    // The block is three text pitches (label / bar / readout) + one pitch of air + the CPU row.
    let base = (h as usize).saturating_sub(mt.margin + 4 * mt.line_h + mt.cell_h);

    // VUG-FPS: erase this widget's own footprint (the crystal-region clear does not reach the
    // bottom-left corner). Opaque bars over-paint themselves, but the changing text readouts and
    // the swept "breath" segment would ghost without a background clear first. Two blocks: the
    // RENDER label/bar/readout stack, and the CPU pulse row (sized to the per-core bars).
    pal.draw_rect(x0, base, mt.text_w(28), 3 * mt.line_h + mt.cell_h, BG);
    {
        let seg_w = mt.cell_w / 2;
        let gap = mt.scale;
        let bar_w = PULSE_SEGS * (seg_w + gap);
        let per_core = mt.text_w(2) + 2 * gap + bar_w + mt.cell_w;
        let row_w = mt.text_w(4) + cpu.ncpu * per_core;
        pal.draw_rect(x0, base + 4 * mt.line_h, row_w, mt.cell_h, BG);
    }

    // --- RENDER meter --------------------------------------------------------------------
    pal.draw_text(x0, base, "RENDER", METER_LABEL);
    let bar_w = mt.text_w(16);
    let bar_h = mt.cell_h;
    pal.draw_rect(x0, base + mt.line_h, bar_w, bar_h, METER_DIM);
    let fill = (m.load as usize * bar_w) / 100;
    pal.draw_rect(x0, base + mt.line_h, fill, bar_h, METER_LILAC);
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
    pal.draw_text(x0, base + 2 * mt.line_h, &line, METER_LABEL);

    // --- CPU pulse row (UI1-M2) ------------------------------------------------------------
    // `CPU 1 ▮▮▮▮▯▯ 2 ▮▮▯▯▯▯ …` — number, then a PULSE_SEGS-segment bar, per core. Corner
    // sizing: half-cell segments one cell tall; the full-screen `pulse` view draws the same
    // widget larger.
    let y = base + 4 * mt.line_h;
    let seg_w = mt.cell_w / 2;
    let seg_h = mt.cell_h;
    let gap = mt.scale;
    let mut x = x0;
    pal.draw_text(x, y, "CPU", METER_LABEL);
    x += mt.text_w(4);
    for c in 0..cpu.ncpu {
        let num = alloc::format!("{}", c + 1);
        pal.draw_text(x, y, &num, METER_LABEL);
        x += mt.text_w(num.len()) + 2 * gap;
        x = draw_pulse_bar(pal, x, y, seg_w, seg_h, gap, cpu.load[c], METER_PURPLE);
        x += mt.cell_w; // inter-core air
    }

    // --- BATMON-1 battery meter (x86, smc knob) -------------------------------------------
    // An honest on-screen battery bar fed by the cached SMC snapshot (no port I/O on this
    // per-frame path — `battery::cached()` is a cheap lock read; the main loop / the 200 ms meter
    // cadence own the refresh). When no SMC battery answers (QEMU, or a machine without one) it
    // renders an explicit empty state — never a fabricated number. Two text pitches above RENDER.
    #[cfg(all(target_arch = "x86_64", feature = "smc"))]
    {
        // BATMON-HOLD: `cached()` now returns the last GOOD snapshot plus its age — a failed SMC
        // sweep no longer clobbers the cache (the metal intermittency defect), so the widget holds
        // the last good reading and annotates its staleness instead of going dark.
        let (snap, age_ms) = crate::drivers::smc::battery::cached();
        /// A reading older than this (a few refresh periods) is flagged stale on the meter.
        const STALE_MS: u64 = 3000;
        let stale = snap.present && age_ms >= STALE_MS;
        let by = base.saturating_sub(2 * mt.line_h);
        // VUG-FPS: erase the battery block's footprint (two text lines) before repaint.
        pal.draw_rect(x0, by, mt.text_w(30), 2 * mt.line_h, BG);
        pal.draw_text(x0, by, "BATT", METER_LABEL);
        let bx = x0 + mt.text_w(5);
        let bw = mt.text_w(10);
        pal.draw_rect(bx, by, bw, mt.cell_h, METER_DIM);
        if let Some(soc) = snap.soc_pct {
            let fill = (soc.min(100) as usize * bw) / 100;
            // A stale bar renders in the label grey, not the live lilac — held, not fresh.
            let color = if stale { METER_LABEL } else { METER_LILAC };
            pal.draw_rect(bx, by, fill, mt.cell_h, color);
        }
        let line = if snap.present {
            let soc = snap.soc_pct.map(|v| v as i32).unwrap_or(-1);
            let mv = snap.volt_mv.map(|v| v as i32).unwrap_or(-1);
            let flow = match snap.amp_ma {
                Some(a) if a > 0 => "chg",
                Some(a) if a < 0 => "dis",
                _ => "--",
            };
            if stale {
                alloc::format!("{}%  {}mV  {}  stale {}s", soc, mv, flow, age_ms / 1000)
            } else {
                alloc::format!("{}%  {}mV  {}", soc, mv, flow)
            }
        } else {
            alloc::string::String::from("no SMC battery data")
        };
        pal.draw_text(x0, by + mt.line_h, &line, METER_LABEL);
    }
}

/// UI1-M3 — `pulse`: the full-screen system monitor (a BeOS Pulse homage) — the in-kernel half of
/// the pulse ask (a host-native `vessels/pulse` vessel is a separate arc). Big per-core segmented
/// pulse bars (the M2 widget, larger) plus the honest system lines available today: core count,
/// uptime, frame/present time while the view is open.
///
/// The loop follows the vug contract: it runs INSIDE the shell command, pumps input itself
/// (`pal::pump_and_poll`), presents exactly ONCE per frame, busy-polls + `yield_now`s between
/// frames (never WFI/sleeps — the post-drop aarch64 rule), and ANY key exits back to the console
/// (the caller repaints; `took_screen` honored). Per-core loads ride the shared [`CpuPulse`]
/// sampler — the honest two-source rule, verbatim; this loop's own busy-vs-yield fraction is the
/// unscheduled-demo-core fallback, exactly like the crystal's.
pub fn run_pulse(pal: &mut TargetPal) {
    let mt = pal.metrics();
    let mut cpu = CpuPulse::new("PULSE");
    serial_println!(":: PULSE: live — {} cores ::", cpu.ncpu);

    // Big-bar geometry: double-size segments, one row per core, all metrics-derived.
    let seg_w = 2 * mt.cell_w;
    let seg_h = 2 * mt.cell_h;
    let gap = 2 * mt.scale;
    let row_pitch = seg_h + mt.line_h;
    let label_y_off = (seg_h - mt.cell_h) / 2; // centre a text cell on the bar

    let mut frame: u64 = 0;
    let mut own_load: u32 = 0; // this loop's busy% — the demo-core fallback source
    let mut fps_x10: u64 = 0;
    let mut ms_x10: u64 = 0;
    let mut prev_top = crate::arch::now_cycles();
    let mut work_acc: u64 = 0;
    let mut total_acc: u64 = 0;
    let mut win_frames: u32 = 0;
    let mut win_ms = crate::arch::ms();

    loop {
        let top = crate::arch::now_cycles();
        // --- input: exit on any key; track the mouse (CURSOR-VIS, the crystal's idiom) ----
        if drain_input(pal) {
            break;
        }

        // --- draw the monitor ------------------------------------------------------------
        pal.clear_screen(BG);
        pal.draw_text(mt.margin, mt.margin, "PULSE // UnaOS system monitor", 0x00FFFFFF);
        pal.draw_text(mt.margin, mt.margin + mt.line_h, "press any key to exit", 0x00707070);

        // One row per core: `CPU n`, the big segment bar, the load percent.
        let mut y = mt.margin + 3 * mt.line_h;
        for c in 0..cpu.ncpu {
            let label = alloc::format!("CPU {}", c + 1);
            pal.draw_text(mt.margin, y + label_y_off, &label, METER_LABEL);
            let bar_x = mt.margin + mt.text_w(7);
            let end_x = draw_pulse_bar(pal, bar_x, y, seg_w, seg_h, gap, cpu.load[c], METER_PURPLE);
            // VUG-HONESTY: a parked core prints `park`, not a fabricated percent.
            let pct = if cpu.load[c] == PARKED {
                alloc::string::String::from("park")
            } else {
                alloc::format!("{:>3}%", cpu.load[c])
            };
            pal.draw_text(end_x + mt.cell_w, y + label_y_off, &pct, METER_LABEL);
            y += row_pitch;
        }

        // The honest system lines available today.
        y += mt.line_h;
        let line = alloc::format!(
            "cores {}   uptime {} ms   frame {}",
            cpu.ncpu,
            crate::arch::ms(),
            frame
        );
        pal.draw_text(mt.margin, y, &line, METER_LABEL);
        y += mt.line_h;
        let line = alloc::format!(
            "frame {}.{} ms   {}.{} fps   (draw+present, while open)",
            ms_x10 / 10,
            ms_x10 % 10,
            fps_x10 / 10,
            fps_x10 % 10
        );
        pal.draw_text(mt.margin, y, &line, METER_LABEL);

        // CURSOR-VIS: cursor last, over everything (frame cleared above; no erase needed).
        crate::pal::cursor::draw(pal);

        pal.render(); // present ONCE per frame

        // --- window accounting (the M3b render-load idiom) --------------------------------
        let end = crate::arch::now_cycles();
        work_acc += end.wrapping_sub(top);
        total_acc += top.wrapping_sub(prev_top);
        prev_top = top;
        win_frames += 1;
        let now_ms = crate::arch::ms();
        let dt = now_ms.wrapping_sub(win_ms);
        if dt >= 200 && win_frames > 0 {
            fps_x10 = (win_frames as u64 * 10_000) / dt.max(1);
            ms_x10 = (dt * 10) / win_frames as u64;
            own_load = if total_acc > 0 {
                ((work_acc * 100) / total_acc).min(100) as u32
            } else {
                0
            };
            cpu.refresh(own_load);
            win_ms = now_ms;
            win_frames = 0;
            work_acc = 0;
            total_acc = 0;
        }

        frame += 1;
        crate::arch::sched::yield_now();
    }

    serial_println!(":: PULSE: exit clean — {} frames ::", frame);
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
