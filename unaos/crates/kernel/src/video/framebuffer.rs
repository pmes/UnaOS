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
    pub fn base(&self) -> usize {
        self.base
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
        // VPERF (bench builds only): count per-pixel pokes — the per-glyph/per-fill cost model.
        #[cfg(all(target_arch = "x86_64", feature = "videobench"))]
        crate::video::vperf::on_put_pixel();
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

    /// Read one pixel back as 0x00RRGGBB, or `None` when out of bounds / format undecodable.
    /// CURSOR-SAVE-UNDER support — for CACHED-RAM surfaces ONLY (a `Screen` back buffer, an
    /// fbcon shadow). Never call this on a handle over real VRAM: on the rMBP every read of the
    /// WC/uncached GOP surface is a PCIe round trip, the exact cost the back buffers exist to
    /// remove. The one caller (`Screen::read_back_pixel`) reads only its heap store.
    /// For the deliberate, witness-gated VRAM read-back the pi compositor's verify path needs,
    /// see [`read_pixel`](Self::read_pixel) — the two exist separately ON PURPOSE (wc-op1 merge):
    /// this one stays cheap and cached; that one is volatile and paid for knowingly.
    #[inline]
    pub fn get_pixel(&self, x: usize, y: usize) -> Option<u32> {
        if self.base == 0 || x >= self.info.width || y >= self.info.height {
            return None;
        }
        let offset = (y * self.info.stride + x) * self.info.bytes_per_pixel;
        if offset + self.info.bytes_per_pixel > self.len {
            return None;
        }
        unsafe {
            let p = (self.base + offset) as *const u8;
            match self.info.pixel_format {
                PixelFormat::Rgb => Some(
                    ((p.read() as u32) << 16)
                        | ((p.add(1).read() as u32) << 8)
                        | p.add(2).read() as u32,
                ),
                PixelFormat::Bgr => Some(
                    (p.read() as u32)
                        | ((p.add(1).read() as u32) << 8)
                        | ((p.add(2).read() as u32) << 16),
                ),
                PixelFormat::U8 => {
                    let g = p.read() as u32;
                    Some((g << 16) | (g << 8) | g)
                }
                _ => None,
            }
        }
    }

    /// WC-D: the mapped base address of this surface. Needed by callers that must run cache maintenance
    /// over a sub-range they computed themselves (the compositor's scan-out verification), which
    /// [`flush_range`](Self::flush_range) cannot express — it only cleans, and the verification needs an
    /// invalidate so its reads come from RAM.
    #[inline]
    pub fn base_addr(&self) -> usize {
        self.base
    }

    /// WC-D: read one panel pixel back as `0x00RRGGBB` — the exact inverse of [`put_pixel`](Self::put_pixel)'s
    /// encoding, so `read_pixel(x, y) == color` for any `color` a `put_pixel(x, y, color)` landed.
    /// `None` when the coordinate is off-panel, past the mapped length, or the layout has no colour
    /// inverse (`U8` is lossy — averaging is not invertible, so it refuses rather than inventing one).
    ///
    /// This exists so the compositor can VERIFY its own blit against the source surface instead of
    /// asserting it (see `wm::verify_window`). A checksum of what we intended to draw proves nothing
    /// about the panel; a read-back of what is actually in the scan-out buffer does.
    /// ⚠ VOLATILE VRAM READ-BACK — the documented ban on reading uncached surfaces (see
    /// [`get_pixel`](Self::get_pixel)) is LIFTED here alone, deliberately, for witness/verify use
    /// only: the verification's whole point is to pay for a real read of the scan-out memory.
    #[inline]
    pub fn read_pixel(&self, x: usize, y: usize) -> Option<u32> {
        if self.base == 0 || x >= self.info.width || y >= self.info.height {
            return None;
        }
        let offset = (y * self.info.stride + x) * self.info.bytes_per_pixel;
        if offset + 3 > self.len {
            return None;
        }
        let p = (self.base + offset) as *const u8;
        // SAFETY: bounds-checked against `self.len` above; volatile so the read is not hoisted or
        // folded with the stores the blit just performed.
        let (a, b, c) = unsafe {
            (
                core::ptr::read_volatile(p),
                core::ptr::read_volatile(p.add(1)),
                core::ptr::read_volatile(p.add(2)),
            )
        };
        match self.info.pixel_format {
            PixelFormat::Rgb => Some(((a as u32) << 16) | ((b as u32) << 8) | c as u32),
            PixelFormat::Bgr => Some(((c as u32) << 16) | ((b as u32) << 8) | a as u32),
            _ => None,
        }
    }

    /// VPERF M4: encode `color` as the little-endian 4-byte pixel this surface stores, when the
    /// layout is a full 4-byte pixel (bpp 4, Rgb/Bgr). Lets callers hoist the per-pixel format
    /// decode out of their inner loops (`put_raw4` / the `fill_rows` fast path / COMPOSITE-2's
    /// span writer). The X byte is encoded 0: `put_pixel` leaves it untouched, but every store
    /// this kernel allocates starts zeroed and scan-out ignores it, so the visible bytes agree.
    /// (x86-only until COMPOSITE-2; the aarch64 compositor's bulk paths now hoist through it too.)
    #[inline]
    pub fn encode4(&self, color: u32) -> Option<u32> {
        if self.info.bytes_per_pixel != 4 {
            return None;
        }
        let r = (color >> 16) & 0xFF;
        let g = (color >> 8) & 0xFF;
        let b = color & 0xFF;
        match self.info.pixel_format {
            PixelFormat::Rgb => Some(r | (g << 8) | (b << 16)),
            PixelFormat::Bgr => Some(b | (g << 8) | (r << 16)),
            _ => None,
        }
    }

    /// VPERF M4 (x86 only): write one pre-encoded 4-byte pixel (from [`encode4`](Self::encode4)
    /// of this surface). Same bounds checks as `put_pixel`, no per-pixel format match. Unaligned
    /// write: a heap `Vec<u8>` shadow store only guarantees byte alignment (compiles to a plain
    /// `mov` on x86 either way).
    #[cfg(target_arch = "x86_64")]
    #[inline]
    pub fn put_raw4(&self, x: usize, y: usize, raw: u32) {
        #[cfg(feature = "videobench")]
        crate::video::vperf::on_put_pixel();
        if self.base == 0 || x >= self.info.width || y >= self.info.height {
            return;
        }
        let offset = (y * self.info.stride + x) * 4;
        if offset + 4 > self.len {
            return;
        }
        unsafe { core::ptr::write_unaligned((self.base + offset) as *mut u32, raw) };
    }

    /// COMPOSITE-2 — whether the WORD fast paths may run on this surface: a full 4-byte pixel
    /// layout AND a word-aligned base, so every pixel offset `(y * stride + x) * 4` is 4-aligned
    /// (the build is `+strict-align`; a misaligned `str` is not merely slow, it traps). Both real
    /// framebuffers are page-aligned and the staged back layer is heap-allocated (16-aligned), so
    /// this is true everywhere it matters — the check exists so a surface it is NOT true of falls
    /// back to `put_pixel` instead of faulting.
    #[inline]
    pub fn word4(&self) -> bool {
        self.info.bytes_per_pixel == 4 && self.base & 3 == 0
    }

    /// COMPOSITE-2 — fill a horizontal span of `w` pixels at `(x, y)` with one pre-encoded 4-byte
    /// pixel (from [`encode4`](Self::encode4) of this surface). The compositor's inner-loop
    /// primitive: the bounds checks and the format decode happen ONCE per span instead of once per
    /// pixel, and the body is a word-wide fill (64-bit pairs where alignment allows) instead of
    /// three byte stores per pixel. Clips exactly as a `put_pixel` loop over the same span would:
    /// to the visible width and to the mapped length.
    ///
    /// Caller contract: only meaningful when [`word4`](Self::word4) is true (callers gate on it;
    /// the checks here make a violation a no-op, never a fault). Writes the pad byte as 0 where
    /// `put_pixel` leaves it untouched — no reader of these surfaces decodes the pad (scan-out
    /// ignores it, `read_pixel` reads 3 bytes), and the staged back layer's pads are already 0.
    #[inline]
    pub fn fill_span4(&self, x: usize, y: usize, w: usize, raw: u32) {
        if self.base == 0 || self.base & 3 != 0 || x >= self.info.width || y >= self.info.height {
            return;
        }
        let w = w.min(self.info.width - x);
        let off = (y * self.info.stride + x) * 4;
        if off + 4 > self.len {
            return;
        }
        let n = w.min((self.len - off) / 4);
        if n == 0 {
            return;
        }
        unsafe {
            let mut p = (self.base + off) as *mut u32;
            let end = p.add(n);
            // Head: one 32-bit store to reach 8-byte alignment for the pair loop.
            if (p as usize) & 7 != 0 && p < end {
                p.write(raw);
                p = p.add(1);
            }
            let raw2 = ((raw as u64) << 32) | raw as u64;
            let mut q = p as *mut u64;
            while (q as usize) + 8 <= end as usize {
                q.write(raw2);
                q = q.add(1);
            }
            let mut p = q as *mut u32;
            while p < end {
                p.write(raw);
                p = p.add(1);
            }
        }
    }

    /// Fill pixel rows `[y0, y1)` (clamped to the frame) with a colour.
    pub fn fill_rows(&self, y0: usize, y1: usize, color: u32) {
        let y_end = y1.min(self.info.height);
        // VPERF M4 (x86 only): word-wide band fill for full-4-byte-pixel formats — the format
        // decode and bounds checks hoisted to once per row instead of once per pixel. Writes
        // only the visible width (the stride gap stays untouched, like `put_pixel`).
        #[cfg(target_arch = "x86_64")]
        if let Some(raw) = self.encode4(color) {
            if self.base == 0 {
                return;
            }
            let row_bytes = self.info.stride * self.info.bytes_per_pixel;
            let w = self.info.width;
            for y in y0..y_end {
                let off = y * row_bytes;
                // Clamp to the buffer like `put_pixel` does per pixel, so a firmware-short
                // framebuffer (len < stride*height*bpp — the Apple Retina GOP quirk) still gets
                // its partial last row filled instead of dropped.
                let wmax = (self.len.saturating_sub(off) / 4).min(w);
                for x in 0..wmax {
                    unsafe {
                        core::ptr::write_unaligned((self.base + off + x * 4) as *mut u32, raw)
                    };
                }
                if wmax < w {
                    break;
                }
            }
            return;
        }
        // COMPOSITE-2 (aarch64): the same hoist, through the span writer, with the same
        // firmware-short-buffer semantics — a row past the mapped length writes nothing and every
        // partial row writes its prefix.
        #[cfg(target_arch = "aarch64")]
        if self.word4() {
            if let Some(raw) = self.encode4(color) {
                for y in y0..y_end {
                    self.fill_span4(0, y, self.info.width, raw);
                }
                return;
            }
        }
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
    ///
    /// COMPOSITE-2 (aarch64): word-wide row spans when the surface supports them — this is the
    /// compositor's dense chrome/border fill (the whole outer box, every pass) and was the single
    /// largest per-pixel population in the measured ~13 ms blit term. x86 keeps the per-pixel loop
    /// so `videobench`'s poke counters keep their meaning.
    pub fn fill_rect(&self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        #[cfg(target_arch = "aarch64")]
        if self.word4() {
            if let Some(raw) = self.encode4(color) {
                let y1 = y.saturating_add(h).min(self.info.height);
                for row in y..y1 {
                    self.fill_span4(x, row, w, raw);
                }
                return;
            }
        }
        for row in 0..h {
            for col in 0..w {
                self.put_pixel(x + col, y + row, color);
            }
        }
    }

    /// Draw a line from `(x0, y0)` to `(x1, y1)` with the integer Bresenham algorithm. Endpoints
    /// are signed so off-screen segments clip cleanly (each pixel is bounds-checked by `put_pixel`).
    /// The engine primitive the `vug` renderer draws wireframes with.
    pub fn draw_line(&self, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) {
        if self.base == 0 {
            return;
        }
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (x0, y0);
        loop {
            if x >= 0 && y >= 0 {
                self.put_pixel(x as usize, y as usize, color);
            }
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Copy `src` bytes into the framebuffer starting at `byte_offset` (bounds-checked, no-op if
    /// it would overrun). This is the double-buffer flush primitive: a bulk sequential copy onto
    /// the framebuffer, which on real hardware is slow/write-combining — sequential bulk copies
    /// are far friendlier to it than the per-pixel pokes drawing uses on the cached back buffer.
    ///
    /// Does **not** clean the cache: callers that present to a non-coherent scan-out (the Pi 4 HVS)
    /// must call [`flush_range`](Self::flush_range) over the region they wrote. `Screen::flush`
    /// blits row-by-row and then cleans the whole damaged span once, so the (no-op-on-coherent)
    /// `DC CVAC` sweep + `DSB` happen a single time per frame rather than per scanline.
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

    /// Make CPU writes to `[byte_offset, byte_offset+len)` of this framebuffer visible to a
    /// non-coherent display controller (the Pi 4 HVS). Bounds-clamped; delegates to the arch
    /// cache-clean (a no-op on cache-coherent targets).
    pub fn flush_range(&self, byte_offset: usize, len: usize) {
        if self.base == 0 {
            return;
        }
        let end = (byte_offset + len).min(self.len);
        if end <= byte_offset {
            return;
        }
        crate::arch::flush_framebuffer_range(self.base + byte_offset, end - byte_offset);
    }

    /// COMPOSITE-2 — make CPU writes to the pixel RECT `[x, x+w) x [y, y+h)` visible to a
    /// non-coherent display controller, cleaning only the rect's own bytes per row instead of the
    /// full-width scanlines [`flush_range`](Self::flush_range) forces. One `DSB` for the whole
    /// rect (see `arch::flush_framebuffer_rows`), so the cost scales with the bytes the caller
    /// actually wrote: a 514-wide box on a 1920-wide panel cleans 3.7x less than the scanline
    /// sweep did. Clips like the writers it covers: to the visible width/height and to the mapped
    /// length. No-op on cache-coherent targets, exactly as `flush_range` is.
    pub fn flush_rect(&self, x: usize, y: usize, w: usize, h: usize) {
        if self.base == 0 || self.info.bytes_per_pixel == 0 || self.info.stride == 0 || x >= self.info.width {
            return;
        }
        let bpp = self.info.bytes_per_pixel;
        let stride_b = self.info.stride * bpp;
        let w = w.min(self.info.width - x);
        let y1 = y.saturating_add(h).min(self.info.height);
        if w == 0 || y >= y1 {
            return;
        }
        let off = y * stride_b + x * bpp;
        let row_len = w * bpp;
        if off + row_len > self.len {
            return;
        }
        // Rows whose full span fits the mapped length (a firmware-short buffer trims the tail,
        // the same clamp `flush_range` applies at its end).
        let rows = (((self.len - off - row_len) / stride_b) + 1).min(y1 - y);
        // aarch64: the strided clean with ONE trailing `DSB` (see cache::clean_rows). Elsewhere the
        // drain is range-independent (x86's SFENCE), so the rect collapses to the range call.
        #[cfg(target_arch = "aarch64")]
        crate::arch::aarch64::cache::clean_rows(self.base + off, row_len, rows, stride_b);
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = rows;
            crate::arch::flush_framebuffer_range(self.base + off, row_len);
        }
    }

    /// Flush the whole framebuffer to RAM. Used by the boot console (`fbcon`), which pokes scattered
    /// glyph pixels rather than going through `blit`, so a single whole-surface clean after each
    /// update is the simplest way to keep the on-screen boot log coherent on the Pi.
    pub fn flush_all(&self) {
        self.flush_range(0, self.len);
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
        // VPERF (bench builds only): count the memmove payload, attributing the source read to
        // VRAM when this surface IS the real framebuffer (the uncached-PCIe read being measured).
        #[cfg(all(target_arch = "x86_64", feature = "videobench"))]
        crate::video::vperf::on_scroll((total - shift) as u64, self.base);
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
