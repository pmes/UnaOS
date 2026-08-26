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
//! ## The menu, stated plainly
//!
//! Peter asked for *"the first menu option to switch between the 2 views"*. This kernel has exactly one
//! menu framework — `crystal`'s SHARD dropdown — and it is hard-wired to the menu bar's brand mark: it
//! owns a `const` tree of power verbs, anchors itself under `menubar::crystal_box_abs` and paints
//! through the `strip` primitive onto the PANEL. It is not a per-window menu and generalising it into
//! one is a different arc from this one.
//!
//! So the pulse window carries **its own menu strip as the first row of its own content**, drawn into
//! its own surface and hit-tested in its own coordinates — which is what a windowed app's menu is. One
//! title, `View`; clicking it drops a two-row menu whose FIRST option is the Pi's lamps and whose second
//! is the x86-style segments, with a mark against whichever is live. No framework was invented and none
//! was generalised; nothing outside this window can see or reach this menu.
//!
//! ## Why the press arm is where it is
//!
//! [`press_route`] is called from the aarch64 click router's PI-DESK furniture line, ahead of every
//! window arm, and it is the reason that line reads as an `||`. It cannot steal a click from anything:
//! it re-asks `wm::hit_test` and declines unless the TOPMOST window at that point is this one, so a
//! window stacked over the pulse window keeps its own presses. It claims exactly two regions — this
//! window's close disc and this window's menu — and answers `false` everywhere else, so chrome drags,
//! minimise, zoom and focus all still reach the arms below it untouched.
//!
//! **And it does not PAINT.** A press flips one atomic and returns; the repaint is [`service`]'s, on the
//! render core, one paced call per pulse period. That split is not tidiness — it is the ledger at the
//! tail of `desktop_firmware::activate` applied before the fact. The Pi's live routed console was MEASURED turning
//! a 108/108 bench-geometry run into 97/108 with a synchronous exception, and the diagnosis was not "who
//! writes the panel" but *who drives the COMPOSITOR*: a surface that presents from arbitrary call
//! context is an unsynchronised compositor client. A menu painting from the input task would be exactly
//! that, for a picture whose whole content changes four times a second anyway. The cost is that a pick
//! appears on the next tick rather than instantly — bounded by `ui_status::PSTRIP_PERIOD_MS`, 250 ms.
//!
//! ## Gating
//!
//! `any(x86 + wc, aarch64 + desktop_firmware)` — the furniture family's gate, and for the furniture family's
//! reason: this is EXPERIENCE-layer code with no hardware in it, so it compiles on every chip and is
//! turned on by a knob rather than by an arch. Only `desktop_firmware::activate` ARMS the window today; on x86 the
//! module compiles, type-checks and is unreferenced, which is what keeps the port from rotting.

use super::{theme, wm};
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

/// The menu strip's face and its keyline — the kit's chrome, so the strip reads as furniture rather
/// than as content that happens to be at the top.
const MENU_BG: u32 = theme::CHROME_FACE;
const MENU_LINE: u32 = theme::FRAME_LINE;
const MENU_TEXT: u32 = theme::BUTTON_TEXT;
/// The highlight behind an open menu title and behind the marked option.
const MENU_HILITE: u32 = theme::ACCENT;
const MENU_HILITE_TEXT: u32 = 0x00_FFFFFF;

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

/// **The menu tree.** Order is the arc's, and it is load-bearing: Peter asked for *"the first menu
/// option to switch between the 2 views"*, and the FIRST option is the Pi's — the view the window opens
/// on, so option one is always the one you are looking at and option two is always the switch.
const OPTIONS: [View; 2] = [View::Lamps, View::Segments];

/// The live view. `u8`, holding a [`View::ord`], so the whole of the modal state is one relaxed load on
/// the paint path.
static VIEW: AtomicU32 = AtomicU32::new(0);

/// Is the `View` menu dropped? `false` at open and after every pick or dismissal.
static MENU_OPEN: AtomicBool = AtomicBool::new(false);

/// Has a desktop asked for this window? Set by [`arm`], consumed by [`service`]'s open arm.
static ARMED: AtomicBool = AtomicBool::new(false);

/// Falsifiable counters for the witness: how many times the menu was opened, and how many picks
/// actually MOVED the view (a pick of the live view is a dismissal, not a switch).
static OPENS: AtomicU64 = AtomicU64::new(0);
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
const OWNER: u64 = wm::KERNEL_OWNER_BASE + 0x60;

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

/// The menu strip's height: one line pitch, the same band `ui_status::draw` gives the status line.
fn menu_h(m: &crate::ui::Metrics) -> usize {
    m.line_h
}

/// One core row's target height — three glyph cells, which is what puts the LED pitch (half the row)
/// at the density the desktop band draws at. See [`content_extent`].
fn row_target(m: &crate::ui::Metrics) -> usize {
    m.cell_h * 3
}

/// A menu option row's height: the glyph cell plus clearance, `crystal`'s `ITEM_H` rule at this
/// window's own scale.
fn item_h(m: &crate::ui::Metrics) -> usize {
    m.cell_h + 8
}

/// The `View` title's box in CONTENT coordinates: `(x, y, w, h)`.
fn title_box(m: &crate::ui::Metrics) -> (usize, usize, usize, usize) {
    (pad(m), 0, m.cell_w * 6, menu_h(m))
}

/// The dropdown's box in CONTENT coordinates, or `None` when the content cannot seat it below the
/// strip. Anchored under the title's left edge, `crystal`'s rule.
fn menu_box(m: &crate::ui::Metrics, cw: usize, ch: usize) -> Option<(usize, usize, usize, usize)> {
    let (tx, _, _, _) = title_box(m);
    let w = 2 * pad(m) + (2 + max_option_glyphs()) * m.cell_w;
    let h = OPTIONS.len() * item_h(m) + 2;
    let y = menu_h(m);
    if tx + w > cw || y + h > ch {
        return None;
    }
    Some((tx, y, w, h))
}

/// The longest option label in glyphs, walked at compile time so a relabelled option cannot silently
/// overflow the dropdown — `crystal::max_label_glyphs`'s rule.
const fn max_option_glyphs() -> usize {
    let mut m = 0;
    let mut i = 0;
    while i < OPTIONS.len() {
        let l = OPTIONS[i].label().len();
        if l > m {
            m = l;
        }
        i += 1;
    }
    m
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
    WIN.store(id, Ordering::Release);
    serial_println!(
        "[pulsewin] open win={} panel={}x{} surf={}x{} box={}x{} at ({},{}) view={} \
         (menu: click `View` for the two faces — first option is the Pi's)",
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
    // Order matters: the row goes first, so the compositor can no longer read the surface, and only
    // then is the store dropped. The reverse would leave one composite pass reading freed memory.
    wm::close(id);
    SURF.store(0, Ordering::Release);
    SURF_W.store(0, Ordering::Relaxed);
    SURF_H.store(0, Ordering::Relaxed);
    MENU_OPEN.store(false, Ordering::Relaxed);
    *STORE.lock() = None;
    serial_println!(
        "[pulsewin] close win={} opens={} switches={} (surface freed; the desktop LED band is untouched)",
        id,
        OPENS.load(Ordering::Relaxed),
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
    h = super::strip::fnv1a_u64(h, MENU_OPEN.load(Ordering::Relaxed) as u64);
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
        View::Lamps => ui_status::draw_panel_at(
            &mut p,
            Some((0, menu_h(&m), cw, ch.saturating_sub(menu_h(&m)))),
        ),
        View::Segments => draw_segments(&mut p, &m, cw, ch, &loads, ncpu),
    }
    draw_menu(&mut p, &m, cw, ch);
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
/// yet, by construction.
pub fn arm() {
    ARMED.store(true, Ordering::Release);
}

/// The menu strip, and the dropdown when it is open.
fn draw_menu<P: GneissPal>(p: &mut P, m: &crate::ui::Metrics, cw: usize, ch: usize) {
    let mh = menu_h(m);
    p.draw_rect(0, 0, cw, mh, MENU_BG);
    p.draw_rect(0, mh.saturating_sub(1), cw, 1, MENU_LINE);
    let (tx, ty, tw, th) = title_box(m);
    let open = MENU_OPEN.load(Ordering::Relaxed);
    let (face, ink) = if open {
        (MENU_HILITE, MENU_HILITE_TEXT)
    } else {
        (MENU_BG, MENU_TEXT)
    };
    p.draw_rect(tx, ty, tw, th.saturating_sub(1), face);
    p.draw_text(tx, ty + (th.saturating_sub(m.cell_h)) / 2, "View", ink);
    if !open {
        return;
    }
    let Some((mx, my, mw, mmh)) = menu_box(m, cw, ch) else {
        return;
    };
    p.draw_rect(mx, my, mw, mmh, MENU_BG);
    p.draw_rect(mx, my, mw, 1, MENU_LINE);
    p.draw_rect(mx, my + mmh - 1, mw, 1, MENU_LINE);
    p.draw_rect(mx, my, 1, mmh, MENU_LINE);
    p.draw_rect(mx + mw - 1, my, 1, mmh, MENU_LINE);
    let live = view();
    for (i, opt) in OPTIONS.iter().enumerate() {
        let iy = my + 1 + i * item_h(m);
        let marked = *opt == live;
        let (face, ink) = if marked {
            (MENU_HILITE, MENU_HILITE_TEXT)
        } else {
            (MENU_BG, MENU_TEXT)
        };
        p.draw_rect(mx + 1, iy, mw.saturating_sub(2), item_h(m), face);
        let ty = iy + (item_h(m).saturating_sub(m.cell_h)) / 2;
        // The mark is a glyph, not a tick sprite: the 8x8 base font is the only type this window has,
        // and inventing a second glyph source for one checkmark would be a font for a checkmark.
        p.draw_text(mx + pad(m), ty, if marked { ">" } else { " " }, ink);
        p.draw_text(mx + pad(m) + 2 * m.cell_w, ty, opt.label(), ink);
    }
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

/// What a press at a CONTENT point resolves to. Pure — no side effect — so the resolution can be
/// reasoned about (and, if a fixture ever wants it, asserted) without firing a switch.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    /// The `View` title: toggle the dropdown.
    Title,
    /// An option row: make it the live view.
    Option(usize),
    /// Inside the open dropdown but not on a row (its border): swallow, do not dismiss — the same
    /// courtesy `crystal` gives a press on its own frame.
    MenuFrame,
    /// Anywhere else in the content while the menu is open: dismiss.
    Elsewhere,
}

/// The pure hit resolver over CONTENT coordinates.
fn hit_at(m: &crate::ui::Metrics, cw: usize, ch: usize, x: usize, y: usize) -> Hit {
    let (tx, ty, tw, th) = title_box(m);
    if x >= tx && x < tx + tw && y >= ty && y < ty + th {
        return Hit::Title;
    }
    if MENU_OPEN.load(Ordering::Relaxed) {
        if let Some((mx, my, mw, mh)) = menu_box(m, cw, ch) {
            if x >= mx && x < mx + mw && y >= my && y < my + mh {
                let ly = y - my;
                if ly >= 1 {
                    let idx = (ly - 1) / item_h(m);
                    if idx < OPTIONS.len() {
                        return Hit::Option(idx);
                    }
                }
                return Hit::MenuFrame;
            }
        }
    }
    Hit::Elsewhere
}

/// **The click arm.** `true` when this window CONSUMED the press.
///
/// Called from the aarch64 router's PI-DESK furniture line, ahead of every window arm. Three properties
/// make that position safe, and all three are checked here rather than assumed by the caller:
///
/// 1. **It declines unless this window is the TOPMOST at that point.** `wm::hit_test` answers with the
///    front-most row, so a window stacked over the pulse window keeps every press inside its own box —
///    the menu cannot be clicked through another window's face.
/// 2. **It claims only its own two regions**: this window's close disc, and this window's menu. Chrome
///    drags, the minimise and zoom discs, focus changes and every other window's everything fall
///    through to the arms below, untouched.
/// 3. **A press on this window's CONTENT with the menu closed is not consumed** — it falls through to
///    the ordinary select arm, so clicking the instrument raises the window exactly as clicking any
///    other window's content does.
pub fn press_route(x: i32, y: i32) -> bool {
    let id = WIN.load(Ordering::Acquire);
    if id == wm::WIN_NONE || x < 0 || y < 0 {
        return false;
    }
    match wm::hit_test(x, y) {
        Some((top, _, _)) if top == id => {}
        _ => {
            // Not on this window at all (or occluded by one in front of it). If the menu is open, the
            // press is a dismissal — a dropdown that survives a click somewhere else is a modal the
            // operator did not ask for — but the press itself belongs to whoever it landed on, so it
            // is NOT consumed. `crystal` makes exactly this distinction for the SHARD menu.
            if MENU_OPEN.swap(false, Ordering::AcqRel) {
                serial_println!("[pulsewin] menu dismiss reason=outside");
            }
            return false;
        }
    }
    // The close disc, claimed here because `wm`'s own close arm routes through `close_owner`, which
    // refuses kernel owners — see [`close`].
    if wm::close_box_hit(id, x, y) {
        serial_println!("[pulsewin] close-box win={} at ({},{})", id, x, y);
        close();
        return true;
    }
    let Some(info) = wm::info(id) else { return false };
    let scale = info.scale.max(1);
    if (x as usize) < info.x || (y as usize) < info.y {
        return false; // chrome above/left of the content: the caller's arms own it
    }
    let (cx, cy) = ((x as usize - info.x) / scale, (y as usize - info.y) / scale);
    if cx >= info.w || cy >= info.h {
        return false; // chrome below/right of the content
    }
    let Some(p) = pal() else { return false };
    let m = p.metrics();
    match hit_at(&m, info.w, info.h, cx, cy) {
        Hit::Title => {
            // One atomic RMW, so two cores pressing at once cannot both read "closed" and both open.
            let open = !MENU_OPEN.fetch_xor(true, Ordering::AcqRel);
            if open {
                OPENS.fetch_add(1, Ordering::Relaxed);
            }
            serial_println!(
                ":: PULSEWIN-MENU: title_press={} options={} live={} ::",
                if open { "open" } else { "dismiss" },
                OPTIONS.len(),
                view().label()
            );
            true
        }
        Hit::Option(i) => {
            MENU_OPEN.store(false, Ordering::Release);
            let picked = OPTIONS[i];
            let was = view();
            VIEW.store(picked.ord() as u32, Ordering::Release);
            if picked != was {
                SWITCHES.fetch_add(1, Ordering::Relaxed);
            }
            serial_println!(
                ":: PULSEWIN-MENU: pick idx={} view={} was={} switched={} ::",
                i,
                picked.label(),
                was.label(),
                picked != was
            );
            true
        }
        Hit::MenuFrame => true,
        Hit::Elsewhere => {
            if MENU_OPEN.swap(false, Ordering::AcqRel) {
                serial_println!("[pulsewin] menu dismiss reason=content");
                return true;
            }
            false // ordinary content press: the select arm below raises the window
        }
    }
}

/// `<Esc>` dismisses an open menu, asked from the same position the aarch64 router asks
/// `crystal::key_escape`. `true` when a menu was open and has been torn down.
pub fn key_escape(ev: crate::pal::Event) -> bool {
    if !matches!(ev, crate::pal::Event::Key(0x1B)) {
        return false;
    }
    if MENU_OPEN.swap(false, Ordering::AcqRel) {
        serial_println!("[pulsewin] menu dismiss reason=escape");
        return true;
    }
    false
}

/// The cost/state rollup, on the furniture's precedent — NOT `witness`-gated, because the metal image
/// is built without `witness` and a claim absent from it is not a claim.
pub fn rollup(scope: &str) {
    serial_println!(
        "[pulsewin] rollup scope={} win={} view={} menu={} opens={} switches={} surf={}x{}",
        scope,
        WIN.load(Ordering::Relaxed),
        view().label(),
        MENU_OPEN.load(Ordering::Relaxed),
        OPENS.load(Ordering::Relaxed),
        SWITCHES.load(Ordering::Relaxed),
        SURF_W.load(Ordering::Relaxed),
        SURF_H.load(Ordering::Relaxed)
    );
}
