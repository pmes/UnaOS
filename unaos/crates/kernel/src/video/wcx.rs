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
//! The console window is the only window this module creates. The SECOND window on the desktop is a
//! ring-3 PROCESS — see *The desktop's second window is a program* below — created later, by its own
//! `SYS_WIN_CREATE`, and therefore carrying the later `z`. The sitting shows two real windows, one of
//! which now has a pid.
//!
//! **It does NOT show occlusion between them, and an earlier draft of this doc claimed it did.** The
//! console is PINNED by `create_at` and the desktop app is not: it goes through the ordinary tiler,
//! which places a 128x128 surface at `(9,21)` on this panel. On the 2880x1800 bench panel the console
//! box spans x 783..2097 and the app's box spans x 8..778 — they miss by five pixels. Occlusion was
//! one of the two things the deleted demo window existed to demonstrate, and it is not demonstrated
//! any more. Deliberately NOT fixed by moving the app's row: `SPAWN-PLACE` exists because
//! create-then-move showed on the metal (s41) as a window that appears at the tiler's origin and
//! jumps, plus an abandoned box the move has to erase — re-introducing that to stage a screenshot
//! would be paying a real defect for a demonstration. Recorded as lost coverage instead.
//!
//! ### The desktop's second window is a PROGRAM (kernel-apps eviction, move #1)
//!
//! This module used to author a 96x64 calibration surface and open a kernel-owned window over it —
//! ~110 ring-0 lines that shipped on every boot. `STAT.ELF` is a ring-3 fixture of the same shape, it
//! is already staged on the DATA volume, and the x86 metal capture has it passing end-to-end 24
//! times. Peter's directive is that the apps in the kernel move into user space, and panel furniture
//! drawn by the kernel for a human to look at is an app by the boundary rule in the eviction survey.
//!
//! **What the eviction COSTS, named rather than glossed.** The deleted `paint_demo` was a calibration
//! target with four distinct diagnostic jobs, and `STAT.ELF` — which draws a pid and a frame counter
//! — replaces none of them one-for-one:
//!
//!   * *content extent vs the kernel-drawn border* — PARTLY covered. Any window's chrome is drawn by
//!     `wm` from its own constants, and the console window's `[wc-x] console-window … box=` line
//!     states the box arithmetic on the wire, so the relationship is checkable without a photograph.
//!   * *GOP channel-order survival* (the R/G/B bars) — **LOST.** Nothing else on the panel proves
//!     `put_pixel`'s per-layout re-encode by eye, and a swapped pair used to show instantly.
//!   * *isotropic nearest-neighbour upscale* (the diagonal) — **LOST as a visual check.** The app is
//!     still upscaled (128x128 at 6x here), so a gross stride error still deforms it, but there is no
//!     figure whose distortion is diagnostic.
//!   * *dropped/duplicated rows* (the single-pixel checkerboard) — **LOST as a visual check**, and
//!     this is the one with an instrumented replacement: `video/wcg.rs` checksums a surface at four
//!     moments around one blit, which asks the same question on the wire instead of on glass.
//!
//! Two of the four are genuinely gone and the eviction survey did not record it. That is a knowing
//! trade — ~110 ring-0 lines on every boot against two by-eye checks nobody has needed since the
//! panel path settled — and it is written here so it is a decision and not an accident.
//!
//! So the demo window is GONE and what replaces it is a launch. The launch cannot happen HERE:
//! [`activate`] runs mid-`takeover_display`, from inside PCI enumeration, where there is no FAT
//! volume (xHCI has not enumerated storage), no shell, and — on the SCHED-X86 split — no core yet
//! dispatching ring 3. What this module does at the seam is ARM the request and say so on the wire;
//! [`desktop_app_service`] performs it, from the device-service task, on the first pass where storage
//! is up. The two lines are deliberately distinct: `ARMED` says the compositor asked for its second
//! window, `LAUNCH`/`DECLINE` says what became of the request. Keeping the ARMED line is what
//! preserves the early-failure signal the kernel-drawn window used to give for free — a boot that
//! arms and never launches is now visibly a boot that lost its program, not a boot that lost its
//! compositor.
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
//! This module authors NO window pixels at all any more — the calibration surface went with the demo
//! window, and the desktop's second window paints itself from ring 3. It performs no framebuffer
//! write of its own and reads the front buffer never — `wm` owns the panel writes and does them
//! through its staged path. There is no direct-front-buffer fallback in this file.
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
/// This REPLACED a re-entry guard (`DEMO_WIN`) that was stored LATE — after the desktop clear, the
/// console window and the demo window — so every DECLINE path returned with it still clear. With one
/// caller that was harmless; with two it is a hole a second entry walks straight through, running a
/// second full-panel `fill_screen` over a live compositor (the exact "second writer" this module's
/// front-buffer law forbids) and then finding `panel_console_window_open` hand back the EXISTING
/// console window against the new geometry.
///
/// So the latch is claimed at the TOP and RELEASED on every decline: a genuinely-failed first
/// attempt must not permanently kill the compositor for the other origin, while a SUCCESSFUL first
/// attempt is final and the second caller is refused with one line.
///
/// ⚠ RESIDUAL, named rather than hidden — and NARROWED to one path by the kernel-apps eviction. The
/// hole is a decline reached AFTER the desktop clear, because the release then hands the next origin
/// an entry that would clear the panel underneath a live console window. There used to be three such
/// paths; deleting the demo window deleted two of them (`geometry-unavailable` and `create-failed`,
/// both of which were the demo's own `spawn_geometry`/`create_at`), and the arming that replaced them
/// cannot fail. **The one that remains is `console-window-declined`** — and it is the least harmful
/// of the three, because it fires when the window table itself refused the console, at which point
/// the compositor owns no window and has nothing to protect. A second caller must still not retry
/// past a post-console decline; the `latch=released` on the line is what tells it which decline it
/// was.
static ACTIVATED: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(ORIGIN_NONE);

/// DESKTOP-APP — the program that IS the desktop's second window, by its 8.3 name in the root of the
/// volume `crate::fs::fat::mount()` binds (on x86 always the USB mass-storage DATA volume — the ESP
/// the firmware booted is unreachable after `ExitBootServices`, so `target/x86_64_data/` is the tree
/// that has to carry it).
///
/// Spelled in the on-disk 8.3 case, and the match is case-insensitive regardless: `DirEntry::eq_name`
/// compares with `eq_ignore_ascii_case` against the short name and the long name alike, so this
/// constant would resolve `stat.elf` as readily. Uppercase is used because it is what the directory
/// literally holds and what every other witness line in the tree quotes.
const DESKTOP_APP: &str = "STAT.ELF";

/// DESKTOP-APP — has [`activate_on`] asked for the desktop's second window yet?
///
/// The whole of the deferral. `activate_on` cannot launch a program (no FAT volume, no ELF loader
/// reachable from inside PCI enumeration, and on the SCHED-X86 split no core dispatching ring 3 yet),
/// so it sets this and prints one line; [`desktop_app_service`] watches it from the device-service
/// task and performs the launch on the first pass where storage is up.
///
/// Deliberately a SECOND flag rather than a read of [`ACTIVATED`]: the latch is RELEASED on every
/// decline path, so a boot whose activation failed late would otherwise look identical to a boot that
/// never ran the seam, and the service task would launch a window onto a desktop the compositor had
/// disowned. This is set exactly once, at the end of a SUCCESSFUL activation, and never cleared.
static DESKTOP_APP_ARMED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// DESKTOP-APP — how long [`desktop_app_service`] waits for a block device before giving up OUT LOUD.
///
/// Generous against this bench: the deferred SCSI bring-up behind `service_storage` can run for
/// seconds after xHCI enumeration, and refusing early would turn a slow stick into a missing one.
/// The number matters far less than the fact that the wait TERMINATES in a line rather than in
/// silence — see the bounded-wait note in `desktop_app_service`.
const STORAGE_WAIT_MS: u64 = 30_000;

/// DESKTOP-APP — `ticks()` at the first service pass that found no block device, or `0` for "the
/// wait has not started". Written and read only by the single pinned service task, hence `Relaxed`.
static WAIT_SINCE_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// DMG-REFUSE HOLD — how long the desktop-app launch defers to the refusal witness before giving up
/// out loud. The witness settles ~1s after storage is ready on the boots in this capture (Boot AL:
/// launch 13.257s, DMG entry 13.859s; DMG's own clean run measured 719ms on AI-2). The chain's own
/// bounded sub-waits (U6BX 10s + U8x 5+2s + WINX-7 10s) could stack past this bound if fixtures time
/// out — that case degrades to today's behaviour (HOLD-EXPIRED, launch, witness NOT RUN), out loud,
/// which is the deliberate trade: 15s is the cap on what a healthy desktop will ever pay for a
/// witness, not a promise the witness fits. Like `STORAGE_WAIT_MS` the job is to TERMINATE in a
/// line rather than in silence.
const DMG_HOLD_MS: u64 = 15_000;

/// DMG-REFUSE HOLD — `ticks()` at the first held pass, or `0` for "the hold has not started".
/// Same single-task access pattern as `WAIT_SINCE_MS`, hence `Relaxed`.
static DMG_HOLD_SINCE_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Gap in panel pixels between a pinned window's outer box and the panel edge. Matches `wm`'s own
/// tiling gap in spirit. Witness-only since the demo window went to ring 3 — the MOVE-VACATE probe is
/// the last thing in this file that pins its own geometry.
#[cfg(feature = "witness")]
const EDGE_GAP: usize = 8;

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

    // RULED — the console becomes a window. Opened FIRST, so the desktop app launched below carries
    // the higher z when its own `SYS_WIN_CREATE` lands.
    // Its own witness lines (`[wc-x] console-window …`) report the geometry and the panic fallback.
    let cwin = super::fbcon::panel_console_window_open();
    if cwin == wm::WIN_NONE {
        ACTIVATED.store(ORIGIN_NONE, Ordering::Release);
        serial_println!("[wc-x] activate DECLINE reason=console-window-declined latch=released");
        return;
    }
    serial_println!("[wc-x] activate panel={}x{} console_win={}", pw, ph, cwin);

    // DESKTOP-APP — ARM the desktop's second window. It is a PROGRAM now, so it cannot be opened
    // from here; see the module docs for why this seam is structurally too early for a launch, and
    // [`desktop_app_service`] for the pass that performs it.
    //
    // Set AFTER the console window, i.e. only on a fully successful activation: every decline above
    // returns with this still clear, so the service task cannot put a window on a desktop the
    // compositor released.
    DESKTOP_APP_ARMED.store(true, Ordering::Release);
    serial_println!(
        "[wc-x] desktop-app ARMED name=/{} (deferred to the device-service pass — no FAT volume or ELF loader at this seam)",
        DESKTOP_APP
    );

    #[cfg(feature = "witness")]
    move_vacate_probe(pw, ph);

    // INSTGUI — the graphical installer dialog, in front of everything (created last = top z).
    // Double-gated: `wc` (this module) AND `instgui` (`UNAOS_INSTGUI=1`).
    #[cfg(feature = "instgui")]
    super::instgui::open();
}

/// DESKTOP-APP — perform the deferred launch of the desktop's second window. **One shot per boot**,
/// and a no-op on every pass but one.
///
/// ### Where this must be called from, and why it is not a choice
///
/// From `x86_usb_pump`, the SCHED-X86 DEVICE-SERVICE task, beside `fat::probe_once`. Three
/// constraints pick that site and together they leave no other:
///
///  1. **It reads the FAT volume**, and a FAT read takes `XHCI_CONTROLLER` — a raw `spin::Mutex`.
///     `main.rs`'s placement rule is explicit that no xHCI taker may be added to the RENDER core, so
///     the render/shell task is out; the service core is where the xHCI takers live.
///  2. **It spawns a ring-3 task**, and `spawn_user_image_bg` places it on `bg_place_cpu()` = the
///     CALLER's core. That is only correct from a context that is itself dispatching. The BSP's
///     inline GUI loop never enters the scheduler — the exact defect SCHED-X86 named, where two BGRUN
///     spawns produced zero `SYS_WIN_CREATE` — so the call must come from a scheduled task, and this
///     task is one. Nothing here fires on the inline-loop fallback, and that is deliberate rather
///     than an omission: a launch there would be a job that never runs.
///  3. **It must WAIT for storage.** The seam that arms it runs during PCI enumeration, long before
///     xHCI has enumerated a mass-storage device. A service pass repeats, so "not yet" costs one
///     atomic load and the launch lands on the first pass that can serve it.
///
/// ### Failure is one line and boot continues
///
/// Every exit after the storage gate prints exactly one `[wc-x] desktop-app …` line naming the
/// reason, and the one-shot is consumed either way — a volume with no `STAT.ELF` refuses once, not
/// once per millisecond. Nothing else about the boot changes: the compositor, the console window and
/// the desktop are already up, and the panel simply carries one window instead of two.
pub fn desktop_app_service() {
    use core::sync::atomic::Ordering;
    static DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

    // Not armed = the activation never completed. Cheapest test first, and it is the one that is
    // false on every boot without the Kepler takeover.
    if !DESKTOP_APP_ARMED.load(Ordering::Acquire) {
        return;
    }
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    // The same storage gate `fat::probe_once` uses, for the same reason: `mount()` on a boot whose
    // xHCI has not yet enumerated a stick is a "no volume" that means "not yet", not "not ever", and
    // consuming the one-shot on it would refuse a launch the media could have served.
    //
    // BOUNDED, not open-ended. Waiting forever was the honest-looking version and it is the weakest
    // form of evidence this tree accepts: on a boot where nothing ever enumerates, an unbounded wait
    // discharges the ARMED line's stated job — "a boot that arms and never launches is visibly a boot
    // that lost its program" — purely by ABSENCE. So the wait gives up OUT LOUD. The clock starts at
    // the first pass rather than at boot, so a late activation does not eat the budget, and
    // `.max(1)` keeps `0` meaning "not started" even in the vanishing case where `ticks()` reads 0.
    //
    // APPLOAD: `program_source()`, not `info()`. `info()` reads the GLOBAL handle alone, so a machine
    // booted from the INTERNAL SD reader with no USB stick declined here — for thirty seconds and
    // then permanently — while `:: SDHCBLK: … STAT.ELF …` sat a few lines up in the same capture,
    // naming the very file this launch wanted. The question this gate asks is "can I load a program
    // from anywhere", and that question has a block-layer answer now.
    if crate::drivers::block::program_source().is_none() {
        let now = crate::arch::ticks();
        let started = WAIT_SINCE_MS.load(Ordering::Relaxed);
        if started == 0 {
            WAIT_SINCE_MS.store(now.max(1), Ordering::Relaxed);
            return;
        }
        let waited = now.saturating_sub(started);
        if waited < STORAGE_WAIT_MS {
            return;
        }
        DONE.store(true, Ordering::Relaxed);
        // `waited`, the MEASUREMENT — not `STORAGE_WAIT_MS`, the threshold. At the 1 kHz service
        // pass the two differ by a millisecond, so printing the constant was accurate and still
        // wrong in kind: a constant sitting in a measurement field is the exact shape this tree's
        // instrument laws exist to catch, and the real value was already computed one line up. If
        // this ever reads far above the threshold, the service pass itself stalled — which the
        // constant could never have told anyone.
        //
        // APPLOAD: the census, not a boot-wide claim. The old tail read "no block device ever
        // enumerated" — an assertion about the whole machine drawn from ONE of three registry slots,
        // and it was false on the boot that exposed this. `handles=` says what was actually
        // inspected, so this refusal is falsifiable by the capture it appears in.
        serial_println!(
            "[wc-x] desktop-app DECLINE reason=no-storage name=/{} waited={}ms threshold={}ms handles={} — no handle offered a program source; the desktop keeps the console window only",
            DESKTOP_APP, waited, STORAGE_WAIT_MS, crate::drivers::block::source_census()
        );
        return;
    }
    // DMG-REFUSE HOLD. The refusal witness requires the window table EMPTY at entry, and this launch
    // is permanent — on Boot AL it took row 1 at 13.257s, the witness entered at 13.859s, and the
    // capture got its first `NOT RUN` after three boots of `19/19 — witness OK`. On a `witness`
    // build the flag settles when the launcher chain completes; on every other shape (witness off —
    // the default esp-x86 media — or a chain leg that never arrives) the flag is BORN settled or the
    // bound expires OUT LOUD — a desktop that never appears is worse than one lost witness, and the
    // loss goes on the record either way.
    if !crate::arch::syscall::DMG_REFUSE_SETTLED.load(Ordering::Acquire) {
        let now = crate::arch::ticks();
        let started = DMG_HOLD_SINCE_MS.load(Ordering::Relaxed);
        if started == 0 {
            DMG_HOLD_SINCE_MS.store(now.max(1), Ordering::Relaxed);
            return;
        }
        let waited = now.saturating_sub(started);
        if waited < DMG_HOLD_MS {
            return;
        }
        serial_println!(
            "[wc-x] desktop-app HOLD-EXPIRED reason=dmg-refuse-unsettled name=/{} waited={}ms threshold={}ms — launching anyway; the refusal witness will print NOT RUN if it arms after this",
            DESKTOP_APP, waited, DMG_HOLD_MS
        );
        // fall through: the launch proceeds and the lost coverage is on the serial record.
    }
    DONE.store(true, Ordering::Relaxed);

    // APPLOAD: the same handle ladder the storage gate above cleared — mounting `Default` here after
    // clearing the gate on `program_source` would be the identical defect one line later.
    let Ok(fs) = crate::fs::fat::mount_program_source() else {
        serial_println!(
            "[wc-x] desktop-app DECLINE reason=no-fat-volume name=/{} handles={} — the desktop keeps the console window only",
            DESKTOP_APP, crate::drivers::block::source_census()
        );
        return;
    };
    let Ok(de) = fs.find_in_root(DESKTOP_APP) else {
        // Name the VOLUME, not "the boot volume": `fat::mount()` binds the USB mass-storage device
        // xHCI enumerated, and the volume UEFI booted from is a different thing the kernel cannot
        // read after `ExitBootServices`. The WINX-2 witness learned this the hard way on an attended
        // rMBP boot and its message is the model for this one.
        serial_println!(
            "[wc-x] desktop-app DECLINE reason=absent name=/{} — not on the mounted DATA volume (the USB mass-storage device, NOT the UEFI boot volume); stage target/x86_64_data/ onto it",
            DESKTOP_APP
        );
        return;
    };
    // A DIRECTORY called `STAT.ELF` would otherwise reach `read_file`, come back `IsDirectory`, and
    // be reported as `read-failed` — contained, but mislabelled, and the operator would go looking
    // for a bad volume instead of a bad name.
    if de.is_dir {
        serial_println!(
            "[wc-x] desktop-app DECLINE reason=is-directory name=/{}", DESKTOP_APP
        );
        return;
    }
    let cap = crate::arch::syscall::user_window_size();
    if de.size == 0 || de.size as usize > cap {
        serial_println!(
            "[wc-x] desktop-app DECLINE reason=size name=/{} bytes={} cap={} (the ring-3 program window)",
            DESKTOP_APP, de.size, cap
        );
        return;
    }
    let mut bytes = alloc::vec![0u8; de.size as usize];
    if fs.read_file(&de, &mut bytes, cap).is_err() {
        serial_println!("[wc-x] desktop-app DECLINE reason=read-failed name=/{}", DESKTOP_APP);
        return;
    }
    // SHORT READ. `read_file`'s own doc: a file whose cluster chain ends before `de.size` yields a
    // SHORT READ rather than an error. FATREAD-1 was exactly this class of silent mismatch — it is
    // what blocked `bg /fat/STAT.ELF` on x86 — and the shell's `read_el0_image` carries this check
    // for that reason. Without it the loader gets a truncated image and reports something unrelated.
    if bytes.len() != de.size as usize {
        serial_println!(
            "[wc-x] desktop-app DECLINE reason=short-read name=/{} got={} want={}",
            DESKTOP_APP, bytes.len(), de.size
        );
        return;
    }
    if !(bytes.len() >= 4 && bytes[0..4] == [0x7F, b'E', b'L', b'F']) {
        // The loader would otherwise take this down its FLAT-blob path. Same refusal, and the same
        // reason, as the shell's BARE-EXEC arm: a name launches ELF programs, nothing else.
        //
        // `not-elf64-MAGIC`, not `not-elf64`. The bare form is a strict PREFIX of the
        // `not-elf64-class` reason below, and this tree reads captures with `awk '/pattern/'` — so
        // `awk '/reason=not-elf64/'` would match BOTH refusals and an analyst counting them after
        // the flight would silently conflate "not an ELF at all" with "an ELF32". It was the only
        // prefix collision among the image's `reason=` literals; every one is now independently
        // greppable, which is a property worth keeping as reasons are added.
        serial_println!(
            "[wc-x] desktop-app DECLINE reason=not-elf64-magic name=/{} bytes={}",
            DESKTOP_APP, bytes.len()
        );
        return;
    }
    // THE HEADER CHECKS THE SIBLING LOADER PATH ALREADY MAKES — `read_el0_image`'s, verbatim in
    // meaning if not in wording. The `e_machine` one is the load-bearing addition and the reason this
    // block exists at all: **both arches stage a file called `STAT.ELF`**, so an aarch64 image on x86
    // media is the single most likely media mistake this arc will meet, and it is precisely the
    // mistake the eviction's whole failure story is about. Without these it lands as
    // `reason=spawn-rejected why=<loader string>` — contained, but naming the loader's complaint
    // rather than the operator's error.
    if bytes.len() < 20 {
        serial_println!(
            "[wc-x] desktop-app DECLINE reason=truncated-header name=/{} bytes={} (need >= 20 for e_machine)",
            DESKTOP_APP, bytes.len()
        );
        return;
    }
    if bytes[4] != 2 {
        serial_println!(
            "[wc-x] desktop-app DECLINE reason=not-elf64-class name=/{} ei_class={} (want 2)",
            DESKTOP_APP, bytes[4]
        );
        return;
    }
    if bytes[5] != 1 {
        serial_println!(
            "[wc-x] desktop-app DECLINE reason=not-little-endian name=/{} ei_data={} (want 1)",
            DESKTOP_APP, bytes[5]
        );
        return;
    }
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    if machine != 62 {
        // 62 = EM_X86_64; 183 = EM_AARCH64, i.e. the Pi's STAT.ELF staged on x86 media.
        serial_println!(
            "[wc-x] desktop-app DECLINE reason=wrong-arch name=/{} e_machine={} (want 62 = EM_X86_64; 183 = EM_AARCH64 means the aarch64 STAT.ELF is on this stick)",
            DESKTOP_APP, machine
        );
        return;
    }

    // THE SAME LOADER `bg` AND BARE-EXEC TAKE. Not a new path: per-segment W^X, the ring-3 window
    // bound and the fault-kill net are whatever `spawn_user_image_bg` already enforces, and the slot
    // is marked DETACHED before the task can run, so the program's first `SYS_WIN_CREATE` sees the
    // flag exactly as a `bg`'d one does.
    //
    // The ONE divergence, and it is the difference between furniture and a launch the operator asked
    // for: `..._no_autofocus` suppresses the first-window focus GRANT. `sys_win_create` hands the
    // keyboard to any program that opens a window while nobody holds focus — which the shell, at
    // boot, definitionally does not — and `STAT.ELF` has no `SYS_INPUT_POLL` at all, so an
    // unqualified launch here would end the boot with the keyboard published to a program that
    // cannot read it. The window this replaces was `KERNEL_OWNER_DESKTOP` and was outside the focus
    // ring by construction; this is that property carried across the eviction. The app stays
    // hittable and stays in the TAB ring — only the automatic grant is withheld.
    match crate::arch::syscall::spawn_user_image_bg_no_autofocus(&bytes) {
        Ok((pid, slot, entry)) => {
            // Register it in the shell's job table, under the same canonical path a typed launch
            // would record. `jobs` must list the desktop app and `kill <pid>` must reach it: the
            // verb resolves a pid through that table and refuses one it cannot find, so skipping
            // this would leave a ring-3 program running that the operator can neither see nor stop.
            // Unlike BARE-EXEC this does NOT kill the job when the table is full — the table is
            // empty at boot so it cannot happen, and killing the desktop's only window over a
            // bookkeeping failure would be the worse outcome if it somehow did. Say so instead.
            let tracked = crate::shell::adopt_bg_job(pid, slot, "/STAT.ELF");
            serial_println!(
                "[wc-x] desktop-app LAUNCH name=/{} bytes={} entry={:#x} pid={} slot={} DETACHED, left RUNNING, tracked={}",
                DESKTOP_APP, bytes.len(), entry, pid, slot, tracked
            );
        }
        Err(why) => serial_println!(
            "[wc-x] desktop-app DECLINE reason=spawn-rejected name=/{} why={}",
            DESKTOP_APP, why
        ),
    }
}

/// The probe surface for [`move_vacate_probe`] — one flat colour, chosen to be nothing else on the
/// panel: not [`wm::DESKTOP_BG`], not the chrome, not any bar the desktop app draws.
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
