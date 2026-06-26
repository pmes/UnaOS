// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Framebuffer console — a log sink for hardware bring-up.
//
// Every diagnostic in this kernel goes through `serial_println!`, which writes to the 16550
// UART at I/O port 0x3F8 (x86) / the PL011 (aarch64). Real laptops (the 2012 MacBook Retina,
// the Zenbook S16) have NO serial port there, so on metal that output — including panics and
// the double-fault handler — vanishes and you debug blind. This module mirrors that same
// output onto the UEFI framebuffer: a tiny scrolling text terminal (8x8 font) drawn straight
// to the GOP/ramfb buffer, so boot diagnostics and panics are visible on screen.
//
// It owns the framebuffer by physical address (stored as `usize`, so the state is `Send`) and
// renders with the same font8x8 glyphs the GUI console uses. It is initialised as early as
// possible in `kernel_main` (before the heap, before the GUI), so it captures the whole boot.
// On a successful boot the GUI later repaints over it; if anything wedges or panics first, the
// log (or a red panic screen) stays up.

use spin::Mutex;
use unaos_boot_info::{FrameBufferInfo, PixelFormat};

const CELL_W: usize = 8;
const CELL_H: usize = 8;

const FG_DEFAULT: u32 = 0x00C0_C0C0; // light grey text
const BG_DEFAULT: u32 = 0x0000_0000; // black background
const PANIC_BG: u32 = 0x0030_0000; // dark red

struct FbCon {
    fb_addr: usize,
    fb_len: usize,
    info: Option<FrameBufferInfo>,
    cols: usize,
    rows: usize,
    col: usize,
    row: usize,
    fg: u32,
    bg: u32,
    ready: bool,
}

// The only non-Send field would be a raw pointer; we keep the framebuffer as a `usize` address
// precisely so the whole struct is `Send` and can live in a `static Mutex`.
unsafe impl Send for FbCon {}

impl FbCon {
    const fn new() -> Self {
        FbCon {
            fb_addr: 0,
            fb_len: 0,
            info: None,
            cols: 0,
            rows: 0,
            col: 0,
            row: 0,
            fg: FG_DEFAULT,
            bg: BG_DEFAULT,
            ready: false,
        }
    }

    #[inline]
    fn put_pixel(&self, x: usize, y: usize, color: u32) {
        let Some(info) = self.info else { return };
        // Reject out-of-frame coordinates explicitly: the `fb_len` check below stops memory
        // unsafety, but without this an x >= width would wrap onto the next scanline.
        if x >= info.width || y >= info.height {
            return;
        }
        let offset = (y * info.stride + x) * info.bytes_per_pixel;
        if offset + info.bytes_per_pixel > self.fb_len {
            return;
        }
        let r = ((color >> 16) & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = (color & 0xFF) as u8;
        unsafe {
            let p = (self.fb_addr + offset) as *mut u8;
            match info.pixel_format {
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

    /// Fill pixel rows [y0, y1) with a colour.
    fn fill_rows(&self, y0: usize, y1: usize, color: u32) {
        let Some(info) = self.info else { return };
        for y in y0..y1.min(info.height) {
            for x in 0..info.width {
                self.put_pixel(x, y, color);
            }
        }
    }

    /// Draw a glyph at pixel (cx, cy). Cells are background-clean when first reached (initial
    /// fill / post-scroll clear), so we only paint the foreground pixels — no per-cell clear.
    fn glyph(&self, ch: u8, cx: usize, cy: usize) {
        let bitmap = font8x8::legacy::BASIC_LEGACY[ch as usize];
        for (ry, byte) in bitmap.iter().enumerate() {
            for rx in 0..8 {
                if byte & (1 << rx) != 0 {
                    self.put_pixel(cx + rx, cy + ry, self.fg);
                }
            }
        }
    }

    /// Scroll the whole framebuffer up by one text row and clear the freed bottom row.
    fn scroll(&mut self) {
        let Some(info) = self.info else { return };
        let row_bytes = info.stride * info.bytes_per_pixel;
        let shift = CELL_H * row_bytes;
        let total = (info.height * row_bytes).min(self.fb_len);
        if shift >= total {
            return;
        }
        unsafe {
            core::ptr::copy(
                (self.fb_addr + shift) as *const u8,
                self.fb_addr as *mut u8,
                total - shift,
            );
        }
        let last_y = self.rows.saturating_sub(1) * CELL_H;
        self.fill_rows(last_y, info.height, self.bg);
    }

    fn newline(&mut self) {
        self.col = 0;
        self.row += 1;
        if self.row >= self.rows {
            self.scroll();
            self.row = self.rows.saturating_sub(1);
        }
    }

    fn write_byte(&mut self, b: u8) {
        match b {
            b'\n' => self.newline(),
            b'\r' => self.col = 0,
            0x20..=0x7E => {
                if self.col >= self.cols {
                    self.newline();
                }
                self.glyph(b, self.col * CELL_W, self.row * CELL_H);
                self.col += 1;
            }
            _ => {} // ignore other control bytes
        }
    }
}

static FBCON: Mutex<FbCon> = Mutex::new(FbCon::new());

/// Bring the framebuffer console online. Call as early as possible in `kernel_main` (the
/// framebuffer details come straight from `BootInfo`). No-op when there is no framebuffer.
pub fn init(fb_addr: u64, fb_len: usize, info: FrameBufferInfo) {
    if fb_addr == 0 || fb_len == 0 || info.width == 0 || info.height == 0 {
        return;
    }
    crate::arch::without_interrupts(|| {
        let mut c = FBCON.lock();
        c.fb_addr = fb_addr as usize;
        c.fb_len = fb_len;
        c.info = Some(info);
        c.cols = info.width / CELL_W;
        c.rows = info.height / CELL_H;
        c.col = 0;
        c.row = 0;
        c.fg = FG_DEFAULT;
        c.bg = BG_DEFAULT;
        c.ready = true;
        c.fill_rows(0, info.height, BG_DEFAULT);
    });
}

/// Mirror formatted output to the framebuffer (called from the serial `_print`). Lock-free of
/// deadlock: interrupts are masked and the lock is `try_lock`ed, so a contended line just skips
/// the screen (serial still has it) rather than spinning.
pub fn _print(args: core::fmt::Arguments) {
    crate::arch::without_interrupts(|| {
        if let Some(mut c) = FBCON.try_lock() {
            if c.ready {
                let _ = core::fmt::Write::write_fmt(&mut Sink { con: &mut c }, args);
            }
        }
    });
}

struct Sink<'a> {
    con: &'a mut FbCon,
}

impl core::fmt::Write for Sink<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            self.con.write_byte(b);
        }
        Ok(())
    }
}

/// Repaint the screen as a panic backdrop (dark red) and home the cursor, so the panic message
/// that follows is unmissable on hardware. Best-effort: `try_lock` to avoid hanging if the lock
/// was held when the panic fired.
pub fn panic_screen() {
    crate::arch::without_interrupts(|| {
        if let Some(mut c) = FBCON.try_lock() {
            if c.ready {
                c.bg = PANIC_BG;
                c.fg = 0x00FF_FFFF;
                c.col = 0;
                c.row = 0;
                let h = c.info.map(|i| i.height).unwrap_or(0);
                c.fill_rows(0, h, PANIC_BG);
            }
        }
    });
}
