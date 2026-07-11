// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Framebuffer console — a log sink for hardware bring-up.
//
// Every diagnostic in this kernel goes through `serial_println!`, which writes to the 16550
// UART at I/O port 0x3F8 (x86) / the PL011 (aarch64). Real laptops (the 2012 MacBook Retina,
// the Zenbook S16) and the Pi 4 over HDMI have NO serial port there, so on metal that output —
// including panics and the double-fault handler — vanishes and you debug blind. This module
// mirrors that same output onto the UEFI/mailbox framebuffer: a tiny scrolling text terminal
// (8x8 font) drawn through the shared `FrameBuffer` surface, so boot diagnostics and panics are
// visible on screen.
//
// It owns its own `FrameBuffer` handle (addressed by physical address, so the state is `Send`)
// and renders with the same font8x8 glyphs the GUI console uses. It is initialised as early as
// possible in `kernel_main` (before the heap, before the GUI), so it captures the whole boot.
// On a successful boot the GUI later repaints over it; if anything wedges or panics first, the
// log (or a red panic screen) stays up.

use crate::video::FrameBuffer;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use unaos_boot_info::FrameBufferInfo;

/// Once the GUI takes ownership of the screen (the double-buffered `Screen`), fbcon must stop
/// mirroring serial output onto the framebuffer — otherwise post-boot diagnostics (e.g. the
/// xHCI enumeration the main loop drives) would scribble over the GUI, and the damage-tracked
/// flush would not repaint over them. Set by `detach()`; cleared by `panic_screen()` so a panic
/// is still drawn on hardware with no serial port. Serial output itself is unaffected.
static GUI_ACTIVE: AtomicBool = AtomicBool::new(false);

const CELL_W: usize = 8;
const CELL_H: usize = 8;

const FG_DEFAULT: u32 = 0x00C0_C0C0; // light grey text
const BG_DEFAULT: u32 = 0x0000_0000; // black background
const PANIC_BG: u32 = 0x0030_0000; // dark red

struct FbCon {
    fb: FrameBuffer,
    cols: usize,
    rows: usize,
    col: usize,
    row: usize,
    fg: u32,
    bg: u32,
    ready: bool,
    /// Accumulated dirty pixel-row band `[dirty_y0, dirty_y1)` since the last flush; empty when
    /// `dirty_y0 >= dirty_y1`. Lets a print clean only the rows it drew, instead of sweeping the
    /// whole surface each line (a real cost on the Pi's cacheable framebuffer; a no-op elsewhere).
    dirty_y0: usize,
    dirty_y1: usize,
}

impl FbCon {
    const fn new() -> Self {
        FbCon {
            fb: FrameBuffer::new(),
            cols: 0,
            rows: 0,
            col: 0,
            row: 0,
            fg: FG_DEFAULT,
            bg: BG_DEFAULT,
            ready: false,
            dirty_y0: 0,
            dirty_y1: 0,
        }
    }

    /// Grow the dirty band to include pixel rows `[y0, y1)`.
    fn mark_rows(&mut self, y0: usize, y1: usize) {
        if self.dirty_y0 >= self.dirty_y1 {
            self.dirty_y0 = y0;
            self.dirty_y1 = y1;
        } else {
            self.dirty_y0 = self.dirty_y0.min(y0);
            self.dirty_y1 = self.dirty_y1.max(y1);
        }
    }

    /// Clean just the dirtied rows out to RAM (for the HVS), then reset the band. No-op when
    /// nothing was drawn or on cache-coherent targets.
    fn flush_dirty(&mut self) {
        if self.dirty_y0 >= self.dirty_y1 {
            return;
        }
        let info = self.fb.info();
        let row_bytes = info.stride * info.bytes_per_pixel;
        let y1 = self.dirty_y1.min(info.height);
        if y1 > self.dirty_y0 {
            self.fb.flush_range(self.dirty_y0 * row_bytes, (y1 - self.dirty_y0) * row_bytes);
        }
        self.dirty_y0 = 0;
        self.dirty_y1 = 0;
    }

    /// Draw a glyph at pixel (cx, cy). Cells are background-clean when first reached (initial
    /// fill / post-scroll clear), so we only paint the foreground pixels — no per-cell clear.
    fn glyph(&self, ch: u8, cx: usize, cy: usize) {
        let bitmap = font8x8::legacy::BASIC_LEGACY[ch as usize];
        for (ry, byte) in bitmap.iter().enumerate() {
            for rx in 0..8 {
                if byte & (1 << rx) != 0 {
                    self.fb.put_pixel(cx + rx, cy + ry, self.fg);
                }
            }
        }
    }

    /// Scroll the whole framebuffer up by one text row and clear the freed bottom row.
    fn scroll(&mut self) {
        self.fb.scroll_up(CELL_H, self.bg);
        // A scroll moves the entire surface — the whole visible frame is now dirty.
        self.mark_rows(0, self.fb.info().height);
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
                let cy = self.row * CELL_H;
                self.glyph(b, self.col * CELL_W, cy);
                self.mark_rows(cy, cy + CELL_H);
                self.col += 1;
            }
            _ => {} // ignore other control bytes
        }
    }
}

// `FbCon` is `Send` because its only state with a raw address is the `FrameBuffer`, which is
// itself `Send` (address held as `usize`).
unsafe impl Send for FbCon {}

static FBCON: Mutex<FbCon> = Mutex::new(FbCon::new());

/// Bring the framebuffer console online. Call as early as possible in `kernel_main` (the
/// framebuffer details come straight from `BootInfo`). No-op when there is no framebuffer.
pub fn init(fb_addr: u64, fb_len: usize, info: FrameBufferInfo) {
    if fb_addr == 0 || fb_len == 0 || info.width == 0 || info.height == 0 {
        return;
    }
    // VPERF (bench builds only): register the real framebuffer range, so scroll instrumentation
    // can attribute source reads to VRAM (heap shadows/back buffers never match this range).
    #[cfg(all(target_arch = "x86_64", feature = "videobench"))]
    crate::video::vperf::set_vram_range(fb_addr as usize, fb_len);
    crate::arch::without_interrupts(|| {
        let mut c = FBCON.lock();
        c.fb.init(fb_addr as usize, fb_len, info);
        c.cols = info.width / CELL_W;
        c.rows = info.height / CELL_H;
        c.col = 0;
        c.row = 0;
        c.fg = FG_DEFAULT;
        c.bg = BG_DEFAULT;
        c.ready = true;
        c.fb.fill_screen(BG_DEFAULT);
        c.fb.flush_all();
    });
}

/// Mirror formatted output to the framebuffer (called from the serial `_print`). Lock-free of
/// deadlock: interrupts are masked and the lock is `try_lock`ed, so a contended line just skips
/// the screen (serial still has it) rather than spinning.
pub fn _print(args: core::fmt::Arguments) {
    // Once the GUI owns the screen, don't mirror to the framebuffer (serial still gets it).
    if GUI_ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    crate::arch::without_interrupts(|| {
        if let Some(mut c) = FBCON.try_lock() {
            if c.ready {
                let _ = core::fmt::Write::write_fmt(&mut Sink { con: &mut c }, args);
                // Clean just the freshly-drawn rows out to RAM (no-op off the Pi). fbcon pokes
                // pixels directly rather than via `blit`, so this is what keeps the boot log visible
                // on the HVS-scanned framebuffer — but only the touched rows, not the whole surface.
                c.flush_dirty();
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

/// Hand the framebuffer to the GUI: stop mirroring serial output onto it. Call once the GUI has
/// painted its first frame. Serial output is unaffected; panics re-enable the mirror.
pub fn detach() {
    GUI_ACTIVE.store(true, Ordering::Relaxed);
}

/// Clear the framebuffer console to the default background and home the cursor. Used to give the
/// USB-debug boot mode a clean screen so the (post-boot) hot-plug enumeration and live input aren't
/// buried under the boot spam on serial-less hardware. No-op if the console isn't ready.
pub fn clear() {
    crate::arch::without_interrupts(|| {
        if let Some(mut c) = FBCON.try_lock() {
            if c.ready {
                c.col = 0;
                c.row = 0;
                c.fb.fill_screen(BG_DEFAULT);
            }
        }
    });
}

/// Repaint the screen as a panic backdrop (dark red) and home the cursor, so the panic message
/// that follows is unmissable on hardware. Best-effort: `try_lock` to avoid hanging if the lock
/// was held when the panic fired. Re-enables the serial mirror first (the GUI may have detached
/// it) so the panic text that `serial_println!` emits next lands on this red backdrop.
pub fn panic_screen() {
    GUI_ACTIVE.store(false, Ordering::Relaxed);
    crate::arch::without_interrupts(|| {
        if let Some(mut c) = FBCON.try_lock() {
            if c.ready {
                c.bg = PANIC_BG;
                c.fg = 0x00FF_FFFF;
                c.col = 0;
                c.row = 0;
                c.fb.fill_screen(PANIC_BG);
                c.fb.flush_all();
            }
        }
    });
}
