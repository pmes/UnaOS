// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! RAST-1 demo: a spinning, flat-shaded, z-buffered cube rendered through the
//! `rast` software rasterizer and presented via the existing panel `Screen`.
//!
//! This is the knob-gated (`UNAOS_RAST=1` → `rast` feature) x86/virt wire-in of
//! the platform-neutral `rast` crate. It is **call-never-edit** with respect to
//! the shared video path: it renders into its own heap-owned RGBA8 back buffer
//! (the "double buffer"), then presents each frame through the public
//! `Screen::put_pixel` / `Screen::flush` API — it does not touch `FrameBuffer`,
//! `Screen`, or any other shared surface code.
//!
//! With the feature off the whole module is unlinked and the kernel image is
//! byte-identical to baseline.

extern crate alloc;
use alloc::vec;

use rast::math::PI;
use rast::raster::{Rgba, Target};
use rast::{render_mesh, Mat4, Vec3};

/// Number of frames the demo renders before handing the panel back to the shell.
/// Bounded so QEMU boots straight through to the interactive path (no hang), and
/// so the honest fps line has a fixed sample count.
const FRAMES: u32 = 90;

/// The demo renders at a fixed modest resolution and blits the result centered on
/// the panel. Presenting a full 1280×800 through per-pixel `Screen::put_pixel`
/// every frame is far too slow (~1 M pokes/frame); a fixed render size keeps the
/// software rasterizer witnessable and the fps line honest regardless of panel
/// geometry. Rendering itself is resolution-independent (the crate is general).
const DEMO_W: usize = 320;
const DEMO_H: usize = 240;

/// Target wall-clock frame interval for honest pacing. Without it the render+present
/// loop runs as fast as the platform allows — on the Orin panel all 90 frames land in
/// ~91 ms (989 fps), so the "spinning cube" presents as a ~0.1 s blue flash. Holding
/// each frame to a fixed cadence makes the spin visible and platform-consistent.
/// 33 ms ≈ 30 fps. Pacing only ever DELAYS a frame that finished early; a platform
/// whose present is already SLOWER than this (x86 panel present at ~22 fps) never
/// waits and runs at its own speed. The emitted fps line reports MEASURED time, so it
/// stays honest either way (~30 fps when paced, present-bound fps when not).
const FRAME_MS: u64 = 33;

/// Finite backstop for the pace busy-wait: never poll `ms()` more than this many times
/// waiting for one frame slot. On real hardware the monotonic clock reaches the slot
/// deadline long before this cap; the cap only guards against a stuck/degenerate clock
/// (e.g. a timerless fallback returning a constant) so the demo can never hang and QEMU
/// still boots straight through to the interactive path.
const PACE_POLL_CAP: u64 = 200_000_000;

/// The unit cube: 8 corners, 12 outward-wound triangles (front = CCW-on-screen,
/// see `rast::raster::Target::triangle`).
fn cube() -> ([Vec3; 8], [u32; 36]) {
    (
        [
            Vec3::new(-1.0, -1.0, -1.0),
            Vec3::new(1.0, -1.0, -1.0),
            Vec3::new(1.0, 1.0, -1.0),
            Vec3::new(-1.0, 1.0, -1.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-1.0, 1.0, 1.0),
        ],
        [
            0, 2, 1, 0, 3, 2, // -Z
            4, 5, 6, 4, 6, 7, // +Z
            0, 1, 5, 0, 5, 4, // -Y
            3, 7, 6, 3, 6, 2, // +Y
            0, 4, 7, 0, 7, 3, // -X
            1, 2, 6, 1, 6, 5, // +X
        ],
    )
}

/// Render the spinning cube for [`FRAMES`] frames into `screen`, then return so
/// the caller resumes the normal interactive loop. Emits one honest fps line.
pub fn run(screen: &mut crate::video::Screen) {
    let pw = screen.width();
    let ph = screen.height();
    if pw < DEMO_W || ph < DEMO_H {
        serial_println!(":: RAST: panel too small for the demo — skipped ::");
        return;
    }
    // Fixed render size; centered blit offset on the panel.
    let (w, h) = (DEMO_W, DEMO_H);
    let off_x = (pw - w) / 2;
    let off_y = (ph - h) / 2;

    // Paint the whole panel to the demo backdrop once, so the centered render
    // sits on a clean frame (the boot log stays outside the demo region until
    // the shell repaints below).
    screen.fill_screen(0x0010_1018);

    // The rast back buffer: RGBA8 color + f32 depth, one entry per pixel.
    let mut color = vec![0u8; 4 * w * h];
    let mut depth = vec![0f32; w * h];

    let (verts, idx) = cube();
    let proj = Mat4::perspective(PI / 3.0, w as f32 / h as f32, 0.5, 100.0);
    let view = Mat4::look_at(
        Vec3::new(0.0, 0.0, 5.0),
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    );
    let view_proj = proj.mul(&view);
    let light = Vec3::new(0.4, 0.8, 0.6);

    serial_println!(
        ":: RAST: software rasterizer demo — {}x{} spinning cube centered on {}x{} panel, {} frames ::",
        w,
        h,
        pw,
        ph,
        FRAMES
    );
    let t_start = crate::arch::ms();

    for frame in 0..FRAMES {
        let angle = frame as f32 * 0.035;
        let model = Mat4::rotation_y(angle).mul(&Mat4::rotation_x(angle * 0.5));

        // Render the scene into the owned RGBA back buffer.
        {
            let mut target = match Target::new(&mut color, &mut depth, w, h, w) {
                Some(t) => t,
                None => {
                    serial_println!(":: RAST: target alloc mismatch — demo aborted ::");
                    return;
                }
            };
            target.clear(Rgba::rgb(0x10, 0x10, 0x18));
            render_mesh(
                &mut target,
                &model,
                &view_proj,
                &verts,
                &idx,
                Rgba::rgb(0x40, 0xB0, 0xFF),
                light,
                0.25,
                true,
            );
        }

        // Present: copy the RGBA back buffer to the centered panel region via the
        // public Screen API (format-aware `put_pixel`), then flush the damaged region.
        for y in 0..h {
            let row = y * w * 4;
            for x in 0..w {
                let p = row + x * 4;
                let c = ((color[p] as u32) << 16)
                    | ((color[p + 1] as u32) << 8)
                    | (color[p + 2] as u32);
                screen.put_pixel(off_x + x, off_y + y, c);
            }
        }
        screen.flush();

        // Pace: hold this frame until its wall-clock slot so the spin is visible at a
        // steady cadence. Pure delay — the slot deadline is measured from `t_start`, so
        // if this frame's render+present already overran the slot (slow present, e.g.
        // x86) the loop condition is false immediately and we never wait. Fast platforms
        // slow to the target; slow ones run at their own present speed. `PACE_POLL_CAP`
        // is the finite backstop (never an unbounded spin).
        let slot = t_start + (frame as u64 + 1) * FRAME_MS;
        let mut polls = 0u64;
        while crate::arch::ms() < slot && polls < PACE_POLL_CAP {
            polls += 1;
            core::hint::spin_loop();
        }
    }

    let elapsed = crate::arch::ms().saturating_sub(t_start).max(1);
    let fps_x1000 = (FRAMES as u64 * 1000 * 1000) / elapsed;
    serial_println!(
        ":: RAST: {} frames in {} ms — {}.{:03} fps (software rasterizer, panel present) ::",
        FRAMES,
        elapsed,
        fps_x1000 / 1000,
        fps_x1000 % 1000
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// RAST-MC — the multi-core software-rasterizer rung (Orin `tegra` + `rast`; x86 `rastmc`)
// ════════════════════════════════════════════════════════════════════════════════════════════════
//
// WHAT THIS IS. `orin-3d.md` §3 names the one blob-free performance rung the Jetson actually has:
// "more CPU". This is that rung, at the only granularity the SHARED `rast` crate's public API can
// express without being edited — **frame pipelining**. Core `k` renders frame `f` where
// `f % nslots == k` into ITS OWN full-size back buffer; the boot core presents the finished frames
// through `Screen::put_pixel` in strict frame order. Rendering overlaps across cores; presentation
// stays serial and ordered, so the pixels and their sequence are bit-for-bit what the single-core
// path produces (each frame is still one whole-frame `rast::render_mesh` into a whole-frame
// `Target` — the SAME call, on a different core).
//
// WHY NOT BANDS/TILES, WHICH WOULD SCALE BETTER. `rast`'s public surface offers no way to render a
// SUB-RECTANGLE of a frame: `render_mesh` maps NDC to `target.width()/height()` (`lib.rs:110-113`,
// `to_screen`), so a band-sized `Target` renders the whole scene squashed into the band rather than
// the band's slice of the scene, and `Target` has no viewport origin, no scissor rect, and no
// separate transform stage to feed offset `ScreenVert`s from (`clip_near`/`divide_and_map`/
// `to_screen` are all private). Band/tile decomposition therefore REQUIRES a `rast` API change, and
// `rast` is shared-lane + golden-pinned (`orin-3d.md` §4.3): STOP and report, do not fork. The
// missing API is written up in the arc report; frame pipelining is what is available today at zero
// shared-lane cost.
//
// AMDAHL, STATED HONESTLY UP FRONT. Only the RENDER half is parallel here. The present half
// (76 800 `put_pixel` calls + one `flush` per frame) stays on the boot core, so the achievable
// speedup is capped at `total / max(present_total, render_total / nslots)` — roughly 2x when render
// and present cost about the same, no matter how many cores are online. The witness reports the
// MEASURED ratio against a 1-core baseline taken in the SAME boot, unpaced, so the number is what it
// is rather than what the core count suggests.
//
// MEMORY. One full-size RGBA8 + f32-depth pair per pipeline slot: 320x240 => 300 KiB + 300 KiB =
// 600 KiB per core, off the live 48 MiB heap. Five secondaries => 3.0 MiB. That is a real bump on
// the seat whose documented RAS trigger is exactly "grew live heap use" (ORIN-VUG-RAS / the XCARVE
// ledger), so the witness prints the footprint and the slot count is capped by the cores that
// actually check in.
//
// WHERE THE WORKERS RUN — aarch64/tegra. The tegra image arms `tegrasmp` by DEFAULT
// (`arroyo:573`), so `smp_virt::start_secondaries_tegra` has already brought the DTB-`/cpus`
// secondaries online at EL2 before the JM6 drop, and each one entered `sched::secondary_run` ->
// `mark_online` -> `run()` (ORIN-SMP-RUN, `smp_virt.rs:329-345`). They are real dispatching
// scheduler cores, so a plain `sched::spawn(..., cpu)` reaches them. The boot core is at EL1 with
// `mmu.ttbr0_el1` while the APs are still at EL2 with the EL2 table; both map RAM Normal-WB
// **inner-shareable** (`mmu_tegra.rs:488,505-512`), so the shared buffers and the handshake atomics
// are hardware-coherent across the EL split.
//
// WHERE THE WORKERS RUN — x86 (RASTPORT). The same shape, arrived at from the opposite direction,
// and the reason it works is a gate nothing else documents: `main.rs`'s SCHED-X86 render/service
// handoff is `#[cfg(all(target_arch = "x86_64", not(feature = "rast")))]`, so arming `rast`
// COMPILES THE HANDOFF OUT — the BSP never enters `run_bsp`, and the GUI (hence this demo) runs
// inline on core 0. Meanwhile `sched::enable()` is called UNCONDITIONALLY on x86 (the PULSE-NCPU
// fix), which releases every AP from `wait_and_run` into `run()` — `mark_online` and then idle in
// `sti;hlt`. So the presenter is a core with no render-lane peer competing for it, and the APs are
// live, idle and pinnable. That is the *best* arrangement this rung can be handed; the Orin's is
// strictly busier. Cache coherence needs no argument on x86 (one address space, hardware-coherent
// throughout) — the EL-split paragraph above has no x86 analogue.
//
// Two consequences of that gate, stated because they bound what an x86 run may CLAIM:
//   * There is no `c1` render pin and no device-service core on an x86 `rast` build — those three
//     tasks are not compiled. A claim that this rung coexists with the x86 compositor's SCHEDULED
//     render lane is not merely unproven here, it is unfalsifiable in this build shape. Not made.
//   * `online_cpu_count()` reads as "dispatching SECONDARIES" on x86 only because this build never
//     reaches `run_bsp` and so never marks core 0 — matching aarch64, where the boot core is never
//     in the mask. Correct here, but correct-by-build rather than by construction; see the x86
//     `online_cpu_count` doc comment.
//
// WHAT AN x86 RUN DOES *NOT* PROVE — the compositor swallows the pixels (RASTPORT). This module
// presents by poking PANEL coordinates: `Screen::put_pixel` into a centred `DEMO_W`x`DEMO_H` block,
// then `flush`. On aarch64/tegra the occluder set is empty and those pixels reach the panel. On x86
// with `wc` armed AND a successful Kepler takeover it is different in kind: `wcx`/`desktop_uefi`'s
// `activate()` — whose one call site is the Kepler takeover — opens a CENTRED console window and a
// menu bar, and `Screen::present_background` SUBTRACTS occluder boxes before copying anything to
// the framebuffer. A pixel under the console window is not composited over; it is never written at
// all, and `flush()` reports success regardless. So on such a boot the demo's rendering, timing and
// serial witnesses are all real and the GLASS IS UNCHANGED.
//
// The measurement this rung exists for — a speedup ratio against a same-boot 1-core baseline — is
// unaffected: both arms pay the same present cost, occluded or not. But "first 3D pixels on x86
// under the compositor" is NOT something this code can claim, and the trap is that QEMU cannot show
// it: QEMU has no Kepler, so `activate()` never runs, the occluder set stays empty, and a headless
// or GUI QEMU run displays the cube exactly as intended. Only the bench rMBP, with `UNAOS_WC=1` and
// the real takeover, exercises the occluded path. Making the demo visible there means rendering
// into a compositor WINDOW instead of poking panel coordinates — a design change to a
// `call-never-edit` module, deliberately not attempted here.
//
// FAIL-CLOSED. Every wait is bounded (`MC_SPIN_CAP` polls / an `ms()` deadline); a miss sets
// `MC_ABORT`, prints, and returns. Buffers are only dropped once every enlisted worker has published
// `MC_DONE`; on a timeout they are deliberately LEAKED (`forget`) rather than freed under a core
// that might still be writing them.

/// Which cores this module can address. Matches the scheduler's per-CPU array bound, so a probe
/// spawn can never index a run queue out of range.
///
/// RASTPORT: was `crate::arch::percpu::NUM_CPUS` — an aarch64-only spelling of a fact both arches
/// have. `sched::sched_cpu_slots()` is the neutral accessor for it (`NUM_CPUS` on aarch64,
/// `gdt::MAX_CPUS` on x86); neither constant was renamed. Both arches size the run-queue array by
/// their own answer, which is exactly the property the sentence above claims.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
const MC_MAX: usize = crate::arch::sched::sched_cpu_slots();

/// Finite backstop for every RAST-MC spin (same role as `PACE_POLL_CAP`): no wait here is ever
/// unbounded, so a core that never checks in degrades the demo instead of wedging the boot.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
const MC_SPIN_CAP: u64 = 200_000_000;

/// How long the presenter waits for probe workers to announce themselves before it closes the roster.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
const MC_ENLIST_MS: u64 = 300;

/// How long the presenter waits for enlisted workers to retire before it frees their buffers.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
const MC_DRAIN_MS: u64 = 2000;

#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// Worker check-in, indexed by CPU: "a task of mine is actually dispatching on this core".
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
static MC_ALIVE: [AtomicBool; MC_MAX] = [const { AtomicBool::new(false) }; MC_MAX];
/// The presenter's roster verdict, indexed by CPU.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
static MC_ENLISTED: [AtomicBool; MC_MAX] = [const { AtomicBool::new(false) }; MC_MAX];
/// Worker retirement, indexed by CPU (the buffer-lifetime gate).
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
static MC_DONE: [AtomicBool; MC_MAX] = [const { AtomicBool::new(false) }; MC_MAX];
/// CPU -> pipeline slot.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
static MC_SLOT: [AtomicUsize; MC_MAX] = [const { AtomicUsize::new(0) }; MC_MAX];
/// CPU -> frames this core actually rendered (the per-core witness).
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
static MC_FRAMES_BY: [AtomicU32; MC_MAX] = [const { AtomicU32::new(0) }; MC_MAX];
/// Slot -> RGBA8 back-buffer base (published by the presenter before `MC_GO`).
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
static MC_COLOR: [AtomicUsize; MC_MAX] = [const { AtomicUsize::new(0) }; MC_MAX];
/// Slot -> f32 depth-plane base.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
static MC_DEPTH: [AtomicUsize; MC_MAX] = [const { AtomicUsize::new(0) }; MC_MAX];
/// Slot -> frames produced into that slot's buffer.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
static MC_RENDERED: [AtomicU32; MC_MAX] = [const { AtomicU32::new(0) }; MC_MAX];
/// Slot -> frames consumed out of that slot's buffer. `RENDERED == PRESENTED` means "buffer free".
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
static MC_PRESENTED: [AtomicU32; MC_MAX] = [const { AtomicU32::new(0) }; MC_MAX];
/// Pipeline width (number of enlisted render cores).
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
static MC_NSLOTS: AtomicUsize = AtomicUsize::new(0);
/// Release: the roster is closed and every buffer pointer is published.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
static MC_GO: AtomicBool = AtomicBool::new(false);
/// Any bounded wait expired: every participant unwinds to its retirement store.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
static MC_ABORT: AtomicBool = AtomicBool::new(false);

/// Render ONE frame of the same scene `run` draws, into caller-owned planes. Deliberately a separate
/// function rather than a refactor of `run`: `run` is the metal-witnessed RAST-TEGRA/RAST-PACE path
/// and its behaviour is left byte-for-byte alone by this arc. The scene constants are duplicated
/// here, not shared, for exactly that reason.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
fn mc_render_frame(color: &mut [u8], depth: &mut [f32], frame: u32) {
    let (w, h) = (DEMO_W, DEMO_H);
    let (verts, idx) = cube();
    let proj = Mat4::perspective(PI / 3.0, w as f32 / h as f32, 0.5, 100.0);
    let view = Mat4::look_at(
        Vec3::new(0.0, 0.0, 5.0),
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    );
    let view_proj = proj.mul(&view);
    let light = Vec3::new(0.4, 0.8, 0.6);
    let angle = frame as f32 * 0.035;
    let model = Mat4::rotation_y(angle).mul(&Mat4::rotation_x(angle * 0.5));
    if let Some(mut target) = Target::new(color, depth, w, h, w) {
        target.clear(Rgba::rgb(0x10, 0x10, 0x18));
        render_mesh(
            &mut target,
            &model,
            &view_proj,
            &verts,
            &idx,
            Rgba::rgb(0x40, 0xB0, 0xFF),
            light,
            0.25,
            true,
        );
    }
}

/// Present one finished RGBA8 plane to the centered panel region — the same public `Screen` path
/// `run` uses (call-never-edit on the shared video surface).
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
fn mc_present(screen: &mut crate::video::Screen, color: &[u8], off_x: usize, off_y: usize) {
    for y in 0..DEMO_H {
        let row = y * DEMO_W * 4;
        for x in 0..DEMO_W {
            let p = row + x * 4;
            let c = ((color[p] as u32) << 16)
                | ((color[p + 1] as u32) << 8)
                | (color[p + 2] as u32);
            screen.put_pixel(off_x + x, off_y + y, c);
        }
    }
    screen.flush();
}

/// RASTPORT — pin one render worker to `cpu`. The ONE place the two arches' `spawn` signatures
/// differ, confined to a shim rather than paid for at the call site or papered over by changing a
/// public API with many other callers:
///
///   * aarch64 — `spawn(name, entry, arg, cpu) -> u64` (`arch/aarch64/sched.rs`)
///   * x86     — `spawn(name, entry, arg, target_cpu, priority: u8)` (`arch/x86_64/sched.rs`)
///
/// The four shared arguments mean the identical thing on both, including the part this module
/// depends on: an EXPLICIT core index is a no-migrate pin (x86 sets `steal_ok = target_cpu ==
/// CPU_AUTO`, and `!steal_ok` is filtered out of the steal path), which is what makes `MC_ALIVE`
/// an honest "this core dispatches" probe rather than a guess. The returned task id is unused on
/// either arch — the roster is built from `MC_ALIVE`, not from spawn results — so the differing
/// return types cost nothing.
///
/// **WHY `PRIO_NORMAL` AND NOT HIGHER.** It is the band the x86 boot already gives its own
/// render / input / usb-pump tasks, so a render worker is a peer of the panel services rather
/// than something that displaces them. It also matters that a `rast` build may carry `sched_demo`
/// (the `x86-all` feature set does): those workloads sit on the same APs, and a compute-bound
/// 90-frame loop in an elevated band would starve them for no panel benefit. The demo is not
/// latency-critical — it is throughput measured against a same-boot baseline that runs in the
/// same band — so nothing here earns `PRIO_HIGH`.
#[cfg(all(feature = "rastmc", target_arch = "x86_64"))]
fn mc_spawn(cpu: usize) {
    crate::arch::sched::spawn(
        "rast-mc",
        mc_worker,
        cpu,
        cpu,
        crate::arch::sched::PRIO_NORMAL,
    );
}

/// RASTPORT — the aarch64 half of the `spawn`-arity shim. See the x86 twin above for the contract;
/// this is byte-for-byte the call RAST-MC always made.
#[cfg(all(feature = "tegra", target_arch = "aarch64"))]
fn mc_spawn(cpu: usize) {
    crate::arch::sched::spawn("rast-mc", mc_worker, cpu, cpu);
}

/// The per-core render worker. `arg` is the CPU it was pinned to (an explicit `spawn` index is a
/// no-migrate pin, `sched::pick_cpu_slot`), so a worker that runs at all runs on the core it names —
/// which is what makes `MC_ALIVE` an honest "this core dispatches" probe rather than a guess.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
fn mc_worker(cpu: usize) {
    if cpu >= MC_MAX {
        return;
    }
    MC_ALIVE[cpu].store(true, Ordering::Release);

    // Wait for the roster verdict + buffer publication. Bounded; an abort or a lapsed cap retires.
    let mut polls = 0u64;
    while !MC_GO.load(Ordering::Acquire) && !MC_ABORT.load(Ordering::Acquire) && polls < MC_SPIN_CAP
    {
        polls += 1;
        core::hint::spin_loop();
    }
    if MC_ABORT.load(Ordering::Acquire) || !MC_ENLISTED[cpu].load(Ordering::Acquire) {
        MC_DONE[cpu].store(true, Ordering::Release);
        return;
    }

    let slot = MC_SLOT[cpu].load(Ordering::Acquire);
    let n = MC_NSLOTS.load(Ordering::Acquire);
    if slot >= MC_MAX || n == 0 {
        MC_DONE[cpu].store(true, Ordering::Release);
        return;
    }
    let cptr = MC_COLOR[slot].load(Ordering::Acquire) as *mut u8;
    let dptr = MC_DEPTH[slot].load(Ordering::Acquire) as *mut f32;
    if cptr.is_null() || dptr.is_null() {
        MC_DONE[cpu].store(true, Ordering::Release);
        return;
    }
    // SAFETY: the presenter allocated exactly `4*DEMO_W*DEMO_H` bytes / `DEMO_W*DEMO_H` floats for
    // THIS slot, published the bases with Release *before* `MC_GO` (paired with the Acquire above),
    // and keeps them alive until this worker's `MC_DONE`. Slots are one-per-core and the pipeline
    // handshake (`MC_RENDERED == MC_PRESENTED` means "free") guarantees the presenter never reads a
    // slot while its worker writes it, so this is the only live mutable alias.
    let color = unsafe { core::slice::from_raw_parts_mut(cptr, 4 * DEMO_W * DEMO_H) };
    let depth = unsafe { core::slice::from_raw_parts_mut(dptr, DEMO_W * DEMO_H) };

    let mut made = 0u32;
    let mut f = slot as u32;
    'frames: while f < FRAMES {
        // Wait until the presenter has consumed the previous frame from this slot.
        let mut polls = 0u64;
        while MC_RENDERED[slot].load(Ordering::Acquire) != MC_PRESENTED[slot].load(Ordering::Acquire)
        {
            if MC_ABORT.load(Ordering::Acquire) || polls >= MC_SPIN_CAP {
                break 'frames;
            }
            polls += 1;
            core::hint::spin_loop();
        }
        mc_render_frame(color, depth, f);
        MC_RENDERED[slot].fetch_add(1, Ordering::Release);
        made += 1;
        f += n as u32;
    }
    MC_FRAMES_BY[cpu].store(made, Ordering::Release);
    MC_DONE[cpu].store(true, Ordering::Release);
}

/// RAST-MC entry: measure a 1-core baseline in this boot, then run the same 90 frames frame-pipelined
/// across every secondary that actually dispatches, and report the honest ratio. Returns with the
/// panel in the same state `run` would leave it, so the caller's paced spin is unaffected.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
pub fn run_mc(screen: &mut crate::video::Screen) {
    let pw = screen.width();
    let ph = screen.height();
    if pw < DEMO_W || ph < DEMO_H {
        serial_println!(":: RAST-MC: panel too small for the demo — skipped ::");
        return;
    }
    let off_x = (pw - DEMO_W) / 2;
    let off_y = (ph - DEMO_H) / 2;

    // Is there anything to parallelize ONTO? `online_cpu_count` counts cores registered in the
    // scheduler's `ONLINE_MASK` — on tegra that is exactly the set of secondaries that reached
    // `secondary_run`; on an x86 `rast` build it is likewise the secondaries only, because that
    // build never reaches `run_bsp` and so never marks core 0 (see the header). Zero means the boot
    // came up single-core (UNAOS_NOTEGRASMP=1, or every CPU_ON failed; on x86, no AP dispatching):
    // say so and leave, rather than fabricating a multi-core number.
    let online = crate::arch::sched::online_cpu_count();
    if online == 0 {
        serial_println!(
            ":: RAST-MC: no secondary core is dispatching (scheduler reports 0 online) — the \
             multi-core rung is unavailable on this boot; single-core path unchanged ::"
        );
        return;
    }
    serial_println!(
        ":: RAST-MC: {} secondary core(s) online and dispatching — probing for render workers ::",
        online
    );

    // ── 1-core baseline, THIS boot, UNPACED ────────────────────────────────────────────────────
    // The published 989 fps is a different build on a different sitting; a speedup ratio is only
    // honest against a baseline measured on the same silicon, same boot, same panel, same frame
    // count, with pacing out of the way (paced, both arms would read 30.303 fps by construction).
    let mut base_color = vec![0u8; 4 * DEMO_W * DEMO_H];
    let mut base_depth = vec![0f32; DEMO_W * DEMO_H];
    let t0 = crate::arch::ms();
    for f in 0..FRAMES {
        mc_render_frame(&mut base_color, &mut base_depth, f);
        mc_present(screen, &base_color, off_x, off_y);
    }
    let base_ms = crate::arch::ms().saturating_sub(t0).max(1);
    let base_fps_x1000 = (FRAMES as u64 * 1000 * 1000) / base_ms;
    serial_println!(
        ":: RAST-MC: 1-core baseline — {} frames in {} ms — {}.{:03} fps (same boot, unpaced) ::",
        FRAMES,
        base_ms,
        base_fps_x1000 / 1000,
        base_fps_x1000 % 1000
    );

    // ── Probe: which secondaries actually dispatch a task RIGHT NOW ────────────────────────────
    // A pinned spawn onto a core that never dispatches simply sits in that core's run queue (the
    // scheduler is no-migrate and an explicit-index spawn is not steal-eligible), so a core that is
    // registered-but-wedged costs one queued task and never a hang. Only cores that check in are
    // enlisted.
    for cpu in 1..MC_MAX {
        mc_spawn(cpu);
    }
    let enlist_deadline = crate::arch::ms() + MC_ENLIST_MS;
    let mut polls = 0u64;
    loop {
        let alive = (1..MC_MAX).filter(|&c| MC_ALIVE[c].load(Ordering::Acquire)).count();
        if alive >= online || crate::arch::ms() >= enlist_deadline || polls >= MC_SPIN_CAP {
            break;
        }
        polls += 1;
        core::hint::spin_loop();
    }

    // ── Close the roster and publish the buffers ───────────────────────────────────────────────
    let mut slots: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
    for cpu in 1..MC_MAX {
        if MC_ALIVE[cpu].load(Ordering::Acquire) {
            MC_SLOT[cpu].store(slots.len(), Ordering::Release);
            slots.push(cpu);
        }
    }
    let n = slots.len();
    if n == 0 {
        MC_ABORT.store(true, Ordering::Release);
        MC_GO.store(true, Ordering::Release);
        serial_println!(
            ":: RAST-MC: {} core(s) online but none dispatched a render worker within {} ms — \
             multi-core pass SKIPPED (fail-closed) ::",
            online,
            MC_ENLIST_MS
        );
        return;
    }
    let mut bufs: alloc::vec::Vec<(alloc::vec::Vec<u8>, alloc::vec::Vec<f32>)> =
        alloc::vec::Vec::with_capacity(n);
    for _ in 0..n {
        bufs.push((vec![0u8; 4 * DEMO_W * DEMO_H], vec![0f32; DEMO_W * DEMO_H]));
    }
    for (slot, b) in bufs.iter_mut().enumerate() {
        MC_COLOR[slot].store(b.0.as_mut_ptr() as usize, Ordering::Release);
        MC_DEPTH[slot].store(b.1.as_mut_ptr() as usize, Ordering::Release);
    }
    for &cpu in slots.iter() {
        MC_ENLISTED[cpu].store(true, Ordering::Release);
    }
    MC_NSLOTS.store(n, Ordering::Release);
    let heap_kib = (n * (4 * DEMO_W * DEMO_H + 4 * DEMO_W * DEMO_H)) / 1024;
    serial_println!(
        ":: RAST-MC: pipeline width {} (render cores {:?}, present on boot core 0) — {} KiB of \
         back/depth buffers off the 48 MiB heap ::",
        n,
        slots.as_slice(),
        heap_kib
    );

    // ── The multi-core pass: workers render, this core presents IN FRAME ORDER ─────────────────
    let t1 = crate::arch::ms();
    MC_GO.store(true, Ordering::Release);
    let mut presented = 0u32;
    let mut aborted = false;
    for f in 0..FRAMES {
        let slot = (f as usize) % n;
        let mut polls = 0u64;
        while MC_RENDERED[slot].load(Ordering::Acquire) <= MC_PRESENTED[slot].load(Ordering::Acquire)
        {
            if polls >= MC_SPIN_CAP {
                aborted = true;
                break;
            }
            polls += 1;
            core::hint::spin_loop();
        }
        if aborted {
            break;
        }
        mc_present(screen, &bufs[slot].0, off_x, off_y);
        MC_PRESENTED[slot].fetch_add(1, Ordering::Release);
        presented += 1;
    }
    let mc_ms = crate::arch::ms().saturating_sub(t1).max(1);
    if aborted {
        MC_ABORT.store(true, Ordering::Release);
        serial_println!(
            ":: RAST-MC: FAIL — a render worker stopped producing after {} of {} frames (spin cap \
             hit); no speedup claimed ::",
            presented,
            FRAMES
        );
    }

    // ── Retire the workers before their buffers die ────────────────────────────────────────────
    let drain_deadline = crate::arch::ms() + MC_DRAIN_MS;
    let mut polls = 0u64;
    let all_done = loop {
        if slots.iter().all(|&c| MC_DONE[c].load(Ordering::Acquire)) {
            break true;
        }
        if crate::arch::ms() >= drain_deadline || polls >= MC_SPIN_CAP {
            break false;
        }
        polls += 1;
        core::hint::spin_loop();
    };

    // ── Witnesses ──────────────────────────────────────────────────────────────────────────────
    let mut total = 0u32;
    for &cpu in slots.iter() {
        let made = MC_FRAMES_BY[cpu].load(Ordering::Acquire);
        total += made;
        serial_println!(":: RAST-MC: core {} rendered {} frame(s) ::", cpu, made);
    }
    serial_println!(
        ":: RAST-MC: core 0 presented {} frame(s) (ordered, boot core) ::",
        presented
    );
    if !aborted {
        let fps_x1000 = (presented as u64 * 1000 * 1000) / mc_ms;
        let speed_x1000 = (base_ms * 1000) / mc_ms;
        serial_println!(
            ":: RAST-MC: {} core(s), {} frames, {}.{:03} fps — speedup {}.{:03}x vs 1-core ::",
            n + 1,
            presented,
            fps_x1000 / 1000,
            fps_x1000 % 1000,
            speed_x1000 / 1000,
            speed_x1000 % 1000
        );
        serial_println!(
            ":: RAST-MC: verdict {} — {} frame(s) rendered off the boot core, {} presented in order \
             ({} ms vs {} ms 1-core) ::",
            if total == presented && presented == FRAMES { "PASS" } else { "PARTIAL" },
            total,
            presented,
            mc_ms,
            base_ms
        );
    }

    if !all_done {
        // A worker is still (or forever) inside its loop: its slot's buffer must NOT be freed under
        // it. Leak deliberately — 600 KiB per stuck core, once, on a demo path — rather than hand
        // the allocator memory another core may still write. Fail closed.
        serial_println!(
            ":: RAST-MC: WARNING a render worker did not retire within {} ms — {} KiB of back \
             buffers deliberately leaked (never freed under a live writer) ::",
            MC_DRAIN_MS,
            heap_kib
        );
        core::mem::forget(bufs);
    }
}
