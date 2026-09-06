// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CONSWIN-PI / MENUBAR-PI — **the Pi's DESKTOP-READY seam.**
//!
//! ## What this module is
//!
//! [`super::desktop_uefi::activate`] is x86's answer to one question: *at which single point in the boot does
//! the machine stop being a kernel with a framebuffer and start being a desktop?* Everything the
//! crispy desktop needs to be true at once — the console is a window, the menu bar is on, the
//! compositor owns the glass — is decided there, in order, with a DECLINE arm for every way it can
//! fail and a witness line for every decision.
//!
//! The Pi had no such point. `desktop_firmware` compiled the furniture (`strip`, `dock`, `menubar`, `crystal`)
//! and routed presses to it (`arch::aarch64::syscall`'s furniture arm), but nothing ever *turned the
//! desktop on*: `menubar::ENABLED` starts `false` and every `set_enabled(true)` in the tree was a
//! `witness` fixture that restored the flag before returning — so the bar was compiled, composed
//! (three relaxed atomics per pass) and permanently invisible. The console had no window at all.
//!
//! This is that point, for the Pi. It is deliberately NOT a port of `desktop_uefi::activate`'s body: the
//! Kepler takeover, the backbuffer resync, the ARGB origin latch and the deferred desktop-app launch
//! are all x86 display-driver concerns with no Pi counterpart. What is shared is the DECISION
//! SEQUENCE and its order, and where the two arches do the same thing they call the same function —
//! `fbcon::panel_console_window_open`, `menubar::set_enabled`, `wm::composite` are one implementation
//! each, reached from two seams.
//!
//! ## The order, and why each step is where it is
//!
//! 1. **The panel must exist.** Geometry off `video::WRITER`, live, never assumed: QEMU raspi4b is
//!    640x480 and the bench Pi is 1920x1200, and every floor below is evaluated against whichever
//!    one this boot actually has.
//! 1b. **The panel is CLEARED to the desktop colour** — `desktop_uefi`'s WC-X DESKTOP-CLEAR, on this arch for
//!    the first time. Everything painted before the desktop existed (`video::init_panel`'s
//!    `PANEL_BG` fill, the direct-painted fbcon boot log) sits outside every damage box the
//!    compositor has, so nothing else in the system will ever repaint it. It goes above step 2
//!    because it is the last instant at which the window table is empty and a direct front-buffer
//!    write therefore collides with no compositor-owned pixel. See the call site for the full
//!    argument, and for why the WC-BBSYNC half of x86's pairing is deliberately not armed here.
//! 2. **The dock must be able to host a full strip** — `desktop_uefi`'s CONSOLEWIN law, unchanged and for the
//!    unchanged reason. The console window carries a minimise disc, the only route back from that
//!    park is the dock, and `dock::Layout::for_panel` returns `None` when the strip will not fit at
//!    `MAX_WINDOWS` rows. A control that hides a window with no way back is worse than no control, so
//!    a panel that cannot guarantee the dock gets no console window. `MAX_WINDOWS` and not the live
//!    count, because the check must hold for every table state the boot can reach.
//! 3. **The console becomes a window** — and on the Pi it becomes a LIVE one. See
//!    [`super::fbcon::console_is_routed`] for the full argument; the short form is that the handoff's
//!    `fbcon::detach()` exists to keep two cores off the panel, a routed console does not touch the
//!    panel, and so the caller skips the detach when this step succeeded.
//! 4. **The bar is enabled** — the tenancy claimed, by the seam that is the Pi's shell coming up.
//!    After every decline above, on `desktop_uefi`'s ownership argument: turning the bar on is a SHELL's
//!    decision and this is where the Pi's spatial shell is brought up.
//! 5. **One composite** — load-bearing, not a flourish. From the instant the bar is enabled,
//!    `Screen::present_background` subtracts its rect (this arc widened that subtraction to aarch64),
//!    so those rows stop being desktop pixels and only a COMPOSITE can fill them. `wm::service_damage`
//!    returns without compositing while no row is dirty, so on a quiet boot nothing would ever paint
//!    them and the bar would be enabled, its rows withheld, and the strip never drawn — SHELLDESK's
//!    exact symptom, reached on a second arch. One pass at the seam closes it by construction.
//!
//! ## What this does NOT do
//!
//! No `DESKTOP_APP_ARMED` equivalent: the Pi's window population comes from the `u11` fixture cascade
//! and the ring-3 vug loader, both already running by this point, and inventing a second launcher here
//! would duplicate them. No shell window either — that is `exec-shellport`'s lane and a DIFFERENT
//! window from this one; x86 carries both (`fbcon`'s `KERNEL_OWNER_CONSOLE` row and `main.rs`'s
//! `open_shell_window` `KERNEL_OWNER_DESKTOP` row) and the Pi is meant to end up the same way.

use super::{fbcon, menubar, wm};

/// Has [`activate`] already run? One-shot: the seam is reached once per boot from the GUI handoff,
/// and a second pass would re-enable an already-enabled bar and ask `panel_console_window_open` for a
/// window it would idempotently hand straight back. Latched rather than trusted, because the handoff
/// block is inside a `cfg` maze that a future arc could reach twice.
static ACTIVATED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// CONSWIN-PI / MENUBAR-PI — **bring the Pi desktop up. Returns `true` iff the console is ROUTED**,
/// which is the caller's cue to skip the GUI handoff's `fbcon::detach()`.
///
/// The return value is deliberately narrow. It is not "did the desktop come up" (the bar can be on
/// with no console window, and that is a perfectly good desktop); it is the single fact the caller
/// has to act on, and it is read back from [`fbcon::console_is_routed`] rather than inferred from
/// this function's own control flow — so a route that was declined deep inside the open path can
/// never be reported as installed by a caller reading a stale local.
pub fn activate() -> bool {
    if ACTIVATED.swap(true, core::sync::atomic::Ordering::AcqRel) {
        serial_println!("[pidesk] activate SKIP reason=already-active");
        return fbcon::console_is_routed();
    }

    // 1. The panel, live off the surface the compositor itself composites onto.
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        let i = fb.info();
        (i.width, i.height)
    };
    if pw == 0 || ph == 0 {
        serial_println!("[pidesk] activate DECLINE reason=no-panel");
        return false;
    }

    // 1b. PIDESK DESKTOP-CLEAR — **paint the whole panel to the desktop colour ONCE, here.** The
    //    aarch64 counterpart of `desktop_uefi`'s WC-X DESKTOP-CLEAR, and the gap `wcf::chrome_truth` was
    //    built to name: its desktop probe read `(pw-2, ph-2)` on the bench panel and printed
    //    `want=0x2d2b55 got=0x1e1e1e -> NOCLEAR` — `video::PANEL_BG`, the fill `video::init_panel`
    //    puts down at FB attach, still on the glass at desktop-ready.
    //
    //    Why the compositor cannot do this from inside a pass, on either arch: `composite` paints
    //    its windows' boxes and `erase` paints boxes windows have VACATED (that half is already
    //    arch-neutral — `wm::erase` is ungated, so close/move exposure on the Pi has always been
    //    repainted `DESKTOP_BG`). Neither has any claim on panel pixels the window layer has never
    //    owned, so everything painted before the desktop existed — `init_panel`'s `PANEL_BG`, the
    //    direct-painted fbcon boot log, whatever a demo left — is outside every damage box there is
    //    and stays on glass for the rest of the boot.
    //
    //    Why a DIRECT front-buffer write is sound HERE and nowhere else, restated for this arch
    //    rather than inherited: `wm` exposes no full-panel erase verb, and the no-direct-writes law
    //    protects COMPOSITOR-OWNED pixels from a second writer — at this line there are none. The
    //    window table is empty (the boot capture proves it rather than assuming it: `[wc-a] create
    //    win=1` is the console window minted by `panel_console_window_open` twenty lines below, and
    //    it is the first create of the boot), `STAGE` has no pass to collide with, and the GUI
    //    handoff has not yet spawned the render or input tasks. It is also the LAST moment that is
    //    true, which is why the clear goes above the console window rather than beside the bar.
    //
    //    CURSOR-1's bracket applies for `erase`'s reason: a sprite on the panel would be painted
    //    over and its save-under would later restore pre-clear pixels as a stale patch. Take it off
    //    first; the composite at step 5 puts it back.
    //
    //    WC-BBSYNC is deliberately NOT armed here. x86 pairs its clear with
    //    `screen::adopt_desktop_bg` so a `Screen` built later is born agreeing with the glass; on
    //    the Pi that latch would also be read by `video::witness::run`, which builds a `Screen` over
    //    a HEAP buffer and asserts "baseline flush left non-zero front" — seeding it would fail a
    //    passing gate for a surface that is not the panel. The Pi does not need it either way: its
    //    `render_service` calls `console.draw` (a whole-panel `clear_screen` at `Console::BG`, which
    //    is the same number as `wm::DESKTOP_BG`) before its first `pal.render`, so no zeroed back
    //    buffer ever reaches the glass. Named rather than silently skipped.
    //    DESKHOLD — **and the panel mirror goes off with the same statement that takes the glass.**
    //
    //    The clear above is the line after which the compositor owns every panel pixel. Until this
    //    arc, fbcon went on painting those same pixels: aarch64's `_print` has no counterpart of
    //    x86's QUIET-PANEL gate, so the whole serial stream kept mirroring onto the glass, from
    //    every core, all the way until the GUI handoff's `detach()`. That is a SECOND WRITER on
    //    compositor-owned pixels — precisely what the paragraph above calls the no-direct-writes
    //    law's whole subject — and it is loud: `plan_newline` bands are FULL PANEL WIDTH and one
    //    cell tall, so a printing core repaints `2 * cell_h` rows edge to edge per line, which
    //    FONT-PI's 8x8 -> 7x16 cell doubled while also making every glyph paint an opaque cell.
    //
    //    It convicted three witnesses at once on the armed gate, all of them reading a panel this
    //    writer had just been over: `[wc-d] verify win=1 band=0..80 … got=0x1b1b1b want=0x000000`
    //    (an anti-aliased `FG_DEFAULT` edge over `BG_DEFAULT` — a value the pre-FONT-PI 1-bit face
    //    could not produce), `[wc-f] twin … comp_bad=4096 direct_bad=4096 got=0x000000` (both probe
    //    blocks wholly `BG_DEFAULT` between the probe's paint and its read-back), and
    //    `[wc-x] console-window DECLINE reason=install-contended` (a printing core holding
    //    `FBCON` at the instant the route is installed).
    //
    //    x86 has had the right behaviour all along — `desktop_uefi`'s desktop never mirrors — so this is a
    //    ONE OS defect on the same reading REALDESK gave the two retired bands, and the fix is the
    //    same shape: the Pi stops doing the aarch64-only thing. Nothing is lost that this kernel
    //    keeps evidence in — every line is still on the serial wire, and from the route install
    //    thirty lines below the console's way to glass is its WINDOW, which is why the hold lifts
    //    there by construction rather than by a second call. See `fbcon::panel_mirror_held` for that
    //    term, for the panic override, and for what the decline path gets.
    {
        super::cursor::undraw();
        fbcon::panel_mirror_hold(true);
        let fb = *super::WRITER.lock();
        fb.fill_screen(wm::DESKTOP_BG);
        fb.flush_all();
    }
    serial_println!(
        "[pidesk] desktop-clear panel={}x{} bg={:08X} (pre-desktop residue off the glass; the window table is empty at this line; the panel mirror is held from this line — DESKHOLD)",
        pw,
        ph,
        wm::DESKTOP_BG
    );

    // 2a. FONT-PI — **the console leaves font8x8, here.** x86 arms the shared anti-aliased face at
    //    the Kepler takeover (`fbcon::panel_console_resume`); the Pi has no takeover and, until this
    //    line, had no arming at all — `FbCon::aa`'s only writer was `#[cfg(target_arch = "x86_64")]`,
    //    so a Pi desktop boot put anti-aliased captions, bar, menu and dock captions around a console
    //    still drawing a 1-bit 8x8 cell at scale 1 (~0.8 mm on the 1920x1200 bench panel).
    //
    //    ABOVE the dock check on purpose, and above the window open by necessity:
    //      * by NECESSITY, because `panel_console_window_open` sizes the console's surface in whole
    //        CELLS and must therefore see the face's cell, not the bitmap's;
    //      * on PURPOSE above the dock check, because the face is a legibility decision and the dock
    //        check is a routing one.
    //
    //    DESKHOLD retired the third reason this line used to give — "on the decline path the boot log
    //    stays on the glass, and that is where legible glass text is worth MORE". It does not stay:
    //    the mirror is held from the DESKTOP-CLEAR above, so from here the face decides how the
    //    console looks IN ITS WINDOW and nothing else. The arming stays where it is for the two
    //    reasons above, both of which are unaffected — and it now also has the property the old
    //    ordering did not: the bigger cell can no longer be painted onto the glass in the window
    //    between this line and the route install, because nothing is painted onto the glass there.
    let face_cell = fbcon::panel_console_face_arm();
    // 2b. CASCADEFIT — **the desktop DECLARES its boot windows before it PLACES any of them.**
    //
    //     `super::pulsewin::arm()` is step 6's, moved to run here as well and left there untouched
    //     (it is two release stores and idempotent, so the second call is a no-op and step 6's
    //     ordering argument — after the bar, after the composite — is about `open`, which is still
    //     the render pass's and still happens exactly where it did).
    //
    //     It has to be said HERE because of what it lets the next step do. `fbcon`'s console window
    //     opens twenty lines below and the pulse window opens minutes later on a render pass, and
    //     until this arc the first knew nothing about the second — so on the bench panel the console
    //     was centred in a work area whose bottom 232 rows the pulse window was going to claim, and
    //     its box came down over the pulse's entire title band (render8: 61 rows, close disc and drag
    //     handle unreachable for the life of the boot). `fbcon::console_work_bottom` now asks
    //     `pulsewin::boot_keepout_top` where that window will be, and that question answers `None`
    //     until the desktop has armed it. Arming after the console was placed made the answer a lie.
    //
    //     Nothing else moves and no `cfg` is involved: an x86 `desktop_uefi` desktop never reaches
    //     this function, so `ever_armed()` stays false there and its console is placed exactly where
    //     it always was. The fit itself is stated on the wire by `[deskcascade] fit … overlap_rows=`.
    super::pulsewin::arm();
    if face_cell.is_none() {
        serial_println!("[pidesk] console-face DECLINE reason=console-not-ready (the console keeps font8x8)");
    }

    // 2-3. CONSOLEWIN — the dock must be able to host the worst-case strip, or the console gets no
    //    window (and therefore no minimise disc, and therefore nothing to strand). `desktop_uefi`'s law,
    //    unchanged in substance and NARROWED in scope, which is the one place the Pi's sequence
    //    deliberately differs from x86's.
    //
    //    On x86 this check is a TOTAL decline: `desktop_uefi::activate` returns, and the menu bar — enabled
    //    thirty lines further down — never happens. That is sound there because on x86 the console
    //    window is a precondition of the desktop being built around it (the desktop app is armed only
    //    on a fully successful activation; the shell window is minted against the same panel).
    //
    //    It is NOT sound here, and the gate surface proves it rather than the argument. The law's own
    //    justification is about ONE control on ONE window: "the console's minimise disc would have no
    //    way back". The bar is a strip tenant that reads the panel geometry and the window table, and
    //    the console window is part of neither; its floors (`menubar::FLOOR_W`/`FLOOR_H`) are
    //    `const`-asserted to fit 640x480 precisely so no gate declines it. Under the x86 ordering the
    //    first `UNAOS_PIDESK=1 kernel8-test` run of this arc printed
    //    `[desktop_firmware] activate DECLINE reason=dock-cannot-host-full-strip panel=640x480 rows=12` and
    //    stopped — QEMU raspi4b's 640x480 cannot host a twelve-tile dock — so the menu bar half of the
    //    arc would have been unwitnessed on the only surface the DONE gate runs, and invisible to any
    //    operator on a small panel, for a reason that has nothing to do with menu bars.
    //
    //    So the check guards exactly what it argues about: the console window. The bar follows either
    //    way. Nothing about the law is weakened — a panel that cannot guarantee the dock still gets no
    //    console window, hence no minimise disc, hence nothing to strand.
    let cwin = if super::dock::Layout::for_panel(wm::MAX_WINDOWS, pw, ph).is_none() {
        serial_println!(
            "[pidesk] console-window DECLINE reason=dock-cannot-host-full-strip panel={}x{} rows={} \
             (the console's minimise disc would have no way back) — the bar is unaffected and follows",
            pw,
            ph,
            wm::MAX_WINDOWS
        );
        wm::WIN_NONE
    } else {
        // The console becomes a window. Its own `[wc-x] console-window …` witness reports the
        // geometry and the panic fallback — that tag is the SHARED routed-console channel's, emitted
        // by the shared `fbcon` code this arc widened rather than by anything Pi-specific, and it is
        // left exactly as written because `scripts/specs/x86-witness.spec` REQUIREs that wording.
        fbcon::panel_console_window_open()
    };
    let routed = fbcon::console_is_routed();
    if cwin == wm::WIN_NONE {
        serial_println!(
            "[pidesk] console-window ABSENT — continuing to the bar (DESKHOLD: the boot log is serial-only from the desktop-clear, exactly as on an x86 `wc` desktop, and the handoff will detach as before)"
        );
    } else {
        // `routed=true` states that the glyph ROUTE is installed — every console line from here lands
        // in the window's surface instead of on the panel. It is NOT a claim that lines keep coming:
        // the handoff's `fbcon::detach()` follows this seam and stops them, so the window holds the
        // desktop-bringup tail and then freezes. See the live-console ledger at the tail of this
        // function for why the detach stays, and `desktop_uefi.rs` for x86 doing the identical thing.
        // ⚠ THE WIRE WORD BELOW IS THE KNOB-OFF TRUTH and is left exactly as written because the
        // regression specs anchor on it. With LIVECON armed the window does NOT freeze, and the
        // correction is stated in its own `[desktop_firmware] livecon ARMED` line at the tail rather than by
        // rewording a string a passing spec matches on.
        serial_println!("[pidesk] activate panel={}x{} console_win={} routed={} (the window freezes at the handoff detach that follows — x86's desktop lane does the same)", pw, ph, cwin, routed);
    }

    // 3b. FONT-WITNESS — **which face each text surface on this desktop actually drew with.**
    //
    //    Every claim in this arc is about a face, and a face is the one thing a serial log cannot
    //    show by accident: an anti-aliased caption and a bitmap caption print the same characters.
    //    So the seam states it. Each term is READ from the surface's own metric rather than asserted
    //    here — the chrome names come from `font::Face::Chrome`, whose raster is a function of
    //    `theme::TITLE_HEIGHT`, and the console term is the cell `panel_console_face_arm` actually
    //    installed (or `font8x8`, honestly, if it declined). A boot that regressed any surface back
    //    to the bitmap prints it, on the metal, with nobody looking at the glass.
    //
    //    `chrome=` is the FONT-METRIC number: Peter on the bench (PA43, 1920x1200) reported the
    //    caption reading small for a 34 px bar, and the fix was to derive the chrome raster from the
    //    bar rather than fix it at the body face's 16. This line is where that derivation's result
    //    becomes visible without a rebuild.
    let (cface, ccell) = match face_cell {
        Some(c) => (super::font::Face::Body.name(), c),
        None => ("font8x8", (0, 0)),
    };
    serial_println!(
        "[pidesk] faces=title:{},menu:{},crystal:{},dock:{},console:{} chrome={}x{} body={}x{} bar={} ::",
        super::font::Face::Chrome.name(),
        super::font::Face::Chrome.name(),
        super::font::Face::Chrome.name(),
        super::font::Face::Chrome.name(),
        cface,
        super::font::Face::Chrome.cell_w(),
        super::font::Face::Chrome.cell_h(),
        ccell.0,
        ccell.1,
        wm::TITLE_H,
    );

    // 4. SHELLDESK, on the Pi — the tenancy is CLAIMED. Before this line every `set_enabled(true)` in
    //    the tree was a `witness` fixture that put the flag back before it returned, on aarch64 as on
    //    x86; a bar nothing enables is a bar the operator cannot see, however correct its geometry.
    let bar_was = menubar::set_enabled(true);
    serial_println!(
        "[pidesk] menubar ENABLED panel={}x{} rect={:?} was={} (the desktop scene owns the top of the glass)",
        pw,
        ph,
        menubar::strip_rect(pw, ph),
        bar_was
    );

    // 5. And COMPOSITE, so the rows the enable just took are painted. See the module header: without
    //    this, a quiet boot withholds the strip's rows from the desktop present and never fills them.
    wm::composite();
    // BRINGUP-PAINT (PA41) — **read the paint back; do not infer it from having called `composite`.**
    //
    // M1's step 5 asserted "menubar PAINTED" from its own control flow, which is the exact inference
    // this function refuses two steps above for `console_is_routed`. It is not sound here either:
    // `strip::paint` declines the pass without touching a pixel on a contended `SCRATCH` (the dock is
    // tenant #1 and takes the same scratch in the same pass) or a surface that is not yet `word4`, and
    // `menubar::compose` then returns `false` with its slot untouched. The bar is left ENABLED — so
    // `screen::present_background` is already subtracting its rows from every desktop present — with
    // nothing on the glass and no damage condition able to notice, which is a bar that is invisible
    // for the rest of the boot on exactly the quiet desktop this seam exists to serve. One re-run
    // discharges it, and `owns_pixels` is the same packed load `compose` acts on, so the retry runs
    // iff the first pass did not land. PA41's metal reading — a bar that "came up INCOMPLETE" and
    // filled in only under pointer activity — is this hole and the clobber hole below, together.
    let repainted = if menubar::enabled() && !menubar::owns_pixels() {
        wm::composite();
        true
    } else {
        false
    };
    serial_println!(
        "[pidesk] menubar PAINTED owns_pixels={} retried={} (composite at the enable seam, read back rather than assumed)",
        menubar::owns_pixels(),
        repainted
    );

    // The SHARD menu is reachable from this instant: `crystal::press_at` hit-tests
    // `menubar::crystal_corner_abs` (FITTS-CORNER — the bar's whole upper-left corner cell, not just
    // the glyph), and the aarch64 click router's furniture arm
    // (`arch::aarch64::syscall::wc_click_route`, `feature = "desktop_firmware"`) already calls
    // `strip::press_route` ahead of every window arm. Stated on the wire because "the bar is painted"
    // and "the bar is live" are two claims and a capture should not have to infer the second.
    serial_println!(
        "[pidesk] crystal LIVE — press the crystal for the SHARD menu (About is real on aarch64; Sleep/Restart/Shut Down print their honest unimplemented lines — no PSCI wiring on this track yet)"
    );

    // SHARD-PRESS (PA41) — and that claim is now WITNESSED rather than asserted. This is the Pi's
    // first furniture fixture: `crystal::selftest`, `dock::selftest` and `menubar::selftest` are all
    // invoked from `arch/x86_64/syscall.rs` alone, so before this line no aarch64 boot had ever run a
    // menu-bar or crystal fixture and the whole family was unwitnessed on the arch it had just been
    // ported to. It is invoked HERE, at the seam, and not from the aarch64 selftest cascade for two
    // reasons: this is the only point on the Pi where the bar is known to be enabled and painted, and
    // `arch/aarch64/syscall.rs` is compiled into the knob-off `kernel8.img` whose byte-identity proof
    // forbids adding a line to it (PARITY.md §5.3) — this file is not.
    //
    // It presses the crystal through `strip::press_route` (the live shared router core) and asserts
    // the dropdown reached the GLASS, which is the leg that reds without MENU-DRIVE. Side-effect-free:
    // both presses are consumed by the menu band, and it leaves the menu closed and the bar as it
    // found it.
    #[cfg(feature = "witness")]
    super::crystal::routed_selftest();
    // 6. PULSEWIN — **the core-load instrument gets a window.** Peter, this cycle: *"core load
    //    distribution — pulse needs a window."*
    //
    //    Placed after the bar and after the composite, and both orderings are load-bearing. After the
    //    BAR, because `pulsewin::open` centres its box in the WORK AREA and the work area's top is
    //    `ui_status::top_chrome_h` — which is zero until the bar is enabled, so opening first would
    //    seat the window against a panel that is about to shrink and leave it a bar's height too high.
    //    After the COMPOSITE, because `wm::create_at` composites the new row itself: doing the bar's
    //    catch-up pass first means this window's first frame lands on a desktop whose strip rows are
    //    already painted, rather than one composite before them.
    //
    //    Its decline arms are its own and none of them is fatal here — a desktop without a pulse
    //    window is a desktop, exactly as this function argues for the console window. Nothing about
    //    the DESKTOP band changes: `ui_status` still draws it, the tiler still reserves its rows, and
    //    the window is a SECOND seat for the same instrument reading the same envelope.
    //    ARMED here and OPENED by the render pass, which is not a deferral for its own sake: see
    //    `pulsewin::service`'s open arm for the readback that convicted the direct call (a window
    //    minted from this path is minted by a core that is not the one driving the compositor, and
    //    `[chrome-truth]` read its chrome off the glass between the create and the blit).
    super::pulsewin::arm();
    serial_println!(
        "[pidesk] pulse-window ARMED view={} (the render pass opens it on its first live instrument sample; menu: click `View` in the window's own strip — first option is the Pi LED face, second is the x86 segment face; the desktop LED band is unchanged)",
        super::pulsewin::view().label()
    );
    // 6. QUARRY — the file manager, when `UNAOS_QUARRY=1` armed it. This is the Pi's answer to the
    //    question `desktop_uefi`'s `DESKTOP_APP_ARMED` answers on x86: what does the desktop OPEN once it is
    //    up? The module header above says this seam deliberately mints no second launcher, and it
    //    still does not — `open()` is not a launch, it is the same kind of kernel-furniture window
    //    `panel_console_window_open` mints thirty lines above, over a surface this module never
    //    touches. It is LAST on purpose: it reads directories, and every step before it is either
    //    pure geometry or a flag, so a declined or slow volume read cannot delay the bar's paint.
    //
    //    The witness runs FIRST and unconditionally under `witness`, because its legs are pure
    //    functions over synthetic input (geometry, the scroll clamp, the tree splice, press-to-row).
    //    It needs no panel, no volume and no window, so it must not be able to be skipped by a
    //    DECLINE in the thing it is proving the arithmetic of.
    #[cfg(all(feature = "quarry", feature = "witness"))]
    super::quarry::selftest();
    #[cfg(feature = "quarry")]
    {
        super::quarry::open();
        serial_println!(
            "[pidesk] quarry open={} — the file manager is a desktop tenant (arrows/Enter/Backspace move it ONLY WHILE IT HOLDS FOCUS — SO9; click it to focus, click the console to hand the keyboard back; Esc dismisses menus only and never closes a window — R24; the close disc and the dock's pinned tile are the way out and back)",
            super::quarry::is_open()
        );
    }

    // ── THE LIVE-CONSOLE DECISION, and the measurement that settled it ──────────────────────────
    //
    // Knob off, this returns `false` — **detach as before** — and the honest name for that is: the
    // Pi's console window is a FROZEN BOOT-LOG SNAPSHOT, exactly as x86's desktop-lane console window
    // is. It is parity, achieved, and it is not the parity that arc set out to get. What follows is
    // why, in the terms of what was actually run rather than what was argued — and then, at the
    // bottom, what LIVECON does about it.
    //
    // The argument for `routed` (keep the console LIVE by skipping the detach) is in
    // `fbcon::console_is_routed` and it is, as far as it goes, correct: the detach exists so exactly
    // one core writes the PANEL, a routed console writes kernel RAM instead, so the detach's reason is
    // discharged and skipping it costs nothing. It was implemented, and `activate` returned `routed`.
    //
    // It was then MEASURED, and the argument is incomplete. Discharging "who writes the panel" does
    // not discharge "who drives the COMPOSITOR". A routed console presents from PRINT context, on
    // whatever core printed, and after the handoff the Pi prints from every core it has. At bench
    // geometry (`UNAOS_PIDESK=1 UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test`) that turned a
    // 108/108 run into **97/108 with 37 forbidden hits and 2063 lines scanned against the knob-off
    // control's 6020** — the console's 1296x736 presents interleaving with the video witness battery,
    // which asserts exact panel pixels and cannot survive an extra compositor client it does not know
    // about. The battery said so precisely and in its own words:
    //
    //   [wc-g] win=1 … us=38907 rectscan_us=10222 slow=yes -> BLIT
    //   [wc-g] win=1 … after=0x709b… slow=yes -> RACE-BLIT
    //   [wc-c] side-by-side windows=2 drawn=1
    //   [wc-d] verify win=3 … bad_ram=23104 … got=0x000000 want=0x20ff20 -> FAIL
    //   === AARCH64 EXCEPTION: SYNCHRONOUS ===
    //
    // A synchronous exception is not a pacing problem and would not have been fixed by tuning one.
    //
    // **And this is why x86's desktop lane freezes its console too.** The live routed console exists
    // on x86 only on the `usbdebug` bench lane — a lane with no render service, no witness battery and
    // effectively one service loop — while the lane an operator actually boots (`desktop_uefi::activate` →
    // `main.rs`'s handoff) detaches unconditionally and calls the result "a FROZEN BOOT-LOG SNAPSHOT
    // for the rest of the boot". That was not documented as a hazard anywhere; this arc rediscovered
    // it from the other end, on the other arch, and the note above is the first place either tree
    // says WHY the desktop lane detaches.
    //
    // So the blocker is REAL, and it is NOT the one PI-DESK M4 named. M4 said a Pi console window
    // "would frame a frozen log" because the Pi detaches; the premise was right, the inference was
    // that this made the window pointless, and that inference is wrong — x86 ships exactly that
    // window and it is the desktop's boot-log pane. The real blocker is one layer down and applies to
    // BOTH arches: a console that presents from arbitrary print context is an unsynchronised
    // compositor client.
    //
    // What the next arc should try — named here rather than attempted, because it is a scheduler
    // change and this is a video arc: move the console's presents off print context entirely and onto
    // the RENDER core, one paced call per frame, via the hook `fbcon::console_service` already
    // provides for exactly this on x86's bench lane. That makes the console a single-core compositor
    // client at frame cadence instead of an N-core one at line cadence, which is the shape the witness
    // battery can survive. It needs a line in the Pi render service — `main.rs`'s render task, which
    // is `exec-shellport`'s lane this cycle, not this one.
    //
    // ── LIVECON — that arc, taken ────────────────────────────────────────────────────────────────
    //
    // The paragraph above is the design of record and this is its implementation, in the three
    // statements it asks for and no more. `fbcon::console_present_defer(true)` makes the three
    // PRINT-CONTEXT present entries record their rows in the `PEND` ledger and return without
    // compositing; `main.rs`'s `render_service` calls `fbcon::console_service()` once per pass, which
    // is the paced take of that ledger on ONE core; and so the detach's skip — the thing measured and
    // reverted — is sound this time, because the console is no longer an N-core compositor client at
    // line cadence but a single-core one at frame cadence.
    //
    // THE ARMING IS HERE, at the tail, and the placement is load-bearing. Everything `activate` has
    // printed above — the desktop-clear line, the face census, the console-window witness, the
    // menubar and crystal lines — reached the window's surface INLINE, on the BSP, before the render
    // or input tasks exist. That is the window's first content and there is no render pass yet to
    // present it, so deferring from the top would leave the console blank until the first event.
    // Arming here is the last instant that is still single-core through this seam, which is the same
    // argument the DESKTOP-CLEAR above makes for its own placement.
    //
    // WHAT IT COSTS, STATED HONESTLY. `render_service` blocks on `GUI_CHANNEL.recv()`, so a line
    // printed by a core that generates no GUI event waits for the next pass — and the floor on that
    // is the strip pulse's `ui_status::PSTRIP_PERIOD_MS` timer, the same free wake `armed()` below
    // rides. So the console is live at the pulse rate at worst and immediately on interaction at
    // best; it is not a 60 Hz console. That is a deliberate trade: adding a wake from print context
    // would put channel traffic back on the very path this arc is taking work OFF, which is how the
    // reverted cut failed.
    //
    // KNOB OFF: neither statement is compiled, `routed` is discarded as before, and the boot is the
    // frozen snapshot byte for byte.
    let _ = routed;
    #[cfg(feature = "livecon")]
    if routed {
        fbcon::console_present_defer(true);
        serial_println!(
            "[pidesk] livecon ARMED console_win={} (presents deferred off print context; the render service takes the ledger once per pass via `fbcon::console_service` — the handoff SKIPS the detach and the window stays LIVE)",
            cwin
        );
    }
    #[cfg(feature = "livecon")]
    return routed;
    #[cfg(not(feature = "livecon"))]
    return false;
}

// ── SHELLWIN-PI — the CASCADE latch, and why it is a SECOND question ────────────────────────────
//
// [`activate`] above answers *"is the Pi a desktop yet?"* — the bar is on, the console has a window,
// the compositor owns the glass. That is the question `desktop_uefi::activate` answers on x86, and it is asked
// at the SAME place: the GUI handoff in `kernel_main`.
//
// It is not the question the shell window has to ask. The boot WITNESS CASCADE is still running when
// the handoff returns — `kernel_main`'s own comment at that line says so ("the whole M6b..U7 fixture
// cascade is already spawned and running on the APs") — and those fixtures OWN the panel: they create
// windows, close them, and read the vacated boxes back expecting `DESKTOP_BG` byte-for-byte, `[wc-f]`
// needs a clear probe strip, `[clickroute]` needs to know which row is topmost. Furniture that is on
// the glass while they read is furniture INSIDE THEIR ANSWER.
//
// The first cut of SHELLWIN-PI argued the Pi needs no such latch at all — `desktop_firmware` is compile-time, so
// `desktop_owns_backdrop()` returned a constant `true` and the window was minted at the head of the
// render service. `UNAOS_PIDESK=1 ./arroyo kernel8-test` refuted it, in the fixtures' own words:
//
//   [wc-j] vacate close_painted=true close_desktop=false (2/5) owner_desktop=false (2/5) -> FAIL
//   [wc-j] move-once ... old_desktop=false (2/3) ... overlap_px=30336 ...                -> FAIL
//   [wc-f] twin -> DEFER (a live window overlaps the probe strip)
//   [clickroute] hit-test ... shell=skip ...                                             -> FAIL
//
// `MBENCH FAIL — 104/108`, and not one of the four misses was about the shell. `overlap_px=30336` is
// the fixture naming the collision outright. x86 never met this because its latch happens to encode
// the ordering: the Kepler takeover fires AFTER the cascade, so on that arch "is the desktop up?" and
// "has the cascade released the panel?" are accidentally the same question. On the Pi they are two,
// and this is the second one, asked in its own right and in the terms this arch actually has.
//
// **The standing rule, stated once.** *A desktop that appears before the boot witness cascade has
// released the panel is not early, it is wrong.* Any future Pi furniture takes [`armed`] for the same
// reason the shell window does. `activate`'s own console window is the counter-example that proves
// the rule rather than an exception to it: it is minted at the handoff, ahead of the cascade, and
// CONSWIN-PI M2 measured exactly what that costs — at bench geometry the console box occludes the
// `wc-c`/`wc-d`/`wc-j` fixtures (`first=(307,158) got=0x2d2b55 want=0xc3c3c3`, `bad_cache=0`), a
// standing conflict left for the integrator. The shell window declines to repeat it.
//
// Deliberately NOT a `desktop_uefi` twin: there is no origin to record, no panel to re-describe and nothing to
// deactivate. A boolean is the whole of what is known, so a boolean is the whole of what is stored.

use core::sync::atomic::{AtomicBool, Ordering};

/// `false` until the boot witness cascade has released the panel. Never cleared: the cascade is
/// one-shot and boot-time, and a desktop that could un-arm would be a second state nothing reads.
static ARMED: AtomicBool = AtomicBool::new(false);

/// The boot witness cascade has finished with the panel — the desktop may place furniture on it.
///
/// Called once, from the CALLER of the last panel-reading fixture (`arch::aarch64::syscall`'s
/// `u7_launcher`, right after `wcb_launcher` returns) — at the caller and not inside that fn's tail,
/// because it has three early SKIP returns and arming from inside would let a skipped fixture leave
/// the Pi desktop with no shell window at all and no line saying why. Idempotent by construction (a
/// second call stores the same value), so a future second call site is a no-op rather than a hazard.
pub fn arm() {
    ARMED.store(true, Ordering::Release);
    serial_println!(
        ":: PI-DESK: desktop armed — the witness cascade released the panel, furniture may land ::"
    );
}

/// Has the desktop been armed? Polled once per render pass; `Acquire` pairs with [`arm`]'s `Release`
/// so a reader that sees `true` also sees every panel write the cascade made before it.
///
/// The poll is one load and the WAKE is free: the strip pulse already posts an `Event::Timer` every
/// `ui_status::PSTRIP_PERIOD_MS` from a cooperative `yield_now` loop that needs no timer IRQ, so the
/// render service sees the arming within ~250 ms of the cascade ending, on metal and in QEMU raspi4b
/// alike. No polling task, no new event variant.
pub fn armed() -> bool {
    ARMED.load(Ordering::Acquire)
}
