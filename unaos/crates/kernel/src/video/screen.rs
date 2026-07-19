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

    /// CURSOR-SAVE-UNDER: read one pixel from the BACK buffer (cached heap RAM — a cheap read;
    /// the front framebuffer is never read back, keeping the WC/write-only VRAM contract).
    /// This is what lets `pal::cursor` stash the pixels under the sprite and restore them on
    /// move/hide, so every `Screen`-backed surface inherits trail-free cursor motion without
    /// per-surface damage tracking.
    #[inline]
    pub fn read_back_pixel(&self, x: usize, y: usize) -> Option<u32> {
        self.back.get_pixel(x, y)
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        self.back.fill_rect(x, y, w, h, color);
        self.mark(x, y, x + w, y + h);
    }

    pub fn fill_screen(&mut self, color: u32) {
        self.back.fill_screen(color);
        self.mark_full();
    }

    /// Draw a Bresenham line into the back buffer and mark its bounding box damaged. Endpoints are
    /// signed; the surface clips per-pixel. The `vug` wireframe primitive.
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) {
        self.back.draw_line(x0, y0, x1, y1, color);
        let lo_x = x0.min(x1).max(0) as usize;
        let lo_y = y0.min(y1).max(0) as usize;
        let hi_x = x0.max(x1).max(0) as usize;
        let hi_y = y0.max(y1).max(0) as usize;
        self.mark(lo_x, lo_y, hi_x + 1, hi_y + 1);
    }

    /// Fill the triangle `(a, b, c)` (pixel coordinates) with a flat colour, marking its bounding
    /// box damaged. Half-space scanline rasteriser: for each row spanning the triangle's vertical
    /// extent, fill between the two edge intersections. The `vug` solid-facet primitive; the caller
    /// does backface culling and painter's-order sorting, so this just fills.
    pub fn fill_triangle(
        &mut self,
        a: (i32, i32),
        b: (i32, i32),
        c: (i32, i32),
        color: u32,
    ) {
        // Sort vertices by y ascending: p0.y <= p1.y <= p2.y.
        let mut p = [a, b, c];
        p.sort_unstable_by_key(|v| v.1);
        let (p0, p1, p2) = (p[0], p[1], p[2]);
        let w = self.info.width as i32;
        let h = self.info.height as i32;

        // Interpolate an x at scanline `y` along the edge from `q` to `r` (in 1/65536 px).
        let edge_x = |q: (i32, i32), r: (i32, i32), y: i32| -> i32 {
            if r.1 == q.1 {
                return q.0;
            }
            q.0 + ((r.0 - q.0) as i64 * (y - q.1) as i64 / (r.1 - q.1) as i64) as i32
        };

        let y_top = p0.1.max(0);
        let y_bot = p2.1.min(h - 1);
        let mut min_x = w;
        let mut max_x = 0;
        let mut y = y_top;
        while y <= y_bot {
            // Long edge p0->p2 spans the whole height; the short edge switches at p1.y.
            let xa = edge_x(p0, p2, y);
            let xb = if y < p1.1 {
                edge_x(p0, p1, y)
            } else {
                edge_x(p1, p2, y)
            };
            let (mut xl, mut xr) = if xa <= xb { (xa, xb) } else { (xb, xa) };
            xl = xl.max(0);
            xr = xr.min(w - 1);
            if xl <= xr {
                let run = (xr - xl + 1) as usize;
                self.back.fill_rect(xl as usize, y as usize, run, 1, color);
                min_x = min_x.min(xl);
                max_x = max_x.max(xr);
            }
            y += 1;
        }
        if min_x <= max_x {
            self.mark(min_x as usize, y_top as usize, max_x as usize + 1, y_bot as usize + 1);
        }
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
