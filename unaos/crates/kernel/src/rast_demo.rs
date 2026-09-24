// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! RAST-1 demo: a spinning, flat-shaded, z-buffered cube rendered through the
//! `rast` software rasterizer and presented **into a compositor window**.
//!
//! This is the knob-gated (`UNAOS_RAST=1` → `rast` feature) wire-in of the
//! platform-neutral `rast` crate, shared by x86/virt, aarch64/virt and
//! aarch64/tegra. It is **call-never-edit** with respect to the shared video
//! path: it renders into its own heap-owned RGBA8 back buffer, converts that
//! into a `wm` window's own ARGB8888 surface, and presents through `wm`'s
//! public `create_at` / `present_rows` / `close` API — it does not touch
//! `FrameBuffer`, `Screen`, or any other shared surface code.
//!
//! With the feature off the whole module is unlinked (`lib.rs`'s
//! `#[cfg(feature = "rast")] pub mod rast_demo;` — a cfg'd-out `pub mod` is
//! never lexed) and the kernel image is byte-identical to baseline on BOTH
//! arches.
//!
//! ════════════════════════════════════════════════════════════════════════════
//! RASTWIN — 3D ON THE DESKTOP, not 3D INSTEAD OF IT.
//! ════════════════════════════════════════════════════════════════════════════
//!
//! WHAT CHANGED AND WHY. This module was built before the desktop existed, so it
//! owned the PANEL: [`run`] filled the whole screen with `0x0010_1018`, blitted a
//! centred 320x240 render through `Screen::put_pixel` for [`FRAMES`] frames, and
//! handed the panel back. On a machine that now has a compositor, a menu bar, a
//! dock and windows, a full-panel takeover is the wrong shape — and the tegra
//! call site even had to `fbcon::detach()` around it to stop a second writer,
//! which is the shape of a design that cannot share the glass.
//!
//! The renderer now draws into an ordinary `wm` row: a titled, damage-tracked,
//! hit-testable window that composites beside Console / Shell / Quarry. The
//! measurable consequences, each asserted by [`WIN_VERDICT`]'s fixture line:
//!
//!   * **Nothing outside the row is touched.** There is no `fill_screen`, no
//!     `Screen::put_pixel` and no `Screen::flush` on this path at all — the only
//!     writer of panel pixels is `wm::composite`, through the row's own clip.
//!     The `Screen` the callers still hand in is unused, which is why the
//!     parameter is `_screen`.
//!   * **Damage is the cube's rect, never the panel.** The RGBA→ARGB blit below
//!     compares each word it is about to write and tracks the row band that
//!     actually moved, so a frame presents `present_rows(id, dy0, dy1)` over the
//!     rows the cube crossed and nothing else. A frame that changed nothing
//!     presents nothing at all.
//!   * **It is a real window.** The row is minted under [`RW_OWNER`], its own
//!     slot in the kernel owner band. That choice is load-bearing rather than
//!     cosmetic: `wm::hit_test` skips `owner_asid == 0` outright, so a row minted
//!     under owner 0 (the compat/login shape) is furniture the pointer cannot
//!     name — not draggable, not raisable, no control cluster. An owner of its
//!     own is what makes this window behave like every other window, and the
//!     fixture measures it by hit-testing the row's own title bar.
//!
//! WHAT DID NOT CHANGE, deliberately. The scene, the transform, the 320x240
//! render size, the [`FRAMES`] bound and the [`FRAME_MS`] pacing are the metal-
//! witnessed RAST-TEGRA/RAST-PACE values and are left alone; this arc moves where
//! the pixels GO, not what they are. The backdrop constant `0x0010_1018` is still
//! the clear colour, so `display_tegra`'s ORIN-RASTGLASS probe can still IDENTIFY
//! rast ink — but that probe's two sampling REGIONS are now wrong, and it will
//! report `NO-RAST-INK` on a windowed boot. See [`RW_CLEAR`].

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

use crate::video::wm;

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

/// RASTWIN — the compositor owner this window is minted under: its own slot in the kernel owner
/// band, next to `pulsewin`'s `0x60` (census of the band at this HEAD: `0x40`-`0x45`, `0x50`,
/// `0x51`, `0x60`, `0x7F`, `0xFF`, and `+1`..`+4`; `0x61` is free).
///
/// **An owner of its own is what makes this a window rather than furniture.** `wm::hit_test` skips
/// every row with `owner_asid == 0` before it looks at geometry, so a row minted under owner 0 —
/// the shape `login.rs` uses precisely to make the login screen unclosable — can never be named by
/// either router: no drag, no raise, no control cluster. Minting under `KERNEL_OWNER_DESKTOP`
/// would be worse than cosmetic in the other direction: `dock::pin_shell` appends a pinned shell
/// tile iff no live `KERNEL_OWNER_DESKTOP` row exists, so borrowing that owner would make the dock
/// read this cube as the shell. `is_kernel_owner` covers the whole band, so `above_shell` exempts
/// this row from the shell-z floor exactly as it exempts the console window.
const RW_OWNER: u64 = wm::KERNEL_OWNER_BASE + 0x61;

/// RASTWIN — the window's title. R36: a window's title is the APP's name, and numbering is for
/// untitled documents only, so this is a name and not `3D Demo 1`. `wm::MAX_TITLE` is 16.
const RW_TITLE: &[u8] = b"3D";

/// RASTWIN — **where the row was SEATED, in panel pixels**, published for ORIN-RASTGLASS (A67).
///
/// The probe in `arch/aarch64/display_tegra.rs` used to locate RAST's ink by RESTATING this
/// module's private constants — a centred `DEMO_W` x `DEMO_H` box — because it had no other way to
/// know where to look. That worked only while `run` owned the panel and painted a box it computed
/// itself. A windowed renderer does not choose its own geometry: the compositor does, including an
/// integer upscale the demo never sees until it asks. So the geometry is PUBLISHED rather than
/// guessed, and it comes from [`wm::info`] — what the compositor actually seated — never from what
/// this module requested.
#[derive(Clone, Copy)]
pub struct RastwinSeat {
    /// Content rect on the panel: origin, and the SOURCE dimensions already multiplied by `scale`.
    pub cx: usize,
    pub cy: usize,
    pub cw: usize,
    pub ch: usize,
    /// Outer box (content plus chrome: title bar above, border all round).
    pub ox: usize,
    pub oy: usize,
    pub ow: usize,
    pub oh: usize,
}

/// RASTWIN — the published seat, as plain atomics so a reader needs no lock. That matters to the one
/// reader there is: ORIN-RASTGLASS samples the panel with the `WRITER` guard already dropped and is
/// forbidden to take `wm`'s table lock (ORIN-WM1's acyclic `WRITER` -> `TABLE` rule), so a snapshot
/// it can read with eight relaxed loads is the only shape that fits. It also answers AFTER the row
/// has closed, which `wm::info(id)` cannot: the census's `late` sample runs seconds later, when the
/// window is long gone, and "is there ink outside the rect the window occupied" is still a perfectly
/// good question then — after the close it is the ONLY one still worth asking.
static RW_SEAT: [core::sync::atomic::AtomicUsize; 8] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; 8];
/// RASTWIN — whether [`RW_SEAT`] has ever been published. Separate from the values because a seat at
/// the panel origin is legal and `(0,0,0,0,…)` must not be mistaken for "never opened": the probe's
/// whole discipline is that it says what it could not determine rather than guessing.
static RW_SEAT_VALID: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
/// RASTWIN — whether a row is composited RIGHT NOW, as opposed to [`RW_SEAT_VALID`]'s "a row was
/// seated at some point and here is where".
///
/// The two facts are genuinely different and ORIN-RASTGLASS needs both. Its `post` sample runs while
/// the row is live and asks "did the blit reach the scan-out". Its `late` census sample runs seconds
/// later, by which time this demo's BOUNDED window has closed itself and the compositor has
/// repainted the desktop over the vacated box. Without this flag the census would read that healthy,
/// specified ending as `RAST-PAINTED-OVERWRITTEN` on every good boot — a verdict that fires on a
/// working machine and a broken one alike, which is the exact instrument failure this rung exists to
/// avoid.
static RW_SEAT_LIVE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// RASTWIN — is a 3D row composited right now? See [`RW_SEAT_LIVE`] for why this is not the same
/// question as [`rastwin_seat`] returning `Some`.
pub fn rastwin_live() -> bool {
    RW_SEAT_LIVE.load(core::sync::atomic::Ordering::Acquire)
}

/// RASTWIN — the published seat, or `None` if this module has never seated a row in this boot.
///
/// `None` is a REPORTABLE STATE, not an error: it means `run`/`run_mc` declined before creating a
/// window (headless, alloc refused, panel cannot seat the box), each of which names itself on its
/// own `[rastwin] … DECLINE reason=` line. A probe that read `None` as "no ink found" would be
/// converting "I could not look" into "I looked and saw nothing".
pub fn rastwin_seat() -> Option<RastwinSeat> {
    use core::sync::atomic::Ordering;
    if !RW_SEAT_VALID.load(Ordering::Acquire) {
        return None;
    }
    let v = |i: usize| RW_SEAT[i].load(Ordering::Relaxed);
    Some(RastwinSeat {
        cx: v(0), cy: v(1), cw: v(2), ch: v(3),
        ox: v(4), oy: v(5), ow: v(6), oh: v(7),
    })
}

/// RASTWIN — publish the seat from what `wm` actually seated. Called once per open, after
/// `create_at` has returned a live id, so `info` answers about a row that exists.
///
/// The `scale` multiply is the part that could not have been guessed: `wm::place_scale` picks an
/// integer upscale from the panel's geometry (`pw / 2 / w`, the usable height, and a legibility
/// cap), so on the 1920x1200 bench panel a 320x240 surface is composited at 640x480 while the
/// SOURCE dimensions `info` reports stay 320x240. A probe sampling `info.w` x `info.h` would have
/// read a quarter of the window and called the rest of it foreign.
fn rw_publish_seat(id: wm::WinId) {
    use core::sync::atomic::Ordering;
    let Some(i) = wm::info(id) else {
        // The row went away between `create_at` and here. Leave the seat unpublished rather than
        // storing a rect nothing stands behind.
        return;
    };
    let (cw, ch) = (i.w.saturating_mul(i.scale), i.h.saturating_mul(i.scale));
    let ox = i.x.saturating_sub(wm::BORDER);
    let oy = i.y.saturating_sub(wm::TITLE_H + wm::BORDER);
    let ow = cw.saturating_add(2 * wm::BORDER);
    let oh = ch.saturating_add(wm::TITLE_H + 2 * wm::BORDER);
    for (slot, val) in [i.x, i.y, cw, ch, ox, oy, ow, oh].iter().enumerate() {
        RW_SEAT[slot].store(*val, Ordering::Relaxed);
    }
    // Release LAST: a reader that sees the flag sees all eight values (paired with the Acquire in
    // `rastwin_seat`).
    RW_SEAT_VALID.store(true, Ordering::Release);
    RW_SEAT_LIVE.store(true, Ordering::Release);
}

/// RASTWIN — the surface clear colour, and it is the SAME `0x0010_1018` the panel-owning shape
/// filled the whole screen with. Kept identical on purpose, with a cost that has to be stated
/// rather than discovered:
///
/// `arch/aarch64/display_tegra.rs`'s ORIN-RASTGLASS probe discriminates rast ink by exact equality
/// with this constant (it restates it as `RG_PAPER`, because this module is outside that track's
/// lane). Keeping the value keeps that IDENTIFICATION true. What the windowed shape does falsify is
/// the probe's two sampling REGIONS: its SURROUND arm samples the panel OUTSIDE a centred 320x240
/// box and requires every pixel there to be this constant — a premise that was only ever true
/// because `run` owned the whole panel. A windowed renderer paints no pixel outside its row, so the
/// surround is now desktop, `paper == 0`, and the probe latches `NO-RAST-INK` (verdict 3, outside
/// `rg_painted`'s passing set).
///
/// That is the probe going BLIND, which is the direction its own header declares acceptable: "if
/// `rast_demo` changes its backdrop, this probe reports `NO-RAST-INK` — it goes BLIND, never falsely
/// green." It cannot produce a false PASS. The repair belongs to `display_tegra.rs`, which this arc
/// has no grant for, and the assertion it should carry instead is written up in the arc report and
/// in `docs/dev/OS/08_VIDEO/rasterizer.md`: sample INSIDE the row's content rect (`wm::info(id)`
/// gives `x`, `y`, `w`, `h` and `scale`), require this constant in the content's border margin and
/// at least one non-backdrop pixel in the content's middle, and require the panel OUTSIDE the row
/// to carry NO pixel of this value at all — the old surround test with its sense inverted, which is
/// the same two-population design its `blevels` note argues for.
const RW_CLEAR: Rgba = Rgba::rgb(0x10, 0x10, 0x18);

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

/// RASTWIN — render one frame of the scene into the caller's RGBA8 back buffer. Extracted from
/// `run`'s loop body so the frame the window is CREATED with and the frames it is PRESENTED with
/// are produced by one piece of code rather than two copies that can drift.
///
/// Returns `false` only when `Target::new` refuses the planes, which is a caller bug (mis-sized
/// buffers) and is reported by the caller rather than swallowed here.
fn rw_render(color: &mut [u8], depth: &mut [f32], frame: u32) -> bool {
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
    let Some(mut target) = Target::new(color, depth, w, h, w) else {
        return false;
    };
    target.clear(RW_CLEAR);
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
    true
}

/// RASTWIN — blit the RGBA8 render into the window's ARGB8888 surface and return the SOURCE row
/// band that actually changed, as `Some((dy0, dy1))` with `dy1` exclusive, or `None` when the frame
/// is pixel-identical to what the surface already holds.
///
/// THE FORMAT SEAM, stated once. `rast` renders RGBA8 — bytes `[R, G, B, A]`, the crate's one
/// canonical format (`raster.rs`'s module docs). A `wm` surface is ARGB8888 stored as the
/// little-endian word `0x00RRGGBB`, which is exactly the pixel `wm::draw_window` reads back out
/// (`pulsewin::SurfacePal`'s note). Those are different byte orders, so the conversion is real work
/// and not a `copy_from_slice`; it is also the cheapest place to do it, because the comparison this
/// function needs for damage has to touch every word anyway.
///
/// WHY THE COMPARISON IS THE DAMAGE MODEL. The rasterizer clears the whole 320x240 to [`RW_CLEAR`]
/// every frame and then draws a cube over part of it, so "what the renderer wrote" is the whole
/// surface and is useless as damage. What MOVED is a much smaller band, and it is exact rather than
/// estimated: a word that compares equal is not written and cannot have changed the glass. Tracking
/// the band (rather than a per-row bitmap) is what `wm::present_rows` can consume — it takes one
/// contiguous source band — so a finer model here would have nothing to spend itself on.
fn rw_blit(color: &[u8], surf: &mut [u8], stride: usize) -> Option<(usize, usize)> {
    let (w, h) = (DEMO_W, DEMO_H);
    let mut dy0 = usize::MAX;
    let mut dy1 = 0usize;
    for y in 0..h {
        let src = y * w * 4;
        let dst = y * stride;
        let mut moved = false;
        for x in 0..w {
            let p = src + x * 4;
            // RGBA8 -> 0x00RRGGBB. Alpha is dropped: `wm` surfaces are opaque, and the crate's
            // own kernel-blit note says the wire-in converts on blit.
            let c = ((color[p] as u32) << 16)
                | ((color[p + 1] as u32) << 8)
                | (color[p + 2] as u32);
            let o = dst + x * 4;
            let old = u32::from_le_bytes([surf[o], surf[o + 1], surf[o + 2], surf[o + 3]]);
            if old != c {
                surf[o..o + 4].copy_from_slice(&c.to_le_bytes());
                moved = true;
            }
        }
        if moved {
            if y < dy0 {
                dy0 = y;
            }
            dy1 = y + 1;
        }
    }
    if dy0 == usize::MAX {
        None
    } else {
        Some((dy0, dy1))
    }
}

/// Render the spinning cube for [`FRAMES`] frames into a compositor WINDOW, close the window, and
/// return so the caller resumes the normal interactive loop. Emits one honest fps line, one damage
/// line, and one machine-checkable verdict line.
///
/// `_screen` is the caller's panel `Screen` and is deliberately UNUSED: the whole point of RASTWIN
/// is that this path writes no panel pixel of its own. The parameter is kept so the three call
/// sites (`main.rs`'s x86/virt block, `tegra_rast_demo_maybe`, `pi_rast_demo_maybe`) are unchanged
/// — two of them sit on folded, line-neutral statements whose line numbers are load-bearing for the
/// knob-off byte-identity proof, and changing a signature to save an underscore would have meant
/// editing them.
///
/// EVERY DECLINE IS NAMED ON THE WIRE AND NONE IS FATAL. There is no panel fallback: a build that
/// cannot seat a row says so and paints nothing, because "must not touch pixels outside its row" is
/// not a property that may hold only on the boots where it is convenient.
pub fn run(_screen: &mut crate::video::Screen) {
    let (w, h) = (DEMO_W, DEMO_H);

    // The rast back buffer: RGBA8 color + f32 depth, one entry per pixel. Separate from the window
    // surface because the two formats differ (see `rw_blit`).
    let mut color = vec![0u8; 4 * w * h];
    let mut depth = vec![0f32; w * h];
    if !rw_render(&mut color, &mut depth, 0) {
        serial_println!("{} -> FAIL reason=target-mismatch ::", WIN_VERDICT);
        return;
    }
    let Some(mut win) = rw_take_or_open("spin", &color) else {
        // Every decline named its own reason above. The verdict line is still emitted so a capture
        // never has to infer the outcome from the ABSENCE of a PASS.
        serial_println!("{} -> FAIL reason=no-window ::", WIN_VERDICT);
        return;
    };

    // ── The spin ───────────────────────────────────────────────────────────────────────────────
    // Frame 0 is already on the glass: `create_at` composited it. Frames 1.. present the row band
    // that moved and nothing else.
    let t_start = crate::arch::ms();
    let mut rows_presented = 0u64;
    let mut presents = 0u64;
    let mut still = 0u64;
    let mut widest = 0usize;
    for frame in 1..FRAMES {
        if !rw_render(&mut color, &mut depth, frame) {
            serial_println!("[rastwin] frame {} target-mismatch — spin ended early", frame);
            break;
        }
        let band = win.present(&color);
        if band == 0 {
            // Nothing moved: the compositor is handed NOTHING. A demo that presents an identical
            // frame asks the panel to repaint itself for no reason, and on a WC aperture that is
            // the expensive half of the loop.
            still += 1;
        } else {
            rows_presented += band as u64;
            if band > widest {
                widest = band;
            }
            presents += 1;
        }

        // Pace: hold this frame until its wall-clock slot so the spin is visible at a steady
        // cadence. Pure delay, measured from `t_start`; a platform whose present is already slower
        // than the slot never waits. `PACE_POLL_CAP` is the finite backstop.
        let slot = t_start + (frame as u64) * FRAME_MS;
        let mut polls = 0u64;
        while crate::arch::ms() < slot && polls < PACE_POLL_CAP {
            polls += 1;
            core::hint::spin_loop();
        }
    }
    let elapsed = crate::arch::ms().saturating_sub(t_start).max(1);
    let fps_x1000 = (FRAMES as u64 * 1000 * 1000) / elapsed;

    // ── The fixture, taken while the row is still LIVE ─────────────────────────────────────────
    // Read the row back out of `wm` rather than restating what we asked for: `info` answers what
    // the compositor actually seated, which is the only thing a later reader can act on.
    let seated = wm::info(win.id);
    let geom_ok = matches!(seated, Some(ref i) if i.w == w && i.h == h && i.owner_asid == RW_OWNER);
    // HITTABLE == draggable and raisable by the ordinary route. The probe point is the centre of
    // the row's own TITLE BAR, which is the strip `wm::drag_begin` accepts a press on, so a hit
    // here is the drag handle answering and not merely "some pixel of the box". `hit_test` skips
    // `owner_asid == 0` and rows at or below the shell floor, so this leg measures exactly the pair
    // of properties `RW_OWNER` was chosen for.
    let (hx, hy) = (win.ox + win.ow / 2, win.oy + wm::TITLE_H / 2);
    let hit = wm::hit_test(hx as i32, hy as i32);
    let hit_ok = matches!(hit, Some((hid, howner, _)) if hid == win.id && howner == RW_OWNER);
    // The damage leg: the band presented per frame never exceeded the surface, and the total is
    // strictly less than whole-box-every-frame. `widest <= h` is the "never whole-panel" property
    // in the only unit the compositor was ever handed.
    let whole_box = (FRAMES as u64 - 1) * h as u64;
    let dmg_ok = widest <= h && rows_presented < whole_box;
    let pass = geom_ok && hit_ok && dmg_ok;
    serial_println!(
        "[rastwin] damage frames={} presents={} still={} rows_presented={} rows_if_whole_box={} \
         widest_band={} of {} panel_px_written_outside_row=0",
        FRAMES - 1,
        presents,
        still,
        rows_presented,
        whole_box,
        widest,
        h
    );
    serial_println!(
        ":: RAST: {} frames in {} ms — {}.{:03} fps (software rasterizer, compositor window) ::",
        FRAMES,
        elapsed,
        fps_x1000 / 1000,
        fps_x1000 % 1000
    );
    serial_println!(
        "{} win={} geom={} hit={} dmg={} rows={}/{} probe=({},{}) -> {} ::",
        WIN_VERDICT,
        win.id,
        if geom_ok { "OK" } else { "BAD" },
        if hit_ok { "OK" } else { "BAD" },
        if dmg_ok { "OK" } else { "BAD" },
        rows_presented,
        whole_box,
        hx,
        hy,
        if pass { "PASS" } else { "FAIL" }
    );

    // ── Park the window ────────────────────────────────────────────────────────────────────────
    // WHAT IS BOUNDED IS THE SPIN, NOT THE WINDOW. `FRAMES` is what makes QEMU boot straight through
    // to the interactive path and it is untouched; when it runs out the row stays on the desktop
    // with its last frame, draggable, raisable and closable by the ordinary route. The first shape
    // of this arc tore the row down here; it no longer does, because a window that erases itself
    // three seconds after it opens is a demo and one that stays is an app — the brief's own
    // preferred option. Parking costs ~300 KiB retained for the boot. See `RwWin::park`, including
    // the note on a causal claim about `wm::close` that the evidence did NOT support.
    //
    // A67 — ORIN-RASTGLASS's `post` read-back is taken HERE, on the last composited frame, and NOT
    // at the terminus line where it used to sit. THAT MOVE IS THE WHOLE REPAIR: `post` asks "did the
    // blit reach the scan-out at all", and the only instant at which that question has an answer is
    // while the row is composited and carrying the cube. At the terminus — after `run` returns — the
    // answer would be whatever the desktop looks like there, which is an instrument that says the
    // same thing on a working machine and a broken one. It is taken before the park rather than
    // after only so that nothing at all can come between the sample and the last presented frame.
    // The call is arch-gated on the one panel that HAS a read-back probe, and `main.rs:7141`'s
    // now-duplicate call is removed in the same commit, so the sample is taken exactly once.
    #[cfg(all(feature = "tegra", target_arch = "aarch64"))]
    crate::arch::display_tegra::orin_rast_glass_post();
    win.park("spin");
}

/// RASTWIN — the fixture's line head, as one constant so the emit sites cannot drift from each
/// other. The verdict tail is ` -> PASS ::` or ` -> FAIL ::`, and `arroyo`'s `FAULT_PATTERNS`
/// (`-> FAIL|FAIL ::|FAIL — |PANIC|panicked at |EXCEPTION:`) is what turns a FAIL into a red leg —
/// so this fixture reds `./arroyo test` and `./arroyo test-arm` without either verb needing a spec
/// change. Go-red proven by mutation, not by reading: see the arc report.
const WIN_VERDICT: &str = ":: RASTWIN: 3D-in-a-window";

/// RASTWIN — the demo's compositor window: its `wm` row, the ARGB8888 surface that row points at,
/// and the geometry the compositor actually seated.
///
/// It exists because there are TWO renderers in this module — the paced single-core spin ([`run`])
/// and the frame-pipelined multi-core pass ([`run_mc`]) — and before this arc they each poked panel
/// coordinates through `Screen::put_pixel`. Giving only `run` a window would have left `run_mc`
/// owning the glass on the one target where it is compiled by default (aarch64/tegra), so the
/// invariant "this module writes no panel pixel outside its row" would have been true of a build
/// nobody boots and false of the Orin. One window type, opened by both.
struct RwWin {
    id: wm::WinId,
    /// The row's surface. Owned here: dropped only after [`RwWin::close`] has run `wm::close`'s
    /// drain barrier, so no composite pass can be reading it.
    store: Vec<u8>,
    stride: usize,
    /// Outer box and content origin, as [`wm::spawn_geometry`] sized it and [`wm::create_at`]
    /// seated it — kept so the fixture can hit-test the title bar without asking `wm` twice.
    ox: usize,
    oy: usize,
    ow: usize,
}

impl RwWin {
    /// Open the window with `first` already rendered into it. `tag` names the caller on the wire so
    /// a capture carrying both passes can tell the two opens apart. `None` on any decline, each one
    /// named on its own line.
    fn open(tag: &str, first: &[u8]) -> Option<Self> {
        let (w, h) = (DEMO_W, DEMO_H);
        let stride = w * 4;
        let len = h * stride;
        let mut store: Vec<u8> = Vec::new();
        if store.try_reserve_exact(len).is_err() {
            serial_println!("[rastwin] {} open DECLINE reason=alloc len={}", tag, len);
            return None;
        }
        store.resize(len, 0);
        // PAINT BEFORE THE WINDOW NAMES THE SURFACE: `create_at` composites the new row before it
        // returns, so a surface still full of zeros would put one frame of a BLACK BOX on the
        // glass. `instgui` and `login` both order it this way and say so.
        rw_blit(first, &mut store, stride);

        let (scale, ow, oh) = wm::spawn_geometry(w, h)?;
        let (pw, ph) = {
            let fb = *crate::video::WRITER.lock();
            let i = fb.info();
            (i.width, i.height)
        };
        if pw < ow || ph < oh {
            serial_println!(
                "[rastwin] {} open DECLINE reason=panel-cannot-seat panel={}x{} box={}x{}",
                tag, pw, ph, ow, oh
            );
            return None;
        }
        let ox = pw.saturating_sub(ow) / 2;
        let oy = ph.saturating_sub(oh) / 2;
        let base = store.as_mut_ptr() as usize;
        let id = wm::create_at(
            RW_OWNER,
            base,
            len,
            w as u32,
            h as u32,
            stride as u32,
            RW_TITLE,
            ox + wm::BORDER,
            oy + wm::TITLE_H + wm::BORDER,
        );
        if id == wm::WIN_NONE {
            serial_println!("[rastwin] {} open DECLINE reason=create-refused", tag);
            return None;
        }
        // A67 — publish the SEATED geometry before the first witness line, so a capture that carries
        // an `[orinrast]` verdict always carries the open line that explains where it looked.
        rw_publish_seat(id);
        // The row is PARKED on the desktop when the spin ends rather than closed (see `park`), so it
        // outlives this function and an operator can close it. Register the cell so `wm::close` on
        // ANY route clears it and this module never hands back a re-issued id.
        RW_WIN.store(id, core::sync::atomic::Ordering::Release);
        wm::winid_register_holder(&RW_WIN, "rastwin");
        serial_println!(
            "[rastwin] {} open win={} owner={:#x} surf={}x{} box={}x{} scale={} at ({},{}) \
             panel={}x{} title={} seat={:?} (3D renders INTO this row; this module writes no panel \
             pixel outside it)",
            tag, id, RW_OWNER, w, h, ow, oh, scale, ox, oy, pw, ph,
            core::str::from_utf8(RW_TITLE).unwrap_or("?"),
            rastwin_seat().map(|s| (s.cx, s.cy, s.cw, s.ch))
        );
        Some(RwWin { id, store, stride, ox, oy, ow })
    }

    /// Blit one rendered frame in and present ONLY the source rows that moved. Returns the band
    /// height presented (0 when the frame was pixel-identical and nothing was handed to the
    /// compositor at all).
    fn present(&mut self, color: &[u8]) -> usize {
        match rw_blit(color, &mut self.store, self.stride) {
            Some((dy0, dy1)) => {
                wm::present_rows(self.id, dy0, dy1);
                dy1 - dy0
            }
            None => 0,
        }
    }

    /// **Park the window on the desktop instead of tearing it down.**
    ///
    /// THIS REPLACED A `wm::close(id)` + `drop(store)`, on design grounds: a 3D window that erases
    /// itself three seconds after it opens is a demo, and a window that stays is an app. The brief's
    /// own preferred option was "closable by the ordinary route", and that is what parking buys —
    /// the row stays on the desktop beside Console / Shell / Quarry with its last frame, draggable
    /// and raisable, and the operator closes it when they are done with it.
    ///
    /// ⚠ A CAUSAL CLAIM WAS ALMOST MADE HERE AND IS NOT SUPPORTED — recorded so nobody re-derives
    /// it. This change was first made because `wm::dmgovlp_selftest` failed (`drag_evt=0 … -> FAIL`)
    /// on a run that closed the row and passed (`drag_evt=5 … -> PASS`) on a run that did not. A
    /// wider census killed that reading: `dmgovlp` also produced BOTH outcomes with `rast` OFF, on
    /// builds where this module is not linked at all and the image is byte-identical to baseline.
    /// The fixture is load-sensitive on this box, so it cannot convict or acquit anything here.
    /// See orin-ledger A68 for the six-run table. **Parking stands on the design argument above and
    /// on nothing else.**
    ///
    /// WHAT PARKING COSTS, stated rather than discovered: the ~300 KiB surface is retained for the
    /// life of the boot, held by [`RW_KEEP`]. That is not a leak — it is a module-owned allocation
    /// with a live owner, exactly `pulsewin`'s `STORE` shape — and it buys the behaviour this window
    /// should have had anyway: the cube stays ON the desktop beside Console / Shell / Quarry,
    /// draggable and raisable, instead of vanishing three seconds after it appears. The [`FRAMES`]
    /// bound is untouched, so QEMU still boots straight through; what ends is the SPIN, not the
    /// window.
    ///
    /// The row stays closable by the ordinary route: [`RW_WIN`] is registered with
    /// `wm::winid_register_holder`, so a close disc, a Quit or `wc_close_furniture` clears the cell
    /// through `wm::close` exactly as for every other furniture row. The surface then outlives the
    /// row, which is the SAFE direction — the unsafe one is freeing a surface a live row still
    /// points at, which is the rule `pulsewin::close` exists to state.
    fn park(self, tag: &str) {
        let id = self.id;
        serial_println!(
            "[rastwin] {} park win={} -> ON-DESKTOP (spin ended, window stays; surface retained, \
             closable by the ordinary route)",
            tag, id
        );
        *RW_KEEP.lock() = Some(self);
    }
}

/// RASTWIN — the parked window, owned for the life of the boot. See [`RwWin::park`].
///
/// It is also how the two renderers SHARE one row: on tegra `run_mc` runs first, opens the window
/// and parks it; `run` then takes it back out and keeps spinning in the same row rather than
/// minting a second one. One 3D window per boot, not two in sequence.
static RW_KEEP: spin::Mutex<Option<RwWin>> = spin::Mutex::new(None);

/// RASTWIN — the parked row's id, registered with `wm::winid_register_holder` so ANY close route
/// (the title-bar close disc, a Quit, `wc_close_furniture`) clears it through `wm::close`, and this
/// module can never hand back an id the table has re-issued.
static RW_WIN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(wm::WIN_NONE);

/// RASTWIN — reuse the parked window if there is one, else open a fresh one.
///
/// `first` is blitted only on a fresh open: a parked row already carries the last frame the previous
/// pass presented, so re-blitting would be a whole-surface write that the damage comparison then
/// correctly reports as zero rows moved — work for nothing.
fn rw_take_or_open(tag: &str, first: &[u8]) -> Option<RwWin> {
    if let Some(w) = RW_KEEP.lock().take() {
        if wm::info(w.id).is_some() {
            serial_println!("[rastwin] {} reuse win={} (parked by an earlier pass)", tag, w.id);
            RW_SEAT_LIVE.store(true, core::sync::atomic::Ordering::Release);
            return Some(w);
        }
        // The operator closed it between the two passes. Drop the orphaned surface — the row it
        // backed is already gone, so nothing can be reading it — and mint a fresh window.
        serial_println!("[rastwin] {} parked win={} was closed by the operator — re-minting", tag, w.id);
        drop(w.store);
    }
    RwWin::open(tag, first)
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
// (`arroyo:573`), so `smp_virt::start_secondaries` has already brought the DTB-`/cpus`
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

/// Present one finished RGBA8 plane into the demo's compositor WINDOW — the same `wm` path [`run`]
/// uses, and no longer a poke at panel coordinates.
///
/// RASTWIN changed this from `Screen::put_pixel` + `Screen::flush` over a centred `DEMO_W`x`DEMO_H`
/// block. The old shape is what the RAST-MC header above calls out as the reason an x86 run could
/// not claim "first 3D pixels under the compositor": it wrote panel coordinates, so
/// `Screen::present_background` subtracted any occluding window box and the pixels were never
/// written at all — with `flush()` reporting success regardless. A row's pixels are composited by
/// the compositor's own clip, so they are occluded when the window is behind something and drawn
/// when it is not, which is what every other window already gets.
///
/// The RATIO this rung exists to measure is unaffected, and for the same reason the header gives:
/// both arms (the 1-core baseline and the pipelined pass) present through this one function, so
/// whatever a present costs, it costs both equally. The absolute fps moves — a window present is a
/// different amount of work from a panel poke — which is exactly why the baseline is re-measured in
/// the SAME boot rather than compared against a published number.
#[cfg(any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc", target_arch = "x86_64")))]
fn mc_present(win: &mut RwWin, color: &[u8]) {
    win.present(color);
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
pub fn run_mc(_screen: &mut crate::video::Screen) {
    // RASTWIN — the panel-geometry test that stood here (`screen.width() < DEMO_W`) is now
    // `RwWin::open`'s `panel-cannot-seat` decline, which asks the stricter and more correct
    // question: not "is the panel bigger than the render" but "can the panel seat the whole OUTER
    // box", chrome and border included. It is asked once, by the type that knows the answer.
    // `_screen` is unused for the same reason it is unused in `run`: this module writes no panel
    // pixel of its own any more.

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
    // RASTWIN — the window is opened with frame 0 already in it, BEFORE the clock starts, so the
    // open cost (one allocation, one `create_at`, one composite) is charged to neither arm. Both
    // arms then present through this one row.
    mc_render_frame(&mut base_color, &mut base_depth, 0);
    let Some(mut win) = rw_take_or_open("mc", &base_color) else {
        serial_println!(
            ":: RAST-MC: no compositor row could be seated — the multi-core rung is unavailable on \
             this boot; single-core path unchanged ::"
        );
        return;
    };
    let t0 = crate::arch::ms();
    for f in 0..FRAMES {
        mc_render_frame(&mut base_color, &mut base_depth, f);
        mc_present(&mut win, &base_color);
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
        // RASTWIN — the baseline arm already opened the row, so this fail-closed exit owns it.
        // A return that left the window live would strand a surface this frame drops.
        win.park("mc");
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
        mc_present(&mut win, &bufs[slot].0);
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

    // RASTWIN — retire the row. AFTER the drain above, never before: the workers' buffers and this
    // window's surface are different allocations, but the ORDER still matters for the reason
    // `RwWin::close` states — `wm::close` waits out in-flight composites, and this core is the one
    // that has been feeding them. `run` opens its own row for the paced spin immediately after this
    // function returns, so the glass is never left holding a dead box.
    win.park("mc");
}
