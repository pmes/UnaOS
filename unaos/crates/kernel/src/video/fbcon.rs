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
#[cfg(target_arch = "x86_64")]
use alloc::vec::Vec;
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
    /// VPERF M2 (videocap bench lever): `fb` above is HEIGHT-CAPPED (its `info.height` is
    /// shrunk, so `scroll_up` — which sizes its memmove from that height — moves proportionally
    /// fewer bytes). This uncapped twin exists so full-surface paints (init fill, `clear`, the
    /// panic backdrop) still cover the whole real panel.
    #[cfg(all(target_arch = "x86_64", feature = "videocap"))]
    fb_full: FrameBuffer,
    /// VPERF M3 (x86 only): the cached-RAM shadow. On the 2012 rMBP every CPU *read* of the
    /// GOP framebuffer is an uncached PCIe round trip, and `scroll_up`'s memmove READS the whole
    /// surface — ~28 MiB read + ~28 MiB written per 8-px text line. Once attached (late, at the
    /// post-heap usbdebug seam — fbcon initialises pre-heap by design, so init-time allocation is
    /// impossible), all text drawing and scrolling go to this heap store and VRAM only ever
    /// receives whole rows as sequential write-only blits (`flush_dirty`). `None` = direct-VRAM
    /// (boot-early, GUI builds — which never attach; the `Screen` back buffer owns the heap
    /// budget — and the panic path, which forcibly detaches).
    #[cfg(target_arch = "x86_64")]
    shadow_store: Option<Vec<u8>>,
    /// A surface handle pointing into `shadow_store` (same geometry as `fb`, including a
    /// videocap'd height). SAFETY/INVARIANT: the store is allocated once at its final size in
    /// `attach_shadow` and never grown/shrunk, so its heap buffer never moves (the `Screen`
    /// back-store idiom).
    #[cfg(target_arch = "x86_64")]
    shadow: FrameBuffer,
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
            #[cfg(all(target_arch = "x86_64", feature = "videocap"))]
            fb_full: FrameBuffer::new(),
            #[cfg(target_arch = "x86_64")]
            shadow_store: None,
            #[cfg(target_arch = "x86_64")]
            shadow: FrameBuffer::new(),
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

    /// The surface for FULL-SURFACE paints (init fill / `clear` / panic backdrop): the uncapped
    /// handle under the videocap bench lever, `fb` itself otherwise. Text drawing and scrolling
    /// always go through the (possibly capped) `fb`.
    #[cfg(all(target_arch = "x86_64", feature = "videocap"))]
    #[inline]
    fn full_fb(&self) -> &FrameBuffer {
        &self.fb_full
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "videocap")))]
    #[inline]
    fn full_fb(&self) -> &FrameBuffer {
        &self.fb
    }

    /// The surface text drawing and scrolling target: the cached-RAM shadow once attached
    /// (VRAM then only ever sees write-only blits), the framebuffer itself before/without it.
    #[cfg(target_arch = "x86_64")]
    #[inline]
    fn draw_fb(&self) -> &FrameBuffer {
        if self.shadow_store.is_some() { &self.shadow } else { &self.fb }
    }
    #[cfg(not(target_arch = "x86_64"))]
    #[inline]
    fn draw_fb(&self) -> &FrameBuffer {
        &self.fb
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
            // VPERF M3: with the shadow attached, present the dirty band to VRAM as ONE
            // contiguous write-only blit (full-width rows, so [y0,y1) is a single byte range).
            // On a scroll the band is the whole viewport — a sequential ~viewport-sized write,
            // and zero VRAM reads (the memmove ran in cached RAM).
            #[cfg(target_arch = "x86_64")]
            if let Some(store) = &self.shadow_store {
                let off = self.dirty_y0 * row_bytes;
                let end = (y1 * row_bytes).min(store.len());
                if end > off {
                    self.fb.blit(off, &store[off..end]);
                }
            }
            self.fb.flush_range(self.dirty_y0 * row_bytes, (y1 - self.dirty_y0) * row_bytes);
        }
        self.dirty_y0 = 0;
        self.dirty_y1 = 0;
    }

    /// Draw a glyph at pixel (cx, cy). Cells are background-clean when first reached (initial
    /// fill / post-scroll clear), so we only paint the foreground pixels — no per-cell clear.
    fn glyph(&self, ch: u8, cx: usize, cy: usize) {
        let surf = self.draw_fb();
        let bitmap = font8x8::legacy::BASIC_LEGACY[ch as usize];
        // VPERF M4 (x86): hoist the per-pixel format decode out of the 8x8 bit loop — encode the
        // foreground once, poke pre-encoded 4-byte pixels (bounds checks stay per pixel).
        #[cfg(target_arch = "x86_64")]
        if let Some(raw) = surf.encode4(self.fg) {
            for (ry, byte) in bitmap.iter().enumerate() {
                for rx in 0..8 {
                    if byte & (1 << rx) != 0 {
                        surf.put_raw4(cx + rx, cy + ry, raw);
                    }
                }
            }
            return;
        }
        for (ry, byte) in bitmap.iter().enumerate() {
            for rx in 0..8 {
                if byte & (1 << rx) != 0 {
                    surf.put_pixel(cx + rx, cy + ry, self.fg);
                }
            }
        }
    }

    /// Scroll the whole (draw-surface) frame up by one text row and clear the freed bottom row.
    /// With the M3 shadow attached the memmove runs in cached RAM; the following `flush_dirty`
    /// presents the viewport to VRAM write-only. Without it, this is the direct-VRAM memmove.
    fn scroll(&mut self) {
        self.draw_fb().scroll_up(CELL_H, self.bg);
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
        // VPERF M2 (videocap bench lever): cap the fbcon-PRIVATE handle's height. Capping the
        // `rows` field alone would be a no-op — `scroll_up` sizes its memmove from
        // `info.height` — so the cap must live in the handle's own info. The uncapped twin
        // (`fb_full`) keeps full-surface paints covering the whole real panel.
        #[cfg(all(target_arch = "x86_64", feature = "videocap"))]
        {
            c.fb_full.init(fb_addr as usize, fb_len, info);
            let mut capped = info;
            capped.height = ((info.height / 2) / CELL_H).max(1) * CELL_H;
            c.fb.init(fb_addr as usize, fb_len, capped);
            c.cols = capped.width / CELL_W;
            c.rows = capped.height / CELL_H;
        }
        #[cfg(not(all(target_arch = "x86_64", feature = "videocap")))]
        {
            c.fb.init(fb_addr as usize, fb_len, info);
            c.cols = info.width / CELL_W;
            c.rows = info.height / CELL_H;
        }
        c.col = 0;
        c.row = 0;
        c.fg = FG_DEFAULT;
        c.bg = BG_DEFAULT;
        c.ready = true;
        c.full_fb().fill_screen(BG_DEFAULT);
        c.full_fb().flush_all();
    });

    // VPERF-WC (x86, ALL builds — GUI blits benefit too): mark the framebuffer mapping
    // Write-Combining now that its range is known. The M3 shadow already made VRAM traffic
    // write-only sequential blits; WC lets the CPU coalesce those posted writes (~10x on the write
    // path, metal). Memory-TYPE only — no page-permission change (seat-signed). Called OUTSIDE the
    // FBCON lock (the retype's confirmation line mirrors back through fbcon), after the range is set.
    #[cfg(target_arch = "x86_64")]
    crate::arch::memory::set_framebuffer_wc(fb_addr, fb_len as u64);
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
                // VPERF M3: with the shadow attached, clear the WHOLE store in cached RAM (a
                // full-geometry handle over the same bytes — the capped `shadow` handle would
                // leave a videocap'd below-cap band unfilled) and present it as one write-only
                // blit. VRAM is never read either way.
                #[cfg(target_arch = "x86_64")]
                {
                    let full_info = c.full_fb().info();
                    let fb = *c.full_fb();
                    if let Some(store) = &mut c.shadow_store {
                        let mut whole = FrameBuffer::new();
                        whole.init(store.as_mut_ptr() as usize, store.len(), full_info);
                        whole.fill_screen(BG_DEFAULT);
                        fb.blit(0, store);
                        return;
                    }
                }
                // Whole real surface, even when the videocap lever caps the text viewport.
                c.full_fb().fill_screen(BG_DEFAULT);
            }
        }
    });
}

/// VPERF M3 — attach the cached-RAM shadow (x86 only; LATE-ATTACH by design). fbcon initialises
/// pre-heap, so this runs at the post-heap seam instead (the usbdebug path calls it right before
/// its `clear()`). From here on, text drawing and scrolling happen in cached RAM and the real
/// framebuffer only ever receives sequential write-only blits — eliminating the uncached VRAM
/// *reads* that made `scroll_up` nightmarishly slow on the rMBP's PCIe-scanned surface.
///
/// The shadow is NEVER seeded from VRAM (reading the surface back is the exact cost being
/// removed): the store starts blank, the cursor is homed, and the panel is repainted from the
/// blank store, so screen and shadow are coherent from the first byte. GUI builds never call
/// this (they `detach()` fbcon; the `Screen` back buffer owns the heap budget) and a
/// belt-and-braces GUI_ACTIVE check keeps it a no-op even if one did. Idempotent.
#[cfg(target_arch = "x86_64")]
pub fn attach_shadow() {
    let mut attached = false;
    crate::arch::without_interrupts(|| {
        if let Some(mut c) = FBCON.try_lock() {
            if !c.ready || c.shadow_store.is_some() || GUI_ACTIVE.load(Ordering::Relaxed) {
                return;
            }
            let len = c.fb.len();
            if len == 0 {
                return;
            }
            let mut store: Vec<u8> = alloc::vec![0u8; len];
            let mut sh = FrameBuffer::new();
            // INVARIANT: the store is at its final size and never grows, so this captured heap
            // address stays valid for the shadow's lifetime (the Screen back-store idiom).
            sh.init(store.as_mut_ptr() as usize, len, c.fb.info());
            c.shadow = sh;
            c.shadow_store = Some(store);
            c.col = 0;
            c.row = 0;
            // Present the blank store once (write-only), so the panel matches the shadow.
            let fb = c.fb;
            if let Some(store) = &c.shadow_store {
                fb.blit(0, store);
            }
            attached = true;
        }
    });
    if attached {
        serial_println!(":: fbcon: cached-RAM shadow attached — VRAM is now write-only ::");
    }
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
                // VPERF M3: force DIRECT-VRAM for the whole panic path. A panic mid-blit (or
                // mid-allocation) must still paint the red screen and its text, so the shadow is
                // dropped WITHOUT freeing (`mem::forget`) — touching the allocator from a panic
                // context that may already hold its lock would deadlock a dying machine.
                #[cfg(target_arch = "x86_64")]
                if let Some(s) = c.shadow_store.take() {
                    core::mem::forget(s);
                }
                c.bg = PANIC_BG;
                c.fg = 0x00FF_FFFF;
                c.col = 0;
                c.row = 0;
                // The panic backdrop covers the FULL panel even when the videocap lever caps the
                // text viewport (the message itself renders within the capped viewport).
                c.full_fb().fill_screen(PANIC_BG);
                c.full_fb().flush_all();
            }
        }
    });
}
