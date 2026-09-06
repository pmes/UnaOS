// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PULSEWIN — **the core-load instrument, in a window, with both of its faces.**
//!
//! ## The two views this window switches between, and where each one came from
//!
//! The machine has had two pulse instruments for a while, on two arches, and neither could see the
//! other:
//!
//! * **The x86 one is WINDOWED and is a ring-3 program.** `crates/user-pulse` is a 128x128 EL0/ring-3
//!   ELF that asks for a compositor window (`SYS_WIN_CREATE`), paints a ten-segment bar per core into
//!   its own surface and presents it (`SYS_WIN_PRESENT`). Peter: *"x86 has it in one but had redone the
//!   app"* — the redone app is that ten-segment face. It **cannot run on the Pi**: the syscall it takes
//!   its sample through, `SYS_CPUPULSE` (49), is dispatched only by the x86 kernel (the `una-abi`
//!   divergence ledger records this as D2), so on aarch64 `PULSE.ELF` refuses its first sample and
//!   exits. A port of the SYSCALL is not what this arc is for; a port of the FACE is.
//!
//! * **The Pi one is the LED lamp band, and it is not a window at all.** `ui_status::draw_panel` draws
//!   one row per core of individually-lit lamps — vertical lens gradient per lamp, green→amber→red by
//!   position on the scale, fill measured as a lit LENGTH so the meter moves continuously instead of
//!   clicking between whole segments — straight into the desktop back buffer, in rows the tiler
//!   reserves (`ui_status::chrome_h`) so no window is ever laid out over them. Peter: *"Pi's is cool so
//!   do not throw it out"*. **Nothing here throws it out**: the desktop band is untouched, still drawn
//!   by the same render pass, still reserved by the same budget. This module gives it a second seat.
//!
//! ## What this module is
//!
//! One kernel-owned compositor window — an ordinary row in `wm`'s table, so it drags by its title bar,
//! minimises to the dock and closes by its close disc like any app window — whose CONTENT is a menu
//! strip and, below it, whichever of the two faces is selected:
//!
//! * [`View::Lamps`] is drawn by [`crate::ui_status::draw_panel_at`], which IS `ui_status::draw_panel`'s
//!   own body against an arbitrary rect. **One LED renderer, reached from two seams** — the desktop band
//!   and this window — rather than two that can drift apart. It is also fed by the same sampler: the
//!   window never reads telemetry, it reads [`crate::ui_status::loads`], the envelope `ui_status::tick`
//!   already published for the band. So the two faces of the same machine can never disagree about a
//!   number, which is the entire reason the accessor exists instead of a second sampler here.
//!
//! * [`View::Segments`] is the x86 app's face, ported to ring 0 against that same feed: ten segments per
//!   core, dim track, proportional fill with the leading segment blended, a swept "breath" block for an
//!   idle-but-scheduled core and a dashed track for a parked one. The three shared colours come from
//!   `ui_status` (`METER_DIM`/`METER_BREATH`/`METER_PARKED`); the two the demo family owns are copied
//!   with attribution below, exactly as `user-pulse` copied them.
//!
//! ## The menu — ⚠ IT IS NOT IN THIS WINDOW ANY MORE (R21, Peter, 2026-09-06)
//!
//! **What this header used to say, and it is recorded rather than deleted because the PREMISE was
//! right and the CONCLUSION was wrong:** *"This kernel has exactly one menu framework — `crystal`'s
//! SHARD dropdown — and it is hard-wired to the menu bar's brand mark … It is not a per-window menu
//! and generalising it into one is a different arc from this one. So the pulse window carries its own
//! menu strip as the first row of its own content."*
//!
//! Peter, on seeing that row on glass: *"WHO PUT THE GOD DAMN MENU IN THE WINDOW IT GOES IN THE -----
//! GOD DAMN MENU BAR"*. The premise was a true statement about the code; the conclusion drew the wrong
//! thing from it. One menu framework hard-wired to one publisher is an argument for giving the
//! framework a SECOND publisher, not for building a private one inside a window. That generalisation
//! is [`super::winmenu`], and this window is its first client:
//!
//! * On [`open`] it PUBLISHES a `&'static` tree — one title, `View`, two items, the Pi's lamps first
//!   (Peter: *"the first menu option to switch between the 2 views"*, and the first option is the view
//!   the window opens on) with a mark against whichever is live. On [`close`] it CLEARS it.
//! * The bar draws that title right of the caption slot; the dropdown opens under it, through the
//!   SHARD dropdown's own paint discipline and its own row metrics.
//! * A pick arrives back here as [`on_menu_pick`], which sets the view and re-publishes the other
//!   `const` tree — which is how the check mark moves without a single byte of allocation.
//!
//! **The window's first content row is back with the instrument.** The surface height is unchanged
//! this arc (`content_extent` still budgets [`menu_h`]), so the meter simply gains the row the strip
//! used to occupy; reclaiming the pixels is a geometry change and belongs to whoever next has a
//! reason to move this box.
//!
//! ## Why the press arm is where it is
//!
//! [`press_route`] is called from the aarch64 click router's PI-DESK furniture line, ahead of every
//! window arm, and it is the reason that line reads as an `||`. It cannot steal a click from anything:
//! it re-asks `wm::hit_test` and declines unless the TOPMOST window at that point is this one, so a
//! window stacked over the pulse window keeps its own presses. Since R21 it claims exactly ONE region
//! — this window's close disc — and answers `false` everywhere else, so chrome drags, minimise, zoom
//! and focus all still reach the arms below it untouched. The menu's press is `winmenu`'s, in the bar,
//! through the ONE shared furniture router.
//!
//! **And it does not PAINT.** A press flips one atomic and returns; the repaint is [`service`]'s, on the
//! render core, one paced call per pulse period. That split is not tidiness — it is the ledger at the
//! tail of `desktop_firmware::activate` applied before the fact. The Pi's live routed console was MEASURED turning
//! a 108/108 bench-geometry run into 97/108 with a synchronous exception, and the diagnosis was not "who
//! writes the panel" but *who drives the COMPOSITOR*: a surface that presents from arbitrary call
//! context is an unsynchronised compositor client. The cost is that a face switch appears on the next
//! tick rather than instantly — bounded by `ui_status::PSTRIP_PERIOD_MS`, 250 ms. (The BAR's half of
//! the gesture is not paced that way and must not be: `winmenu` drives its own composite, exactly as
//! `crystal` does, because on a static desktop no other pass is coming.)
//!
//! ## Gating
//!
//! `any(x86 + wc, aarch64 + desktop_firmware)` — the furniture family's gate, and for the furniture family's
//! reason: this is EXPERIENCE-layer code with no hardware in it, so it compiles on every chip and is
//! turned on by a knob rather than by an arch. Only `desktop_firmware::activate` ARMS the window today; on x86 the
//! module compiles, type-checks and is unreferenced, which is what keeps the port from rotting.

use super::{winmenu, wm};
use crate::pal::GneissPal;
use crate::ui_status::{
    self, METER_BREATH, METER_DIM, METER_PARKED, PARKED, PERMILLE_FULL, PSTRIP_MAX_CPUS,
};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

// ------------------------------------------------------------------------------------------------
// Palette
// ------------------------------------------------------------------------------------------------

/// The window's content ground. A shade off the desktop band's `PANEL_BG` on purpose: the window is a
/// second seat for the instrument, not a cut-out of the first, and an operator with both on the glass
/// should be able to say which is which without reading a title.
const WIN_BG: u32 = 0x00_100E16;

/// `vug::METER_PURPLE` — the load fill of the ten-segment face. COPIED, with attribution, exactly as
/// `user-pulse/src/main.rs` copies it: `vug` is aarch64-only and declares it privately, so importing it
/// would arch-gate this module for a colour. The three colours that ARE shared (`METER_DIM`,
/// `METER_BREATH`, `METER_PARKED`) are imported from `ui_status`, which owns them.
const METER_PURPLE: u32 = 0x00_9B59B6;
/// `vug::METER_LABEL` — core numbers and percents. Copied on the same terms.
const METER_LABEL: u32 = 0x00_8A8296;

/// Fixed segment count of a ten-segment bar — `vug::PULSE_SEGS` / `user-pulse`'s `PULSE_SEGS`. The
/// number IS the x86 face; it is not a tuning knob.
const PULSE_SEGS: usize = 10;

// ------------------------------------------------------------------------------------------------
// The view, and the menu that switches it
// ------------------------------------------------------------------------------------------------

/// Which face the window is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The Pi's LED lamp band, drawn by `ui_status`'s own renderer. **The default** — Peter's is the
    /// one the machine already had, so it is the one the window opens on.
    Lamps,
    /// The x86 app's ten-segment face, ported.
    Segments,
}

impl View {
    /// The option's label in the menu. Both name the ARCH they came from, because that is the only
    /// thing about them an operator has to remember.
    pub const fn label(self) -> &'static str {
        match self {
            View::Lamps => "Pi LED lamps",
            View::Segments => "x86 segments",
        }
    }
    /// The stable ordinal for the witness, independent of the row order.
    const fn ord(self) -> u8 {
        match self {
            View::Lamps => 0,
            View::Segments => 1,
        }
    }
}

/// **The menu tree, in the MENU BAR** — R21. Two `const` trees, one per live view, and the ONLY
/// difference between them is which item carries [`winmenu::FLAG_CHECKED`].
///
/// Two trees rather than one mutable tree is what keeps every published tree `&'static`: a face switch
/// hands the registry the OTHER `const`, so the mark moves with no allocation, no interior mutability,
/// and nothing on the paint path that a second core could be halfway through writing.
///
/// Order is the arc's and is load-bearing: Peter asked for *"the first menu option to switch between
/// the 2 views"*, and the FIRST item is the Pi's — the view the window opens on, so item one is always
/// the one you are looking at and item two is always the switch.
const VIEW_ITEMS_LAMPS: [winmenu::MenuItem; 2] = [
    winmenu::MenuItem { id: 0, label: "Pi LED lamps", flags: winmenu::FLAG_CHECKED },
    winmenu::MenuItem { id: 1, label: "x86 segments", flags: 0 },
];
const VIEW_ITEMS_SEGS: [winmenu::MenuItem; 2] = [
    winmenu::MenuItem { id: 0, label: "Pi LED lamps", flags: 0 },
    winmenu::MenuItem { id: 1, label: "x86 segments", flags: winmenu::FLAG_CHECKED },
];
const TREE_LAMPS: [winmenu::MenuTitle; 1] =
    [winmenu::MenuTitle { label: "View", items: &VIEW_ITEMS_LAMPS }];
const TREE_SEGS: [winmenu::MenuTitle; 1] =
    [winmenu::MenuTitle { label: "View", items: &VIEW_ITEMS_SEGS }];

/// The tree that shows `v` as the live view.
const fn tree_for(v: View) -> &'static [winmenu::MenuTitle] {
    match v {
        View::Lamps => &TREE_LAMPS,
        View::Segments => &TREE_SEGS,
    }
}

/// The live view. `u8`, holding a [`View::ord`], so the whole of the modal state is one relaxed load on
/// the paint path.
static VIEW: AtomicU32 = AtomicU32::new(0);

/// Has a desktop asked for this window? Set by [`arm`], consumed by [`service`]'s open arm, and
/// CLEARED by [`close`] — A30: an open arm that outlives the operator's close is not a close.
static ARMED: AtomicBool = AtomicBool::new(false); static EVER_ARMED: AtomicBool = AtomicBool::new(false); // A30 — the STICKY half of the latch: set by `arm()`, never cleared, and the ONLY question `dock::pin_pulse` asks. It exists so the pinned reopen tile is a no-op on every board that has no pulse window to reopen: `desktop_firmware::activate` is the sole caller of `arm()`, so an x86 `desktop_uefi` desktop leaves this `false` forever and the dock's tile model there is byte-for-byte what it was. ⚠ FOLDED onto this line rather than added below it — knob-off line numbers are load-bearing (panic `Location`), PARITY.md §5.3.

/// Falsifiable counter for the witness: how many picks actually MOVED the view (a pick of the live
/// view is a dismissal, not a switch).
///
/// R21 — the `opens` counter that stood beside it is GONE rather than left reading zero. The menu is
/// no longer this window's, so "how many times was it opened" is a question `[winmenu]`'s ledger
/// answers and this one cannot; a field that could only ever print `0` is an instrument asserting
/// that a working menu was never opened.
static SWITCHES: AtomicU64 = AtomicU64::new(0);

/// The live view, as a value.
pub fn view() -> View {
    if VIEW.load(Ordering::Relaxed) == View::Segments.ord() as u32 {
        View::Segments
    } else {
        View::Lamps
    }
}

// ------------------------------------------------------------------------------------------------
// The window and its surface
// ------------------------------------------------------------------------------------------------

/// The window id, or [`wm::WIN_NONE`] while there is no window.
static WIN: AtomicU32 = AtomicU32::new(wm::WIN_NONE);
/// Kernel-visible address of the ARGB8888 surface, its byte length, and its extent in source pixels.
static SURF: AtomicUsize = AtomicUsize::new(0);
static SURF_LEN: AtomicUsize = AtomicUsize::new(0);
static SURF_W: AtomicUsize = AtomicUsize::new(0);
static SURF_H: AtomicUsize = AtomicUsize::new(0);

/// The surface's backing store. Held here for its LIFETIME, not for access: every draw goes through
/// [`SURF`], and this exists so the allocation outlives the window and is freed exactly when the window
/// closes. `Vec` at its final size and never grown — the `attach_shadow` / `Screen` back-store idiom
/// `fbcon::panel_console_window_open` uses for the same reason.
static STORE: Mutex<Option<Vec<u8>>> = Mutex::new(None);

/// The signature of the last painted frame — view, menu state, core count and every displayed load.
/// [`service`] repaints only when it moves, so a quiet machine costs one accessor call and one FNV
/// fold per pulse period and puts nothing on the panel. The DEFAULT-QUIET LAW, on this window.
static PAINTED_SIG: AtomicU64 = AtomicU64::new(0);

/// The window's id, or [`wm::WIN_NONE`]. The one read every other module needs.
pub fn win() -> wm::WinId {
    WIN.load(Ordering::Relaxed)
}

/// Kernel FURNITURE, and deliberately neither [`wm::KERNEL_OWNER_CONSOLE`] nor
/// [`wm::KERNEL_OWNER_DESKTOP`]: it must satisfy `wm::is_kernel_owner` (so the click router hands the
/// keyboard to the shell rather than to a ring that does not exist, and so an ASID-scoped close sweep
/// can never reap it by accident) while being its OWN owner, so `close_owner` on either of the other
/// two cannot take this window with it.
pub const OWNER: u64 = wm::KERNEL_OWNER_BASE + 0x60; // A30 — published (was private) so `dock::pin_pulse`'s reopen tile carries THIS window's owner rather than borrowing the desktop's, which is the whole reason the owner is its own: a pin under `KERNEL_OWNER_DESKTOP` would make `pin_shell`'s "one live shell max" test read a pulse tile as a shell. Line-neutral: a visibility keyword, no line added — PARITY.md §5.3.

// ------------------------------------------------------------------------------------------------
// Geometry — all derived from the panel and the metrics, none guessed
// ------------------------------------------------------------------------------------------------

/// The window's content extent for a `pw` x `ph` panel, or `None` when the panel cannot seat it.
///
/// Width is two thirds of the panel: the instrument is a BAR, and every pixel of width is another lamp
/// of sensitivity (the same argument `ui_status::row_geometry` records for spanning the desktop).
///
/// Height is computed from the CORE COUNT rather than from the panel, and that is the whole of the
/// sizing rule worth arguing about. `ui_status`'s row layout derives its LED pitch from the row height
/// (`led_pitch_for` = half of it), so a band made tall for its own sake produces a handful of enormous
/// lamps instead of a meter: at four cores in a 620 px band the bench panel would draw fifteen lamps
/// 115 px tall. Sizing the content to `ncpu` rows of three glyph cells each reproduces the desktop
/// band's proportions — ~20 px rows, ~100 lamps on the bench, ~27 on the 640x480 gate surface — in a
/// window a third of the panel's height.
fn content_extent(pw: usize, ph: usize) -> Option<(usize, usize)> {
    // THE METRICS RULE, on the right surface. Every renderer below is handed `SurfacePal::metrics()`,
    // which is a function of the WINDOW's height, not the panel's — so the layout must be computed from
    // the same one or a 1800-row panel would size the box at scale 2 and then paint it at scale 1. The
    // recursion (`ch` needs the metric, the metric needs `ch`) has a fixed point of scale 1 because
    // `ch` is a menu line plus `ncpu` three-cell rows and cannot approach `ui::SCALE_STEP`; the guard
    // below turns "cannot" into a decline rather than a comment.
    let m = crate::ui::Metrics::for_height(1);
    let ncpu = PSTRIP_MAX_CPUS.min(crate::arch::sched::meter_cpu_count()).max(1);
    let work_h = ph
        .saturating_sub(ui_status::top_chrome_h(pw, ph))
        .saturating_sub(ui_status::chrome_h(ph));
    let cw = (pw * 2 / 3).min(pw.saturating_sub(2 * wm::BORDER));
    let ch = menu_h(&m) + 2 * pad(&m) + ncpu * row_target(&m);
    // The OUTER box has to fit the work area, chrome included, or the tiler would seat a window the
    // operator cannot reach the controls of. Decline rather than squeeze — the strip constructors' rule.
    let (ow, oh) = (cw + 2 * wm::BORDER, ch + wm::TITLE_H + 2 * wm::BORDER);
    if cw < 2 * FLOOR_CELLS * m.cell_w || ow > pw || oh > work_h || ch >= crate::ui::SCALE_STEP {
        return None;
    }
    Some((cw, ch))
}

/// The narrowest content this window will accept, in glyph cells per half: a face needs a `cN` label, a
/// bar worth calling a bar and a verdict cell, and below this it would be a strip pretending to be an
/// instrument. `ui_status::row_geometry` declines on the same principle one level down (`nled < 8`).
const FLOOR_CELLS: usize = 20;

/// The gap the content is inset by — half a line, the kit's own padding, matching the desktop band.
fn pad(m: &crate::ui::Metrics) -> usize {
    (m.line_h / 2).max(2)
}

/// R21 — **the row the in-window menu strip used to occupy.** One line pitch.
///
/// It is still in [`content_extent`]'s budget, so the window's outer box and its `surf=` witness are
/// byte-for-byte what they were before the strip was deleted, and the operator's window does not
/// change size because a menu moved. What changed is that nothing DRAWS here any more: the row went
/// back to the instrument, which is now given the whole content rect. Reclaiming the pixels is a
/// geometry change and belongs to whoever next has a reason to move this box; it is named here so the
/// row is a known slack rather than a mystery.
fn menu_h(m: &crate::ui::Metrics) -> usize {
    m.line_h
}

/// One core row's target height — three glyph cells, which is what puts the LED pitch (half the row)
/// at the density the desktop band draws at. See [`content_extent`].
fn row_target(m: &crate::ui::Metrics) -> usize {
    m.cell_h * 3
}

// ------------------------------------------------------------------------------------------------
// The surface pal — a `GneissPal` over the window's own ARGB8888 store
// ------------------------------------------------------------------------------------------------

/// A palette over the window's surface, so every existing UI renderer — `ui_status`'s LED bar, the
/// 8x8 font, the rect fills — draws into this window with no second implementation of anything.
///
/// Pixels are stored as the little-endian word `0x00RRGGBB`, which is exactly the ARGB8888 pixel
/// `wm::draw_window` reads back out. Same bytes, not a conversion.
struct SurfacePal {
    base: usize,
    w: usize,
    h: usize,
}

impl GneissPal for SurfacePal {
    fn draw_pixel(&mut self, x: u32, y: u32, color: u32) {
        let (x, y) = (x as usize, y as usize);
        if x >= self.w || y >= self.h {
            return; // clipped, never wrapped: a stray coordinate must not land on another row
        }
        // SAFETY: `base` is the start of a `self.h * self.w * 4` byte allocation owned by `STORE` and
        // alive for as long as the window is (see `STORE`), and the index is bounds-checked above.
        unsafe {
            core::ptr::write_volatile((self.base as *mut u32).add(y * self.w + x), color);
        }
    }
    fn read_pixel(&self, x: u32, y: u32) -> Option<u32> {
        let (x, y) = (x as usize, y as usize);
        if x >= self.w || y >= self.h {
            return None;
        }
        // SAFETY: as `draw_pixel`. Cached kernel RAM, never the write-only panel.
        Some(unsafe { core::ptr::read_volatile((self.base as *const u32).add(y * self.w + x)) })
    }
    fn poll_event(&mut self) -> crate::pal::Event {
        crate::pal::Event::None // the window has no event source of its own; presses arrive routed
    }
    fn render(&mut self) {} // the present is `wm`'s and is issued explicitly by `paint`
    fn width(&self) -> u32 {
        self.w as u32
    }
    fn height(&self) -> u32 {
        self.h as u32
    }
}

/// The live surface pal, or `None` when there is no window.
fn pal() -> Option<SurfacePal> {
    let base = SURF.load(Ordering::Acquire);
    let (w, h) = (SURF_W.load(Ordering::Relaxed), SURF_H.load(Ordering::Relaxed));
    if base == 0 || w == 0 || h == 0 {
        return None;
    }
    Some(SurfacePal { base, w, h })
}

// ------------------------------------------------------------------------------------------------
// Open / close
// ------------------------------------------------------------------------------------------------

/// **Open the pulse window.** Returns its id, or [`wm::WIN_NONE`] on any decline. Idempotent — a second
/// call hands back the existing window rather than minting a second one.
///
/// Every decline is NAMED on the wire and none of them is fatal to the caller: a desktop without a
/// pulse window is a perfectly good desktop, exactly as `desktop_firmware` argues for the console window.
pub fn open() -> wm::WinId {
    let existing = WIN.load(Ordering::Relaxed);
    if existing != wm::WIN_NONE {
        return existing;
    }
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            serial_println!("[pulsewin] open DECLINE reason=no-panel");
            return wm::WIN_NONE;
        }
        let i = fb.info();
        (i.width, i.height)
    };
    let (cw, ch) = match content_extent(pw, ph) {
        Some(e) => e,
        None => {
            serial_println!("[pulsewin] open DECLINE reason=panel-cannot-seat panel={}x{}", pw, ph);
            return wm::WIN_NONE;
        }
    };
    let stride = cw * 4;
    let len = ch * stride;

    // Allocated before any lock is taken and outside every interrupt mask: this is a heap request of
    // hundreds of kilobytes and the allocator is not something to enter with interrupts off.
    let mut store: Vec<u8> = Vec::new();
    if store.try_reserve_exact(len).is_err() {
        serial_println!("[pulsewin] open DECLINE reason=alloc len={}", len);
        return wm::WIN_NONE;
    }
    store.resize(len, 0);
    let base = store.as_mut_ptr() as usize;

    // SPAWN-PLACE — the outer box is centred in the WORK AREA and the row is created THERE, pinned, so
    // no pixel of this window is ever presented at a position it will not occupy. `fbcon`'s console
    // window learned this on the metal; there is no reason to relearn it.
    let (_scale, ow, oh) = match wm::spawn_geometry(cw, ch) {
        Some(g) => g,
        None => {
            serial_println!("[pulsewin] open DECLINE reason=geometry-unavailable");
            return wm::WIN_NONE;
        }
    };
    // WHERE IT OPENS — bottom-left of the work area, one gap in, flush above the reserved rows.
    //
    // NOT centred, and the reason is the instrument rather than the layout: this window is a SECOND
    // SEAT for the desktop LED band, and the band is the last `ui_status::chrome_h(ph)` rows of the
    // panel. Opening the window directly above its own twin puts the two faces of one number in the
    // same glance, which is the comparison an operator opens it to make; centring would have put the
    // copy as far from the original as the panel allows.
    //
    // STATED PLAINLY because it is also load-bearing for a gate: `wm::hittest_selftest` probes
    // `(width/3, height/4 + TITLE_H + BORDER)` — upper-middle — and asserts that with its own rows
    // pushed below the shell that point resolves to NOTHING (`hidden`), and that a press there is a
    // MISS (`bare`). A kernel row is hittable and is not pushed below the shell, so a centred pulse
    // window sat on that point and failed both legs; the same collision the rMBP bench already sees
    // from the console window, and which that fixture's own header documents. Moving to the bottom
    // left clears the probe box, and it does NOT resolve the underlying conflict — a witness battery
    // written against a Pi desktop with no furniture rows still has to be reconciled with one that has
    // them, and that reconciliation is the integrator's. See engine.md §PULSEWIN.
    let wtop = ui_status::top_chrome_h(pw, ph);
    let gap = wm::BORDER * 2;
    let ox = gap.min(pw.saturating_sub(ow));
    let oy = ph
        .saturating_sub(ui_status::chrome_h(ph))
        .saturating_sub(gap)
        .saturating_sub(oh)
        .max(wtop);

    SURF.store(base, Ordering::Release);
    SURF_LEN.store(len, Ordering::Relaxed);
    SURF_W.store(cw, Ordering::Relaxed);
    SURF_H.store(ch, Ordering::Relaxed);
    let id = wm::create_at(
        OWNER,
        base,
        len,
        cw as u32,
        ch as u32,
        stride as u32,
        b"pulse",
        ox + wm::BORDER,
        oy + wm::TITLE_H + wm::BORDER,
    );
    if id == wm::WIN_NONE {
        SURF.store(0, Ordering::Release);
        serial_println!("[pulsewin] open DECLINE reason=create-failed");
        return wm::WIN_NONE;
    }
    *STORE.lock() = Some(store);
    WIN.store(id, Ordering::Release); wm::winid_register_holder(&WIN, "pulsewin"); // WINID (SO1(b)) — ⚠ SAME-LINE fold, line-NEUTRAL. `close()` below clears this cell, and `press_route` is the only caller of it — so a close that arrives through the operator's close disc on the ROUTER's furniture arm (`wc_close_furniture` -> `wm::close`) frees the row behind this cell's back and leaves it naming a slot the table is free to re-issue. Registering it makes `wm::close` the backstop on every path.
    // R21 — **publish the `View` menu into the BAR.** After `WIN` is stored, because the registry is
    // keyed by window id and a tree published against `WIN_NONE` is refused; before the witness, so a
    // capture reads `[winmenu] publish` and `[pulsewin] open` in the order they happened. A refused
    // publish is NOT fatal — every refusal names itself on the wire and a pulse window without menus
    // is still a pulse window — which is `open`'s own decline rule applied one level down.
    winmenu::publish(id, tree_for(view()), on_menu_pick);
    serial_println!(
        "[pulsewin] open win={} panel={}x{} surf={}x{} box={}x{} at ({},{}) view={} \
         (menu: `View` is in the MENU BAR — first option is the Pi's)",
        id,
        pw,
        ph,
        cw,
        ch,
        ow,
        oh,
        ox,
        oy,
        view().label()
    );
    paint();
    id
}

/// **Close the pulse window** and free its surface. Returns `true` if a window went. Idempotent.
///
/// The close is by ID, not by owner, and that is deliberate: `wm::close_owner` REFUSES kernel owners
/// (an ASID sweep must never be able to reap furniture), so a kernel row that wants to be closable has
/// to close itself. This is the only place that happens for this window, and it is reached from the
/// close disc the compositor already draws on the title bar — no second control, no second rule.
pub fn close() -> bool {
    let id = WIN.swap(wm::WIN_NONE, Ordering::AcqRel);
    if id == wm::WIN_NONE {
        return false;
    }
    // R21 — the MENU goes FIRST, before the row: `winmenu::clear` dismisses this window's dropdown if
    // it is down and drives the composite that erases it, and that erase must happen while the window
    // is still a legible member of the table. Clearing after `wm::close` would leave one pass in which
    // the bar is laying out a title for a window that no longer exists.
    winmenu::clear(id);
    // Order matters: the row goes first, so the compositor can no longer read the surface, and only
    // then is the store dropped. The reverse would leave one composite pass reading freed memory.
    wm::close(id);
    ARMED.store(false, Ordering::Release); SURF.store(0, Ordering::Release); // A30 — DISARM FIRST, and disarm HERE. `service`'s open arm fires on `ARMED && ncpu > 0` every pass, so a close that leaves the latch set is not a close: render7 shut this window twice from its own close disc and the very next render pass re-opened it five lines later (6923->6932, 10519->10528), which is also why A18's cascade census read `pulsewin_open=3` for ONE window. The latch means "the desktop wants this window", and a user close is the operator saying it does not; only `arm()` says it does again, and after this the sole caller that can say so post-boot is the dock's pinned tile. Store `false` before the surface teardown so no pass can observe a window-less ARMED state. ⚠ FOLDED onto this line rather than added below it: knob-off line numbers are load-bearing (panic `Location`) — PARITY.md §5.3.
    SURF_W.store(0, Ordering::Relaxed);
    SURF_H.store(0, Ordering::Relaxed);
    *STORE.lock() = None;
    serial_println!(
        "[pulsewin] close win={} -> CLOSED (reopen only via dock) switches={} (surface freed; menu cleared from the bar; the desktop LED band is untouched; A30 — the ARMED latch is cleared, so no render pass re-opens this window)",
        id,
        SWITCHES.load(Ordering::Relaxed)
    );
    true
}

// ------------------------------------------------------------------------------------------------
// Painting
// ------------------------------------------------------------------------------------------------

/// The signature of what a frame WOULD show: the view, the menu state, the core count and every
/// displayed load. Folded with `strip::fnv1a_u64`, the same hash the furniture's damage slots use.
fn frame_sig(loads: &[u32; PSTRIP_MAX_CPUS], ncpu: usize) -> u64 {
    let mut h = super::strip::FNV_BASIS;
    h = super::strip::fnv1a_u64(h, view().ord() as u64);
    // R21 — the menu's open state is NOT folded in any more, and that is the correct reading rather
    // than an omission: the dropdown is a panel surface owned by `winmenu` with its own damage slot,
    // and nothing about it changes a pixel of THIS surface. The view still is folded in, because a
    // pick still changes the face this window draws.
    h = super::strip::fnv1a_u64(h, ncpu as u64);
    for l in loads.iter().take(ncpu) {
        h = super::strip::fnv1a_u64(h, *l as u64);
    }
    super::strip::seal(h)
}

/// **Paint the whole window and present it.** No-op without a window.
///
/// The whole surface, every time, and not a damage rect: the content is one menu strip and `ncpu`
/// rows in a few hundred kilobytes of cached RAM, the compositor's present is the expensive half, and
/// a per-row dirty test here would be a second damage model shadowing `wm`'s for no measurable gain.
/// What IS filtered is how OFTEN this runs — see [`service`].
pub fn paint() {
    let id = WIN.load(Ordering::Acquire);
    if id == wm::WIN_NONE {
        return;
    }
    let Some(mut p) = pal() else { return };
    let (cw, ch) = (p.width() as usize, p.height() as usize);
    let m = p.metrics();
    let mut loads = [PARKED; PSTRIP_MAX_CPUS];
    let ncpu = ui_status::loads(&mut loads);

    p.draw_rect(0, 0, cw, ch, WIN_BG);
    match view() {
        // ONE renderer, two seams: this is the desktop band's own body, given this window's rect
        // instead of `panel_geometry`'s reserved one.
        //
        // R21 — the rect is the WHOLE content now. It was `(0, menu_h, cw, ch - menu_h)`, the rows
        // below the in-window menu strip; with the strip gone the instrument gets its first row back.
        View::Lamps => ui_status::draw_panel_at(&mut p, Some((0, 0, cw, ch))),
        View::Segments => draw_segments(&mut p, &m, cw, ch, &loads, ncpu),
    }
    PAINTED_SIG.store(frame_sig(&loads, ncpu), Ordering::Relaxed);
    wm::present(id);
}

/// **The paced entry point** — called once per pulse sample from the Pi render pass, immediately after
/// `ui_status::tick` has published this window's numbers. Repaints only when the frame signature moves.
///
/// It reads the envelope rather than sampling: `ui_status::tick` is the ONE sampler on this machine and
/// this window is a second VIEW of it, so a quiet machine costs one accessor call and one hash fold per
/// period here and puts nothing on the panel.
pub fn service() {
    let mut loads = [PARKED; PSTRIP_MAX_CPUS];
    let ncpu = ui_status::loads(&mut loads);
    if WIN.load(Ordering::Acquire) == wm::WIN_NONE {
        // THE OPEN ARM. `desktop_firmware::activate` ARMS this window; the render pass OPENS it, on the first
        // pass where `ui_status` reports a live instrument. Two things are being bought, and both were
        // paid for by a measurement rather than argued into existence:
        //
        // 1. **The window is minted by the core that drives the compositor.** `create_at` composites
        //    the new row before it returns, so opening from the bringup path meant a second core
        //    presenting a window while the render task was still painting the first desktop frame —
        //    the unsynchronised-compositor-client shape CONSWIN-PI's ledger names. The gate caught it
        //    in the act: `[chrome-truth] win=1 box=(10,230,436x164) … keyline_top want=0xb4b4b9
        //    got=0x000000 -> MISS`, five for five, printed THREE LINES BEFORE `[pulsewin] open`
        //    finished its own witness — a readback of the row's chrome taken between the create and
        //    the blit reaching glass. Opened from here there is no second core to race.
        //
        // 2. **No instrument window before the instrument has a reading.** `ui_status::loads` answers
        //    `0` until `tick` has armed, and a monitor that opens as an empty box has told the
        //    operator nothing about the machine and something wrong about itself.
        if ARMED.load(Ordering::Acquire) && ncpu > 0 {
            open();
        }
        return;
    }
    if frame_sig(&loads, ncpu) == PAINTED_SIG.load(Ordering::Relaxed) {
        return;
    }
    paint();
}

/// **Ask for the pulse window.** The desktop seam calls this; [`service`] does the opening. See its
/// open arm for why the two are split. Idempotent, and it does not report a window — there is not one
/// yet, by construction. A30 — post-boot the dock's pinned tile is the only other caller, and that
/// makes this the single re-entry point a user close can be undone through.
pub fn arm() {
    ARMED.store(true, Ordering::Release); EVER_ARMED.store(true, Ordering::Release); // A30 — see EVER_ARMED: sticky, so the dock keeps a reopen tile after a close. Folded, not added — PARITY.md §5.3.
}

/// **The pick sink** — R21. Handed to [`winmenu::publish`] as a bare `fn` pointer and called from the
/// bar's press arm with the item id THIS module chose ([`View::ord`]), never a kernel-assigned one.
///
/// Two things happen and both are one atomic each: the live view moves, and the OTHER `const` tree is
/// re-published so the check mark lands on the item that is now live. Nothing paints here — the window
/// repaints on [`service`]'s next paced pass, which is the same 250 ms bound a pick has always had,
/// and the BAR's dropdown was already torn down by `winmenu` before this was called.
fn on_menu_pick(id: u32) {
    let picked = if id == View::Segments.ord() as u32 { View::Segments } else { View::Lamps };
    let was = view();
    VIEW.store(picked.ord() as u32, Ordering::Release);
    if picked != was {
        SWITCHES.fetch_add(1, Ordering::Relaxed);
    }
    let win = WIN.load(Ordering::Acquire);
    if win != wm::WIN_NONE {
        // Re-publish so the mark follows the live view. A REPLACE, not a second slot — the registry
        // keys on the window id and swaps the tree in place.
        winmenu::publish(win, tree_for(picked), on_menu_pick);
    }
    serial_println!(
        ":: PULSEWIN-MENU: pick id={} view={} was={} switched={} ::",
        id,
        picked.label(),
        was.label(),
        picked != was
    );
}

// ------------------------------------------------------------------------------------------------
// The x86 face — `user-pulse`'s ten-segment bar, ported to ring 0
// ------------------------------------------------------------------------------------------------

/// Linear blend `a`→`b` by `num/den`, per channel. `ui_status::mix`'s rule; that one is private to its
/// module and this is the only arithmetic this face needs that `ui_status` does not export.
fn mix(a: u32, b: u32, num: u32, den: u32) -> u32 {
    let den = den.max(1);
    let num = num.min(den);
    let ch = |sh: u32| {
        let (x, y) = ((a >> sh) & 0xFF, (b >> sh) & 0xFF);
        ((x * (den - num) + y * num) / den) << sh
    };
    ch(16) | ch(8) | ch(0)
}

/// **The x86 app's face.** One row per core: a `cN` label, a ten-segment bar, and a verdict cell.
///
/// The three cases, and their meanings, are `user-pulse::draw_pulse_bar`'s verbatim — they are the same
/// VUG-HONESTY rules the LED face draws, at ten-segment resolution:
///
/// * [`PARKED`] — nothing schedules this core. Alternating segments in `METER_PARKED`, a broken track
///   that can never be read as 0 %.
/// * `0` — scheduled but idle. One segment sweeps (`METER_BREATH`), which is PULSE-ALIVE's answer to
///   Peter's *"pulse shows 1 CPU"*: an idle core must look alive, not absent.
/// * anything else — whole segments in `METER_PURPLE`, with the ONE segment the fill boundary falls
///   inside blended from `METER_DIM` in proportion to how much of it the fill covers. That partial
///   segment is the only thing this face has of the LED face's continuous length, and it is why a
///   ten-segment bar still moves under a load creeping between two tenths.
fn draw_segments<P: GneissPal>(
    p: &mut P,
    m: &crate::ui::Metrics,
    cw: usize,
    ch: usize,
    loads: &[u32; PSTRIP_MAX_CPUS],
    ncpu: usize,
) {
    if ncpu == 0 {
        return;
    }
    let pad = pad(m);
    let top = menu_h(m) + pad;
    let avail_h = ch.saturating_sub(top).saturating_sub(pad);
    let row_h = avail_h / ncpu;
    if row_h < m.cell_h {
        return; // too short to seat a legible row: say nothing rather than draw a smear
    }
    let label_w = m.cell_w * 3;
    let val_w = m.cell_w * 6;
    let bar_x = pad + label_w;
    let bar_w = cw
        .saturating_sub(2 * pad)
        .saturating_sub(label_w)
        .saturating_sub(val_w);
    let seg_pitch = bar_w / PULSE_SEGS;
    if seg_pitch < 2 {
        return;
    }
    let gap = (seg_pitch / 6).max(1);
    let seg_w = seg_pitch - gap;
    let seg_h = row_h.saturating_sub((row_h / 4).max(1)).max(3);
    for c in 0..ncpu {
        let ry = top + c * row_h;
        let ty = ry + (seg_h.saturating_sub(m.cell_h)) / 2;
        p.draw_text(pad, ty, &alloc::format!("c{}", c), METER_LABEL);
        let load = loads[c];
        for s in 0..PULSE_SEGS {
            let x = bar_x + s * seg_pitch;
            let color = if load == PARKED {
                if s % 2 == 0 { METER_PARKED } else { WIN_BG }
            } else if load == 0 {
                // The sweep, on the shared 300 ms phase the LED face breathes at, so the two faces of
                // an idle machine pulse together instead of beating against each other.
                let phase = ((crate::arch::ms() / 300) as usize) % PULSE_SEGS;
                if s == phase { METER_BREATH } else { METER_DIM }
            } else {
                // Fill in thousandths of a segment, so the boundary segment's blend is exact rather
                // than rounded to whole percent.
                let fill = (load.min(PERMILLE_FULL) as usize) * PULSE_SEGS;
                let whole = fill / PERMILLE_FULL as usize;
                if s < whole {
                    METER_PURPLE
                } else if s == whole {
                    mix(
                        METER_DIM,
                        METER_PURPLE,
                        (fill % PERMILLE_FULL as usize) as u32,
                        PERMILLE_FULL,
                    )
                } else {
                    METER_DIM
                }
            };
            p.draw_rect(x, ry, seg_w, seg_h, color);
        }
        let val = if load == PARKED {
            alloc::string::String::from("  park")
        } else if load == 0 {
            alloc::string::String::from("   run")
        } else {
            alloc::format!("{:>4}%", load / 10)
        };
        p.draw_text(bar_x + PULSE_SEGS * seg_pitch + gap, ty, &val, METER_LABEL);
    }
}

// ------------------------------------------------------------------------------------------------
// The press route
// ------------------------------------------------------------------------------------------------

/// **The click arm.** `true` when this window CONSUMED the press.
///
/// Called from the aarch64 router's PI-DESK furniture line, ahead of every window arm. Three properties
/// make that position safe, and all three are checked here rather than assumed by the caller:
///
/// 1. **It declines unless this window is the TOPMOST at that point.** `wm::hit_test` answers with the
///    front-most row, so a window stacked over the pulse window keeps every press inside its own box.
/// 2. **It claims exactly ONE region**: this window's close disc. R21 removed the second — the menu —
///    because the menu is no longer in this window; it is in the bar, and its press is
///    [`super::winmenu::press_at`]'s, reached through the same shared furniture router this arm sits
///    beside. Chrome drags, the minimise and zoom discs, focus changes and every other window's
///    everything fall through to the arms below, untouched.
/// 3. **A press on this window's CONTENT is not consumed** — it falls through to the ordinary select
///    arm, so clicking the instrument raises the window exactly as clicking any other window's content
///    does. Before R21 that was true only while the in-window menu was closed; now it is unconditional,
///    which is one fewer state an operator has to be in to raise their own window.
pub fn press_route(x: i32, y: i32) -> bool {
    let id = WIN.load(Ordering::Acquire);
    if id == wm::WIN_NONE || x < 0 || y < 0 {
        return false;
    }
    match wm::hit_test(x, y) {
        // Not on this window at all (or occluded by one in front of it). Nothing to dismiss here any
        // more — the bar's dropdown has its own dismiss-outside arm, ahead of this one in the router —
        // so the press simply belongs to whoever it landed on.
        Some((top, _, _)) if top == id => {}
        _ => return false,
    }
    // The close disc, claimed here because `wm`'s own close arm routes through `close_owner`, which
    // refuses kernel owners — see [`close`].
    if wm::close_box_hit(id, x, y) {
        serial_println!("[pulsewin] close-box win={} at ({},{})", id, x, y);
        close();
        return true;
    }
    false
}

/// The cost/state rollup, on the furniture's precedent — NOT `witness`-gated, because the metal image
/// is built without `witness` and a claim absent from it is not a claim.
pub fn rollup(scope: &str) {
    serial_println!(
        "[pulsewin] rollup scope={} win={} view={} menu=bar switches={} surf={}x{}",
        scope,
        WIN.load(Ordering::Relaxed),
        view().label(),
        SWITCHES.load(Ordering::Relaxed),
        SURF_W.load(Ordering::Relaxed),
        SURF_H.load(Ordering::Relaxed)
    );
}

// ------------------------------------------------------------------------------------------------
// A30 — the reopen seam (TAIL-APPENDED: nothing above this line moved, so knob-off panic `Location`
// line numbers are untouched; PARITY.md §5.3)
// ------------------------------------------------------------------------------------------------

/// **Has this board's desktop ever asked for the pulse window?** Sticky, and the only question the
/// dock's pinned reopen tile asks.
///
/// A30's fix has two halves and this is the second. [`close`] clears `ARMED`, so a user close is
/// final against [`service`]'s open arm — the render pass no longer re-opens the window five lines
/// later, which is what render7 caught twice (close-box win=3, then close, then a fresh open of the
/// same id) and what made A18's cascade census read three opens for a single window. A close that is
/// final AND unreachable is a window the operator has LOST, though — the very failure
/// `dock::pin_shell` and `dock::pin_quarry` exist to prevent — so the dock pins a tile that calls
/// [`arm`] again.
///
/// The stickiness is what keeps that tile off boards that never had the window. `arm()` has exactly
/// one caller outside the dock, `video::desktop_firmware::activate`, which is the Pi/Orin seam; an
/// x86 `desktop_uefi` desktop never reaches it, so this stays `false` and `dock::pin_pulse` returns
/// its input untouched. No `cfg` is needed to say that, and none is used: the answer is a runtime
/// fact about the desktop that actually came up.
pub fn ever_armed() -> bool {
    EVER_ARMED.load(Ordering::Acquire)
}

/// **Is the pulse window on the panel right now?** The dock's other pin question — a live window has
/// its own dock-addressable row (kernel owners are), so the pin must not add a second tile for it.
pub fn is_open() -> bool {
    WIN.load(Ordering::Acquire) != wm::WIN_NONE
}
