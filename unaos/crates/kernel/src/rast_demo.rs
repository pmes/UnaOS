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
