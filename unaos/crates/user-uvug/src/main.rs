#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UVUG-3: the first INTERACTIVE EL0 application — a userspace mini-vug that draws a real vug-style
// wireframe quartz crystal and responds to live keyboard/mouse. A static ELF64 (aarch64) program,
// loaded by the kernel's EXEC-1 machinery (`run_user_image`) into a fresh per-process slot and run at
// EL0 — the identical path the operator drives with `run /fat/UVUG.ELF`.
//
// WHAT IT DOES
//   1. Maps its dedicated 32x32 ARGB8888 off-screen surface via SYS_FB_MAP (a 4 KiB surface page + a
//      read-only info page). 32x32 is the surface size the kernel's SYS_FB_MAP exposes (one page; see
//      boot.rs FB_SURFACE_W/FB_REGION_SIZE) — the crystal projection is screen-space-scaled to it.
//   2. Spawns TWO PERSISTENT EL0 worker threads via SYS_THREAD_SPAWN — one co-located, one on a SIBLING
//      CORE — each of which rasterises HALF of the surface (worker A: rows 0..16, worker B: rows 16..32):
//      it clears its band to the background and Bresenham-draws every crystal edge clipped to its band,
//      from the projected vertex coordinates the parent publishes each frame.
//   3. Each frame the PARENT reads input (SYS_INPUT_POLL), folds it into per-frame rotation/zoom state,
//      rotates + projects the 14 crystal vertices (integer Q16.16 math reimplemented from the kernel
//      vug.rs — no float), publishes the pixel coordinates, RELEASES both workers (the `phase` word),
//      blocks on a FUTEX until both have ARRIVED (the `done` word), and PRESENTS (SYS_FB_PRESENT).
//   4. On exit (ESC / click, or the interactive frame cap) it signals the workers to leave, JOINs both,
//      and prints its witness before exiting 0.
//
// TWO PATHS — deterministic auto (QEMU) vs interactive (metal):
//   * QEMU raspi4b delivers no USB HID, so no input ever arrives. If NO input event is seen within the
//     first DETECT_FRAMES frames, the program COMMITS to the deterministic auto path: it keeps the fixed
//     idle tumble (yaw += 3, pitch += 1 brad/frame) exactly as it did from frame 0, runs to AUTO_FRAMES
//     (300) total, computes a deterministic FNV-1a checksum of the final surface (a pure integer function
//     of the final frame's geometry, independent of thread interleaving), and prints
//     `:: UVUG: frames=300 threads=2 checksum=<hex> ::` — the existing witness, still green and
//     deterministic. This is what the kernel's `uvug_witness` boot self-test asserts exit=0 on.
//   * On metal a keypress/mouse arrives immediately, so the program enters INTERACTIVE mode: WASD/arrows
//     rotate (TRUE held state from KeyDown/KeyUp), Q/E zoom, a mouse drag rotates, a click or ESC exits.
//     It runs until an exit event (bounded by INTERACTIVE_CAP = 36000 frames as a safety) and prints
//     `:: UVUG: interactive exit=<key|click> frames=<n> ::`. Interactive is metal-only (no HID in QEMU).
//
// Barrier direction split (deliberate, robust under QEMU raspi4b's lack of a Group-1 timer IRQ — see
// docs userspace.md M6e): ARRIVAL (worker -> parent) is a real FUTEX (workers atomically bump `done` +
// SYS_FUTEX WAKE, the parent SYS_FUTEX WAITs); RELEASE (parent -> worker) is a SYS_YIELD poll on `phase`
// (keeps each worker runnable on its own core, needing no cross-core wake). Both wait loops re-check their
// condition, so the barrier is lost-wakeup-safe. On metal (real timer IRQs) either direction works.
//
// EL0 owns only the OFF-SCREEN surface bytes — never the scan-out, never a physical address, never a
// kernel mapping (SYS_FB_PRESENT is the only surface->screen path, and it runs in the kernel).
// Page-permission laws (per-page perms, WXN) are untouched.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------------------------
// Syscall ABI (Linux-aarch64): x8 = number, args x0..x5, return in x0. The kernel SVC path preserves
// every GPR except x0.
// ---------------------------------------------------------------------------------------------
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;
const SYS_YIELD: u64 = 4;
const SYS_THREAD_SPAWN: u64 = 21;
const SYS_THREAD_EXIT: u64 = 22;
const SYS_THREAD_JOIN: u64 = 23;
const SYS_FB_MAP: u64 = 24;
const SYS_FB_PRESENT: u64 = 25;
const SYS_FUTEX: u64 = 26;
const SYS_INPUT_POLL: u64 = 27;

#[inline(always)]
unsafe fn sys0(n: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!("svc #0", inout("x0") 0u64 => r, in("x8") n, options(nostack));
    r
}
#[inline(always)]
unsafe fn sys1(n: u64, a0: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!("svc #0", inout("x0") a0 => r, in("x8") n, options(nostack));
    r
}
#[inline(always)]
unsafe fn sys3(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!(
        "svc #0",
        inout("x0") a0 => r,
        in("x1") a1,
        in("x2") a2,
        in("x8") n,
        options(nostack),
    );
    r
}
#[inline(always)]
unsafe fn sys4(n: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!(
        "svc #0",
        inout("x0") a0 => r,
        in("x1") a1,
        in("x2") a2,
        in("x3") a3,
        in("x8") n,
        options(nostack),
    );
    r
}

#[inline(always)]
fn sys_yield() {
    unsafe { sys0(SYS_YIELD) };
}
#[inline(always)]
fn write_bytes(p: *const u8, len: usize) {
    unsafe { sys3(SYS_WRITE, 1, p as u64, len as u64) };
}
#[inline(always)]
fn exit(code: i32) -> ! {
    unsafe { sys1(SYS_EXIT, code as u64) };
    loop {
        core::hint::spin_loop();
    }
}
#[inline(always)]
fn futex_wait(word: *const AtomicU32, val: u32) {
    unsafe { sys3(SYS_FUTEX, word as u64, 0, val as u64) };
}
#[inline(always)]
fn futex_wake(word: *const AtomicU32, n: u32) {
    unsafe { sys3(SYS_FUTEX, word as u64, 1, n as u64) };
}
#[inline(always)]
fn input_poll() -> u64 {
    unsafe { sys0(SYS_INPUT_POLL) }
}

// ---------------------------------------------------------------------------------------------
// Fixed-point maths (Q16.16), reimplemented from kernel vug.rs. No float.
// ---------------------------------------------------------------------------------------------
type Fx = i32;
const ONE: Fx = 1 << 16;

/// sin(theta) in Q16.16, theta in brads (256 brads = one turn). Verbatim from vug.rs::SIN.
static SIN: [Fx; 256] = [
    0, 1608, 3216, 4821, 6424, 8022, 9616, 11204, 12785, 14359, 15924, 17479, 19024, 20557, 22078,
    23586, 25080, 26558, 28020, 29466, 30893, 32303, 33692, 35062, 36410, 37736, 39040, 40320,
    41576, 42806, 44011, 45190, 46341, 47464, 48559, 49624, 50660, 51665, 52639, 53581, 54491,
    55368, 56212, 57022, 57798, 58538, 59244, 59914, 60547, 61145, 61705, 62228, 62714, 63162,
    63572, 63944, 64277, 64571, 64827, 65043, 65220, 65358, 65457, 65516, 65536, 65516, 65457,
    65358, 65220, 65043, 64827, 64571, 64277, 63944, 63572, 63162, 62714, 62228, 61705, 61145,
    60547, 59914, 59244, 58538, 57798, 57022, 56212, 55368, 54491, 53581, 52639, 51665, 50660,
    49624, 48559, 47464, 46341, 45190, 44011, 42806, 41576, 40320, 39040, 37736, 36410, 35062,
    33692, 32303, 30893, 29466, 28020, 26558, 25080, 23586, 22078, 20557, 19024, 17479, 15924,
    14359, 12785, 11204, 9616, 8022, 6424, 4821, 3216, 1608, 0, -1608, -3216, -4821, -6424, -8022,
    -9616, -11204, -12785, -14359, -15924, -17479, -19024, -20557, -22078, -23586, -25080, -26558,
    -28020, -29466, -30893, -32303, -33692, -35062, -36410, -37736, -39040, -40320, -41576, -42806,
    -44011, -45190, -46341, -47464, -48559, -49624, -50660, -51665, -52639, -53581, -54491, -55368,
    -56212, -57022, -57798, -58538, -59244, -59914, -60547, -61145, -61705, -62228, -62714, -63162,
    -63572, -63944, -64277, -64571, -64827, -65043, -65220, -65358, -65457, -65516, -65536, -65516,
    -65457, -65358, -65220, -65043, -64827, -64571, -64277, -63944, -63572, -63162, -62714, -62228,
    -61705, -61145, -60547, -59914, -59244, -58538, -57798, -57022, -56212, -55368, -54491, -53581,
    -52639, -51665, -50660, -49624, -48559, -47464, -46341, -45190, -44011, -42806, -41576, -40320,
    -39040, -37736, -36410, -35062, -33692, -32303, -30893, -29466, -28020, -26558, -25080, -23586,
    -22078, -20557, -19024, -17479, -15924, -14359, -12785, -11204, -9616, -8022, -6424, -4821,
    -3216, -1608,
];

#[inline(always)]
fn fsin(brad: i32) -> Fx {
    SIN[(brad & 0xFF) as usize]
}
#[inline(always)]
fn fcos(brad: i32) -> Fx {
    SIN[((brad + 64) & 0xFF) as usize]
}
#[inline(always)]
fn fmul(a: Fx, b: Fx) -> Fx {
    (((a as i64) * (b as i64)) >> 16) as Fx
}

// ---------------------------------------------------------------------------------------------
// The crystal: an elongated hexagonal bipyramid (a quartz point) — 14 vertices, reimplemented from
// vug.rs. Wireframe: 30 edges.
// ---------------------------------------------------------------------------------------------
const APEX: Fx = 88474; // 1.35
const TY: Fx = 32768; //   0.50 — half prism height
const RING: [(Fx, Fx); 6] = [
    (52429, 0),
    (26214, 45405),
    (-26214, 45405),
    (-52429, 0),
    (-26214, -45405),
    (26214, -45405),
];

/// Base (un-rotated) crystal vertices as (x, y, z) Q16.16 triples.
fn crystal_vertices() -> [(Fx, Fx, Fx); 14] {
    let mut v = [(0, 0, 0); 14];
    v[0] = (0, APEX, 0); // top apex
    let mut i = 0;
    while i < 6 {
        v[1 + i] = (RING[i].0, TY, RING[i].1); // top ring
        v[7 + i] = (RING[i].0, -TY, RING[i].1); // bottom ring
        i += 1;
    }
    v[13] = (0, -APEX, 0); // bottom apex
    v
}

/// The 30 wireframe edges (vertex index pairs).
static EDGES: [(u8, u8); 30] = [
    // top apex -> top ring
    (0, 1), (0, 2), (0, 3), (0, 4), (0, 5), (0, 6),
    // top ring loop
    (1, 2), (2, 3), (3, 4), (4, 5), (5, 6), (6, 1),
    // vertical prism edges
    (1, 7), (2, 8), (3, 9), (4, 10), (5, 11), (6, 12),
    // bottom ring loop
    (7, 8), (8, 9), (9, 10), (10, 11), (11, 12), (12, 7),
    // bottom apex -> bottom ring
    (13, 7), (13, 8), (13, 9), (13, 10), (13, 11), (13, 12),
];

// ---------------------------------------------------------------------------------------------
// Surface geometry + palette.
// ---------------------------------------------------------------------------------------------
const SW: i32 = 32; // surface width  (px)
const SH: i32 = 32; // surface height (px)
const STRIDE: usize = 128; // ARGB8888 row stride (bytes)
const FOCAL: i32 = 6; // pixels-per-unit at the crystal's centre depth
const BG: u32 = 0xFF1E_1E1E; // opaque Can-Am dark grey
const EDGE: u32 = 0xFFC9_A6E8; // opaque paler lilac seam

// ---------------------------------------------------------------------------------------------
// Shared state (same address space across parent + both workers; the phase/done words carry the
// release/acquire handoff, so the plain PX/PY writes the parent makes before the Release store on
// PHASE are visible to a worker after its Acquire load of PHASE).
// ---------------------------------------------------------------------------------------------
static PHASE: AtomicU32 = AtomicU32::new(0); // parent publishes frame+1; workers yield-poll; MAX = exit
static DONE: AtomicU32 = AtomicU32::new(0); // workers bump on arrival; parent futex-waits for 2
static SURF: AtomicU64 = AtomicU64::new(0); // surface VA (parent sets before spawning workers)
static mut PX: [i32; 14] = [0; 14]; // projected pixel X per vertex
static mut PY: [i32; 14] = [0; 14]; // projected pixel Y per vertex

const PHASE_EXIT: u32 = u32::MAX;
const AUTO_FRAMES: u32 = 300; // deterministic QEMU path length
const DETECT_FRAMES: u32 = 60; // frames without input before committing to the auto path
const INTERACTIVE_CAP: u32 = 36000; // interactive safety bound

// ---------------------------------------------------------------------------------------------
// Rasterisation (worker side).
// ---------------------------------------------------------------------------------------------
#[inline(always)]
unsafe fn put_px(surf: *mut u8, x: i32, y: i32, color: u32) {
    if x < 0 || x >= SW || y < 0 || y >= SH {
        return;
    }
    let off = (y as usize) * STRIDE + (x as usize) * 4;
    (surf.add(off) as *mut u32).write_volatile(color);
}

/// Bresenham line, plotting only points whose row is in [y_lo, y_hi) (the worker's band). Off-band and
/// off-surface points are skipped, so a worker never writes outside its half.
unsafe fn draw_line(surf: *mut u8, mut x0: i32, mut y0: i32, x1: i32, y1: i32, y_lo: i32, y_hi: i32, color: u32) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        if y0 >= y_lo && y0 < y_hi {
            put_px(surf, x0, y0, color);
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

/// Render one worker's band for a frame: clear the band to BG, then draw every crystal edge clipped to
/// the band from the shared projected coordinates.
unsafe fn render_band(surf: *mut u8, y_lo: i32, y_hi: i32) {
    // Clear the band.
    let mut y = y_lo;
    while y < y_hi {
        let row = surf.add((y as usize) * STRIDE) as *mut u32;
        let mut x = 0usize;
        while x < SW as usize {
            row.add(x).write_volatile(BG);
            x += 1;
        }
        y += 1;
    }
    // Draw edges (read the parent-published projection).
    let px = &*core::ptr::addr_of!(PX);
    let py = &*core::ptr::addr_of!(PY);
    let mut i = 0usize;
    while i < EDGES.len() {
        let (a, b) = EDGES[i];
        let (a, b) = (a as usize, b as usize);
        draw_line(surf, px[a], py[a], px[b], py[b], y_lo, y_hi, EDGE);
        i += 1;
    }
}

// ---------------------------------------------------------------------------------------------
// Worker thread entry. arg 0 = top half (rows 0..16), arg 1 = bottom half (rows 16..32).
// ---------------------------------------------------------------------------------------------
#[no_mangle]
extern "C" fn uvug_worker(arg: usize) -> ! {
    let surf = SURF.load(Ordering::Acquire) as *mut u8;
    let (y_lo, y_hi) = if arg == 0 { (0, SH / 2) } else { (SH / 2, SH) };
    let mut last: u32 = 0;
    loop {
        // Wait for the parent to release the next frame: yield-poll `phase` until it changes.
        let p = loop {
            let p = PHASE.load(Ordering::Acquire);
            if p != last {
                break p;
            }
            sys_yield();
        };
        last = p;
        if p == PHASE_EXIT {
            unsafe { sys0(SYS_THREAD_EXIT) };
            loop {
                core::hint::spin_loop();
            }
        }
        unsafe { render_band(surf, y_lo, y_hi) };
        // Arrive: atomically bump `done`, then FUTEX WAKE the parent.
        DONE.fetch_add(1, Ordering::Release);
        futex_wake(core::ptr::addr_of!(DONE), 1);
    }
}

// ---------------------------------------------------------------------------------------------
// Input decode (SYS_INPUT_POLL packed u64; see docs userspace.md ELF-5).
// ---------------------------------------------------------------------------------------------
const EV_KEYDOWN: u64 = 1;
const EV_KEYUP: u64 = 2;
const EV_MOUSE_REL: u64 = 3;
const EV_BUTTON: u64 = 5;

// Held-state bits.
const H_YAW_L: u32 = 1 << 0;
const H_YAW_R: u32 = 1 << 1;
const H_PIT_U: u32 = 1 << 2;
const H_PIT_D: u32 = 1 << 3;
const H_ZOOM_IN: u32 = 1 << 4;
const H_ZOOM_OUT: u32 = 1 << 5;

// HID-KEYS arrow C0 codes (see vug.rs) and ESC.
const K_RIGHT: u8 = 0x1C;
const K_LEFT: u8 = 0x1D;
const K_DOWN: u8 = 0x1E;
const K_UP: u8 = 0x1F;
const K_ESC: u8 = 0x1B;

fn key_bit(k: u8) -> u32 {
    let k = k.to_ascii_lowercase();
    match k {
        b'a' | K_LEFT => H_YAW_L,
        b'd' | K_RIGHT => H_YAW_R,
        b'w' | K_UP => H_PIT_U,
        b's' | K_DOWN => H_PIT_D,
        b'e' | b'+' | b'=' => H_ZOOM_IN,
        b'q' | b'-' | b'_' => H_ZOOM_OUT,
        _ => 0,
    }
}

/// Accumulated input for one frame.
#[derive(Default)]
struct FrameInput {
    any: bool,        // any event at all this frame (arms interactive mode)
    exit_key: bool,   // ESC pressed
    exit_click: bool, // a click (button press+release under the motion threshold)
    mdx: i32,         // summed relative mouse dx while dragging
    mdy: i32,         // summed relative mouse dy while dragging
}

/// Drain every queued input event this frame. Updates `held`/`dragging`/`drag_motion` in place and
/// returns the per-frame accumulation.
fn drain_input(held: &mut u32, dragging: &mut bool, drag_motion: &mut i32) -> FrameInput {
    const CLICK_THRESH: i32 = 6;
    let mut fi = FrameInput::default();
    loop {
        let ev = input_poll();
        if ev >> 63 != 0 {
            break; // -EAGAIN: ring empty
        }
        fi.any = true;
        let ty = (ev >> 48) & 0xFF;
        let lo = ev & 0xFFFF_FFFF;
        match ty {
            EV_KEYDOWN => {
                let k = (lo & 0xFF) as u8;
                if k == K_ESC {
                    fi.exit_key = true;
                } else {
                    *held |= key_bit(k);
                }
            }
            EV_KEYUP => {
                let k = (lo & 0xFF) as u8;
                let b = key_bit(k);
                if b != 0 {
                    *held &= !b;
                }
            }
            EV_BUTTON => {
                let mask = lo & 0xFF;
                if mask != 0 {
                    // Press edge: start a drag (also clears held keys — the vug.rs hot-unplug net).
                    *dragging = true;
                    *drag_motion = 0;
                    *held = 0;
                } else {
                    // Release edge: a click (little motion) exits; otherwise end the drag.
                    if *drag_motion < CLICK_THRESH {
                        fi.exit_click = true;
                    }
                    *dragging = false;
                }
            }
            EV_MOUSE_REL => {
                let dx = ((lo >> 16) & 0xFFFF) as u16 as i16 as i32;
                let dy = (lo & 0xFFFF) as u16 as i16 as i32;
                if *dragging {
                    *drag_motion += dx.abs() + dy.abs();
                    fi.mdx += dx;
                    fi.mdy += dy;
                }
            }
            _ => {}
        }
    }
    fi
}

// ---------------------------------------------------------------------------------------------
// Transform: rotate + project the 14 vertices into PX/PY.
// ---------------------------------------------------------------------------------------------
fn project(base: &[(Fx, Fx, Fx); 14], ay: i32, ax: i32, dist: Fx) {
    let (sy, cy) = (fsin(ay), fcos(ay));
    let (sx, cx) = (fsin(ax), fcos(ax));
    let mut i = 0usize;
    while i < 14 {
        let (vx, vy, vz) = base[i];
        // Rotate around Y then X (vug.rs::Vec3::rotate).
        let x1 = fmul(vx, cy) - fmul(vz, sy);
        let z1 = fmul(vx, sy) + fmul(vz, cy);
        let y2 = fmul(vy, cx) - fmul(z1, sx);
        let z2 = fmul(vy, sx) + fmul(z1, cx);
        let zc = (z2 + dist).max(ONE / 4); // keep depth positive
        let ppu = (FOCAL as i64) * (dist as i64) / (zc as i64);
        let sxp = SW / 2 + (((x1 as i64) * ppu) >> 16) as i32;
        let syp = SH / 2 - (((y2 as i64) * ppu) >> 16) as i32;
        unsafe {
            (*core::ptr::addr_of_mut!(PX))[i] = sxp;
            (*core::ptr::addr_of_mut!(PY))[i] = syp;
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------------------------------
// FNV-1a 64-bit over the whole surface (deterministic auto-path witness).
// ---------------------------------------------------------------------------------------------
fn surface_checksum(surf: *const u8) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let prime: u64 = 0x0000_0100_0000_01b3;
    let mut i = 0usize;
    while i < (SH as usize) * STRIDE {
        let byte = unsafe { surf.add(i).read_volatile() };
        h ^= byte as u64;
        h = h.wrapping_mul(prime);
        i += 1;
    }
    h
}

// ---------------------------------------------------------------------------------------------
// Tiny formatting into a byte buffer (no core::fmt — keep the text segment small).
// ---------------------------------------------------------------------------------------------
struct Buf {
    b: [u8; 96],
    n: usize,
}
impl Buf {
    fn new() -> Self {
        Buf { b: [0; 96], n: 0 }
    }
    fn put(&mut self, s: &[u8]) {
        let mut i = 0;
        while i < s.len() && self.n < self.b.len() {
            self.b[self.n] = s[i];
            self.n += 1;
            i += 1;
        }
    }
    fn put_hex64(&mut self, mut v: u64) {
        let digits = b"0123456789abcdef";
        let mut i = 0;
        while i < 16 {
            let nib = ((v >> 60) & 0xF) as usize;
            if self.n < self.b.len() {
                self.b[self.n] = digits[nib];
                self.n += 1;
            }
            v <<= 4;
            i += 1;
        }
    }
    fn put_dec(&mut self, v: u32) {
        let mut tmp = [0u8; 10];
        let mut k = 0;
        let mut x = v;
        if x == 0 {
            self.put(b"0");
            return;
        }
        while x > 0 {
            tmp[k] = b'0' + (x % 10) as u8;
            x /= 10;
            k += 1;
        }
        while k > 0 {
            k -= 1;
            let c = tmp[k];
            self.put(&[c]);
        }
    }
    fn flush(&self) {
        write_bytes(self.b.as_ptr(), self.n);
    }
}

// ---------------------------------------------------------------------------------------------
// Program entry: the parent thread. Forced first in .text so e_entry lands on it.
// ---------------------------------------------------------------------------------------------
#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let base = _start as *const () as u64; // the window base B (entry VA == base, e_entry offset 0)

    // Map the off-screen surface.
    let surf_va = unsafe { sys0(SYS_FB_MAP) };
    SURF.store(surf_va, Ordering::Release);
    let surf = surf_va as *mut u8;

    // Spawn the two worker threads (one co-located, one on a sibling core). Stacks carved from the
    // window: A top = B+0x3000, B top = B+0x3800 (identical layout to UVUG-1).
    let entry = uvug_worker as *const () as u64;
    let handle_a = unsafe { sys4(SYS_THREAD_SPAWN, entry, base + 0x3000, 0, 0) };
    let handle_b = unsafe { sys4(SYS_THREAD_SPAWN, entry, base + 0x3800, 1, 1) };

    let vbase = crystal_vertices();

    // Interactive/auto state.
    let mut ay: i32 = 0;
    let mut ax: i32 = 0;
    let mut dist: Fx = 4 * ONE;
    let mut held: u32 = 0;
    let mut dragging = false;
    let mut drag_motion: i32 = 0;

    let mut interactive = false; // seen input within the detection window
    let mut committed_auto = false; // detection window elapsed with no input
    let mut exit_key = false;
    let mut exit_click = false;

    let mut frame: u32 = 0;
    loop {
        // --- input ---
        let fi = drain_input(&mut held, &mut dragging, &mut drag_motion);
        if !interactive && !committed_auto {
            if fi.any {
                interactive = true;
            } else if frame >= DETECT_FRAMES {
                committed_auto = true;
            }
        }
        if interactive {
            if fi.exit_key {
                exit_key = true;
            }
            if fi.exit_click {
                exit_click = true;
            }
        }

        // --- fold input into rotation/zoom ---
        let manual = interactive && (held != 0 || (dragging && (fi.mdx != 0 || fi.mdy != 0)));
        if manual {
            let mut yaw = 0i32;
            let mut pit = 0i32;
            if held & H_YAW_L != 0 {
                yaw -= 4;
            }
            if held & H_YAW_R != 0 {
                yaw += 4;
            }
            if held & H_PIT_U != 0 {
                pit -= 4;
            }
            if held & H_PIT_D != 0 {
                pit += 4;
            }
            if held & H_ZOOM_IN != 0 {
                dist = (dist - ONE / 16).max(2 * ONE + ONE / 2);
            }
            if held & H_ZOOM_OUT != 0 {
                dist = (dist + ONE / 16).min(8 * ONE);
            }
            if dragging {
                yaw += fi.mdx;
                pit += fi.mdy;
            }
            ay = (ay + yaw) & 0xFF;
            ax = (ax + pit) & 0xFF;
        } else {
            // Idle tumble — the SAME deterministic advance from frame 0, so the auto path's 300-frame
            // checksum is a pure function of the frame count.
            ay = (ay + 3) & 0xFF;
            ax = (ax + 1) & 0xFF;
        }

        // --- transform + publish, then release both workers ---
        project(&vbase, ay, ax, dist);
        DONE.store(0, Ordering::Relaxed);
        PHASE.store(frame + 1, Ordering::Release); // 1-based; never PHASE_EXIT (frame < cap)

        // --- barrier: wait for both workers to arrive (FUTEX) ---
        loop {
            let d = DONE.load(Ordering::Acquire);
            if d >= 2 {
                break;
            }
            futex_wait(core::ptr::addr_of!(DONE), d);
        }

        // --- present ---
        unsafe { sys0(SYS_FB_PRESENT) };
        frame += 1;

        // --- exit conditions ---
        if interactive {
            if exit_key || exit_click || frame >= INTERACTIVE_CAP {
                break;
            }
        } else if committed_auto && frame >= AUTO_FRAMES {
            break;
        }
    }

    // Signal the workers to exit, then join both.
    PHASE.store(PHASE_EXIT, Ordering::Release);
    unsafe { sys1(SYS_THREAD_JOIN, handle_a) };
    unsafe { sys1(SYS_THREAD_JOIN, handle_b) };

    // Witness.
    let mut buf = Buf::new();
    if interactive {
        buf.put(b":: UVUG: interactive exit=");
        buf.put(if exit_key { b"key" } else { b"click" });
        buf.put(b" frames=");
        buf.put_dec(frame);
        buf.put(b" ::\n");
    } else {
        let cksum = surface_checksum(surf);
        buf.put(b":: UVUG: frames=");
        buf.put_dec(frame);
        buf.put(b" threads=2 checksum=0x");
        buf.put_hex64(cksum);
        buf.put(b" ::\n");
    }
    buf.flush();

    exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
