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

//! The framebuffer surface — the one pixel-format-aware drawing primitive.
//!
//! Everything that puts pixels on screen goes through this single type: the GUI renderer
//! (via `pal`/`console`), the boot/panic console (`fbcon`), and the background painter
//! (`vug`). The Rgb/Bgr/U8 decode and the bounds checks therefore live in exactly one place
//! instead of being copy-pasted three times.
//!
//! A surface is addressed by a raw base address held as `usize` (so the struct is `Send` and
//! can live in a `static Mutex`). That matches how both display paths hand us a framebuffer —
//! UEFI GOP and the Pi 4 VideoCore mailbox each give a base address plus a `FrameBufferInfo`
//! (width/height/stride/bpp/format). `physical_memory_offset == 0` (identity map) throughout,
//! so the address is used directly.

use unaos_boot_info::{FrameBufferInfo, PixelFormat};

/// Layout placeholder for an unattached surface; `is_ready()` is false until `init`.
const UNINIT_INFO: FrameBufferInfo = FrameBufferInfo {
    width: 0,
    height: 0,
    stride: 0,
    bytes_per_pixel: 0,
    pixel_format: PixelFormat::Unknown,
};

/// A linear framebuffer addressed by raw base address (identity-mapped). Cheap to copy — it's
/// just a handle to the underlying memory, like a pointer.
#[derive(Clone, Copy)]
pub struct FrameBuffer {
    base: usize,
    len: usize,
    info: FrameBufferInfo,
}

// The only non-`Send` field would be a raw pointer; we hold the base as a `usize` precisely so
// the surface can live in a `static Mutex`. Access is serialised by that lock.
unsafe impl Send for FrameBuffer {}

impl FrameBuffer {
    /// An empty, unattached surface — for `const` static initialisers. Draws are no-ops until
    /// `init` is called.
    pub const fn new() -> Self {
        Self { base: 0, len: 0, info: UNINIT_INFO }
    }

    /// Attach to a framebuffer: base address, byte length, and pixel layout.
    pub fn init(&mut self, base: usize, len: usize, info: FrameBufferInfo) {
        self.base = base;
        self.len = len;
        self.info = info;
    }

    #[inline]
    pub fn is_ready(&self) -> bool {
        self.base != 0 && self.len != 0 && self.info.width != 0 && self.info.height != 0
    }

    /// The byte length of the attached framebuffer (the firmware-reported size). The flush target
    /// and the double-buffer back store are both sized from this, so they cannot disagree.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn width(&self) -> usize {
        self.info.width
    }

    #[inline]
    pub fn height(&self) -> usize {
        self.info.height
    }

    #[inline]
    pub fn info(&self) -> FrameBufferInfo {
        self.info
    }

    /// Write one pixel. Format-aware and fully bounds-checked: coordinates outside the visible
    /// frame are rejected (so an `x >= width` can't wrap onto the next scanline), and the byte
    /// offset is checked against the buffer length.
    #[inline]
    pub fn put_pixel(&self, x: usize, y: usize, color: u32) {
        if self.base == 0 || x >= self.info.width || y >= self.info.height {
            return;
        }
        let offset = (y * self.info.stride + x) * self.info.bytes_per_pixel;
        if offset + self.info.bytes_per_pixel > self.len {
            return;
        }
        let r = ((color >> 16) & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = (color & 0xFF) as u8;
        unsafe {
            let p = (self.base + offset) as *mut u8;
            match self.info.pixel_format {
                PixelFormat::Rgb => {
                    p.write(r);
                    p.add(1).write(g);
                    p.add(2).write(b);
                }
                PixelFormat::Bgr => {
                    p.write(b);
                    p.add(1).write(g);
                    p.add(2).write(r);
                }
                PixelFormat::U8 => {
                    p.write(((r as u16 + g as u16 + b as u16) / 3) as u8);
                }
                _ => {}
            }
        }
    }

    /// Fill pixel rows `[y0, y1)` (clamped to the frame) with a colour.
    pub fn fill_rows(&self, y0: usize, y1: usize, color: u32) {
        let y_end = y1.min(self.info.height);
        for y in y0..y_end {
            for x in 0..self.info.width {
                self.put_pixel(x, y, color);
            }
        }
    }

    /// Fill the whole frame.
    pub fn fill_screen(&self, color: u32) {
        self.fill_rows(0, self.info.height, color);
    }

    /// Fill an axis-aligned rectangle (clipped by `put_pixel`'s bounds checks).
    pub fn fill_rect(&self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        for row in 0..h {
            for col in 0..w {
                self.put_pixel(x + col, y + row, color);
            }
        }
    }

    /// Copy `src` bytes into the framebuffer starting at `byte_offset` (bounds-checked, no-op if
    /// it would overrun). This is the double-buffer flush primitive: a bulk sequential copy onto
    /// the framebuffer, which on real hardware is slow/write-combining — sequential bulk copies
    /// are far friendlier to it than the per-pixel pokes drawing uses on the cached back buffer.
    pub fn blit(&self, byte_offset: usize, src: &[u8]) {
        if self.base == 0 || byte_offset + src.len() > self.len {
            return;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                src.as_ptr(),
                (self.base + byte_offset) as *mut u8,
                src.len(),
            );
        }
    }

    /// Scroll the whole frame up by `dy` pixel rows, clearing the freed bottom band to `fill`.
    /// Used by the text console; a single `memmove` of the scrolled region, not a per-pixel copy.
    pub fn scroll_up(&self, dy: usize, fill: u32) {
        if self.base == 0 || dy == 0 {
            return;
        }
        let row_bytes = self.info.stride * self.info.bytes_per_pixel;
        let shift = dy * row_bytes;
        let total = (self.info.height * row_bytes).min(self.len);
        if shift >= total {
            return;
        }
        unsafe {
            core::ptr::copy(
                (self.base + shift) as *const u8,
                self.base as *mut u8,
                total - shift,
            );
        }
        let cleared_from = self.info.height.saturating_sub(dy);
        self.fill_rows(cleared_from, self.info.height, fill);
    }
}

impl Default for FrameBuffer {
    fn default() -> Self {
        Self::new()
    }
}
