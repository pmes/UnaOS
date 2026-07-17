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

//! Headless witness for the damage-tracked render path (`Screen`).
//!
//! The `tste` `video.geometry` check exercises the *trait-default* Bresenham/scanline primitives on
//! an `OffscreenPal` that overrides `draw_pixel` — it never touches `Screen`'s damage tracking, its
//! `flush` present, or `FrameBuffer`'s real pixel-format encoder. Those are the steady-state
//! on-screen renderer, and until now they were verified only *visually* by `vug` and the GUI
//! console (attended). This module gives them an automated, arch-neutral, headless regression:
//! build a `Screen` over a heap-backed offscreen `FrameBuffer` and assert the present path is
//! format-correct, damage-limited, idempotent on a clean frame, and clip-safe.
//!
//! Arch-neutral (heap + `Vec` only, no `cfg`, no float): compiles and runs on x86_64, aarch64-virt,
//! and the Pi 4 baremetal target. Allocates ~1 KiB and runs only when `tste` is invoked.

use alloc::string::{String, ToString};
use alloc::vec;
use unaos_boot_info::{FrameBufferInfo, PixelFormat};

use super::{FrameBuffer, Screen};

// A tiny offscreen surface. Bgr/4bpp mirrors the real rMBP GOP layout so the format decode is
// exercised on the same path metal uses. `stride == width` (tight) keeps the byte maths obvious.
const W: usize = 16;
const H: usize = 16;
const BPP: usize = 4;
const STRIDE: usize = W;
const LEN: usize = STRIDE * H * BPP;

// A colour with three distinct channels so a byte-order (Rgb vs Bgr) bug can't hide behind equal
// bytes. Bgr layout writes it to memory as [B, G, R, 0] = [0x33, 0x22, 0x11, 0x00].
const TEST_COLOR: u32 = 0x0011_2233;

/// Read the little-endian pixel word at (x, y) out of a tight Bgr/4bpp backing buffer.
fn px(buf: &[u8], x: usize, y: usize) -> u32 {
    let o = (y * STRIDE + x) * BPP;
    u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]])
}

/// Exercise the damage-tracked `Screen` present path end-to-end. `Ok(())` on pass, `Err(why)` with a
/// specific reason on the first failed assertion. Emits one greppable serial line on success.
pub fn run() -> Result<(), String> {
    let info = FrameBufferInfo {
        width: W,
        height: H,
        stride: STRIDE,
        bytes_per_pixel: BPP,
        pixel_format: PixelFormat::Bgr,
    };

    // The "framebuffer" is an ordinary heap buffer; we hold it so we can read presented bytes back
    // and poke sentinels into it. `FrameBuffer::init` takes a raw base precisely so it is `Send`.
    let mut front_store = vec![0u8; LEN];
    let base = front_store.as_mut_ptr() as usize;
    let mut front = FrameBuffer::new();
    front.init(base, LEN, info);

    let mut screen = Screen::new(front);
    // `Screen::new` marks the whole frame dirty; flush the (zeroed) back over the front to reach a
    // known all-zero baseline before we start asserting.
    screen.flush();
    if front_store.iter().any(|&b| b != 0) {
        return Err("baseline flush left non-zero front".to_string());
    }

    // --- Format decode: a known colour must land in Bgr byte order in the front buffer. ---
    screen.fill_rect(4, 4, 4, 4, TEST_COLOR); // damage bbox [4,8) x [4,8)
    screen.flush();
    if px(&front_store, 4, 4) != TEST_COLOR {
        return Err("Bgr decode: presented pixel != source colour".to_string());
    }
    // Spot-check the raw byte order so an Rgb/Bgr swap is caught even if the round-trip word matched
    // by coincidence.
    let o = (4 * STRIDE + 4) * BPP;
    if front_store[o] != 0x33 || front_store[o + 1] != 0x22 || front_store[o + 2] != 0x11 {
        return Err("Bgr byte order wrong (expected [B,G,R] = 33,22,11)".to_string());
    }
    if px(&front_store, 3, 4) != 0 || px(&front_store, 8, 4) != 0 {
        return Err("fill_rect present bled outside its bbox".to_string());
    }

    // --- Damage-limited present: a poke OUTSIDE the next draw's bbox must survive a flush. ---
    // Sentinel at (0,0), far from the (10,10) rect below. If flush blitted the whole surface (no
    // damage tracking) it would overwrite this from the still-zero back buffer.
    const SENTINEL: u32 = 0x00AB_CDEF;
    let s = 0usize; // (0,0) byte offset
    front_store[s] = 0xEF;
    front_store[s + 1] = 0xCD;
    front_store[s + 2] = 0xAB;
    front_store[s + 3] = 0x00;
    screen.fill_rect(10, 10, 4, 4, TEST_COLOR); // damage bbox [10,14) x [10,14), disjoint from (0,0)
    screen.flush();
    if px(&front_store, 0, 0) != SENTINEL {
        return Err("damage-limited present FAILED: flush clobbered a pixel outside the damage bbox"
            .to_string());
    }
    if px(&front_store, 10, 10) != TEST_COLOR {
        return Err("second fill_rect not presented".to_string());
    }

    // --- No-op flush: with no new damage the front must be byte-stable (idempotent). ---
    let snapshot = front_store.clone();
    screen.flush(); // damage is None here
    if front_store != snapshot {
        return Err("no-op flush mutated the front buffer".to_string());
    }

    // --- Clip safety: signed off-screen geometry must clip, not panic or overrun. ---
    screen.draw_line(-8, 8, 24, 8, TEST_COLOR); // horizontal line crossing the whole surface
    screen.fill_triangle((-4, -4), (24, 2), (8, 24), TEST_COLOR); // apex/edges off-screen
    screen.flush();
    // The clipped line must have painted at least the on-screen span of row 8.
    if px(&front_store, 0, 8) != TEST_COLOR || px(&front_store, W - 1, 8) != TEST_COLOR {
        return Err("clipped draw_line did not paint the on-screen span".to_string());
    }
    // The triangle must have filled some in-bounds pixels but not the whole surface.
    let filled = (0..W * H)
        .filter(|&i| {
            let (x, y) = (i % W, i / W);
            px(&front_store, x, y) == TEST_COLOR
        })
        .count();
    if filled == 0 {
        return Err("clipped fill_triangle set 0 in-bounds pixels".to_string());
    }
    if filled >= W * H {
        return Err("clipped fill_triangle overran the surface".to_string());
    }

    serial_println!(":: VWIT: render present — format=Bgr damage=OK noop=OK clip=OK ::");
    Ok(())
}
