// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Framebuffer console — a log sink for hardware bring-up.
//
// Every diagnostic in this kernel goes through `serial_println!`, which writes to the 16550
// UART at I/O port 0x3F8 (x86) / the PL011 (aarch64). Real laptops (the 2012 MacBook Retina,
// the Zenbook S16) and the Pi 4 over HDMI have NO serial port there, so on metal that output —
// including panics and the double-fault handler — vanishes and you debug blind. This module
// can mirror that output onto the UEFI/mailbox framebuffer: a tiny wrap-around text terminal
// (8x8 font, never scrolls, never reads the surface back) drawn through the shared `FrameBuffer`
// surface. QUIET-PANEL policy (x86): headless/test builds paint nothing (serial is the witness),
// GUI boots paint only the boot-milestone lines until the handoff, the `bootlog` build keeps the
// full mirror, and panics force the mirror back on. aarch64 keeps the full mirror.
//
// It owns its own `FrameBuffer` handle (addressed by physical address, so the state is `Send`)
// and renders with the same font8x8 glyphs the GUI console uses. It is initialised as early as
// possible in `kernel_main` (before the heap, before the GUI), so it captures the whole boot.
// On a successful boot the GUI later repaints over it; if anything wedges or panics first, the
// log (or a red panic screen) stays up.

use crate::video::FrameBuffer;
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
use crate::video::wm;
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

/// QUIET-PANEL (x86): the full serial stream no longer mirrors onto the panel by default —
/// headless/test builds (usbdebug, the witness batteries) paint nothing (serial carries the
/// evidence) and GUI boots paint only the boot-milestone lines (`milestone`, fed by
/// `bootlog::record`) until the handoff. This flag is the panic override: `panic_screen` sets it
/// so the panic text that `serial_println!` emits next still lands on the red backdrop on
/// serial-less metal, whatever the build.
#[cfg(target_arch = "x86_64")]
static PANIC_MIRROR: AtomicBool = AtomicBool::new(false);

/// PANEL-CONSOLE (x86, kepler-takeover lane only): the QUIET-PANEL override. When set, `_print`
/// mirrors onto the panel again on builds that otherwise paint nothing (the metal `usbdebug` media
/// is exactly that build — see `_print`), so kernel console text lands on glass after the Kepler
/// takeover's calibration draw has finished with the surface. Set ONLY by `panel_console_resume`,
/// which is called from one seam at the end of `kepler_display::takeover_display`; every other boot
/// path leaves it false and is byte-for-byte unchanged in behaviour.
#[cfg(target_arch = "x86_64")]
static PANEL_CONSOLE: AtomicBool = AtomicBool::new(false);

/// CONSOLE-WINDOW (x86, `wc`) — the compositor window the console paints into, or [`wm::WIN_NONE`]
/// when the console owns panel rows directly (every build without `wc`, and every moment before
/// [`panel_console_window_open`] and after [`panic_screen`]).
///
/// Kept OUTSIDE the `FBCON` mutex on purpose. The damage declaration that follows a paint
/// ([`route_present`]) runs with the lock released and interrupts enabled — a composite must never
/// execute under the console lock — so the id it needs has to be readable without that lock.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static CONSOLE_WIN: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(wm::WIN_NONE);

/// CONSOLE-WINDOW re-entry guard. `wm::composite` prints on witness builds, and a print on a routed
/// console would ask for another composite from inside the one already running. The guard makes the
/// inner request a no-op; the glyphs it painted are carried by the NEXT line's present, because a
/// present repaints the window's whole content rather than a sub-rectangle.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static ROUTE_BUSY: AtomicBool = AtomicBool::new(false);

/// CONSOLE-WINDOW — has the first routed paint been announced yet? One-shot witness latch.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static ROUTE_ANNOUNCED: AtomicBool = AtomicBool::new(false);

/// Base font cell (font8x8). The live cell is `FbCon::cell_w/cell_h` = this times the scale.
const CELL_W: usize = 8;
const CELL_H: usize = 8;

/// PANEL-CONSOLE glyph scale. The 2012 rMBP panel is 2880 px across ~286 mm (~0.1 mm/px), so an
/// unscaled 8x8 cell is ~0.8 mm — metal (sitting #30) confirmed marks that small are simply not
/// visible to the naked eye at bench distance. 4x gives a 32x32 px cell (~3.2 mm) with a 0.4 mm
/// stroke — 90 columns x 56 rows (metal s34 verdict on 6x: "great, slightly large").
#[cfg(target_arch = "x86_64")]
const PANEL_SCALE: usize = 4;

const FG_DEFAULT: u32 = 0x00C0_C0C0; // light grey text
const BG_DEFAULT: u32 = 0x0000_0000; // black background
const PANIC_BG: u32 = 0x0030_0000; // dark red

/// A unit of PIXEL work, produced by the layout pass and executed by the paint pass. Splitting
/// the two is what lets the expensive half run outside the interrupt-masked region (see `_print`).
/// Coordinates are absolute pixels on the draw surface.
#[derive(Clone, Copy)]
enum Op {
    Glyph { ch: u8, x: usize, y: usize },
    Fill { y0: usize, y1: usize },
}

/// Worst case ops emitted by one byte: a wrap `\n` (two band fills) plus the glyph itself.
const OPS_PER_BYTE: usize = 3;

/// A fixed-capacity op buffer — no allocation (fbcon runs pre-heap and from panic context).
/// Overflow drops ops rather than growing; callers size `N` at `OPS_PER_BYTE * bytes`.
struct OpList<const N: usize> {
    ops: [Op; N],
    n: usize,
}

impl<const N: usize> OpList<N> {
    fn new() -> Self {
        OpList { ops: [Op::Fill { y0: 0, y1: 0 }; N], n: 0 }
    }
    fn push(&mut self, op: Op) {
        if self.n < N {
            self.ops[self.n] = op;
            self.n += 1;
        }
    }
    fn as_slice(&self) -> &[Op] {
        &self.ops[..self.n]
    }
}

/// Paint one glyph's foreground pixels at (cx, cy). Cells are background-clean when first
/// reached (initial fill / post-newline band fill), so no per-cell clear is needed.
fn draw_glyph(surf: &FrameBuffer, ch: u8, cx: usize, cy: usize, fg: u32, s: usize) {
    let bitmap = font8x8::legacy::BASIC_LEGACY[ch as usize];
    // VPERF M4 (x86): hoist the per-pixel format decode out of the 8x8 bit loop — encode the
    // foreground once, poke pre-encoded 4-byte pixels (bounds checks stay per pixel).
    #[cfg(target_arch = "x86_64")]
    if let Some(raw) = surf.encode4(fg) {
        for (ry, byte) in bitmap.iter().enumerate() {
            for rx in 0..8 {
                if byte & (1 << rx) != 0 {
                    // scale == 1 collapses to the original single poke.
                    for dy in 0..s {
                        for dx in 0..s {
                            surf.put_raw4(cx + rx * s + dx, cy + ry * s + dy, raw);
                        }
                    }
                }
            }
        }
        return;
    }
    for (ry, byte) in bitmap.iter().enumerate() {
        for rx in 0..8 {
            if byte & (1 << rx) != 0 {
                for dy in 0..s {
                    for dx in 0..s {
                        surf.put_pixel(cx + rx * s + dx, cy + ry * s + dy, fg);
                    }
                }
            }
        }
    }
}

/// Execute a planned op list against a surface. Touches ONLY pixels — no console state — so it is
/// safe to run without the `FBCON` lock once the layout pass has reserved the cells.
fn paint_ops(surf: &FrameBuffer, ops: &[Op], fg: u32, bg: u32, scale: usize) {
    for op in ops {
        match *op {
            Op::Glyph { ch, x, y } => draw_glyph(surf, ch, x, y, fg, scale),
            Op::Fill { y0, y1 } => surf.fill_rows(y0, y1, bg),
        }
    }
}

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
    /// CONSOLE-WINDOW (x86, `wc`): the console window's ARGB8888 surface, in kernel-owned cached
    /// RAM. `Some` exactly while the console is routed into the compositor; the compositor reads it
    /// as a window source and fbcon is its only writer.
    ///
    /// SAFETY/INVARIANT: allocated once at its final size in [`panel_console_window_open`] and never
    /// grown or shrunk, so the heap buffer never moves and the raw address `wm` holds stays valid —
    /// the same idiom `shadow_store` and the `Screen` back store use.
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    win_store: Option<Vec<u8>>,
    /// A surface handle over `win_store`. Its layout is `Bgr`/4 bytes, which makes `put_pixel` store
    /// the little-endian word `0x00RRGGBB` — precisely the ARGB8888 pixel `wm::draw_window` reads
    /// back. Nothing converts between the two representations; they are the same bytes.
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    win_fb: FrameBuffer,
    cols: usize,
    rows: usize,
    /// Live glyph cell size in pixels. `CELL_W`/`CELL_H` (the raw font8x8 cell) everywhere by
    /// default; PANEL-CONSOLE multiplies both by `PANEL_SCALE` so text is legible on a Retina panel.
    cell_w: usize,
    cell_h: usize,
    /// Integer glyph magnification (1 = raw font8x8). Each set font bit paints a `scale`x`scale`
    /// block, so no font data or new asset is needed.
    scale: usize,
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
            #[cfg(all(target_arch = "x86_64", feature = "wc"))]
            win_store: None,
            #[cfg(all(target_arch = "x86_64", feature = "wc"))]
            win_fb: FrameBuffer::new(),
            cols: 0,
            rows: 0,
            cell_w: CELL_W,
            cell_h: CELL_H,
            scale: 1,
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

    /// The surface text drawing and scrolling target: the console WINDOW's surface while the console
    /// is routed into the compositor, the cached-RAM shadow once attached (VRAM then only ever sees
    /// write-only blits), the framebuffer itself before/without either.
    ///
    /// PANIC PATH LAW. The routed branch is guarded by `PANIC_MIRROR` as well as by the route's own
    /// existence, and the two guards are independent on purpose: [`panic_screen`] tears the route
    /// down (so `win_store` is `None`), and this test is the belt to that braces — a panic that
    /// reaches the console without going through `panic_screen` still paints the PANEL. Panic output
    /// therefore never depends on the compositor running, on a window existing, or on `wm`'s locks
    /// being takeable.
    #[cfg(target_arch = "x86_64")]
    #[inline]
    fn draw_fb(&self) -> &FrameBuffer {
        #[cfg(feature = "wc")]
        if self.win_store.is_some() && !PANIC_MIRROR.load(Ordering::Relaxed) {
            return &self.win_fb;
        }
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
        // CONSOLE-WINDOW: the glyphs went into cached kernel RAM that no scan-out reads, and the
        // panel write belongs to the compositor. There is nothing to clean and nothing to blit here;
        // the damage is declared by `route_present`, outside this lock.
        #[cfg(all(target_arch = "x86_64", feature = "wc"))]
        if self.win_store.is_some() {
            self.dirty_y0 = 0;
            self.dirty_y1 = 0;
            return;
        }
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

    /// Advance to the next line — WRAP-AROUND rendering, never a scroll. At the bottom row the
    /// cursor wraps back to the top and overwrites the old pass line by line; the line AFTER the
    /// cursor is kept blank as the moving marker separating fresh text from the previous pass.
    /// Each newline costs two one-line band fills and the new glyphs — nothing moves, and the
    /// framebuffer is never read back (the WC/uncached readback of the old full-frame scroll was
    /// the rMBP's pixel-by-pixel boot killer).
    ///
    /// LAYOUT/PAINT SPLIT: this only advances the cursor and RESERVES the cells it will consume,
    /// emitting the pixel work as ops. The caller decides when to execute them (see `Op`).
    fn plan_newline<const N: usize>(&mut self, out: &mut OpList<N>) {
        self.col = 0;
        self.row = if self.row + 1 >= self.rows { 0 } else { self.row + 1 };
        let next = if self.row + 1 >= self.rows { 0 } else { self.row + 1 };
        let ch = self.cell_h;
        let y = self.row * ch;
        out.push(Op::Fill { y0: y, y1: y + ch });
        self.mark_rows(y, y + ch);
        if next != self.row {
            let ny = next * ch;
            out.push(Op::Fill { y0: ny, y1: ny + ch });
            self.mark_rows(ny, ny + ch);
        }
    }

    /// Layout half of `write_byte`: mutate the console state (cursor, dirty band) and append the
    /// pixel work for this byte to `out`. Emits at most `OPS_PER_BYTE` ops.
    fn plan_byte<const N: usize>(&mut self, b: u8, out: &mut OpList<N>) {
        match b {
            b'\n' => self.plan_newline(out),
            b'\r' => self.col = 0,
            0x20..=0x7E => {
                if self.col >= self.cols {
                    self.plan_newline(out);
                }
                let cy = self.row * self.cell_h;
                out.push(Op::Glyph { ch: b, x: self.col * self.cell_w, y: cy });
                self.mark_rows(cy, cy + self.cell_h);
                self.col += 1;
            }
            _ => {} // ignore other control bytes
        }
    }

    /// Plan and paint one byte immediately (the classic, in-place path).
    fn write_byte(&mut self, b: u8) {
        let mut ops = OpList::<OPS_PER_BYTE>::new();
        self.plan_byte(b, &mut ops);
        paint_ops(self.draw_fb(), ops.as_slice(), self.fg, self.bg, self.scale);
    }

    /// Take the accumulated dirty band and reset it, for a caller that will flush it itself
    /// (the deferred panel path flushes outside the lock). `(y0, y1)`, empty when `y0 >= y1`.
    #[cfg(target_arch = "x86_64")]
    fn take_dirty(&mut self) -> (usize, usize) {
        let band = (self.dirty_y0, self.dirty_y1);
        self.dirty_y0 = 0;
        self.dirty_y1 = 0;
        band
    }
}

// `FbCon` is `Send` because its only state with a raw address is the `FrameBuffer`, which is
// itself `Send` (address held as `usize`).
unsafe impl Send for FbCon {}

static FBCON: Mutex<FbCon> = Mutex::new(FbCon::new());

/// Returns the active framebuffer base address if initialized, or None otherwise.
pub fn current_base() -> Option<u64> {
    let base = FBCON.lock().fb.base();
    if base != 0 {
        Some(base as u64)
    } else {
        None
    }
}

/// Returns the active framebuffer info if initialized, or None otherwise.
pub fn current_info() -> Option<unaos_boot_info::FrameBufferInfo> {
    let fb = FBCON.lock();
    if fb.fb.base() != 0 {
        Some(fb.fb.info())
    } else {
        None
    }
}

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
    // PANEL-CONSOLE is a property of the takeover seam, not of the console: `init` resets the
    // cell size and scale back to the raw 8x8 font, so leaving the mirror armed across a re-init
    // would keep painting the full stream at scale 1 (invisible on a Retina panel) with nothing
    // having re-armed it. `panel_console_resume` re-arms it, and it alone.
    #[cfg(target_arch = "x86_64")]
    PANEL_CONSOLE.store(false, Ordering::Relaxed);
    // CONSOLE-WINDOW: a re-init re-homes the console on the raw panel at scale 1, so any routing is
    // stale by construction — the window's grid no longer describes this console. Drop the route
    // (the window row, if any, is left to `wm`; nothing here can composite).
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    CONSOLE_WIN.store(wm::WIN_NONE, Ordering::Relaxed);
    crate::arch::without_interrupts(|| {
        let mut c = FBCON.lock();
        #[cfg(all(target_arch = "x86_64", feature = "wc"))]
        {
            c.win_store = None;
            c.win_fb = FrameBuffer::new();
        }
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
        c.cell_w = CELL_W;
        c.cell_h = CELL_H;
        c.scale = 1;
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
    // QUIET-PANEL (x86): no full-stream mirroring. Headless/test builds paint nothing at all;
    // GUI boots paint only milestone lines (see `milestone`). The `bootlog` build keeps the full
    // mirror — holding the whole log on the panel is its entire purpose — and a panic re-enables
    // it so the panic text lands on the red backdrop.
    // PANEL-CONSOLE is the third override (alongside the panic mirror): once the Kepler takeover
    // has handed the GOP surface back, kernel console text is meant to be ON the panel.
    #[cfg(all(target_arch = "x86_64", not(feature = "bootlog")))]
    if !PANIC_MIRROR.load(Ordering::Relaxed) && !PANEL_CONSOLE.load(Ordering::Relaxed) {
        return;
    }
    // PANEL-DEFER (x86): mirror through the split layout/paint path when the draw target is VRAM
    // (no cached-RAM shadow) and we are not on the panic path. See `panel_mirror` for the invariant.
    #[cfg(target_arch = "x86_64")]
    if !PANIC_MIRROR.load(Ordering::Relaxed) {
        let mut sink = PanelSink::new();
        let _ = core::fmt::Write::write_fmt(&mut sink, args);
        if sink.finish() {
            return; // the split path owned this line (or there was nothing to paint)
        }
        // The route was decided against the split path on the FIRST flush — before any pixel was
        // painted — so the line is still untouched and the classic path can render all of it.
    }
    print_masked(args);
}

/// CONSOLE-WINDOW — declare the console window damaged so the compositor presents the glyphs that
/// were just painted into its surface. The one place fbcon asks `wm` for anything on the print path.
///
/// **Call with the `FBCON` lock RELEASED and interrupts ENABLED.** `wm::present` composites, which
/// takes the window table and `WRITER`; running that under the console lock with interrupts masked
/// would put a full panel repaint inside the critical section PANEL-DEFER exists to keep short.
///
/// No-op when the console is not routed — which is every build without `wc`, every boot before the
/// window is opened, and every moment after a panic tore the route down.
/// INSTGUI: while a modal dialog owns the screen the console STOPS PRESENTING to glass —
/// its surface still takes the glyphs and serial still carries every line, but a boot that
/// keeps printing must not repaint a 1 MPx window behind the dialog on every line. (`wm`'s
/// dirty set closes upward over occlusion, so each console line was repainting the dialog
/// too: that is the flicker Peter saw on s43.) Suspension is presentation-only.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static CONSOLE_PRESENT_SUSPENDED: AtomicBool = AtomicBool::new(false);

/// INSTGUI seam: suspend/resume the console window's presents (see the static above).
/// Resuming forces one present so the console shows everything it accumulated.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn console_present_suspend(on: bool) {
    CONSOLE_PRESENT_SUSPENDED.store(on, Ordering::Relaxed);
    if !on {
        route_present();
    }
}

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn route_present() {
    let id = CONSOLE_WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return;
    }
    if CONSOLE_PRESENT_SUSPENDED.load(Ordering::Relaxed) {
        return;
    }
    // Re-entry: see `ROUTE_BUSY`.
    if ROUTE_BUSY
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    let ok = wm::present(id);
    if ok && !ROUTE_ANNOUNCED.swap(true, Ordering::Relaxed) {
        serial_println!("[wc-x] console-route first-paint win={} (glyphs -> window surface)", id);
    }
    ROUTE_BUSY.store(false, Ordering::Release);
}

#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
#[inline]
fn route_present() {}

/// No-op off the wc path (see the x86 definition).
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
#[inline]
pub fn console_present_suspend(_on: bool) {}

/// The classic mirror: format straight into the console with interrupts masked for the whole
/// line. Kept verbatim for the PANIC path and for aarch64.
fn print_masked(args: core::fmt::Arguments) {
    let mut drew = false;
    crate::arch::without_interrupts(|| {
        if let Some(mut c) = FBCON.try_lock() {
            if c.ready {
                let _ = core::fmt::Write::write_fmt(&mut Sink { con: &mut c }, args);
                // Clean just the freshly-drawn rows out to RAM (no-op off the Pi). fbcon pokes
                // pixels directly rather than via `blit`, so this is what keeps the boot log visible
                // on the HVS-scanned framebuffer — but only the touched rows, not the whole surface.
                c.flush_dirty();
                drew = true;
            }
        }
    });
    // CONSOLE-WINDOW: outside the mask and the lock, per `route_present`'s contract. Not reached on
    // the panic path in any meaningful sense — `panic_screen` has already cleared `CONSOLE_WIN`.
    if drew {
        route_present();
    }
}

/// Bytes buffered before one layout/paint round-trip. Bounds the stack cost of the op buffer
/// (`CHUNK * OPS_PER_BYTE` ops), which matters because an IRQ-context printer can nest inside the
/// paint pass — see the invariant below.
#[cfg(target_arch = "x86_64")]
const CHUNK: usize = 16;

/// PANEL-DEFER sink (finding: scale²-amplified VRAM pokes ran with IRQs masked).
///
/// THE MASK INVARIANT. The old `_print` held `without_interrupts` across the whole line, which on
/// a PANEL_CONSOLE boot meant 10–20k uncached VRAM pokes per line with interrupts off — bad for the
/// same boots that drive wall-clock USB pumps. What the mask actually protected was NOT the pixels:
/// the lock is `try_lock`ed, so an IRQ-context printer could never deadlock on it in the first
/// place. The mask bought exactly two things:
///   (a) console STATE (cursor, grid, dirty band) is mutated atomically w.r.t. an IRQ-context
///       printer on this CPU — two printers must not both believe they own the same cell;
///   (b) a line contended by an IRQ printer was deferred rather than dropped from the panel.
/// Only (a) is a correctness property, and it needs the mask for the LAYOUT half only. So layout
/// (cursor advance + cell reservation + dirty-band accumulation) still runs masked and under the
/// lock; the pixel work it plans runs afterwards with interrupts enabled and NO lock held.
/// (b) is preserved too: an IRQ printer that lands mid-paint takes the lock (we are not holding
/// it), reserves its OWN cells, and paints them — so the two paints touch DISJOINT pixels and
/// merely interleave visually. Nothing re-enters the console state unserialised, so no re-entry
/// AtomicBool is needed.
///
/// Two cases stay on the classic fully-masked path deliberately:
///   * the PANIC path (`PANIC_MIRROR`) — a panic must never find the `FBCON` lock held by a
///     preempted paint, so panic-path behaviour is byte-for-byte unchanged;
///   * a cached-RAM shadow being attached — drawing then goes to cached RAM (cheap; the cost this
///     fix targets does not exist) and presenting it needs the store, i.e. the lock, held anyway.
#[cfg(target_arch = "x86_64")]
struct PanelSink {
    buf: [u8; CHUNK],
    n: usize,
    /// `false` until the first flush decides the route; `true` = split path owns this line.
    split: bool,
    /// Route decided yet?
    decided: bool,
}

#[cfg(target_arch = "x86_64")]
impl PanelSink {
    fn new() -> Self {
        PanelSink { buf: [0u8; CHUNK], n: 0, split: false, decided: false }
    }

    fn flush(&mut self) {
        if self.n == 0 {
            return;
        }
        let n = self.n;
        self.n = 0;
        if self.decided && !self.split {
            return; // classic route already handled the whole line
        }
        let bytes = &self.buf[..n];
        // MASKED + LOCKED: layout only. No VRAM writes happen in here.
        let plan = crate::arch::without_interrupts(|| {
            let mut c = match FBCON.try_lock() {
                Some(c) => c,
                None => return None,
            };
            if !c.ready || c.shadow_store.is_some() {
                return None;
            }
            let mut ops = OpList::<{ CHUNK * OPS_PER_BYTE }>::new();
            for &b in bytes {
                c.plan_byte(b, &mut ops);
            }
            // CONSOLE-WINDOW: `draw_fb` has already chosen the window surface if the console is
            // routed; the flag only tells the paint half how the result reaches the panel — a
            // compositor present instead of a cache clean of the panel's own rows.
            #[cfg(feature = "wc")]
            let routed = c.win_store.is_some();
            #[cfg(not(feature = "wc"))]
            let routed = false;
            let band = c.take_dirty();
            Some((*c.draw_fb(), c.fg, c.bg, c.scale, ops, band, routed))
        });
        let Some((surf, fg, bg, scale, ops, band, routed)) = plan else {
            if !self.decided {
                self.decided = true;
                self.split = false;
            }
            return;
        };
        self.decided = true;
        self.split = true;
        // UNMASKED, UNLOCKED: the expensive half.
        paint_ops(&surf, ops.as_slice(), fg, bg, scale);
        if routed {
            // No direct front-buffer write on this path: the surface is cached RAM and `wm` owns
            // the panel. Damage is declared here, still outside the lock and the mask.
            route_present();
            return;
        }
        if band.1 > band.0 {
            let info = surf.info();
            let row_bytes = info.stride * info.bytes_per_pixel;
            let y1 = band.1.min(info.height);
            if y1 > band.0 {
                surf.flush_range(band.0 * row_bytes, (y1 - band.0) * row_bytes);
            }
        }
    }

    /// Returns true when the split path owned this line (nothing more to do).
    fn finish(&mut self) -> bool {
        self.flush();
        self.split || !self.decided
    }
}

#[cfg(target_arch = "x86_64")]
impl core::fmt::Write for PanelSink {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            self.buf[self.n] = b;
            self.n += 1;
            if self.n == CHUNK {
                self.flush();
            }
        }
        Ok(())
    }
}

/// GUI-WITNESS on-panel milestone (QUIET-PANEL companion): paint ONE short line for a recorded
/// boot milestone. On a quiet GUI boot these lines are the ONLY thing the panel shows before the
/// handoff — the bench witness channel the full-log mirror used to provide, at a per-milestone
/// cost instead of a per-serial-line cost. No-op on headless/test builds (usbdebug/witness —
/// nothing paints there), after the GUI handoff, and off x86 (aarch64 keeps its full mirror).
pub fn milestone(ms: u64, tag: &str) {
    #[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "witness"))))]
    {
        if GUI_ACTIVE.load(Ordering::Relaxed) {
            return;
        }
        // SPLASH-SEAMLESS (metal defect fix): once the crystal splash owns the pre-GUI panel,
        // milestone lines go serial/ring-only. They used to text-paint straight over the splash
        // — the "jolting flash of text between splash and midden" Peter saw at the end of B5
        // boot (the last one, gui:handoff, landed in the very frame before the GUI repainted).
        // The ring still records every milestone (the `bootlog` shell verb shows them), serial
        // still carries them, and a panic bypasses this via its own red backdrop + PANIC_MIRROR.
        if crate::splash::active() {
            return;
        }
        crate::arch::without_interrupts(|| {
            if let Some(mut c) = FBCON.try_lock() {
                if c.ready {
                    let _ = core::fmt::Write::write_fmt(
                        &mut Sink { con: &mut c },
                        format_args!(":: [{:>8} ms] {} ::\n", ms, tag),
                    );
                    c.flush_dirty();
                }
            }
        });
        // CONSOLE-WINDOW: same contract as `print_masked` — present outside the lock and the mask.
        route_present();
    }
    #[cfg(not(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "witness")))))]
    let _ = (ms, tag);
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
                // CONSOLE-WINDOW: "clear the console" means clear the CONSOLE, and while the console
                // is a window that is its surface — not the panel, which now belongs to the
                // compositor and holds other windows' pixels. Presented after the lock drops.
                #[cfg(all(target_arch = "x86_64", feature = "wc"))]
                if c.win_store.is_some() {
                    c.win_fb.fill_screen(BG_DEFAULT);
                    return;
                }
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
    // CONSOLE-WINDOW: outside the lock and the mask, per `route_present`'s contract. No-op unless
    // the console is routed.
    route_present();
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
    // QUIET-PANEL: headless/test builds never paint the boot log any more, so don't spend
    // ~28 MiB of heap on a shadow nothing draws to. (And with wrap-around rendering the console
    // never reads the surface anyway — the shadow's original reason has retired with the scroll.)
    if cfg!(all(any(feature = "usbdebug", feature = "witness"), not(feature = "bootlog"))) {
        return;
    }
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

/// PANEL-CONSOLE — put the kernel console back on glass after the Kepler takeover.
///
/// ROOT CAUSE this exists for: fbcon's geometry was never wrong (its `current_info()` already
/// matches the scanned GOP surface — stride 4096 px / 16384 B rows). fbcon simply never DREW on
/// the metal media build. QUIET-PANEL makes `_print` return early on x86 builds without `bootlog`,
/// and `milestone` — the one remaining on-panel leg — is `cfg`'d out entirely when `usbdebug` or
/// `witness` is on. The metal Kepler media is a `usbdebug` build, so after the initial black fill
/// fbcon painted exactly zero pixels for the whole boot. Nothing about the framebuffer, the stride
/// or the takeover was hiding the text; there was no text.
///
/// This call is the seam that turns it back on, once — at the END of
/// `kepler_display::takeover_display`, after its calibration pattern has been drawn, held and
/// photographed. It:
///   1. drops the cached-RAM shadow if one is attached, so drawing goes straight at the surface
///      the panel is scanning (no stale full-surface blit can resurrect the calibration pattern);
///   2. re-scales the glyph cell to `PANEL_SCALE` (8x8 is ~0.8 mm on this panel — invisible, per
///      metal sitting #30) and recomputes the column/row grid;
///   3. clears the panel (removing the calibration pattern) and homes the cursor;
///   4. flips `PANEL_CONSOLE`, so every subsequent `serial_println!` mirrors onto the panel;
///   5. replays the boot-milestone ring so the panel carries the boot's history, not just whatever
///      happens to print next.
///
/// Returns the number of text rows painted by the replay. Gated to the Kepler-takeover call site,
/// so no other boot path changes behaviour.
#[cfg(target_arch = "x86_64")]
pub fn panel_console_resume() -> usize {
    let mut base = 0u64;
    let mut pitch = 0usize;
    let mut cell = (0usize, 0usize);
    let mut grid = (0usize, 0usize);
    GUI_ACTIVE.store(false, Ordering::Relaxed);
    crate::arch::without_interrupts(|| {
        if let Some(mut c) = FBCON.try_lock() {
            if !c.ready {
                return;
            }
            // (1) direct-VRAM from here on.
            c.shadow_store = None;
            // (2) legible cell.
            c.scale = PANEL_SCALE;
            c.cell_w = CELL_W * PANEL_SCALE;
            c.cell_h = CELL_H * PANEL_SCALE;
            let info = c.fb.info();
            c.cols = (info.width / c.cell_w).max(1);
            c.rows = (info.height / c.cell_h).max(1);
            // (3) clear the calibration pattern off the whole real surface, home the cursor.
            c.col = 0;
            c.row = 0;
            c.fg = FG_DEFAULT;
            c.bg = BG_DEFAULT;
            c.full_fb().fill_screen(BG_DEFAULT);
            c.full_fb().flush_all();
            base = c.fb.base() as u64;
            pitch = info.stride * info.bytes_per_pixel;
            cell = (c.cell_w, c.cell_h);
            grid = (c.cols, c.rows);
        }
    });
    if base == 0 {
        serial_println!(":: fbcon: glyphs-active ABORT console-not-ready ::");
        return 0;
    }
    // (4) from this line on, serial output is mirrored onto the panel — this marker is itself the
    // first thing drawn, so seeing it on glass is the proof that glyph rendering reaches the panel.
    PANEL_CONSOLE.store(true, Ordering::Relaxed);
    serial_println!(
        ":: fbcon: glyphs-active base={:08X} pitch={} cell={}x{} cols={} rows={} scale={} ::",
        base, pitch, cell.0, cell.1, grid.0, grid.1, PANEL_SCALE
    );
    // (5) replay the boot-milestone ring.
    let mut buf = [(0u64, ""); crate::bootlog::capacity()];
    let n = crate::bootlog::snapshot(&mut buf);
    for (ms, tag) in &buf[..n] {
        serial_println!(":: [{:>8} ms] {} ::", ms, tag);
    }
    n + 1
}

/// CONSOLE-WINDOW — the compositor's staged-present budget, in PANEL PIXELS, as the console window
/// may spend it.
///
/// `wm::stage_window` composes a window's clipped outer box in cached RAM and presents it as
/// contiguous row copies; a box larger than its private `MAX_STAGE_BYTES` (4 MiB) declines the
/// staged path and falls back to poking the front buffer pixel by pixel. On the rMBP's WC-mapped GOP
/// surface that fallback is the regime the whole cached-RAM discipline exists to avoid, and the
/// console is the one window that presents on every printed line — it is the last window that should
/// be paying for it.
///
/// So the console's size is chosen against this budget rather than against taste. The number is
/// `MAX_STAGE_BYTES / 4` restated in pixels; it is duplicated here because it is `wm`-private and
/// `wm` is not this arc's to change. If `wm` ever raises its cap, this constant follows it — the
/// console simply gets more generous, and nothing breaks in the meantime.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
const WIN_BOX_BUDGET_PX: usize = 4 * 1024 * 1024 / 4;

/// CONSOLE-WINDOW — pick the console window's CONTENT extent for a `pw x ph` panel.
///
/// Generous first, budgeted second, aligned last:
///  1. start at 7/8 of the panel in each axis, less the bottom chrome `ui_status` owns (the tiler's
///     PULSE-2 reservation does not apply to a pinned window, so the reservation is honoured here or
///     not at all);
///  2. shrink both axes together — aspect preserved, 15/16 at a time so no float is needed — until
///     the OUTER box fits [`WIN_BOX_BUDGET_PX`];
///  3. round down to whole glyph cells, so the console's grid tiles its surface exactly and no
///     partial cell column can be left holding stale pixels.
///
/// On the 640x480 gate panel step 2 never fires and the console is the full 7/8. On a 1920x1200
/// bench panel it settles near 1200x760; on the rMBP's 2880x1800 near 1280x800. The window is
/// therefore "as much of the panel as the compositor can present cheaply", which is what the
/// generous default actually means once the present path is priced.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn win_content_extent(pw: usize, ph: usize, cell_w: usize, cell_h: usize) -> (usize, usize) {
    let avail_h = ph.saturating_sub(crate::ui_status::chrome_h(ph)).max(1);
    let mut w = (pw * 7 / 8).max(cell_w);
    let mut h = (avail_h * 7 / 8).max(cell_h);
    // Outer box = content + chrome. Budget the box, not the content: the box is what gets staged.
    let box_px = |w: usize, h: usize| {
        (w + 2 * wm::BORDER).saturating_mul(h + wm::TITLE_H + 2 * wm::BORDER)
    };
    while box_px(w, h) > WIN_BOX_BUDGET_PX && w > cell_w && h > cell_h {
        w = (w * 15 / 16).max(cell_w);
        h = (h * 15 / 16).max(cell_h);
    }
    ((w / cell_w).max(1) * cell_w, (h / cell_h).max(1) * cell_h)
}

/// CONSOLE-WINDOW — turn the panel console into a WINDOW: allocate its surface, open a row in the
/// window table, and route every subsequent glyph into it. Returns the window id, or
/// [`wm::WIN_NONE`] if anything declined. Idempotent — a second call returns the existing id.
///
/// ### What changes, and what does not
///
/// Not one line of the console's *rendering* changes. Glyph drawing, the wrap-around line advance,
/// the layout/paint split and the dirty band are all expressed against `draw_fb()` and the `cols` /
/// `rows` grid; this call re-points the first at the window's surface and re-derives the second from
/// the window's extent. `PANEL_SCALE` is untouched, so a glyph is the same size on glass it was —
/// the window is composited at scale 1 (the surface is far too large for `wm`'s scale rule to
/// magnify), so a surface pixel is a panel pixel and the console's own 4x cell is the whole of the
/// magnification, exactly as before.
///
/// ### Front-buffer discipline
///
/// The console stops writing the panel here. Its pixels go to cached kernel RAM and reach glass only
/// through `wm`'s staged present. That is the point of the routing, and it is why the panic path is
/// arranged the way it is: see [`panic_screen`] and [`FbCon::draw_fb`].
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn panel_console_window_open() -> wm::WinId {
    let existing = CONSOLE_WIN.load(Ordering::Relaxed);
    if existing != wm::WIN_NONE {
        return existing;
    }

    // Geometry, read from the console's own panel handle — the same surface `wm` composites onto.
    let (pw, ph, cell_w, cell_h) = {
        let mut got = None;
        crate::arch::without_interrupts(|| {
            if let Some(c) = FBCON.try_lock() {
                if c.ready && c.cell_w != 0 && c.cell_h != 0 {
                    let i = c.fb.info();
                    got = Some((i.width, i.height, c.cell_w, c.cell_h));
                }
            }
        });
        match got {
            Some(g) => g,
            None => {
                serial_println!("[wc-x] console-window DECLINE reason=console-not-ready");
                return wm::WIN_NONE;
            }
        }
    };
    let (cw, ch) = win_content_extent(pw, ph, cell_w, cell_h);
    let stride = cw * 4;
    let len = ch * stride;

    // Allocated OUTSIDE the console lock and the interrupt mask: this is a multi-megabyte heap
    // request and the allocator is not something to enter with interrupts off.
    let mut store: Vec<u8> = alloc::vec::Vec::new();
    if store.try_reserve_exact(len).is_err() {
        serial_println!("[wc-x] console-window DECLINE reason=alloc len={}", len);
        return wm::WIN_NONE;
    }
    store.resize(len, 0);
    let mut surf_fb = FrameBuffer::new();
    // `Bgr` + 4 bytes: `put_pixel` stores b,g,r at bytes 0,1,2 and leaves byte 3 as the zero the
    // allocation gave it — i.e. the little-endian word `0x00RRGGBB`, which is the ARGB8888 pixel
    // `wm::draw_window` reads. Same bytes, not a conversion.
    // INVARIANT: `store` is at its final size and never grows, so this address stays valid for the
    // route's lifetime (the `attach_shadow` / `Screen` back-store idiom).
    surf_fb.init(
        store.as_mut_ptr() as usize,
        len,
        FrameBufferInfo {
            width: cw,
            height: ch,
            stride: cw,
            bytes_per_pixel: 4,
            pixel_format: unaos_boot_info::PixelFormat::Bgr,
        },
    );
    surf_fb.fill_screen(BG_DEFAULT);

    // SPAWN-PLACE — the OUTER box is centred on the panel and the row is created THERE, pinned.
    // `wm::create` + `wm::move_to` reached the same place, but `create` composites the new row before
    // it returns, so the console was shown once at the tiler's top-left origin and then jumped
    // (observed on the metal, s41), leaving a 1314x750 box the move had to erase. `spawn_geometry`
    // gives the box size before any row exists — the console's surface is far too large for the
    // scale rule to magnify, so this is `scale = 1` and `ow`/`oh` are the surface plus chrome, as
    // before. Pinning still keeps the tiler from walking the console around underneath the text.
    let (_scale, ow, oh) = match wm::spawn_geometry(cw, ch) {
        Some(g) => g,
        None => {
            serial_println!("[wc-x] console-window DECLINE reason=geometry-unavailable");
            return wm::WIN_NONE;
        }
    };
    let ox = pw.saturating_sub(ow) / 2;
    let oy = ph.saturating_sub(crate::ui_status::chrome_h(ph)).saturating_sub(oh) / 2;
    // Owner ASID 0 — the console belongs to the KERNEL. That keeps it out of `focus_ring` and out of
    // `close_owner`'s reach: no EL0 task can move, present or close the kernel's console.
    let id = wm::create_at(
        0,
        surf_fb.base(),
        len,
        cw as u32,
        ch as u32,
        stride as u32,
        b"console",
        ox + wm::BORDER,
        oy + wm::TITLE_H + wm::BORDER,
    );
    if id == wm::WIN_NONE {
        serial_println!("[wc-x] console-window DECLINE reason=create-failed");
        return wm::WIN_NONE;
    }

    // Install the route. From the store's move into `FbCon` onwards, every glyph lands in the
    // window's surface.
    let mut installed = false;
    crate::arch::without_interrupts(|| {
        if let Some(mut c) = FBCON.try_lock() {
            if !c.ready {
                return;
            }
            c.win_fb = surf_fb;
            c.win_store = Some(core::mem::take(&mut store));
            c.cols = (cw / c.cell_w).max(1);
            c.rows = (ch / c.cell_h).max(1);
            c.col = 0;
            c.row = 0;
            installed = true;
        }
    });
    if !installed {
        // Fail closed and leave no orphan row: the window is closed and the surface is dropped with
        // `store` at the end of this scope.
        wm::close(id);
        serial_println!("[wc-x] console-window DECLINE reason=install-contended");
        return wm::WIN_NONE;
    }

    CONSOLE_WIN.store(id, Ordering::Relaxed);
    let (gcols, grows) = ((cw / cell_w).max(1), (ch / cell_h).max(1));
    serial_println!(
        "[wc-x] console-window win={} panel={}x{} surf={}x{} box={}x{} at ({},{}) cell={}x{} cols={} rows={}",
        id, pw, ph, cw, ch, ow, oh, ox, oy, cell_w, cell_h, gcols, grows
    );
    serial_println!(
        "[wc-x] console-window panic-fallback armed win={} (panic paints the PANEL, not the window)",
        id
    );
    route_present();
    id
}

/// Repaint the screen as a panic backdrop (dark red) and home the cursor, so the panic message
/// that follows is unmissable on hardware. Best-effort: `try_lock` to avoid hanging if the lock
/// was held when the panic fired. Re-enables the serial mirror first (the GUI may have detached
/// it) so the panic text that `serial_println!` emits next lands on this red backdrop.
pub fn panic_screen() {
    GUI_ACTIVE.store(false, Ordering::Relaxed);
    // Re-arm the full mirror on quiet-panel builds: the panic text must paint whatever the build.
    #[cfg(target_arch = "x86_64")]
    PANIC_MIRROR.store(true, Ordering::Relaxed);
    // PANIC PATH LAW — the panic path does not go through the compositor. `CONSOLE_WIN` is cleared
    // FIRST, before any panic byte can be printed, so `route_present` can never ask a dying machine
    // for a composite; the route's surface is then dropped below and `draw_fb` falls back to the
    // panel handle, which is the pre-window path verbatim. Nothing about panic output depends on
    // `wm` having a live window, on its locks being takeable, or on the compositor running at all.
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    CONSOLE_WIN.store(wm::WIN_NONE, Ordering::Relaxed);
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
                // PANIC PATH LAW: drop the console-window route the same way and for the same
                // reason — `mem::forget` rather than a free, because the allocator's lock may
                // already be held by whatever is panicking. The grid is re-derived from the PANEL,
                // since the window's smaller `cols`/`rows` no longer describe the surface the panic
                // text is about to land on.
                #[cfg(all(target_arch = "x86_64", feature = "wc"))]
                {
                    if let Some(s) = c.win_store.take() {
                        core::mem::forget(s);
                    }
                    c.win_fb = FrameBuffer::new();
                    if c.cell_w != 0 && c.cell_h != 0 {
                        let info = c.fb.info();
                        c.cols = (info.width / c.cell_w).max(1);
                        c.rows = (info.height / c.cell_h).max(1);
                    }
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
