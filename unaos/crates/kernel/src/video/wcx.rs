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

//! WC-X86 — bring the window compositor up on the x86 panel path.
//!
//! [`super::wm`] has always been arch-neutral: it composites through the generic
//! [`super::FrameBuffer`] held in [`super::WRITER`], which on x86 already wraps the UEFI GOP
//! surface. What the x86 path lacked was not a compositor — it was an *activation*: nothing ever
//! created a window, so the table stayed empty and the pass had nothing to paint. This module is
//! that activation and nothing else. **It does not modify `wm` or `cursor` in any way**; every
//! pixel it puts on the panel goes through `wm`'s own public verbs.
//!
//! ### Where it runs, and why exactly there
//!
//! At the END of `kepler_display::takeover_display`, on the line after
//! [`super::fbcon::panel_console_resume`]. That seam is the first moment on the metal path where
//! the panel is settled: the takeover has repointed the scan-out, the calibration pattern has been
//! drawn, held and cleared, and the console has been re-homed on the real surface. Activating
//! *before* it would put windows on a surface the takeover is about to repoint — the compositor
//! would be fighting the takeover for the same pixels, and the takeover would win, silently.
//!
//! ### Console pixel ownership — RULED: the console is a WINDOW
//!
//! Peter has ruled (2026-07-26): console-becomes-a-window, not a reserved band. The provisional
//! band — a clamp on the console's row grid, with the compositor confined below it — is gone, and so
//! is the `panel_console_reserve_band` call that installed it. What replaces it is
//! [`super::fbcon::panel_console_window_open`]: the console allocates a cached-RAM surface, opens an
//! ordinary row in the window table over it, and paints its glyphs there. Z-order now does the job
//! the band was doing, which is the whole reason the band was only ever provisional.
//!
//! The console window is created FIRST and the demo window second, so the demo's later `z` puts it
//! in front — the sitting shows two real windows with real occlusion between them, rather than two
//! windows that were carefully arranged never to meet.
//!
//! Serial is unaffected by any of this: `serial_println!` still writes the UART on every line, and
//! the FTDI mirror the bench reads is unconditional in this build. The window is where the text is
//! *drawn*, not where it is *logged*.
//!
//! ### Damage-row present
//!
//! Not re-implemented here, and deliberately: `wm::composite` already presents by damage. It paints
//! only the outer boxes of the windows in its dirty set (closed upwards over occlusion), it stages
//! them in cached RAM and copies them out as full-width row runs, and it cleans exactly the row span
//! it touched via `FrameBuffer::flush_range`. Nothing about that is aarch64-specific — on x86 the
//! cache clean is the documented no-op and the row-run copy is the same code. The x86 present is
//! therefore already row-damaged; this module's contribution is to SAY SO on the wire
//! (`[wc-x] present ... rows=A..B`) so a bench photo can be checked against a row span rather than
//! against a claim.
//!
//! ### Front-buffer discipline
//!
//! Every pixel this module authors lands in [`DEMO_SURF`], a cached-RAM kernel surface. It performs
//! no framebuffer write of its own and reads the front buffer never — `wm` owns the panel writes
//! and does them through its staged path. There is no direct-front-buffer fallback in this file.
//!
//! The one exception, and its exact scope: [`activate_on`] clears the whole panel to
//! `wm::DESKTOP_BG` once, BEFORE the first window exists (see the DESKTOP-CLEAR comment there). At
//! that instant the compositor owns no pixels and no pass can be in flight, so the fill has no
//! second writer to tear against. From the moment the console window is created onward, this file
//! writes the framebuffer zero times.
//!
//! ### SECOND-IGNITION (M-a) — one body, the surface named rather than assumed
//!
//! [`activate`] used to BE the activation. It is now a shim over [`activate_on`], which takes a
//! [`SurfaceDesc`] naming the scanout it is being asked to own. Nothing about the activation itself
//! moved: `activate_on` holds the former body verbatim, and `activate()` passes the descriptor for
//! the surface `WRITER` already carries — which on the Kepler path is, by identity, the surface the
//! takeover repointed the scan-out at (`kepler_display` locates the GOP framebuffer via
//! `fbcon::current_base()` and draws at that same address; it never re-seeds `WRITER`).
//!
//! The parameterisation is what a SECOND ignition point needs, and it needs nothing else: `wm`
//! re-reads `WRITER` live on every path it has and caches panel geometry nowhere, so "make `WRITER`
//! tell the truth before the call" is the whole of the surface question. M-a does not add that
//! second caller — there is still exactly one, in `kepler_display::takeover_display` — and it does
//! not implement the adoption of a foreign surface either: a descriptor that does not match the live
//! `WRITER` is DECLINED, fail-closed, because adopting one honestly also means re-pointing fbcon's
//! own handle and WC-typing the new aperture, and neither of those is in this milestone.

use super::wm;
use unaos_boot_info::FrameBufferInfo;

/// SECOND-IGNITION — who is asking the compositor to take a surface.
///
/// The vocabulary is fixed here rather than at the call site so the refusal line reads the same
/// whichever origin wins the latch. M-a constructs only `KeplerTakeover`.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The Kepler takeover seam — the one caller that exists today.
    KeplerTakeover,
    /// The iGPU scan-out, after a gmux switch. M-b's caller; named now so the latch's `owner=` /
    /// `requested=` witness has both words from the first boot that can print them.
    #[allow(dead_code)] // constructed by the M-b caller (drivers/gpu/igpu.rs), not by M-a.
    IgpuScanout,
}

/// [`ACTIVATED`]'s alphabet. Kept as plain `u32` constants rather than a `repr` on [`Origin`] so the
/// "nobody has activated yet" state is a value in the same space, not a second flag that can
/// disagree with the first.
const ORIGIN_NONE: u32 = 0;
const ORIGIN_KEPLER: u32 = 1;
const ORIGIN_IGPU: u32 = 2;

impl Origin {
    const fn code(self) -> u32 {
        match self {
            Origin::KeplerTakeover => ORIGIN_KEPLER,
            Origin::IgpuScanout => ORIGIN_IGPU,
        }
    }
    const fn name(self) -> &'static str {
        origin_name(self.code())
    }
}

const fn origin_name(code: u32) -> &'static str {
    match code {
        ORIGIN_KEPLER => "kepler",
        ORIGIN_IGPU => "igpu",
        _ => "none",
    }
}

/// SECOND-IGNITION — the scan-out the compositor is being asked to own.
///
/// Every field is a fact about the SURFACE, never about the GPU that owns it: `wm` composites
/// through the generic [`super::FrameBuffer`] and asks nothing else. `origin` is carried only so the
/// witness lines can name who claimed the compositor.
pub struct SurfaceDesc {
    /// CPU-visible (identity-mapped) base of the scan-out.
    pub base: usize,
    /// Byte length of the scan-out aperture.
    pub len: usize,
    /// Width/height/stride(px)/bpp/format, as [`super::FrameBuffer`] addresses them.
    pub info: FrameBufferInfo,
    /// Who is asking.
    pub origin: Origin,
}

impl SurfaceDesc {
    /// The surface [`super::WRITER`] already describes.
    ///
    /// A SNAPSHOT: it reads the handle and re-initialises nothing, so `activate()` built on it is a
    /// pure no-op wrapper around the body that has always run. This is the whole reason the Kepler
    /// path is unchanged by M-a.
    pub fn from_writer(origin: Origin) -> Self {
        let fb = *super::WRITER.lock();
        Self { base: fb.base(), len: fb.len(), info: fb.info(), origin }
    }

    /// Is this descriptor the surface `WRITER` is live on right now?
    ///
    /// Compared field by field rather than by base alone: a same-base surface with a different
    /// stride or bpp is a different surface for every purpose `wm` has, and the stride unit is
    /// exactly the class of mistake this comparison exists to catch.
    fn is_live(&self) -> bool {
        let fb = *super::WRITER.lock();
        let i = fb.info();
        fb.base() == self.base
            && fb.len() == self.len
            && i.width == self.info.width
            && i.height == self.info.height
            && i.stride == self.info.stride
            && i.bytes_per_pixel == self.info.bytes_per_pixel
    }
}

/// ACTIVATE-ONCE — the origin that owns the compositor, or [`ORIGIN_NONE`].
///
/// This REPLACES `DEMO_WIN` as the re-entry guard, and the replacement is the point. `DEMO_WIN` is
/// stored LATE — after the desktop clear, the console window and the demo window — so every DECLINE
/// path returned with it still clear. With one caller that was harmless; with two it is a hole a
/// second entry walks straight through, running a second full-panel `fill_screen` over a live
/// compositor (the exact "second writer" this module's front-buffer law forbids) and then finding
/// `panel_console_window_open` hand back the EXISTING console window against the new geometry.
///
/// So the latch is claimed at the TOP and RELEASED on every decline: a genuinely-failed first
/// attempt must not permanently kill the compositor for the other origin, while a SUCCESSFUL first
/// attempt is final and the second caller is refused with one line.
///
/// ⚠ RESIDUAL, named rather than hidden: the last two decline paths (`geometry-unavailable`,
/// `create-failed`) are reached AFTER the desktop clear and AFTER the console window exists, so the
/// release hands the next origin an entry that would clear the panel underneath a live console
/// window. That is the lesser of the two evils only because those two declines mean the window table
/// itself refused, and a compositor that cannot open a window has nothing to protect. A second
/// caller must not retry past a post-console decline; the `latch=released` on the line is what tells
/// it which decline it was.
static ACTIVATED: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(ORIGIN_NONE);

/// The demo surface's source dimensions, in pixels. Small on purpose: the compositor's own scale
/// rule magnifies it to the panel (4x on a ≤1799-row panel), so a small source is what exercises the
/// upscale path, and a small source is also what keeps this buffer in BSS at a size nobody has to
/// think about (96 * 64 * 4 = 24 KiB).
const DEMO_W: usize = 96;
const DEMO_H: usize = 64;

/// The demo surface, in ARGB8888 with a stride of exactly one row. Kernel-owned cached RAM: the
/// compositor reads it as a source and never writes it, and nothing in user mode can reach it.
///
/// `static mut` rather than a `Mutex`: `wm::create` takes the surface as a raw address it will read
/// from composite context, so the buffer must have a stable address independent of any guard. It is
/// written exactly once, by [`paint_demo`], before the window that references it exists.
#[repr(align(4))]
struct DemoSurface([u32; DEMO_W * DEMO_H]);
static mut DEMO_SURF: DemoSurface = DemoSurface([0; DEMO_W * DEMO_H]);

/// The window id [`activate_on`] opened, or [`wm::WIN_NONE`] if it has not run or declined.
///
/// NO LONGER THE GUARD — see [`ACTIVATED`], which is claimed at the top of `activate_on` where this
/// one was stored at the bottom. This stays for the witness lines and for anything that wants to ask
/// "did the demo window actually open", which is a different question from "has the compositor been
/// claimed".
static DEMO_WIN: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(wm::WIN_NONE);

/// Gap in panel pixels between the demo window's outer box and the panel edge. Matches `wm`'s own
/// tiling gap in spirit; it is only used to inset the pinned demo window from the bottom-right
/// corner, so the frame the compositor draws is visible on all four sides in a bench photograph.
const EDGE_GAP: usize = 8;

/// Fill [`DEMO_SURF`] with a calibration pattern.
///
/// **Not theme, not chrome.** The window's border and title strip are painted by the compositor from
/// `wm`'s own constants, as they are for every window; nothing here invents a chrome colour or a
/// desktop colour, and this module deliberately defines no colour table of its own. What it draws is
/// *content* — the kind of content a calibration target is made of, chosen so that a photograph of
/// the panel answers questions:
///
///   * a 1-px frame in white — proves the window's content extent, and where it sits relative to the
///     kernel-drawn border (the two must be adjacent, never overlapping);
///   * pure red / green / blue vertical bars — prove the channel order survives the GOP layout
///     conversion (`FrameBuffer::put_pixel` re-encodes per layout; a swapped pair shows instantly);
///   * a corner-to-corner diagonal — proves the nearest-neighbour upscale is isotropic; a stride
///     error in the blit turns a diagonal into a staircase with a visible slope change;
///   * a single-pixel checkerboard block — the highest spatial frequency the surface can carry, so
///     any row that the present drops or duplicates shows up as a band of flat colour.
fn paint_demo() {
    // SAFETY: called exactly once, from `activate`, before the window that names this buffer is
    // created — so no compositor pass can be reading it, and there is no other writer at all.
    let px = unsafe { &mut (*core::ptr::addr_of_mut!(DEMO_SURF)).0 };
    for y in 0..DEMO_H {
        for x in 0..DEMO_W {
            let c = if x == 0 || y == 0 || x == DEMO_W - 1 || y == DEMO_H - 1 {
                0x00FF_FFFF
            } else if x * DEMO_H == y * DEMO_W {
                0x00FF_FFFF
            } else if y >= DEMO_H - 12 && x >= DEMO_W - 12 {
                // Single-pixel checkerboard, bottom-right.
                if (x + y) & 1 == 0 { 0x00FF_FFFF } else { 0x0000_0000 }
            } else if y < 12 {
                // Channel bars across the top third of the width each.
                match x * 3 / DEMO_W {
                    0 => 0x00FF_0000,
                    1 => 0x0000_FF00,
                    _ => 0x0000_00FF,
                }
            } else {
                0x0000_0000
            };
            px[y * DEMO_W + x] = c;
        }
    }
}

/// WC-X86 — activate the compositor on the x86 panel, on whatever surface [`super::WRITER`] already
/// describes. Unchanged signature, unchanged meaning; idempotent, a second call is a no-op.
///
/// This is the Kepler-takeover call site's entry point and it is a SHIM: the descriptor it builds is
/// a snapshot of the live `WRITER`, so the reconciliation step inside [`activate_on`] can only take
/// its already-live branch and the body that runs is the body that has always run.
pub fn activate() {
    activate_on(SurfaceDesc::from_writer(Origin::KeplerTakeover));
}

/// WC-X86 — THE activation body, on a NAMED surface. One per boot, first origin wins.
///
/// Fail-closed at every step. A panel that is not ready, a console that will not yield its band, a
/// window table that will not take a row — each returns after saying which one it was, and each
/// leaves the panel exactly as the takeover left it. There is no path here that half-activates.
///
/// ### FALSIFIABLE PREDICTION (M-a, written before the boot that tests it)
///
/// M-a is a refactor with a witness, and its claim is that it changes NOTHING on a Kepler boot. On
/// `UNAOS_WC=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1`, the `[wc-x]` block of the serial log is
/// line-for-line identical to the pre-change boot **except for exactly one added line**,
/// `[wc-x] surface adopt SKIP (already live)`, printed directly before `[wc-x] desktop-clear`.
/// No `REFUSE` line appears — there is still exactly one caller, so nothing can lose the latch — and
/// no `DECLINE … latch=released` line appears either. `kepler=` in the GPACE line moves by < 5 ms;
/// the prologue is two atomics and two `WRITER` reads and must not be measurable above that.
///
/// Any other diff falsifies M-a: a reordered line, a second `desktop-clear`, a changed
/// `console-window` geometry, a `REFUSE` (something double-calls), or a `DECLINE
/// reason=surface-adopt-not-implemented` (the snapshot and the live handle disagreed, which for a
/// shim over the same lock would mean `WRITER` is being re-seeded under us between the two reads).
pub fn activate_on(desc: SurfaceDesc) {
    use core::sync::atomic::Ordering;

    // ACTIVATE-ONCE — claim the compositor BEFORE touching anything, release it on every decline.
    // The claim has to be first: everything below this line is a step that a second entrant would
    // repeat, and the first of them (the desktop clear) is a full-panel front-buffer write.
    if let Err(owner) = ACTIVATED.compare_exchange(
        ORIGIN_NONE,
        desc.origin.code(),
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        serial_println!(
            "[wc-x] activate REFUSE reason=already-active owner={} requested={} panel={}x{}",
            origin_name(owner),
            desc.origin.name(),
            desc.info.width,
            desc.info.height
        );
        return;
    }

    // The panel, as the compositor will see it. Read through the same `WRITER` handle `wm` uses, so
    // the geometry the band is computed from is the geometry the windows are placed against.
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            ACTIVATED.store(ORIGIN_NONE, Ordering::Release);
            serial_println!("[wc-x] activate DECLINE reason=fb-not-ready latch=released");
            return;
        }
        let i = fb.info();
        (i.width, i.height)
    };

    // SURFACE RECONCILIATION — the ONE place a surface swap would happen, and in M-a it does not.
    //
    // `wm` reads `WRITER` live everywhere and caches panel geometry nowhere, so the entire surface
    // question is "does `WRITER` already tell the truth about the scan-out we were handed". On the
    // Kepler path it does, by identity: the takeover locates the GOP framebuffer, draws its
    // calibration pattern at that same address and keeps the scan-out there, and `WRITER` was seeded
    // from that same `BootInfo` triple — so the answer is yes on every boot, and this says so.
    //
    // The other branch is a REFUSAL, not a swap, and deliberately: adopting a foreign surface
    // honestly means re-pointing fbcon's own handle too (`panel_console_window_open` reads
    // `c.fb.info()`, a second and independent geometry source) and WC-typing the new aperture (the
    // framebuffer WC latch is a consumed one-shot — a new aperture comes up UNCACHED, which is the
    // GR15 defect by construction, 8.7–9.1x on the present path). Neither belongs to this milestone,
    // and a swap that skipped them would be the kind of half-activation the rest of this function
    // refuses to perform.
    if desc.is_live() {
        serial_println!("[wc-x] surface adopt SKIP (already live)");
    } else {
        ACTIVATED.store(ORIGIN_NONE, Ordering::Release);
        serial_println!(
            "[wc-x] activate DECLINE reason=surface-adopt-not-implemented origin={} want=base={:#x} len={} {}x{} stride={} bpp={} latch=released",
            desc.origin.name(),
            desc.base,
            desc.len,
            desc.info.width,
            desc.info.height,
            desc.info.stride,
            desc.info.bytes_per_pixel
        );
        return;
    }

    // WC-X DESKTOP-CLEAR — paint the whole panel to the desktop colour ONCE, here.
    //
    // Observed on the metal (s40 rMBP photo): everything painted to the panel BEFORE activation
    // stays on glass forever — stray direct-painted fbcon rows across the top, and the pale
    // rectangle top-left that kepler's display probe leaves behind. The compositor is not at fault
    // and cannot fix it from inside a pass: `composite` paints its windows' boxes and `erase` paints
    // boxes windows have vacated. Neither has any claim on panel pixels the window layer has never
    // owned, so the pre-activation residue is outside every damage box there is.
    //
    // Why a DIRECT fill and not a staged one: `wm` exposes no full-panel erase or desktop-repaint
    // verb (`erase` is private and box-scoped; `repaint` only re-damages existing window rows), and
    // the WC-J reclaim path is likewise reached only through a window vacating a box. Rather than
    // widen `wm`'s surface for a one-shot, this is the single case where a direct front-buffer write
    // is sound: it runs BEFORE the first window exists, so the compositor owns no pixels at this
    // instant, `STAGE` has no pass to collide with, and there is no second writer to tear against.
    // The no-direct-writes law protects compositor-owned pixels from a second writer; there are no
    // compositor-owned pixels yet. After the console window is created below, this file writes the
    // framebuffer zero times, exactly as its module docs claim.
    //
    // CURSOR-1's bracket applies here for the same reason it applies to `erase`: if the sprite is on
    // the panel, the fill would paint over it and its save-under would later restore pre-clear pixels
    // as a stale patch. Take it off first; the first composite below puts it back.
    {
        super::cursor::undraw();
        let fb = *super::WRITER.lock();
        fb.fill_screen(wm::DESKTOP_BG);
        fb.flush_all();
    }
    serial_println!(
        "[wc-x] desktop-clear panel={}x{} bg={:08X}",
        pw,
        ph,
        wm::DESKTOP_BG
    );

    // WC-BBSYNC — the glass is now `DESKTOP_BG`; make the DESKTOP LAYER agree with it.
    //
    // The desktop layer is `screen::Screen`, and at this instant it does not exist: this seam runs
    // from inside PCI enumeration (`kepler::init` -> `takeover_display`), and `kernel_main` does not
    // build the GUI loop's `Screen` until far below. A `Screen` built after a takeover allocates its
    // back store zeroed and arms FULL-PANEL damage, so the desktop layer would come up holding BLACK
    // over a panel this fill just made `DESKTOP_BG`, with the damage to ship that black already set —
    // the same shape as the s41 ghost-box finding, where the desktop repaints an unoccluded box from
    // content that predates it. (On the nominal path the shell console's first full clear happens to
    // use the same number and lands before the first present, so the black does not currently reach
    // the glass. That is a coincidence between two independently-declared constants, not a guarantee.)
    //
    // So this records the colour rather than filling a buffer: the desktop layer adopts it when it is
    // constructed and reports `[wc-x] backbuffer resync WxH` on the wire there. The colour source is
    // `wm::DESKTOP_BG` — the same constant the fill above used — and no new one is invented. The
    // adoption is a back-buffer (cached RAM) fill; nothing reads the panel back.
    super::screen::adopt_desktop_bg(wm::DESKTOP_BG);
    serial_println!(
        "[wc-x] backbuffer resync ARMED bg={:08X} (desktop layer not yet constructed)",
        wm::DESKTOP_BG
    );

    // RULED — the console becomes a window. Opened FIRST so the demo below carries the higher z.
    // Its own witness lines (`[wc-x] console-window …`) report the geometry and the panic fallback.
    let cwin = super::fbcon::panel_console_window_open();
    if cwin == wm::WIN_NONE {
        ACTIVATED.store(ORIGIN_NONE, Ordering::Release);
        serial_println!("[wc-x] activate DECLINE reason=console-window-declined latch=released");
        return;
    }
    serial_println!("[wc-x] activate panel={}x{} console_win={}", pw, ph, cwin);

    paint_demo();
    // Taking the ADDRESS of a `static mut` is safe — no reference is formed. The one place a
    // reference is formed is `paint_demo`, which carries the safety argument for it.
    let surf = core::ptr::addr_of_mut!(DEMO_SURF) as usize;
    let surf_len = DEMO_W * DEMO_H * 4;
    // SPAWN-PLACE — the demo's OUTER box is pinned into the panel's bottom-right corner, and the
    // geometry is settled BEFORE the row exists. `wm::create` + `wm::move_to` computed the same
    // placement, but only after `create` had already composited a frame of this window at the
    // tiler's top-left origin — visible on the metal (s41) as a window that appears top-left and
    // jumps, and as an abandoned box the move then has to erase. `spawn_geometry` answers the size
    // question (the tiler's own scale rule) without a row, and `create_at` takes the content origin,
    // so the first and only frame this window ever presents is at the position it keeps.
    //
    // Overlapping the console window is still intended: the demo is created later, so it has the
    // higher z, and the overlap is what makes the occlusion visible in a bench photograph.
    let (_scale, ow, oh) = match wm::spawn_geometry(DEMO_W, DEMO_H) {
        Some(g) => g,
        None => {
            ACTIVATED.store(ORIGIN_NONE, Ordering::Release);
            serial_println!("[wc-x] activate DECLINE reason=geometry-unavailable latch=released");
            return;
        }
    };
    let ox = pw.saturating_sub(ow).saturating_sub(EDGE_GAP);
    let oy = ph
        .saturating_sub(crate::ui_status::chrome_h(ph))
        .saturating_sub(oh)
        .saturating_sub(EDGE_GAP);
    // CLICK-X86 — owner [`wm::KERNEL_OWNER_DESKTOP`]: this window belongs to the KERNEL, not to any
    // address space, and both consequences that reading was chosen for still hold — it is outside the
    // focus ring (`focus_ring` skips the reserved band) and outside `close_owner`'s reach (which
    // refuses it), so no user task may present, move or close it. It is now HITTABLE, which owner 0
    // silently prevented: the demo is panel furniture in the corner and the operator will click it.
    // A distinct band value from the console's, so a click raises exactly the window under the hand.
    let id = wm::create_at(
        wm::KERNEL_OWNER_DESKTOP,
        surf,
        surf_len,
        DEMO_W as u32,
        DEMO_H as u32,
        (DEMO_W * 4) as u32,
        b"unaos wc",
        ox + wm::BORDER,
        oy + wm::TITLE_H + wm::BORDER,
    );
    if id == wm::WIN_NONE {
        ACTIVATED.store(ORIGIN_NONE, Ordering::Release);
        serial_println!("[wc-x] activate DECLINE reason=create-failed latch=released");
        return;
    }
    serial_println!(
        "[wc-x] spawn-place win={} box={}x{} at ({},{}) (created in place, no move)",
        id, ow, oh, ox, oy
    );

    DEMO_WIN.store(id, Ordering::Relaxed);
    let ok = wm::present(id);

    if let Some(i) = wm::info(id) {
        let y0 = i.y.saturating_sub(wm::TITLE_H + wm::BORDER);
        let y1 = (i.y + i.h * i.scale + wm::BORDER).min(ph);
        serial_println!(
            "[wc-x] demo win={} surf={}x{} at ({},{}) scale={}x z={}",
            i.id, i.w, i.h, i.x, i.y, i.scale, i.z
        );
        serial_println!("[wc-x] present win={} rows={}..{} ok={}", i.id, y0, y1, ok);
    }

    #[cfg(feature = "witness")]
    move_vacate_probe(pw, ph);

    // INSTGUI — the graphical installer dialog, in front of everything (created last = top z).
    // Double-gated: `wc` (this module) AND `instgui` (`UNAOS_INSTGUI=1`).
    #[cfg(feature = "instgui")]
    super::instgui::open();
}

/// The probe surface for [`move_vacate_probe`] — one flat colour, chosen to be nothing else on the
/// panel: not [`wm::DESKTOP_BG`], not the chrome, not any bar in the demo pattern.
#[cfg(feature = "witness")]
const PROBE_COL: u32 = 0x00FF_00FF;
#[cfg(feature = "witness")]
#[repr(align(4))]
struct ProbeSurface([u32; 64]);
#[cfg(feature = "witness")]
static mut PROBE_SURF: ProbeSurface = ProbeSurface([PROBE_COL; 64]);

/// MOVE-VACATE — does a moved window's vacated box actually reach the glass on x86?
///
/// The s41 metal sitting reported the panel keeping a moved window's old pixels while the wire said
/// the erase was staged and delivered (`[wc-k] erase box=1314x750 staged=yes … -> BUFFERED`). The two
/// statements cannot both be about the same instant, and the wire alone cannot say which one is
/// wrong: `[wc-k]` reports that [`wm`]'s staged fill ran and copied its rows, not that those rows are
/// what the scan-out is showing when the shutter opens. This probe closes that gap the way the
/// compositor's other verdicts close theirs — by READING THE PANEL BACK.
///
/// Three answers, and they are mutually exclusive:
///   * `desktop=5/5` — the erase reached the framebuffer. Any residue in a bench photo is then a
///     LATER writer repainting those rows (the desktop's own damage-limited present is the candidate:
///     `screen::flush` subtracts the window layer's occluders from its damage, so a box no window
///     covers any more is a box the desktop layer is free to paint from its back buffer), not a
///     failure of the erase, and the fix belongs on that path.
///   * `desktop=0/5` with `painted=true` — the window's own pixels are still there: the erase's rows
///     never landed where the window's rows landed, i.e. the two disagree about the box, and the fix
///     belongs in `wm::erase`/`stage_fill`.
///   * `painted=false` — the probe never owned its box in the first place; the leg proves nothing and
///     says so rather than reporting a false PASS.
///
/// Deliberately a SCRATCH window rather than a move of the demo: the windows this module activates
/// are now created in place and never move (SPAWN-PLACE), and moving one to test the move path would
/// re-introduce on every witness boot exactly the defect that change removes. The probe opens its
/// own 8x8 window in a clear corner, moves it once, reads back, and closes it — so it is also the
/// only thing on the panel that a `move_to` disturbs.
///
/// Witness builds only, and one-shot.
#[cfg(feature = "witness")]
fn move_vacate_probe(pw: usize, ph: usize) {
    let (scale, ow, oh) = match wm::spawn_geometry(8, 8) {
        Some(g) => g,
        None => return,
    };
    // Two disjoint boxes along the top edge, clear of the centred console and the corner demo.
    let step = ow + 2 * EDGE_GAP;
    if pw < 2 * step + EDGE_GAP || ph < oh + EDGE_GAP {
        serial_println!("[wc-x] move-vacate SKIP (panel {}x{} too small)", pw, ph);
        return;
    }
    let (ax, ay) = (EDGE_GAP, EDGE_GAP);
    let bx = EDGE_GAP + step;

    let surf = core::ptr::addr_of_mut!(PROBE_SURF) as usize;
    let id = wm::create_at(
        0,
        surf,
        core::mem::size_of::<ProbeSurface>(),
        8,
        8,
        32,
        b"vacate",
        ax + wm::BORDER,
        ay + wm::TITLE_H + wm::BORDER,
    );
    if id == wm::WIN_NONE {
        serial_println!("[wc-x] move-vacate SKIP (create declined)");
        return;
    }
    wm::present(id);

    let read = |x: usize, y: usize| super::WRITER.lock().read_pixel(x, y).unwrap_or(0);
    let (cx, cy) = (ax + wm::BORDER, ay + wm::TITLE_H + wm::BORDER);
    let painted = read(cx + 1, cy + 1) == PROBE_COL;

    // The move. Same verb an app's `WIN_MOVE` takes, so the path under test is the real one.
    wm::move_to(id, bx + wm::BORDER, ay + wm::TITLE_H + wm::BORDER);

    // Five points inside the box just vacated: content origin, two content diagonals, the title
    // strip and the lower border — `vacate_selftest`'s sample set, for the same reason (a fill that
    // covers the content but not the chrome is a different bug from one that covers neither).
    let pts = [
        (cx + 1, cy + 1),
        (cx + 2, cy + 2),
        (cx + 5, cy + 5),
        (ax + ow / 2, ay + wm::TITLE_H / 2),
        (ax + ow / 2, ay + oh - 1),
    ];
    let mut clean = 0usize;
    let mut stale = 0usize;
    for &(x, y) in pts.iter() {
        let px = read(x, y);
        if px == wm::DESKTOP_BG {
            clean += 1;
        } else if px == PROBE_COL {
            stale += 1;
        }
    }
    wm::close(id);
    serial_println!(
        "[wc-x] move-vacate win={} scale={}x from=({},{}) to=({},{}) box={}x{} painted={} desktop={}/5 stale={}/5 -> {}",
        id, scale, ax, ay, bx, ay, ow, oh, painted, clean, stale,
        if painted && clean == pts.len() { "PASS" } else { "FAIL" }
    );
}
