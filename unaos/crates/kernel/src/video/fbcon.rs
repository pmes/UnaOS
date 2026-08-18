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
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
/// inner request a no-op; the glyphs it painted are carried by the NEXT line's present.
///
/// FBCON-DMG — that last clause used to rest on "a present repaints the window's whole content
/// rather than a sub-rectangle", and it does not any more. What carries the inner request's rows now
/// is [`PEND`]: the band is merged into the pending set BEFORE this guard is tested, so a declined
/// present owes those rows to the next one instead of dropping them. Nothing else about the guard
/// changed, and the re-entrant print is still a no-op.
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
/// visible to the naked eye at bench distance. 6x was tried and judged "great, slightly large"
/// (metal s34); 4x — a 32x32 px cell, ~3.2 mm, 0.4 mm stroke — was then CONFIRMED legible on glass.
///
/// WHY IT IS NOW 2. Legibility stopped being the only constraint once the console became a WINDOW.
/// The routed surface is 1312x736, so a 32x32 cell buys 41 columns by 23 rows = 943 characters —
/// fewer than six of the compositor's own 110-200 character witness lines, i.e. the whole console is
/// overwritten every ~6 printed lines. And a line that STRADDLES the row-ring wrap dirties
/// `{row 22, row 0}`, over which `mark_rows`' min/max bounding box IS the whole surface: at 23 rows
/// the console saturates its own damage band by construction, roughly every 23rd newline.
///
/// A 16x16 cell gives 82x46 = 3772 characters — 4x the capacity and half the wrap rate — and costs
/// LESS to paint, not more: `draw_glyph` pokes `scale²` pixels per set font bit, so the same text is
/// a quarter of the pixel work and each line's damage band is half as tall.
///
/// LEGIBILITY AT 2x IS UNPROVEN. 8x8 is known invisible and 32x32 is known good; 16x16 (~1.6 mm
/// cell, ~0.2 mm stroke) has never been on this glass. That is a bench judgement, not a code one,
/// and this constant is the single knob that reverses it — everything downstream (cell size, grid,
/// window extent, damage rows, the panic re-derive) is computed from it, never assumed.
///
/// FONT (GR27) — the scale is RETIRED as the panel console's legibility knob: the takeover seam
/// now arms the shared anti-aliased face (`video::font`, 7x16 cell — the SAME 16 px height the
/// 2x cell drew, so the row arithmetic above still holds; columns roughly double). The whole
/// ledger above stands as the record of how the 16 px height was chosen, and the open bench
/// judgement transfers to the face unchanged; reversing it is `video::font::SIZE` now.

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
///
/// FONT (GR27) — two faces, one invariant. When `aa` is set (the x86 PANEL/window console after
/// the takeover seam) the glyph is the shared anti-aliased Noto face, alpha-composited against
/// the KNOWN background `bg` — computed, never read, so the path stays write-only on VRAM and
/// the same background-clean invariant carries the blend. When `aa` is clear (early boot, the
/// aarch64 console, any pre-takeover x86 print) this is the font8x8 path, byte for byte.
fn draw_glyph(surf: &FrameBuffer, ch: u8, cx: usize, cy: usize, fg: u32, bg: u32, s: usize, aa: bool) {
    if aa {
        crate::video::font::draw_glyph_fb(surf, ch, cx, cy, fg, bg, false);
        return;
    }
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
fn paint_ops(surf: &FrameBuffer, ops: &[Op], fg: u32, bg: u32, scale: usize, aa: bool) {
    for op in ops {
        match *op {
            Op::Glyph { ch, x, y } => draw_glyph(surf, ch, x, y, fg, bg, scale, aa),
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
    /// FONT (GR27) — draw with the shared anti-aliased face (`video::font`) instead of font8x8.
    /// Armed only by the x86 `panel_console_resume` seam (where `PANEL_SCALE` used to arm); every
    /// re-init clears it, so early boot and aarch64 keep the raw 8x8 path unchanged.
    aa: bool,
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
            aa: false,
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
    ///
    /// FBCON-DMG — returns the SURFACE-ROW band the caller owes the compositor, and `None` whenever
    /// the console is not routed into a window (every aarch64 build, every x86 build without `wc`,
    /// and every moment before the route exists or after a panic tore it down). Those callers keep
    /// calling the unbanded [`route_present`], which is a no-op for them, so nothing off the routed
    /// x86 path changes at all.
    #[must_use = "the routed band must reach route_present_rows or the rows never present"]
    fn flush_dirty(&mut self) -> Option<(usize, usize)> {
        // CONSOLE-WINDOW: the glyphs went into cached kernel RAM that no scan-out reads, and the
        // panel write belongs to the compositor. There is nothing to clean and nothing to blit here;
        // the damage is declared outside this lock.
        //
        // FBCON-DMG: the band is HANDED BACK rather than dropped. This is the older of the two places
        // the console's per-line damage was being discarded — it was computed correctly by
        // `mark_rows`, reset here, and the present that followed then repainted all 750 rows of the
        // box because it had nothing narrower to go on.
        #[cfg(all(target_arch = "x86_64", feature = "wc"))]
        if self.win_store.is_some() {
            let band = (self.dirty_y0, self.dirty_y1);
            self.dirty_y0 = 0;
            self.dirty_y1 = 0;
            return if band.1 > band.0 { Some(band) } else { None };
        }
        if self.dirty_y0 >= self.dirty_y1 {
            return None;
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
        None
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
        paint_ops(self.draw_fb(), ops.as_slice(), self.fg, self.bg, self.scale, self.aa);
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
        c.aa = false;
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

/// PANEL-QUIET (x86, PANEL-CONSOLE lane only) — the tags whose lines are written for `serial.log`
/// and for nobody standing at the bench.
///
/// Two costs, and the second is the one that bites. The obvious one is capacity: the routed console
/// holds 943 characters at `PANEL_SCALE = 4` and 3772 at 2, and one `[wc-h] rollup` line is several
/// of its rows either way. The other is RE-ENTRANCY. These lines are emitted from inside the very
/// composite that a console present asked for, so each one mirrors back into the console surface,
/// gets merged into [`PEND`] by [`route_present_banded`], is then declined by the [`ROUTE_BUSY`]
/// guard, and is carried by the NEXT present — widening a band that had just been made narrow. The
/// compositor's own telemetry was inflating the damage it was reporting (see [`ROUTE_BUSY`]).
///
/// WHAT IS EXCLUDED, AND WHY EACH ONE. `[wc-g]` (per-present coherency probe), `[wc-h]` (per-present
/// staging cost) and `[wc-d]` (per-window surface verify) all fire from within a present — they ARE
/// the re-entrant family. `[wcn]` (window-census rollup) and `[schedx86]` (render-queue depth) are
/// periodic telemetry with no on-glass reader. All five are serial-log instruments with spec
/// REQUIREs behind them, and the wire keeps every one of them byte for byte: this policy governs the
/// GLASS only, and the FTDI ring, `UNAOS.LOG` and the `tste` ring all tap upstream of here, in
/// `arch::serial::_print`.
///
/// WHAT IS DELIBERATELY NOT EXCLUDED: `[wc-x]` — the console's own route / first-paint / decline
/// announcements, which are exactly what a bench reader is looking FOR; `[fbcon]` and `[mirror]`,
/// which announce mirror loss; boot milestones; every untagged line; and PANIC text.
#[cfg(all(target_arch = "x86_64", not(feature = "bootlog")))]
const PANEL_MUTE_TAGS: [&[u8]; 5] = [b"[wc-g]", b"[wc-h]", b"[wc-d]", b"[wcn]", b"[schedx86]"];

/// Bytes of a line's head the sniff needs in order to decide — the longest [`PANEL_MUTE_TAGS`] entry.
#[cfg(all(target_arch = "x86_64", not(feature = "bootlog")))]
const TAG_SNIFF: usize = 10;

/// PANEL-QUIET — capture just enough of a line's HEAD to test it against [`PANEL_MUTE_TAGS`].
///
/// A `fmt::Write` sink rather than a string compare, because `_print` is handed
/// `core::fmt::Arguments` and the tag lives inside the format string's leading literal piece, which
/// is not materialised anywhere. Nothing is allocated and nothing is retained — the sink is
/// [`TAG_SNIFF`] bytes of stack — and it returns `Err` the instant it is full, which ABORTS the
/// format walk instead of rendering the whole line a second time. Re-walking a prefix of `args` is
/// free of consequence: `Arguments` is `Copy` and `_print` already hands the same value to two
/// sinks.
#[cfg(all(target_arch = "x86_64", not(feature = "bootlog")))]
struct TagSniff {
    buf: [u8; TAG_SNIFF],
    n: usize,
}

#[cfg(all(target_arch = "x86_64", not(feature = "bootlog")))]
impl TagSniff {
    fn new() -> Self {
        TagSniff { buf: [0u8; TAG_SNIFF], n: 0 }
    }

    /// True when the head is a bracketed tag on the mute list. A line that does not START with `[`
    /// can never match, so the common case costs one byte compare.
    fn muted(&self) -> bool {
        let head = &self.buf[..self.n];
        if head.first() != Some(&b'[') {
            return false;
        }
        PANEL_MUTE_TAGS.iter().any(|t| head.starts_with(t))
    }
}

#[cfg(all(target_arch = "x86_64", not(feature = "bootlog")))]
impl core::fmt::Write for TagSniff {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.n >= TAG_SNIFF {
                // Decided. `Err` stops `write_fmt` walking the rest of the line.
                return Err(core::fmt::Error);
            }
            self.buf[self.n] = b;
            self.n += 1;
        }
        Ok(())
    }
}

/// Mirror formatted output to the framebuffer (called from the serial `_print`). Lock-free of
/// deadlock: interrupts are masked and the lock is `try_lock`ed, so a contended line just skips
/// the screen (serial still has it) rather than spinning.
pub fn _print(args: core::fmt::Arguments) {
    // SERWIT-2: every exit from this function charges the fbcon tap exactly once — absorbed (it reached
    // the panel), suppressed (declined by policy) or dropped (contended, and GONE from the panel).
    // Before this arc there was no counter of any kind, so a panel that was quiet because policy said
    // so was indistinguishable from a panel that was quiet because it lost the lock.
    let tap = &crate::serial_ring::TAP_FBCON;
    tap.submit();
    // Once the GUI owns the screen, don't mirror to the framebuffer (serial still gets it).
    if GUI_ACTIVE.load(Ordering::Relaxed) {
        tap.suppress();
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
        tap.suppress();
        return;
    }
    // PANEL-QUIET (x86): the panel console is a bench-eye channel, not a second copy of the serial
    // log. Once PANEL-CONSOLE has armed the mirror, the compositor's and scheduler's own telemetry
    // is kept off the GLASS — see `PANEL_MUTE_TAGS` for the list and for the re-entrancy that makes
    // this a damage fix and not just a legibility one.
    //
    // THE PANIC OVERRIDE IS PRESERVED TWICE OVER, and the two are independent on purpose (the same
    // belt-and-braces `draw_fb` uses). Belt: the gate demands `PANIC_MIRROR` be CLEAR, so from
    // `panic_screen` onwards not one line can be muted, whatever it says. Braces: the test is a
    // POSITIVE match against a fixed tag list, so panic text — which carries none of those tags —
    // could not be muted even if it reached here before the flag flipped.
    //
    // COUNTED, NEVER VANISHED: a muted line charges the tap `suppressed`, the ledger's term for
    // "declined by policy" as opposed to lost, so `submitted == absorbed + dropped + suppressed`
    // still holds exactly. This is also why the charge is here and not deeper: every exit from this
    // function charges exactly once.
    #[cfg(all(target_arch = "x86_64", not(feature = "bootlog")))]
    if PANEL_CONSOLE.load(Ordering::Relaxed) && !PANIC_MIRROR.load(Ordering::Relaxed) {
        let mut sniff = TagSniff::new();
        let _ = core::fmt::Write::write_fmt(&mut sniff, args);
        if sniff.muted() {
            tap.suppress();
            return;
        }
    }
    // PANEL-DEFER (x86): mirror through the split layout/paint path when the draw target is VRAM
    // (no cached-RAM shadow) and we are not on the panic path. See `panel_mirror` for the invariant.
    #[cfg(target_arch = "x86_64")]
    if !PANIC_MIRROR.load(Ordering::Relaxed) {
        let mut sink = PanelSink::new();
        let _ = core::fmt::Write::write_fmt(&mut sink, args);
        if sink.finish() {
            // The split path owned this line, or there was nothing to paint at all.
            if sink.split {
                tap.absorb();
            } else {
                tap.suppress();
            }
            return;
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

/// FBCON-DMG — THE ROWS THE COMPOSITOR IS OWED BUT HAS NOT BEEN GIVEN.
///
/// A banded present is only sound if a band that is *not* presented is not *forgotten*. Three paths
/// decline a present after the glyphs are already in the surface — the `ROUTE_BUSY` re-entry guard,
/// the INSTGUI suspension, and a `wm` that reports the row gone — and before this arc all three were
/// harmless because the next present repainted everything anyway. They are not harmless now.
///
/// So every band is merged in here BEFORE any decline is tested, and taken out only by the present
/// that is actually going to run. A declined present therefore leaves its rows standing, and the next
/// one carries them.
///
/// `(y0, y1)` with `y1 <= y0` is empty. The SATURATED state — "some band was lost track of, repaint
/// the whole window" — lives beside this in [`PEND_FULL`] rather than in the struct, because it is
/// precisely the state that has to be recordable when the ledger itself cannot be taken. Every
/// degradation here is towards painting MORE, never less.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
#[derive(Clone, Copy)]
struct Pending {
    y0: usize,
    y1: usize,
}

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
impl Pending {
    const EMPTY: Pending = Pending { y0: 0, y1: 0 };
    fn merge(&mut self, y0: usize, y1: usize) {
        if y1 <= y0 {
            return;
        }
        if self.y1 <= self.y0 {
            self.y0 = y0;
            self.y1 = y1;
        } else {
            self.y0 = self.y0.min(y0);
            self.y1 = self.y1.max(y1);
        }
    }
}

/// The ledger itself. A `Mutex` and not a pair of atomics because the merge and the take are each
/// two-field operations that must not interleave: a take that read a new `y0` against an old `y1`
/// could drop rows, which is the one outcome this whole mechanism exists to prevent.
///
/// **Always `try_lock`.** This is reached from print context with interrupts ENABLED, so an
/// IRQ-context printer can land on the same core mid-critical-section; a blocking acquire there would
/// deadlock the machine over a repaint. Contention is instead answered with `full` — a whole-box
/// present, i.e. exactly the behaviour this arc replaces, for one line.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static PEND: Mutex<Pending> = Mutex::new(Pending::EMPTY);

/// FBCON-DMG — record `[y0, y1)` as owed. Never presents; see [`route_present_rows`].
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn pend_merge(y0: usize, y1: usize) {
    match PEND.try_lock() {
        Some(mut p) => p.merge(y0, y1),
        // Contended: we cannot record WHICH rows, so we record that some are outstanding. The next
        // take turns that into a whole-box present.
        None => PEND_FULL.store(true, Ordering::Release),
    }
}

/// FBCON-DMG — take everything owed, as the three-way answer [`Owed`] names.
///
/// FBCON-PACE widened the return: it used to be `Option<(usize, usize)>`, in which `None` meant BOTH
/// "the whole box" and "nothing at all", and the caller could only act on the pessimistic reading.
/// That was sound while every caller had just added something, and it is not once a sync point can
/// arrive with a clean ledger — a whole-box repaint on behalf of a surface that did not change is
/// 750 rows of work for no pixel. The distinction is made HERE because this is the only place that
/// can still see it; after the swap-and-take, it is gone.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn pend_take() -> Owed {
    let full = PEND_FULL.swap(false, Ordering::AcqRel);
    match PEND.try_lock() {
        Some(mut p) => {
            let band = (p.y0, p.y1);
            *p = Pending::EMPTY;
            if full {
                Owed::Whole
            } else if band.1 > band.0 {
                Owed::Band(band.0, band.1)
            } else {
                Owed::Nothing
            }
        }
        // Contended: the ledger may hold rows we cannot read. Present the whole box; anything still
        // recorded in there is then a superset of what is already on the glass, which costs one
        // redundant band later and loses nothing. NEVER `Nothing` — that is the one answer that
        // could drop rows, and it is the answer this arm must not be allowed to give.
        None => Owed::Whole,
    }
}

/// Companion to [`PEND`] for the contended case — see [`pend_merge`].
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static PEND_FULL: AtomicBool = AtomicBool::new(false);

/// FBCON-PACE — what a routed call owes the compositor.
///
/// Three shapes, because "present the whole box", "present these rows" and "present whatever is
/// already owed" are three different statements and only the first two used to be expressible. The
/// third is what a sync point ([`console_flush`]) and a line that painted nothing both want: add
/// nothing to the ledger, and do not invent a whole-box repaint for a surface that did not change.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
#[derive(Clone, Copy)]
enum Owed {
    /// The whole surface changed, or the caller does not know what changed.
    Whole,
    /// These SURFACE rows changed.
    Band(usize, usize),
    /// Nothing new.
    Nothing,
}

/// FBCON-PACE — the present cadence, in Hz. One present per 60 Hz frame is the rate at which a
/// panel can show a change at all; anything faster is work whose result is overwritten before it
/// is scanned out. `wm`'s own `[wc-h]` rollup already prints `frame_us=16667` for the same reason,
/// and this is that number expressed as the rate it is derived from.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
const PACE_HZ: u64 = 60;

/// FBCON-PACE — `now_cycles()` at the END of the last present this seam issued; 0 = none yet.
///
/// Stamped at the END, not the start, on purpose: the guarantee is then "the console starts at most
/// one present per frame period AFTER the previous one finished", which bounds the share of wall
/// clock the console's presents can occupy no matter how long a present takes. Stamping at the start
/// would bound the RATE but not the share.
///
/// A stale stamp (a re-init, a route torn down and rebuilt) degrades toward a LARGE delta, i.e.
/// toward presenting — the same direction every other degradation in this module takes.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static PACE_LAST: AtomicU64 = AtomicU64::new(0);

/// FBCON-PACE census — presents this seam ISSUED (any urgency).
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static PACE_RAN: AtomicU64 = AtomicU64::new(0);

/// FBCON-PACE census — presents the pacing gate HELD, counting ONLY calls that had rows of their own
/// to defer ([`Owed::Band`] / [`Owed::Whole`]). That is the count of coalesced LINES, which is what
/// the name is read as; a service-hook pass that added nothing and was not yet due is `idle`, not
/// held. See [`console_pace_census_once`] for the identity the four counters close.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static PACE_HELD: AtomicU64 = AtomicU64::new(0);

/// FBCON-PACE census — presents the [`ROUTE_BUSY`] re-entry guard declined. Counted for the first
/// time here: it was the one decline in this file with no counter behind it, and a pacing gate that
/// reports its own declines while a neighbouring one does not would be an invitation to misread the
/// difference between the two.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static PACE_BUSY: AtomicU64 = AtomicU64::new(0);

/// FBCON-PACE census — calls that owed nothing and presented nothing: a [`console_service`] pass on
/// a clean or not-yet-due ledger, and a control-only print. Without it the identity does not close —
/// the arm that wins the re-entry guard and finds [`Owed::Nothing`] was landing in no bucket at all,
/// and it is the COMMON shape of the service hook.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static PACE_IDLE: AtomicU64 = AtomicU64::new(0);

/// FBCON-PACE — one-shot latch for [`console_pace_census_once`].
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static PACE_CENSUS_DONE: AtomicBool = AtomicBool::new(false);

/// FBCON-PACE — is a present DUE, i.e. has a frame period elapsed since the last one finished?
///
/// ### The clock discipline (the convicted bug class)
///
/// Only a DELTA between two `now_cycles()` readings is ever formed, and the budget is carried in
/// CYCLES — `hz / PACE_HZ` — so nothing multiplies an absolute counter and nothing scales before it
/// divides. `now_cycles()` on x86 is `rdtsc`, which counts from processor RESET: its absolute value
/// includes firmware and means nothing as an uptime (the `wcg::since_entry_ms` note carries the full
/// argument, and the two defects it names — measuring from reset, and scaling before dividing — are
/// both structurally absent here rather than corrected).
///
/// ### Every degradation presents
///
/// * an UNCALIBRATED TSC (`tsc_hz() == 0`, before `apic::calibrate`) → due. A guessed rate would
///   silently mis-time the gate; presenting is exactly today's per-line behaviour, so the
///   pre-calibration boot is byte-for-byte unchanged. (Calibration lands long before the route is
///   ever installed; this is a belt, not a live path.)
/// * no present yet (`PACE_LAST == 0`) → due.
/// * a reading taken on a core whose TSC is skewed behind ours → `wrapping_sub` yields a huge delta
///   → due.
/// * [`PANIC_MIRROR`] armed → due, unconditionally. See the PANIC PATH LAW note on
///   [`route_present_banded`].
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn pace_due(now: u64) -> bool {
    if PANIC_MIRROR.load(Ordering::Relaxed) {
        return true;
    }
    let hz = crate::arch::apic::tsc_hz();
    if hz == 0 {
        return true;
    }
    let last = PACE_LAST.load(Ordering::Relaxed);
    if last == 0 {
        return true;
    }
    now.wrapping_sub(last) >= hz / PACE_HZ
}

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn route_present() {
    route_present_banded(Owed::Whole, false)
}

/// FBCON-DMG — present the console window over SURFACE ROWS `[band.0, band.1)` only.
///
/// The one call fbcon makes on the hot path, and the one that is PACED (see
/// [`route_present_banded`]). Same contract as [`route_present`] in every other respect: **call with
/// the `FBCON` lock RELEASED and interrupts ENABLED.**
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn route_present_rows(band: (usize, usize)) {
    route_present_banded(Owed::Band(band.0, band.1), true)
}

/// FBCON-PACE — a print that changed NOTHING on the surface. Owes no rows; still gives the pacing
/// gate a chance to retire rows an earlier line left owed. Previously this case forced a whole-box
/// present, which repainted 750 rows on behalf of a line that moved no pixel.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn route_present_pending() {
    route_present_banded(Owed::Nothing, true)
}

/// FBCON-PACE — THE IDLE-LOOP SERVICE HOOK. Retire rows a held present left owed, **still paced**.
///
/// ### Why the bench lane needs it, and why it is not the forced flush
///
/// A held band is carried by the NEXT present, and on the print path the next present is the next
/// print. When printing stops mid-burst the last band waits — after boot, potentially on operator
/// action. [`detach`] is the backstop for GUI boots, and the metal bench lane **never detaches**:
/// the `usbdebug` view keeps fbcon attached by design (`main.rs`, the USB-debug loop), which is
/// exactly the lane the routed console runs on and the lane this arc's prediction is measured on.
///
/// So the `usbdebug` service loop calls this once per pass, beside `bootpace::service_dump`. The
/// loop `hlt()`s and the x86 heartbeat is `apic::TICK_HZ` = 1 kHz, so a pass is at most ~1 ms out
/// and **worst-case staleness of a held band is one frame period plus one pass, ≈ 17.7 ms** — the
/// bound that replaces "until the next print".
///
/// It is deliberately the PACED call and not [`console_flush`]. A forced flush at the loop's 1 kHz
/// cadence would present every held band within a millisecond, which is the pacing gate switched
/// off for every line printed from inside the loop's own services — and those services (xHCI
/// enumeration, storage, FTDI) are where a large share of the routed lines after
/// `console-route first-paint` come from. Paced, this hook adds no present the gate would not
/// already have allowed: it can only ever move a present EARLIER within the frame it was already
/// going to happen in, so the predicted present count is unchanged.
///
/// Silent, and free on a clean ledger: `pend_take` answers `Owed::Nothing` and nothing is composited.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn console_service() {
    route_present_banded(Owed::Nothing, true)
}

/// FBCON-PACE — a SYNC point: present everything owed NOW, whatever the pacing gate would say.
///
/// Called from [`detach`], the moment the GUI takes the panel and no further print will ever route,
/// and from [`console_pace_census_once`] so its own line lands. `pub` so any end-of-boot caller can
/// use it for the same reason. SILENT — the census used to live here and was split out, because a
/// flush that is called once per service pass must not print once per service pass.
///
/// No-op unless the console is routed, and free on a clean ledger.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn console_flush() {
    route_present_banded(Owed::Nothing, false)
}

/// FBCON-PACE — the pacing census, once per boot.
///
/// Emitted from a one-shot in the `usbdebug` service loop beside `xhci::log_summary_once`, i.e.
/// after enumeration and after the boot burst this arc reshapes, so the numbers cover the burst.
/// `[wc-x]` is not on [`PANEL_MUTE_TAGS`], so it reaches the glass as well as the wire.
///
/// THE IDENTITY THE LINE ASSERTS: `ran + held + busy + idle` is every call that reached the gate
/// with a live route and an unsuspended console — one bucket per call, no call in two, no call in
/// none. (Calls that return earlier are outside it by construction: no route, or the INSTGUI
/// suspension, both of which are declines this arc did not author.)
///   * `ran`  — a present was issued to `wm`.
///   * `held` — the gate deferred rows THIS call added. The count of coalesced lines.
///   * `busy` — the [`ROUTE_BUSY`] re-entry guard declined.
///   * `idle` — the call added nothing and presented nothing: a service-hook pass on a clean or
///     not-yet-due ledger, or a control-only print. Kept OUT of `held` deliberately — charging it
///     there would inflate "lines coalesced" with calls that carried no line at all.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn console_pace_census_once() {
    if CONSOLE_WIN.load(Ordering::Relaxed) == wm::WIN_NONE {
        return;
    }
    if PACE_CENSUS_DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // The census goes out BEFORE the flush, so its own glyphs are carried by the flush it describes
    // rather than left owed behind it. The numbers therefore describe the run up to this line, which
    // is what they claim.
    serial_println!(
        "[wc-x] console-pace ran={} held={} busy={} idle={} budget_us={} hz={}",
        PACE_RAN.load(Ordering::Relaxed),
        PACE_HELD.load(Ordering::Relaxed),
        PACE_BUSY.load(Ordering::Relaxed),
        PACE_IDLE.load(Ordering::Relaxed),
        1_000_000 / PACE_HZ,
        crate::arch::apic::tsc_hz()
    );
    console_flush();
}

/// The body every routed present shares.
///
/// ### FBCON-PACE — the console pays one present per FRAME, not one per LINE
///
/// GR17 measured what the routed console costs: every kernel serial line reaches `wm::present_rows`
/// and buys its own banded compose+present. On boot 7 of the `rmbp-gr16-s73` capture that is 799
/// routed lines between `[wc-x] console-route first-paint` and the end of boot, ~64 surface rows
/// each (`[wc-h] span=64 compose_us=192 present_us=233`), ~6.6 µs/row — ~51k rows and ~0.34 s of
/// boot spent presenting single console lines, with steady-state console throughput capped by the
/// same figure.
///
/// The lines are not spread evenly: 764 of the 799 arrive within 17 ms of the line before them.
/// Boot printing is bursty, and a burst repaints the same console faster than the panel can scan it
/// out — every present but the last of each frame is work whose result is overwritten before anyone
/// could see it.
///
/// So a banded present is now DEFERRED until a frame period has passed since the last one finished.
/// The rows are not dropped: they are merged into [`PEND`] BEFORE the gate is tested, exactly as the
/// two older declines ([`ROUTE_BUSY`], the INSTGUI suspension) already are, and the next present that
/// runs carries them. FBCON-DMG built that ledger precisely so a decline could be safe; this arc is
/// its third and first *deliberate* caller.
///
/// ### The three flush triggers
///
/// 1. **Time** — a frame period since the last present finished ([`pace_due`]). This is the trigger
///    the hot path rides: it needs no timer, because the next print asks the question — and, on the
///    lane where printing can stop, because the service loop asks it too ([`console_service`]).
/// 2. **Urgency** — a caller that passes `coalesce = false` is never held: the route's own first
///    paint, [`clear`], the INSTGUI resume, and the [`console_flush`] sync point.
/// 3. **Panic** — [`pace_due`] returns `true` whenever [`PANIC_MIRROR`] is armed, so no panic output
///    can ever wait on a pacing timer. That is the belt; the braces are that [`panic_screen`] clears
///    [`CONSOLE_WIN`] before any panic byte is printed, so this function returns at its first line
///    and panic text goes to the PANEL through [`FbCon::draw_fb`]'s own panic branch. Panic output
///    depends on neither the compositor nor this gate, exactly as before.
///
/// ### Why the damage SPAN is deliberately NOT a fourth trigger
///
/// A span bound ("flush once the pending band gets large") reads like the natural companion to the
/// time bound, and measurement says it is a cost, not a saving. The band is a bounding box capped by
/// the surface height, so once a burst has saturated it the band CANNOT grow — from that moment
/// further coalescing is free, and flushing on saturation only buys an extra whole-box present at
/// ~5 ms each. Replaying boot 7's line timestamps: 57 presents / 24 180 rows with time alone, 78
/// presents / 28 084 rows if saturation also flushed. Staleness is already bounded by the time
/// trigger, which is the property a span bound would have been reached for.
///
/// ### How stale a held band can get
///
/// Bounded, and by a different bound on each lane.
///   * **The `usbdebug` bench lane** — one frame period (the gate) plus one service-loop pass. The
///     loop `hlt()`s against the 1 kHz x86 heartbeat (`apic::TICK_HZ`), so a pass is ~1 ms out and
///     the bound is **≈ 17.7 ms**. See [`console_service`].
///   * **GUI boots** — the next print, with [`detach`] as the backstop: it flushes before the GUI
///     takes the panel, so nothing the console painted is left owed at the handoff.
///   * **Panic** — zero. Trigger 3.
///
/// ### What `[wc-h] span=` means now, and why the censuses stay honest
///
/// `span=` used to be ONE console line's damage. It is now the union of every line since the last
/// present, so the same wire word denotes a larger, rarer band — bands near the surface height on a
/// boot burst, and single-line bands whenever printing is slower than a frame. `[wc-h]`'s
/// `whole=`/`banded=` censuses count real presents either way and are the proof lines for this arc:
/// `banded=` collapsing while the console keeps printing IS the feature working. (`wcg`'s own
/// rollup doc is reconciled with both readings at its `banded=0` and `span=` notes.) Nothing becomes
/// unobservable — every call that reaches the gate lands in exactly one of [`PACE_RAN`],
/// [`PACE_HELD`], [`PACE_BUSY`] or [`PACE_IDLE`], reported by [`console_pace_census_once`], and the
/// rows themselves are in [`PEND`], which no path in this file drops.
///
/// The `[wc-d]` reference discipline is untouched: WCD-PRE takes its reference from the composite
/// loop before the blit, and the print hazard it exists for is a print that MERGES and DECLINES —
/// which is what a held present does, and what the re-entry guard already did.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn route_present_banded(owed_now: Owed, coalesce: bool) {
    let id = CONSOLE_WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return;
    }
    // OWE FIRST, DECLINE SECOND. Every return below happens with the rows recorded, so none can lose
    // them. A whole-box caller saturates the ledger for the same reason.
    match owed_now {
        Owed::Band(y0, y1) => pend_merge(y0, y1),
        Owed::Whole => PEND_FULL.store(true, Ordering::Release),
        Owed::Nothing => {}
    }
    if CONSOLE_PRESENT_SUSPENDED.load(Ordering::Relaxed) {
        return;
    }
    // FBCON-PACE: the gate. Rows stay owed; the next present carries them. The clock is read only on
    // the coalescing path — a caller that will present regardless has no question to ask it.
    if coalesce && !pace_due(crate::arch::now_cycles()) {
        // `held` means "this call's own rows were deferred". A caller that brought no rows brought
        // no line, so it is `idle` — see the identity on `console_pace_census_once`.
        match owed_now {
            Owed::Nothing => PACE_IDLE.fetch_add(1, Ordering::Relaxed),
            _ => PACE_HELD.fetch_add(1, Ordering::Relaxed),
        };
        return;
    }
    // Re-entry: see `ROUTE_BUSY`.
    if ROUTE_BUSY
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        PACE_BUSY.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // Everything owed, including bands from presents that declined earlier.
    let owed = pend_take();
    let ok = match owed {
        // Nothing is owed at all — a sync point, or a line that moved no pixel, arriving on a clean
        // ledger. There is no such thing as an honest whole-box present here: the surface has not
        // changed. (`pend_take`'s CONTENDED arm answers `Whole`, never `Nothing`, so a ledger that
        // MIGHT hold rows still repaints; degradation stays in the paint-MORE direction.)
        Owed::Nothing => {
            PACE_IDLE.fetch_add(1, Ordering::Relaxed);
            ROUTE_BUSY.store(false, Ordering::Release);
            return;
        }
        // NORMALWIN — **the RECYCLED-ID FENCE, and the console now needs it.** `id` was snapshotted
        // from `CONSOLE_WIN` at the head of this function, outside any lock. Until this arc that
        // snapshot could not go stale: nothing could close the console row (`close_owner` refuses the
        // kernel band, and furniture carried no close disc), so the id named the same row forever.
        // The close disc changes that — `wc_close_furniture` clears `CONSOLE_WIN` and then frees the
        // row, and `wm::close` hands the slot straight back to the next `SYS_WIN_CREATE`. A print
        // that stalled between the load above and this line would otherwise present a ring-3 app's
        // window under the console's damage band, through the fence-FREE `expect_owner = None` path
        // whose doc says kernel/furniture callers "have no recycle race". They do now, so this path
        // carries the fence: `KERNEL_OWNER_CONSOLE` is the owner `panel_console_window_open` minted
        // the row with, and a slot that has been re-issued to anyone else declines `NoRow` and takes
        // the rows-go-back arm below. Same three outcomes as the `bool` verbs (`!= NoRow` is exactly
        // their body), so nothing else here moves.
        Owed::Band(y0, y1) => {
            PACE_RAN.fetch_add(1, Ordering::Relaxed);
            wm::present_rows_outcome_owned(id, y0, y1, wm::KERNEL_OWNER_CONSOLE)
                != wm::Presented::NoRow
        }
        Owed::Whole => {
            PACE_RAN.fetch_add(1, Ordering::Relaxed);
            wm::present_outcome_owned(id, wm::KERNEL_OWNER_CONSOLE) != wm::Presented::NoRow
        }
    };
    if ok {
        // The frame window opens when the present FINISHES — see `PACE_LAST`. Not stamped on a
        // failed present: the rows go back below, and a failure must not buy the retry a frame of
        // delay.
        PACE_LAST.store(crate::arch::now_cycles(), Ordering::Relaxed);
    } else {
        // The row is gone (a panic tore the route down, or the window was closed underneath us).
        // Put the rows back rather than dropping them: if a route is ever re-established they are
        // still owed, and if it is not, nothing reads the ledger again.
        match owed {
            Owed::Band(y0, y1) => pend_merge(y0, y1),
            _ => PEND_FULL.store(true, Ordering::Release),
        }
    }
    if ok && !ROUTE_ANNOUNCED.swap(true, Ordering::Relaxed) {
        serial_println!(
            "[wc-x] console-route first-paint win={} (glyphs -> window surface, damage-limited)",
            id
        );
    }
    ROUTE_BUSY.store(false, Ordering::Release);
}

#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
#[inline]
fn route_present() {}

/// No-op off the routed x86 path: `flush_dirty` returns `None` there, so this is never reached with
/// a real band. Defined so the shared print paths need no `cfg` of their own.
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
#[inline]
fn route_present_rows(_band: (usize, usize)) {}

/// No-op off the routed x86 path (see the x86 definition). There is no pacing gate to give a chance
/// to, because there is no present.
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
#[inline]
fn route_present_pending() {}

/// No-op off the routed x86 path (see the x86 definition). `detach` calls this on every arch; off
/// the routed path there is nothing owed and nothing printed.
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
#[inline]
pub fn console_flush() {}

/// No-op off the routed x86 path (see the x86 definition). The `usbdebug` service loop calls this
/// on every arch and every build; off the routed path there is no ledger to service.
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
#[inline]
pub fn console_service() {}

/// No-op off the routed x86 path (see the x86 definition). There is no pacing gate to report on.
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
#[inline]
pub fn console_pace_census_once() {}

/// No-op off the wc path (see the x86 definition).
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
#[inline]
pub fn console_present_suspend(_on: bool) {}

/// The classic mirror: format straight into the console with interrupts masked for the whole
/// line. Kept verbatim for the PANIC path and for aarch64.
fn print_masked(args: core::fmt::Arguments) {
    let tap = &crate::serial_ring::TAP_FBCON;
    let mut drew = false;
    let mut ready = false;
    let mut band = None;
    crate::arch::without_interrupts(|| {
        if let Some(mut c) = FBCON.try_lock() {
            ready = true;
            if c.ready {
                // SERWIT-2: the panel's OWN announcement channel. A reader looking at the glass cannot
                // see a counter, so the gap is named where the gap is — one marker per burst, painted
                // ahead of the line that finally got the lock.
                let missed = PANEL_UNANNOUNCED.swap(0, Ordering::Relaxed);
                if missed > 0 {
                    let _ = core::fmt::Write::write_fmt(
                        &mut Sink { con: &mut c },
                        format_args!("[fbcon] {} line(s) missed the panel\n", missed),
                    );
                }
                let _ = core::fmt::Write::write_fmt(&mut Sink { con: &mut c }, args);
                // Clean just the freshly-drawn rows out to RAM (no-op off the Pi). fbcon pokes
                // pixels directly rather than via `blit`, so this is what keeps the boot log visible
                // on the HVS-scanned framebuffer — but only the touched rows, not the whole surface.
                band = c.flush_dirty();
                drew = true;
            }
        }
    });
    // CONSOLE-WINDOW: outside the mask and the lock, per `route_present`'s contract. Not reached on
    // the panic path in any meaningful sense — `panic_screen` has already cleared `CONSOLE_WIN`.
    // FBCON-DMG: the whole LINE's band, presented once. `None` is either "not routed" (where both
    // calls are no-ops) or "routed and nothing to say".
    // FBCON-PACE: the second of those used to force a WHOLE-BOX present — 750 rows on behalf of a
    // line that moved no pixel. It now asks the pacing gate to retire whatever an earlier line left
    // owed and adds nothing of its own; `pend_take` answering `Owed::Nothing` presents nothing at all.
    if drew {
        tap.absorb();
        match band {
            Some(b) => route_present_rows(b),
            None => route_present_pending(),
        }
    } else if ready {
        // Lock taken, console not up yet. Declined by policy, not lost.
        tap.suppress();
    } else {
        // THE DROP. `try_lock` lost to a printer on another core (or an IRQ printer nested on this
        // one) and this line will never appear on the panel. It is legitimately lost — see the tap
        // discipline in `serial_ring`: painting a deferred backlog would put glyph work inside the
        // masked critical section PANEL-DEFER exists to keep short, and a mirror that can stall the
        // primary wire is worse than one that drops. But it is no longer lost SILENTLY.
        tap.drop_line();
        PANEL_UNANNOUNCED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Panel misses not yet announced ON THE PANEL. Separate from the tap ledger's own pending counter so
/// the glass and the wire do not consume each other's news — the person at the bench sees only the
/// glass, and the person reading `serial.log` sees only the wire.
static PANEL_UNANNOUNCED: AtomicU64 = AtomicU64::new(0);

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
///
/// ### FBCON-DMG — ONE PRESENT PER LINE, NOT ONE PER CHUNK
///
/// `write_str` flushes every [`CHUNK`] (16) bytes, and each flush used to end in its own
/// `route_present`. On the routed console that meant an 80-column line paid FIVE full-box composites
/// and presents — the 24 ms measured in `[wc-h] win=1` multiplied by the line length, for a line
/// whose glyphs occupy one cell row. The chunking exists to bound the stack cost of the op buffer
/// under a nesting IRQ printer; it was never meant to be a present cadence.
///
/// So the paint half still runs per chunk (it is cheap — cached RAM, no lock, no mask) and the
/// PRESENT is deferred to [`PanelSink::finish`], over the union of every chunk's band. The two
/// savings compose: one present per line instead of ~five, and a few rows per present instead of 750.
#[cfg(target_arch = "x86_64")]
struct PanelSink {
    buf: [u8; CHUNK],
    n: usize,
    /// `false` until the first flush decides the route; `true` = split path owns this line.
    split: bool,
    /// Route decided yet?
    decided: bool,
    /// FBCON-DMG — the union of every routed chunk's dirty band, in surface rows; empty when this
    /// line painted nothing routed. Presented once, by `finish`.
    band: (usize, usize),
    /// FBCON-DMG — did any chunk of this line paint into the console WINDOW? Distinguishes "routed
    /// and nothing changed" from "not routed at all", which owe different things at `finish`.
    routed: bool,
}

#[cfg(target_arch = "x86_64")]
impl PanelSink {
    fn new() -> Self {
        PanelSink {
            buf: [0u8; CHUNK],
            n: 0,
            split: false,
            decided: false,
            band: (0, 0),
            routed: false,
        }
    }

    /// FBCON-DMG — union a chunk's dirty band into the line's.
    fn note(&mut self, band: (usize, usize)) {
        if band.1 <= band.0 {
            return;
        }
        if self.band.1 <= self.band.0 {
            self.band = band;
        } else {
            self.band = (self.band.0.min(band.0), self.band.1.max(band.1));
        }
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
            Some((*c.draw_fb(), c.fg, c.bg, c.scale, c.aa, ops, band, routed))
        });
        let Some((surf, fg, bg, scale, aa, ops, band, routed)) = plan else {
            if !self.decided {
                self.decided = true;
                self.split = false;
            } else if self.split {
                // SERWIT-2 — THE MID-LINE TEAR. The FIRST chunk of this line won the lock and painted;
                // a LATER one lost it, so the panel now shows a line that stops in the middle with no
                // indication that it does. `_print`'s fallback to `print_masked` cannot rescue this
                // one: that fallback is sound only while the route is undecided (nothing painted yet),
                // and re-running the whole line here would DUPLICATE the part already on the glass.
                //
                // So the tail is legitimately lost, and — like every other loss in this arc — it is
                // counted and announced rather than left to look like a short line.
                crate::serial_ring::TAP_FBCON.tear();
                PANEL_UNANNOUNCED.fetch_add(1, Ordering::Relaxed);
            }
            return;
        };
        self.decided = true;
        self.split = true;
        // UNMASKED, UNLOCKED: the expensive half.
        paint_ops(&surf, ops.as_slice(), fg, bg, scale, aa);
        if routed {
            // No direct front-buffer write on this path: the surface is cached RAM and `wm` owns
            // the panel.
            //
            // FBCON-DMG — the damage is RECORDED here and declared in `finish`. Presenting per chunk
            // was costing a full composite every 16 bytes; see the type's docs. Nothing is at risk in
            // deferring it: the glyphs are in cached RAM that only the compositor reads, `finish` runs
            // before `_print` returns, and the band accumulated here is exactly what the compositor
            // is then told.
            self.routed = true;
            self.note(band);
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
    ///
    /// FBCON-DMG — and this is where the line's accumulated damage reaches the compositor: once, over
    /// the union of every chunk's band, outside the `FBCON` lock and outside the interrupt mask,
    /// which is `route_present`'s standing contract and is unchanged.
    fn finish(&mut self) -> bool {
        self.flush();
        if self.routed {
            if self.band.1 > self.band.0 {
                route_present_rows(self.band);
            } else {
                // An empty band on a routed line means the bytes were `\r`/control only — no glyph
                // moved, so this line owes no rows of its own.
                //
                // FBCON-PACE: it must still reach the gate. This is the DOMINANT routed print path,
                // and returning here without asking would mean a run of control-only lines gave a
                // band an earlier line left held no chance to retire — compounding staleness on the
                // one path that prints most. `route_present_pending` adds nothing and presents only
                // what is already owed and due, which is what `print_masked` and `milestone` do with
                // the same case.
                route_present_pending();
            }
            self.band = (0, 0);
            self.routed = false;
        }
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
        let mut band = None;
        crate::arch::without_interrupts(|| {
            if let Some(mut c) = FBCON.try_lock() {
                if c.ready {
                    let _ = core::fmt::Write::write_fmt(
                        &mut Sink { con: &mut c },
                        format_args!(":: [{:>8} ms] {} ::\n", ms, tag),
                    );
                    band = c.flush_dirty();
                }
            }
        });
        // CONSOLE-WINDOW: same contract as `print_masked` — present outside the lock and the mask.
        // FBCON-DMG: a milestone is one line, so it damages one cell row like any other.
        // FBCON-PACE: paced like any other line. Milestones are seconds apart on any real boot, so
        // the gate is due for every one of them in practice; the `None` arm presents nothing rather
        // than the whole box, for the reason `print_masked` gives.
        match band {
            Some(b) => route_present_rows(b),
            None => route_present_pending(),
        }
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
///
/// FBCON-PACE — THE SYNC POINT ON THE BOOTS THAT REACH IT. This is the last moment a routed console
/// line can reach glass: from the store below onwards `_print` returns at its first test and no
/// further print will ever ask for a present. Anything the pacing gate is still holding is flushed
/// FIRST, so the guarantee the gate makes ("a held band is carried by the next present") has a next
/// present to be carried by even when the printing simply stops.
///
/// It is NOT the bench lane's backstop and must not be mistaken for one: the metal `usbdebug` view
/// keeps fbcon attached and never calls this at all. That lane is served per pass by
/// [`console_service`]. No-op unless the console is routed into a window, so aarch64 and every x86
/// build without `wc` reach the store exactly as before.
pub fn detach() {
    console_flush();
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
///   2. re-homes the glyph cell on the anti-aliased face (8x8 font8x8 is ~0.8 mm on this panel — invisible, per
///      metal sitting #30) and recomputes the column/row grid;
///   3. clears the panel (removing the calibration pattern) and homes the cursor;
///   4. flips `PANEL_CONSOLE`, so every subsequent `serial_println!` mirrors onto the panel;
///   5. replays the boot-milestone ring so the panel carries the boot's history, not just whatever
///      happens to print next.
///
/// Returns the number of text rows painted by the replay. Gated to the Kepler-takeover call site,
/// so no other boot path changes behaviour.
///
/// ### SECOND-IGNITION guard (M-a)
///
/// Steps 1 and 3 above write `c.full_fb()` — the PANEL handle — unconditionally. Once the console is
/// a compositor window its glyphs belong in `c.win_fb`, and a second resume would clear the glass and
/// re-home the grid underneath a live compositor. The `wcx` activation latch cannot reach that: it
/// refuses the second ACTIVATION, and this function runs BEFORE the activation, from its own call
/// site. So the skip lives here. On the Kepler path `CONSOLE_WIN` is still `wm::WIN_NONE` when this
/// runs (resume, then activate — that is the seam's ordering law), so this guard cannot fire on any
/// boot that exists today and the Kepler wire is unchanged.
#[cfg(target_arch = "x86_64")]
pub fn panel_console_resume() -> usize {
    #[cfg(feature = "wc")]
    if CONSOLE_WIN.load(Ordering::Relaxed) != wm::WIN_NONE {
        serial_println!(":: fbcon: glyphs-active SKIP console-already-windowed ::");
        return 0;
    }
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
            // (2) legible cell. FONT (GR27): the legibility knob used to be `PANEL_SCALE`
            // magnifying the 1-bit 8x8 cell; the panel console now arms the shared anti-aliased
            // face instead — the same 16 px cell height `PANEL_SCALE = 2` produced, with the
            // face's own 7 px advance (more columns, same rows) and alpha edges against `bg`.
            // Legibility at this size is the SAME open bench judgement the 16x16 cell carried;
            // reversing it is `video::font::SIZE` (one constant) rather than this scale.
            c.scale = 1;
            c.aa = true;
            c.cell_w = crate::video::font::CELL_W;
            c.cell_h = crate::video::font::CELL_H;
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
        ":: fbcon: glyphs-active base={:08X} pitch={} cell={}x{} cols={} rows={} face=noto16-aa ::",
        base, pitch, cell.0, cell.1, grid.0, grid.1
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
///  1. start at 7/8 of the panel in each axis, less BOTH chromes `ui_status` owns — the bottom
///     instrument band (PULSE-2) and, MENUFIT, the top furniture band (SHELLDESK's
///     `top_chrome_h`). The tiler's reservations do not apply to a pinned window, so they are
///     honoured here or not at all, and after SHELLDESK enabled the menu bar by default the top one
///     was not: the frozen boot-log window's first rows composited under the bar;
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
    // MENUFIT — the work area's HEIGHT: panel less the top furniture, less the bottom instrument.
    let avail_h = ph
        .saturating_sub(crate::ui_status::top_chrome_h(pw, ph))
        .saturating_sub(crate::ui_status::chrome_h(ph))
        .max(1);
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
/// magnify), so a surface pixel is a panel pixel and the console's own `PANEL_SCALE` cell is the
/// whole of the magnification, exactly as before.
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
    // MENUFIT — centred in the WORK AREA, not on the panel: the same two reservations
    // `win_content_extent` sized against, translated so the offset is taken below the top furniture.
    // At `top_chrome_h == 0` (aarch64, every x86 build without `wc`, and any boot whose shell has not
    // enabled a bar) this is the pre-MENUFIT expression unchanged.
    let wtop = crate::ui_status::top_chrome_h(pw, ph);
    let ox = pw.saturating_sub(ow) / 2;
    let oy = wtop
        + ph.saturating_sub(wtop)
            .saturating_sub(crate::ui_status::chrome_h(ph))
            .saturating_sub(oh)
            / 2;
    // CLICK-X86 — owner [`wm::KERNEL_OWNER_CONSOLE`]: the console still belongs to the KERNEL, and
    // both properties owner 0 was chosen for hold unchanged — it is out of `focus_ring` (which skips
    // the reserved band) and out of `close_owner`'s reach (which refuses it). What changes is the
    // third, unargued consequence of owner 0: `hit_test` skipped it, so the largest object on the
    // panel — the console the operator types into — resolved to `None` for every pointer press and
    // could not be clicked at all. A kernel row is furniture, and furniture is clickable.
    let id = wm::create_at(
        wm::KERNEL_OWNER_CONSOLE,
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

/// NORMALWIN — **the console window was CLOSED by the operator: stop routing presents at it.**
///
/// Peter's 2026-08-11 ruling makes the console window a normal app window with all three normal
/// buttons, so the close disc is live and this is the console side of it. Called from x86's
/// `wc_close_furniture` BEFORE `wm::close(id)` frees the row, so no present can be aimed at a slot
/// that is mid-free.
///
/// ### What is dropped, and what deliberately is NOT
/// Only [`CONSOLE_WIN`] is cleared, which is the single test every `route_present*` path takes
/// first — after this they all return without touching `wm`. The glyph ROUTE stays installed:
/// `c.win_fb` / `c.win_store` are left alone on purpose, so console text keeps landing in the
/// cached-RAM surface. Tearing the route down instead would make [`FbCon::draw_fb`] fall back to the
/// PANEL handle, and the panel belongs to the compositor — every subsequent console line would paint
/// over the desktop. Writing into an unwatched store is the harmless direction of that choice; the
/// text is on serial and in `TERM_RING` regardless, which is where the boot log actually lives.
///
/// The surface allocation therefore outlives the window, which is also what keeps this free of a
/// use-after-free: `wm::close` explicitly does not touch the surface, and `store` is owned by
/// `FbCon`, not by the table row.
///
/// Idempotent and id-checked: returns `true` only if `id` was in fact the routed console window, so
/// a stale or foreign id cannot silently unroute the live console. The panic path clears the same
/// cell independently ([`panic_screen`]) and is unaffected either way.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn panel_console_window_closed(id: wm::WinId) -> bool {
    if id == wm::WIN_NONE {
        return false;
    }
    CONSOLE_WIN
        .compare_exchange(id, wm::WIN_NONE, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
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
