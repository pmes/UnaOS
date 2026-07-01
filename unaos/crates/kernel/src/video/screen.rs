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

//! Double-buffered display surface with damage tracking.
//!
//! The bare `FrameBuffer` writes straight to the framebuffer. That's fine for the boot console
//! (fbcon, pre-heap) but wrong as the steady-state renderer: framebuffer memory is slow and
//! write-combining on real hardware, so per-pixel pokes — and the GUI console repainting the
//! whole screen on every keystroke — crawl on metal and flicker.
//!
//! `Screen` fixes both. All drawing goes to a back buffer in ordinary cached RAM (fast per-pixel
//! writes, no flicker), and [`Screen::flush`] copies only the *damaged* region to the real
//! framebuffer as bulk sequential row copies (write-combining-friendly). The GUI draws to the
//! back buffer and calls `flush()` (via `pal`'s `render()`) once per frame.
//!
//! The back buffer is a second `FrameBuffer` pointing at heap memory, so it reuses all of the
//! surface's format-aware drawing logic — no duplicated pixel poking here.

use alloc::vec;
use alloc::vec::Vec;
use unaos_boot_info::FrameBufferInfo;

use super::FrameBuffer;

/// A damaged region as a half-open pixel rectangle `[x0, x1) x [y0, y1)`.
#[derive(Clone, Copy)]
struct Damage {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

pub struct Screen {
    /// The real framebuffer (flush target).
    front: FrameBuffer,
    /// Owns the back-buffer memory (cached RAM), same byte layout as `front`.
    back_store: Vec<u8>,
    /// A surface handle pointing into `back_store`. SAFETY/INVARIANT: `back_store` is allocated
    /// once at its final size in `new` and never grown/shrunk, so its heap buffer never moves —
    /// the address captured here stays valid for the life of the `Screen` (moving the `Screen`
    /// moves the `Vec` header, not the heap allocation it points at).
    back: FrameBuffer,
    info: FrameBufferInfo,
    /// Accumulated dirty rectangle since the last flush; `None` when nothing changed.
    damage: Option<Damage>,
}

impl Screen {
    /// Build a double-buffered screen over `front`. Allocates a back buffer the same size as the
    /// framebuffer and marks the whole frame dirty so the first `flush` paints everything.
    pub fn new(front: FrameBuffer) -> Self {
        let info = front.info();
        // Single source of truth for the buffer length: the front framebuffer's reported size.
        // Sizing the back store to exactly `front.len()` guarantees the two surfaces agree, so a
        // flush can never have one bounds check pass while the other rejects (which would silently
        // drop rows). Firmware sometimes reports a size != stride*height*bpp — e.g. Apple's Retina
        // GOP — so we warn if it's *short* of the visible image (rows past the end can't be shown).
        let computed = info.stride * info.height * info.bytes_per_pixel;
        let len = front.len();
        if len < computed {
            serial_println!(
                ":: VIDEO WARNING: framebuffer_size {} < stride*height*bpp {} (firmware quirk); \
                 bottom rows may not display ::",
                len,
                computed
            );
        }
        let mut back_store = vec![0u8; len];
        let mut back = FrameBuffer::new();
        back.init(back_store.as_mut_ptr() as usize, len, info);
        Self {
            front,
            back_store,
            back,
            info,
            damage: Some(Damage { x0: 0, y0: 0, x1: info.width, y1: info.height }),
        }
    }

    #[inline]
    pub fn width(&self) -> usize {
        self.info.width
    }

    #[inline]
    pub fn height(&self) -> usize {
        self.info.height
    }

    /// Grow the dirty rectangle to include `[x0, x1) x [y0, y1)` (clamped to the frame).
    fn mark(&mut self, x0: usize, y0: usize, x1: usize, y1: usize) {
        let x1 = x1.min(self.info.width);
        let y1 = y1.min(self.info.height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        self.damage = Some(match self.damage {
            None => Damage { x0, y0, x1, y1 },
            Some(d) => Damage {
                x0: d.x0.min(x0),
                y0: d.y0.min(y0),
                x1: d.x1.max(x1),
                y1: d.y1.max(y1),
            },
        });
    }

    fn mark_full(&mut self) {
        self.damage = Some(Damage { x0: 0, y0: 0, x1: self.info.width, y1: self.info.height });
    }

    #[inline]
    pub fn put_pixel(&mut self, x: usize, y: usize, color: u32) {
        self.back.put_pixel(x, y, color);
        self.mark(x, y, x + 1, y + 1);
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        self.back.fill_rect(x, y, w, h, color);
        self.mark(x, y, x + w, y + h);
    }

    pub fn fill_screen(&mut self, color: u32) {
        self.back.fill_screen(color);
        self.mark_full();
    }

    pub fn scroll_up(&mut self, dy: usize, fill: u32) {
        self.back.scroll_up(dy, fill);
        self.mark_full();
    }

    /// Present the back buffer: copy the damaged region to the framebuffer, row by row (each row
    /// a single bulk copy), then clear the damage. No-op if nothing changed.
    pub fn flush(&mut self) {
        let Some(d) = self.damage.take() else { return };
        if !self.front.is_ready() {
            return;
        }
        let bpp = self.info.bytes_per_pixel;
        let stride = self.info.stride;
        let x1 = d.x1.min(self.info.width);
        let y1 = d.y1.min(self.info.height);
        if d.x0 >= x1 || d.y0 >= y1 {
            return;
        }
        let seg = (x1 - d.x0) * bpp;
        for y in d.y0..y1 {
            let off = (y * stride + d.x0) * bpp;
            if off + seg <= self.back_store.len() {
                self.front.blit(off, &self.back_store[off..off + seg]);
            }
        }
        // Present to a non-coherent scan-out (the Pi 4 HVS) with a single cache clean over the
        // whole damaged span — one `DC CVAC` sweep + one `DSB`, not one per scanline. The span is a
        // contiguous byte range covering every blitted row (its interior may include undamaged
        // left/right margins of middle rows, but cleaning already-clean lines is harmless). No-op on
        // cache-coherent targets (x86, and QEMU which models no caches).
        let span_start = (d.y0 * stride + d.x0) * bpp;
        let span_end = ((y1 - 1) * stride + x1) * bpp;
        self.front.flush_range(span_start, span_end - span_start);
    }
}
