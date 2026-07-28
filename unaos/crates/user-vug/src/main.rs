#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UVUG-3: the first INTERACTIVE EL0 application — a userspace mini-vug that draws a real vug-style
// wireframe quartz crystal and responds to live keyboard/mouse. A static ELF64 (aarch64) program,
// loaded by the kernel's EXEC-1 machinery (`run_user_image`) into a fresh per-process slot and run at
// EL0 — the identical path the operator drives with `run /fat/VUG.ELF`.
//
// WHAT IT DOES
//   1. WC-C: creates its own 128x128 ARGB8888 WINDOW via SYS_WIN_CREATE — a real compositor window with
//      kernel-drawn chrome, tiled beside whatever else is on the panel, rather than the single 32x32
//      full-screen-centred compat surface SYS_FB_MAP exposed. 128x128 is `boot::FB_WIN_MAX_W/H` (one
//      64 KiB window slot); the crystal projection is screen-space-scaled to it.
//   2. Spawns TWO PERSISTENT EL0 worker threads via SYS_THREAD_SPAWN — one co-located, one on a SIBLING
//      CORE — each of which rasterises HALF of the surface (worker A: rows 0..64, worker B: rows 64..128):
//      it clears its band to the background and Bresenham-draws every crystal edge clipped to its band,
//      from the projected vertex coordinates the parent publishes each frame.
//      VUGGUARD: both spawns are CHECKED, and the thread pool is a request, not a guarantee — the
//      kernel's thread-handle table is a small GLOBAL pool that returns -EAGAIN when full. Any band
//      without a worker (spawn refused, or a live worker that misses the frame barrier's pass budget)
//      is rasterised INLINE by the parent instead, so the program degrades to single-threaded and
//      keeps drawing rather than blocking on a barrier that can never complete. See `_start`.
//   3. Each frame the PARENT reads input (SYS_INPUT_POLL), folds it into per-frame rotation/zoom state,
//      rotates + projects the 14 crystal vertices (integer Q16.16 math reimplemented from the kernel
//      vug.rs — no float), publishes the pixel coordinates, RELEASES both workers (the `phase` word),
//      blocks on a FUTEX until both have ARRIVED (the `done` word), and PRESENTS (SYS_WIN_PRESENT).
//   4. On exit (ESC, or the interactive frame cap) it signals the workers to leave, JOINs both,
//      and prints its witness before exiting 0.
//
// VUGCLICK — CLICK SEMANTICS IN A WINDOWED WORLD. Until this arc a click EXITED the program. That rule
// was written for the full-screen takeover era, where the vug owned the panel and any click meant "done".
// Since WC-C there is no takeover mode left to reach: every vug creates its own compositor window
// (SYS_WIN_CREATE, unconditional, below), tiled beside other windows. In that world clicking is how an
// operator focuses or interacts with a window, so "click exits" meant every attempt to touch a vug killed
// it — and with WC-J erasing a dead window instantly, the death read as a spontaneous crash (P62,
// "vug is crashing", on a wire showing no panic, no fault, and the program's own designed exit path).
// So: a click no longer exits. It toggles a PAUSE of the rotation — a harmless, visible, reversible
// interaction that also proves the click reached EL0. A DRAG (motion at or over CLICK_THRESH between
// press and release) still rotates, exactly as before. ESC remains the keyboard exit, unchanged and now
// the only operator-driven one.
//
// TWO PATHS — deterministic auto (QEMU) vs interactive (metal). The switch is INPUT-DRIVEN, not
// time-boxed (UVUG-4): the parent polls SYS_INPUT_POLL EVERY frame for the program's whole life, and
// the FIRST input event AT ANY FRAME flips it to interactive permanently. There is no detection window
// to race — the old DETECT_FRAMES fallback closed in well under a second at EL0 frame rates, before a
// human could touch a key.
//   * QEMU raspi4b delivers no USB HID, so no input ever arrives — zero events ever — and the program
//     stays on the deterministic auto path: it keeps the fixed idle tumble (yaw += 3, pitch += 1
//     brad/frame) exactly as it did from frame 0, runs to AUTO_FRAMES (300) total, computes a
//     deterministic FNV-1a checksum of the final surface (a pure integer function of the final frame's
//     geometry, independent of thread interleaving), and prints
//     `:: UVUG: frames=300 threads=2 checksum=<hex> ::` — the existing witness, still green and
//     deterministic. This is what the kernel's `uvug_witness` boot self-test asserts exit=0 on.
//   * On metal a keypress/mouse arrives whenever the operator acts, so the program enters INTERACTIVE
//     mode at that frame: it prints `:: UVUG: interactive takeover at frame <n> ::` (proving the input
//     arrived on metal), cancels the auto-tumble and the 300-frame cap, and switches to held-state
//     control — WASD/arrows rotate (TRUE held state from KeyDown/KeyUp), Q/E zoom, a mouse drag rotates
//     (per-frame clamped delta, full-panel-drag ≈ one revolution), a click toggles pause, ESC exits. It
//     runs until ESC and prints `:: UVUG: interactive exit=<key|frames …> frames=<n> ::`.
//     Interactive is metal-only (no HID in QEMU).
//
// VUGLIFE — DESKTOP VUGS DO NOT DIE OF OLD AGE. INTERACTIVE_CAP was the last surviving demo-era run
// deadline, and it killed exactly the vugs an operator was using. The kill is worse than the number
// suggests: a DETACHED vug already runs its auto path uncapped (VUG-BG), so it can sit on the desktop
// for hundreds of thousands of frames — and the moment the operator TABS TO IT, the first input event
// flips `interactive` on, the already-past cap is tested for the first time, and the program exits
// instantly. That is P64's "the vugs crash as I tab": four deaths, all
// `:: UVUG: interactive exit=frames frames=<36000..271484> ::`, no fault anywhere on the wire — the
// same shape as the VUGCLICK relic, a designed exit that a long-lived desktop turned into a crash.
//
// The split is by LAUNCH MODE, which the program can already see (the info-page DETACHED bit), not by
// a kernel-side special case:
//   * DETACHED (`bg /fat/VUG.ELF` — the desktop spawn): UNBOUNDED. It exits on ESC or `kill`, never on
//     a frame counter. At the frame the old cap would have fired it prints ONE
//     `[vuglife] budget waived (interactive) frames=<n>` line and keeps running, so the next attended
//     boot PROVES the waiver fired rather than inferring it from an absence.
//   * FOREGROUND (`run`, and every fixture/battery launch — `uvug_witness`, BGRUN-ST): the bounded
//     budget STAYS. Gate liveness depends on a vug that terminates, and a foreground run is exactly
//     what the batteries drive. When that exit is taken it now says
//     `exit=frames_budget frames=<n> (fixture mode)` — a single bare token in the parsed field, the
//     prose qualifier outside it — so no future sitting re-diagnoses this as a crash.
// The deterministic AUTO path (AUTO_FRAMES = 300, the checksum witness) is untouched in both modes.
//
// Barrier direction split (deliberate, robust under QEMU raspi4b's lack of a Group-1 timer IRQ — see
// docs userspace.md M6e): ARRIVAL (worker -> parent) is a real FUTEX (workers atomically bump `done` +
// SYS_FUTEX WAKE, the parent SYS_FUTEX WAITs); RELEASE (parent -> worker) is a SYS_YIELD poll on `phase`
// (keeps each worker runnable on its own core, needing no cross-core wake). Both wait loops re-check their
// condition, so the barrier is lost-wakeup-safe. On metal (real timer IRQs) either direction works.
//
// EL0 owns only the OFF-SCREEN surface bytes — never the scan-out, never a physical address, never a
// kernel mapping (SYS_WIN_PRESENT is the only surface->screen path, and it runs in the kernel). Window
// CHROME is drawn by the kernel from its own copy of the title, so this program cannot forge a frame.
// Page-permission laws (per-page perms, WXN) are untouched.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------------------------
// Syscall ABI (Linux-aarch64): x8 = number, args x0..x5, return in x0. The kernel SVC path preserves
// every GPR except x0.
// ---------------------------------------------------------------------------------------------
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;
const SYS_YIELD: u64 = 4;
const SYS_GETINFO: u64 = 7;
const SYS_THREAD_SPAWN: u64 = 21;
const SYS_THREAD_EXIT: u64 = 22;
const SYS_THREAD_JOIN: u64 = 23;
const SYS_FUTEX: u64 = 26;
const SYS_INPUT_POLL: u64 = 27;
// WC-C: the WINDOW verbs replace the single-surface SYS_FB_MAP/SYS_FB_PRESENT compat pair.
const SYS_WIN_CREATE: u64 = 29;
const SYS_WIN_PRESENT: u64 = 30;

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
unsafe fn sys2(n: u64, a0: u64, a1: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!(
        "svc #0",
        inout("x0") a0 => r,
        in("x1") a1,
        in("x8") n,
        options(nostack),
    );
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
// WC-C: the crystal renders into a 128x128 WINDOW surface (SYS_WIN_CREATE), not the 32x32 compat page.
// 128x128 is `boot::FB_WIN_MAX_W/H` — exactly one 64 KiB window slot — and is 4x the linear resolution
// the compat path allowed, so the wireframe is drawn rather than approximated. FOCAL scales with it (6 ->
// 24 px/unit) so the crystal occupies the SAME fraction of its surface as before; the visible change is
// sharpness, not framing.
const SW: i32 = 128; // surface width  (px)
const SH: i32 = 128; // surface height (px)
const STRIDE: usize = 512; // ARGB8888 row stride (bytes)
const FOCAL: i32 = 24; // pixels-per-unit at the crystal's centre depth
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
const AUTO_FRAMES: u32 = 300; // deterministic QEMU path length (used only while no input ever arrives)
// VUGLIFE: the interactive budget. It is a real deadline ONLY in fixture mode (a foreground launch —
// `run`, and every battery leg); a detached/desktop vug waives it once and runs unbounded.
const INTERACTIVE_CAP: u32 = 36000; // interactive frame budget (fixture mode); waived when detached

// Drag-rotate sensitivity (UVUG-4). The kernel game-mode (vug.rs) maps pointer motion 1 px = 1 brad
// with no scaling; Peter found that too twitchy. The panel is ~1920 px wide (mailbox FALLBACK_W), so we
// scale pointer delta down to make a full-panel drag ≈ one revolution (256 brads over ~2048 px):
// DRAG_DIV = 8 gives 256 brads per 2048 px. Each per-frame step is clamped so one large HID delta can't
// spin the crystal past a quarter-turn in a single frame.
const DRAG_DIV: i32 = 8; // px → brad divisor (full-panel drag ≈ one revolution)
const DRAG_CLAMP: i32 = 64; // max |brad| a single frame's drag may contribute per axis

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

/// UVUG-9 — the per-frame cap on how many input events one frame may consume.
///
/// ROOT CAUSE (P54b freeze). The UVUG-8 drain was an UNBOUNDED `loop { poll() }`: it ran until the ring
/// reported empty, with no bound whatsoever. That is the only phase of this program's frame loop that can
/// call `SYS_INPUT_POLL` indefinitely WITHOUT reaching the present, and the kernel's own UVUG-8r2
/// instrumentation proves that is exactly where P54b sat: the run held its takeover suspension for the full
/// `TAKEOVER_SUSPEND_MAX_SECS` (60 s), which requires the heartbeat — stamped ONLY by `sys_input_poll` — to
/// have stayed fresher than `TAKEOVER_STALE_SECS` (2 s) on every pass, while `EL0_FOCUSED_PRESENT_COUNT` —
/// bumped by `sys_fb_present` under the IDENTICAL focus predicate — never moved once. Polling forever,
/// presenting never, is not a state any other phase of this loop can occupy. Hence: the drain spun.
///
/// A drain that outlives its frame is a rendering freeze even though nothing is deadlocked: the workers stay
/// parked on `phase`, the surface keeps its last frame, the screen shows a static crystal, and the kernel's
/// no-render cap eventually ends the run. Bounding the drain converts that hard freeze into, at worst, input
/// latency — the leftovers are simply consumed by the NEXT frame's drain, which is what a frame budget is for.
///
/// The cap is 2x the kernel's `INPUT_RING_CAP` (32), so a frame can always empty a completely full ring plus a
/// full ring's worth of concurrent arrivals; hitting it means the producer is outrunning a tight EL0 syscall
/// loop, which no HID device can legitimately do. That anomaly is witnessed (`[uvug9] drain saturated`) rather
/// than absorbed silently — it names the remaining upstream suspect for P55 instead of hiding it.
const MAX_DRAIN_PER_FRAME: u32 = 64;

/// Accumulated input for one frame.
#[derive(Default)]
struct FrameInput {
    any: bool,      // any event at all this frame (arms interactive mode)
    exit_key: bool, // ESC pressed
    /// VUGCLICK: a click (button press+release under the motion threshold). NO LONGER AN EXIT — see
    /// the click-semantics note above `_start`'s frame loop. It toggles the pause state.
    clicks: u32,
    mdx: i32, // summed relative mouse dx while dragging
    mdy: i32, // summed relative mouse dy while dragging
    /// UVUG-9: the drain hit `MAX_DRAIN_PER_FRAME` with the ring still non-empty — the freeze signature.
    saturated: bool,
}

/// Drain this frame's queued input events. Updates `held`/`dragging`/`drag_motion` in place and returns the
/// per-frame accumulation. BOUNDED at `MAX_DRAIN_PER_FRAME` events (see that constant for the P54b root
/// cause): whatever is left stays in the ring for the next frame, so the render/present half of the loop is
/// always reached.
fn drain_input(held: &mut u32, dragging: &mut bool, drag_motion: &mut i32) -> FrameInput {
    const CLICK_THRESH: i32 = 6;
    let mut fi = FrameInput::default();
    let mut budget = MAX_DRAIN_PER_FRAME;
    loop {
        if budget == 0 {
            fi.saturated = true;
            break; // frame budget spent — the rest waits for the next frame (never spin here)
        }
        budget -= 1;
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
                    // Release edge: little motion = a CLICK (pause toggle); otherwise it ended a drag.
                    if *drag_motion < CLICK_THRESH {
                        fi.clicks += 1;
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
// UVUG-9 — the per-frame STALL WITNESS.
//
// P54b showed a rendering freeze with no crash, no fault and no deadlock report: the crystal stopped moving
// while the program kept polling. From the outside, "stopped presenting" is all you can see; from in here we
// can say WHICH PHASE of the frame stopped making progress. The loop has exactly three phases that can fail
// to complete, and this witness names them:
//
//   [uvug9] stall frame=<n> phase=poll drained=<64> — the drain hit its frame budget with the ring still
//       non-empty. The P54b signature. Post-fix this is no longer fatal (the frame proceeds), so the line is a
//       DIAGNOSIS of a runaway upstream producer, not a symptom of this program.
//   [uvug9] stall frame=<n> phase=barrier done=<0|1> — the frame-barrier wait burned its pass budget with
//       fewer than its live workers arrived; `done` says how many did (0 = neither worker ran, 1 = one worker
//       wedged). Distinguishes a worker-side wedge from an input-side one. VUGGUARD: this is now a DEADLINE
//       as well as a witness — on the pass that prints it, the parent retires the worker pool and takes every
//       band inline, so the line marks the ONE frame that presents partially-stale content, not a permanent
//       state. A worker that was merely slow rather than wedged sees PHASE_EXIT and leaves.
//   [uvug9] stall frame=<n> phase=present rc=<errno> — SYS_WIN_PRESENT returned an error. UVUG-8 IGNORED this
//       syscall's return entirely, so a present that started failing mid-run would have looked exactly like a
//       freeze: frames advancing, nothing on screen, and the kernel's no-render cap firing. Now it is visible.
//
// GATING. There is no env/knob channel into EL0, so each phase self-gates on its own ANOMALY and latches after
// the first report. Consequences: a healthy run prints nothing new, and — decisive for the gates — the
// deterministic QEMU auto path (no HID, no events, workers always arrive, present always succeeds) reaches no
// anomaly at all, so its 300-frame surface checksum is untouched.
//
// The barrier budget is expressed in PASSES, not milliseconds: EL0 has no clock syscall (SYS_GETINFO's tick
// field would need a copy_to_user round trip per pass, which would itself distort the thing being measured),
// and a pass budget is both deterministic and precisely what "made no progress" means here. LIMITATION, stated
// rather than papered over: this catches a barrier that is SPINNING (futex_wait returning -EAGAIN on a value
// mismatch), not one PARKED forever on a lost wakeup. A parked parent cannot execute a witness at all, and the
// kernel's futex compares `*uaddr` against `val` under the same bucket lock `futex_wake` takes, so that park is
// race-free by construction — a lost wakeup here is refuted in the kernel, not monitored from EL0.
//
// VUGGUARD, on that limitation: P60's wedge WAS a park, and no pass budget could ever have caught it — the
// parent was blocked in `futex_wait` on a `done` count that no living thread would ever bump, because the
// spawns had been refused and it had not looked. That class is closed STRUCTURALLY, not by monitoring: the
// barrier's target is the number of workers that exist, so with none it is never entered. The budget below
// remains for the narrower case it can see — a thread that exists and stops arriving.
const BARRIER_PASS_BUDGET: u32 = 1 << 20;

/// One-shot latches, one per phase, so a witness fires at most once per program run.
static W_POLL: AtomicU32 = AtomicU32::new(0);
static W_BARRIER: AtomicU32 = AtomicU32::new(0);
static W_PRESENT: AtomicU32 = AtomicU32::new(0);

/// Emit one `[uvug9] stall` line: `frame`, the phase name, and one labelled detail value.
fn stall_witness(latch: &AtomicU32, frame: u32, phase: &[u8], label: &[u8], value: u32) {
    if latch.swap(1, Ordering::Relaxed) != 0 {
        return; // already reported this phase — never flood the serial line
    }
    let mut b = Buf::new();
    b.put(b"[uvug9] stall frame=");
    b.put_dec(frame);
    b.put(b" phase=");
    b.put(phase);
    b.put(b" ");
    b.put(label);
    b.put(b"=");
    b.put_dec(value);
    b.put(b"\n");
    b.flush();
}

// ---------------------------------------------------------------------------------------------
// VUGFPS — the on-window frames-per-second readout.
//
// The stagger observation (s1p: replacement vugs visibly outpace the originals) needs a PER-VUG
// number, and the serial line cannot carry one per frame for six windows. So each vug measures and
// draws its own rate in its top-left corner: frames presented per second, from `SYS_GETINFO`'s
// `ticks` field (the 250 Hz scheduler tick — the only EL0-reachable clock; CNTVCT_EL0 is not
// EL0-enabled). One getinfo per frame is one syscall beside the existing input poll; the displayed
// value refreshes once per second, so the digits are readable rather than flickering.
//
// CHECKSUM DISCIPLINE: the overlay is drawn ONLY when `detached || interactive` — a desktop
// (`bg`) or operator-driven vug. The FOREGROUND auto path (every fixture/battery leg, the QEMU
// 300-frame checksum witness) takes neither branch and its surface stays byte-identical.
// ---------------------------------------------------------------------------------------------
const TICK_HZ: u32 = 250; // kernel scheduler tick rate (timer.rs TICK_HZ)
const FPS_C: u32 = 0xFFE8_C98A; // fps digits — warm amber, same as user-stat's pid

/// 5x7 digit glyphs, one byte per row, bit 4 = leftmost column (verbatim from user-stat).
static GLYPHS: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111], // 2
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
];

/// Draw one digit at (x, y), 1:1 scale (5x7 px — the window is only 128 wide).
unsafe fn draw_digit(surf: *mut u8, d: usize, x: i32, y: i32, color: u32) {
    let g = &GLYPHS[d % 10];
    let mut row = 0i32;
    while row < 7 {
        let bits = g[row as usize];
        let mut col = 0i32;
        while col < 5 {
            if bits & (1 << (4 - col)) != 0 {
                put_px(surf, x + col, y + row, color);
            }
            col += 1;
        }
        row += 1;
    }
}

/// `SYS_GETINFO` -> the kernel's 250 Hz tick count, or 0 on error (a 0 delta just skips the update).
fn getinfo_ticks() -> u64 {
    let mut info = [0u64; 2]; // {pid, ticks}, #[repr(C)] — see kernel sys_getinfo
    let p = info.as_mut_ptr() as u64;
    if unsafe { sys1(SYS_GETINFO, p) } >> 63 != 0 {
        return 0;
    }
    info[1]
}

/// Draw the current fps (clamped to 999) in the top-left corner over whatever the frame rendered.
/// Runs in the PARENT, after the frame barrier and before the present, so no worker is writing.
unsafe fn draw_fps(surf: *mut u8, fps: u32) {
    let v = fps.min(999);
    let n: i32 = if v >= 100 { 3 } else if v >= 10 { 2 } else { 1 };
    // Backing box so the digits stay readable over the wireframe: n digits at 6 px advance + 2 px pad.
    let mut y = 0;
    while y < 11 {
        let row = surf.add((y as usize) * STRIDE) as *mut u32;
        let mut x = 0usize;
        while x < (n * 6 + 3) as usize {
            row.add(x).write_volatile(BG);
            x += 1;
        }
        y += 1;
    }
    let mut i = 0i32;
    let mut div = 1u32;
    let mut k = 1i32;
    while k < n {
        div *= 10;
        k += 1;
    }
    while i < n {
        draw_digit(surf, ((v / div) % 10) as usize, 2 + i * 6, 2, FPS_C);
        div /= 10;
        i += 1;
    }
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

    // WC-C: create a 128x128 WINDOW instead of mapping the 32x32 compat surface. `SYS_WIN_CREATE`
    // returns the window ID (>= 0) and maps the negotiated 16-page surface slot; the surface VA is the
    // window region's slot 0, at a FIXED offset from the program's own window base (base + 0x5000 —
    // `boot::fb_info_va() + FB_INFO_SIZE`), which is the same VA `SYS_FB_MAP` used to return. Nothing is
    // guessed: the kernel publishes the geometry in the RO info page at base + 0x4000, and the surface
    // slot layout is part of the window ABI (userspace.md).
    //
    // Fail-closed: a negative return means no window (no free slot, no per-process FB region). There is
    // nothing to draw into, so exit rather than write to an unmapped VA and take a fatal EL0 abort.
    let win = unsafe { sys2(SYS_WIN_CREATE, SW as u64, SH as u64) };
    if win >> 63 != 0 {
        let mut b = Buf::new();
        b.put(b":: UVUG: SYS_WIN_CREATE failed ::\n");
        b.flush();
        exit(1);
    }
    // VUG-BG: read the process-flags word the kernel publishes in the RO info page (base + 0x4000, u32
    // index 0x20/4 — see the info-page layout in userspace.md). Bit 0 says this process was started
    // DETACHED, i.e. by `bg /fat/VUG.ELF` rather than `run`. A detached vug has no operator to press ESC
    // and, being unfocused, never receives input — so the 300-frame auto cap would end it in about a
    // second, which is exactly what read as "the app crashed" on the bench. Detached therefore means: run
    // the SAME deterministic auto path, but with no frame cap, until `kill` takes it down.
    //
    // Read AFTER SYS_WIN_CREATE, which is what maps the info page and publishes the word — reading it
    // before the window exists would fault on an unmapped VA.
    let detached = unsafe { ((base + 0x4000) as *const u32).add(0x20 / 4).read_volatile() } & 1 != 0;
    let surf_va = base + 0x5000;
    SURF.store(surf_va, Ordering::Release);
    let surf = surf_va as *mut u8;

    // Spawn the two worker threads (one co-located, one on a sibling core). Stacks carved from the
    // window: A top = B+0x3000, B top = B+0x3800 (identical layout to UVUG-1).
    //
    // VUGGUARD: CHECK BOTH RETURNS. `SYS_THREAD_SPAWN` returns a negative errno when it cannot give
    // this process a thread — notably `-EAGAIN` when the kernel's fixed thread-handle table is full,
    // which is a GLOBAL, system-wide resource, not a per-process one. Until this arc the two returns
    // were captured only to be joined at exit, so a vug that got NO workers ran the whole frame loop
    // as though it had them: `DONE` could never reach 2, the frame barrier below blocked forever, and
    // the process parked in `futex_wait` BEFORE its first `SYS_WIN_PRESENT` — kernel-drawn chrome with
    // no content, unkillable from the shell. That is P60's empty window, and its root in this program
    // is exactly one thing: the app proceeded as though a resource it requested had been granted.
    //
    // The chosen behaviour is DEGRADE, not fail-fast. Every band a worker does not own, the parent
    // rasterises INLINE with the identical `render_band` on the identical published projection — so a
    // vug launched while the thread table is full still comes up, still draws, still responds to
    // input and still exits cleanly; it is merely single-threaded. It costs no restructuring of the
    // frame loop (the inline raster sits between the release and the barrier, exactly where the
    // parent otherwise idles) and therefore leaves the WC-H present discipline untouched. Because the
    // raster is the same function over the same coordinates, the final surface — and so the
    // deterministic auto-path CHECKSUM — is byte-identical to the two-worker run.
    let entry = uvug_worker as *const () as u64;
    let rc_a = unsafe { sys4(SYS_THREAD_SPAWN, entry, base + 0x3000, 0, 0) };
    let rc_b = unsafe { sys4(SYS_THREAD_SPAWN, entry, base + 0x3800, 1, 1) };
    let ok_a = rc_a >> 63 == 0;
    let ok_b = rc_b >> 63 == 0;
    let spawned = ok_a as u32 + ok_b as u32;

    // Bands the PARENT must rasterise itself this run (top = rows 0..64, bottom = rows 64..128).
    let mut inline_top = !ok_a;
    let mut inline_bot = !ok_b;
    // How many worker arrivals the frame barrier may legitimately wait for. Never more than the number
    // of threads that actually exist — a barrier target that cannot be reached is the wedge itself.
    let mut live: u32 = spawned;
    // Handles are joinable only if they are real. Joining a value that is a negative errno is a bogus
    // syscall; joining a thread that never started would be a lie about what was reclaimed.
    let mut join_a = ok_a;
    let mut join_b = ok_b;

    if spawned < 2 {
        // Name the denied resource on the serial line. This is the diagnostic whose absence made P60
        // look like a compositor fault: the app knew it had been refused and said nothing.
        let mut sb = Buf::new();
        sb.put(b":: UVUG: SYS_THREAD_SPAWN denied a=");
        sb.put_dec(if ok_a { 0 } else { (rc_a as i64).unsigned_abs() as u32 });
        sb.put(b" b=");
        sb.put_dec(if ok_b { 0 } else { (rc_b as i64).unsigned_abs() as u32 });
        sb.put(b" workers=");
        sb.put_dec(spawned);
        sb.put(b" -> inline raster ::\n");
        sb.flush();
    }

    let vbase = crystal_vertices();

    // Interactive/auto state.
    let mut ay: i32 = 0;
    let mut ax: i32 = 0;
    let mut dist: Fx = 4 * ONE;
    let mut held: u32 = 0;
    let mut dragging = false;
    let mut drag_motion: i32 = 0;

    let mut interactive = false; // flipped permanently by the first input event, at any frame
    let mut exit_key = false;
    // VUGCLICK: rotation pause, toggled by a click. Purely cosmetic and interactive-only.
    let mut paused = false;
    // VUGLIFE: one-shot latch for the waived-budget witness (detached/interactive only).
    let mut budget_waived = false;

    // VUGFPS measurement state: the tick/frame pair at the last displayed-value refresh.
    let mut fps_ticks: u64 = getinfo_ticks();
    let mut fps_frame: u32 = 0;
    let mut fps: u32 = 0;

    let mut frame: u32 = 0;
    loop {
        // --- input (polled EVERY frame for the program's whole life) ---
        let fi = drain_input(&mut held, &mut dragging, &mut drag_motion);
        if fi.saturated {
            // UVUG-9: the drain spent its whole frame budget with events still queued. Pre-fix this loop had
            // no budget to spend and simply never returned — the P54b freeze. Report once and carry on.
            stall_witness(&W_POLL, frame, b"poll", b"drained", MAX_DRAIN_PER_FRAME);
        }
        if !interactive && fi.any {
            // First input at any frame takes over: cancel the auto-tumble + the 300-frame cap and
            // switch to held-state control. The witness proves the input arrived on metal.
            interactive = true;
            let mut tb = Buf::new();
            tb.put(b":: UVUG: interactive takeover at frame ");
            tb.put_dec(frame);
            tb.put(b" ::\n");
            tb.flush();
        }
        if interactive {
            if fi.exit_key {
                exit_key = true;
            }
            // VUGCLICK: a click toggles pause; it does NOT exit. Clicks are human-rate, so one line per
            // toggle cannot flood the serial log, and the line doubles as proof that a click reached EL0.
            let mut c = fi.clicks;
            while c > 0 {
                paused = !paused;
                c -= 1;
            }
            if fi.clicks > 0 {
                let mut pb = Buf::new();
                pb.put(b":: UVUG: click pause=");
                pb.put_dec(paused as u32);
                pb.put(b" ::\n");
                pb.flush();
            }
        }

        // --- fold input into rotation/zoom ---
        let manual = interactive && (held != 0 || (dragging && (fi.mdx != 0 || fi.mdy != 0)));
        if paused {
            // VUGCLICK: clicked-to-pause — hold the current orientation. Rendering and presenting
            // continue unchanged, so the window stays live (and killable) rather than going dark.
        } else if manual {
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
                // Pointer motion → rotation, scaled + per-frame clamped (see DRAG_DIV/DRAG_CLAMP).
                yaw += (fi.mdx / DRAG_DIV).clamp(-DRAG_CLAMP, DRAG_CLAMP);
                pit += (fi.mdy / DRAG_DIV).clamp(-DRAG_CLAMP, DRAG_CLAMP);
            }
            ay = (ay + yaw) & 0xFF;
            ax = (ax + pit) & 0xFF;
        } else {
            // Idle tumble — the SAME deterministic advance from frame 0, so the auto path's 300-frame
            // checksum is a pure function of the frame count.
            ay = (ay + 3) & 0xFF;
            ax = (ax + 1) & 0xFF;
        }

        // --- transform + publish, then release any LIVE workers ---
        // VUGGUARD: the release is conditional on there being someone to release. With `live == 0` the
        // phase word is nobody's signal, and storing to it would be the only remaining way for this
        // program to advertise a frame it is rendering entirely by itself.
        project(&vbase, ay, ax, dist);
        if live > 0 {
            DONE.store(0, Ordering::Relaxed);
            PHASE.store(frame + 1, Ordering::Release); // 1-based; never PHASE_EXIT (frame < cap)
        }

        // --- VUGGUARD: rasterise every band no worker owns, inline, while the workers run ---
        // Placed AFTER the release and BEFORE the barrier so the healthy path is untouched (both
        // predicates false, no work) and the degraded path keeps whatever parallelism it still has:
        // with one worker alive, the parent draws the other half concurrently with it. The bands are
        // disjoint by construction and `draw_line`/`put_px` clip to the band, so no two writers ever
        // touch a pixel.
        if inline_top {
            unsafe { render_band(surf, 0, SH / 2) };
        }
        if inline_bot {
            unsafe { render_band(surf, SH / 2, SH) };
        }

        // --- barrier: wait for the live workers to arrive (FUTEX) ---
        // UVUG-9: the wait itself is unchanged (re-check + compare-and-block is lost-wakeup-safe); a pass
        // counter is added, so a barrier that spins without its workers arriving names itself once.
        //
        // VUGGUARD makes the barrier honest in two ways. First, it waits for `live`, never a fixed 2:
        // with no workers it does not execute at all, and with one it waits for one. Second, the pass
        // budget is now a DEADLINE, not just a printed observation — burning it means a thread that
        // does exist is not arriving, so the parent RETIRES the worker pool: it signals PHASE_EXIT,
        // drops `live` to zero and takes both bands inline for the rest of the run. Every later frame
        // then renders and presents with no wait at all. UVUG-9 printed the same witness and went
        // straight back into `futex_wait`, i.e. it diagnosed the wedge and then re-entered it.
        let mut passes: u32 = 0;
        while live > 0 {
            let d = DONE.load(Ordering::Acquire);
            if d >= live {
                break;
            }
            passes = passes.wrapping_add(1);
            if passes == BARRIER_PASS_BUDGET {
                stall_witness(&W_BARRIER, frame, b"barrier", b"done", d);
                PHASE.store(PHASE_EXIT, Ordering::Release); // a worker still alive will see this and leave
                live = 0;
                inline_top = true;
                inline_bot = true;
                // Do NOT join a retired worker. `sys_thread_join` blocks until the thread finishes, so
                // joining the very thread that just failed to arrive would park this parent forever at
                // exit — the exact symptom this arc removes. The kernel handle it holds is leaked, and
                // that is the deliberate trade: a leaked row is the kernel's to reclaim, a parked
                // process is not recoverable from anywhere.
                join_a = false;
                join_b = false;
                break;
            }
            futex_wait(core::ptr::addr_of!(DONE), d);
        }

        // --- VUGFPS: measure, refresh once per second, draw (desktop/interactive only) ---
        if detached || interactive {
            let now = getinfo_ticks();
            if now > fps_ticks {
                let dt = (now - fps_ticks) as u32;
                if dt >= TICK_HZ {
                    // frames since last refresh, scaled to per-second at the 250 Hz tick.
                    fps = ((frame.wrapping_sub(fps_frame) as u64 * TICK_HZ as u64 + (dt / 2) as u64)
                        / dt as u64) as u32;
                    fps_ticks = now;
                    fps_frame = frame;
                }
            } else if now != 0 && now < fps_ticks {
                // A stale first read (getinfo error returned 0) — resync rather than divide nonsense.
                fps_ticks = now;
                fps_frame = frame;
            }
            unsafe { draw_fps(surf, fps) };
        }

        // --- present ---
        // UVUG-9: CHECK the return. UVUG-8 discarded it, so a `sys_fb_present` that began failing mid-run (a
        // lost per-process slot, a torn-down surface) would present as an unexplained freeze — frames still
        // advancing here, nothing changing on screen, and the kernel's no-render cap firing on a program that
        // believed it was drawing. An error has bit 63 set (negative errno), exactly like an empty input poll.
        let rc = unsafe { sys1(SYS_WIN_PRESENT, win) };
        if rc >> 63 != 0 {
            stall_witness(&W_PRESENT, frame, b"present", b"rc", (rc as i64).unsigned_abs() as u32);
        }
        frame += 1;

        // --- exit conditions ---
        if interactive {
            // VUGCLICK: ESC ends an interactive run. A click does not.
            if exit_key {
                break;
            }
            // VUGLIFE: the frame budget binds only a FIXTURE-mode (foreground) run. A detached vug is a
            // desktop window with an operator in front of it: waive the budget once, say so on the wire,
            // and keep tumbling until ESC or `kill`. The witness is one-shot — `budget_waived` latches —
            // because the test is true on every frame after the cap, and a per-frame line would drown
            // the serial log it exists to be found in.
            if frame >= INTERACTIVE_CAP {
                if !detached {
                    break;
                }
                if !budget_waived {
                    budget_waived = true;
                    let mut wb = Buf::new();
                    wb.put(b"[vuglife] budget waived (interactive) frames=");
                    wb.put_dec(frame);
                    wb.put(b"\n");
                    wb.flush();
                }
            }
        } else if !detached && frame >= AUTO_FRAMES {
            // No input has ever arrived (QEMU): the deterministic auto path ends at 300 frames — the
            // surface at that frame is what the checksum witness asserts. VUG-BG: a DETACHED launch skips
            // this cap entirely and tumbles until it is killed. The two are disjoint by construction —
            // the checksum witness runs through `run_user_image` (foreground), which clears the detached
            // bit — so the 300-frame checksum is untouched by this branch.
            break;
        }
    }

    // Signal the workers to exit, then join the ones that exist and are still expected to answer.
    PHASE.store(PHASE_EXIT, Ordering::Release);
    if join_a {
        unsafe { sys1(SYS_THREAD_JOIN, rc_a) };
    }
    if join_b {
        unsafe { sys1(SYS_THREAD_JOIN, rc_b) };
    }

    // Witness.
    let mut buf = Buf::new();
    if interactive {
        buf.put(b":: UVUG: interactive exit=");
        // VUGCLICK: the reason must be true. Pre-arc the only two spellings were `key` and `click`, so a
        // run that ran out its INTERACTIVE_CAP reported `click` — a click that never happened. With click
        // no longer an exit, the honest pair is `key` (ESC) and `frames` (the safety cap).
        // VUGLIFE: the `frames` spelling now names its own cause. Post-arc it can only be reached by a
        // FOREGROUND (fixture/battery) run — a detached desktop vug waives the budget instead — so the
        // line says so, and the next sitting that meets it need not re-derive that this was designed.
        // The reason stays a SINGLE BARE TOKEN (`frames_budget`, not `frames (budget, …)`): the bench
        // parses this line with `exit=(\w+)`, and spaces or parens inside the field would break it. The
        // human-readable qualifier therefore rides AFTER `frames=<n>`, outside every parsed field.
        buf.put(if exit_key {
            b"key" as &[u8]
        } else {
            b"frames_budget"
        });
        buf.put(b" frames=");
        buf.put_dec(frame);
        if !exit_key {
            buf.put(b" (fixture mode)");
        }
        buf.put(b" ::\n");
    } else {
        let cksum = surface_checksum(surf);
        buf.put(b":: UVUG: frames=");
        buf.put_dec(frame);
        // VUGGUARD: report the workers this run actually GOT, not the two it asked for. On the healthy
        // path that is the literal `2` this line has always carried (the gate REQUIREs the exact
        // string); on a degraded run it is the honest count, and the checksum beside it is unchanged
        // because the parent rasterised the orphaned bands with the same code over the same geometry.
        buf.put(b" threads=");
        buf.put_dec(spawned);
        buf.put(b" checksum=0x");
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
