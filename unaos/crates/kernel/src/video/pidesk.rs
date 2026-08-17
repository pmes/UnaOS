// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CONSWIN-PI / MENUBAR-PI — **the Pi's DESKTOP-READY seam.**
//!
//! ## What this module is
//!
//! [`super::wcx::activate`] is x86's answer to one question: *at which single point in the boot does
//! the machine stop being a kernel with a framebuffer and start being a desktop?* Everything the
//! crispy desktop needs to be true at once — the console is a window, the menu bar is on, the
//! compositor owns the glass — is decided there, in order, with a DECLINE arm for every way it can
//! fail and a witness line for every decision.
//!
//! The Pi had no such point. `pidesk` compiled the furniture (`strip`, `dock`, `menubar`, `crystal`)
//! and routed presses to it (`arch::aarch64::syscall`'s furniture arm), but nothing ever *turned the
//! desktop on*: `menubar::ENABLED` starts `false` and every `set_enabled(true)` in the tree was a
//! `witness` fixture that restored the flag before returning — so the bar was compiled, composed
//! (three relaxed atomics per pass) and permanently invisible. The console had no window at all.
//!
//! This is that point, for the Pi. It is deliberately NOT a port of `wcx::activate`'s body: the
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
//! 1b. **The panel is CLEARED to the desktop colour** — `wcx`'s WC-X DESKTOP-CLEAR, on this arch for
//!    the first time. Everything painted before the desktop existed (`video::init_panel`'s
//!    `PANEL_BG` fill, the direct-painted fbcon boot log) sits outside every damage box the
//!    compositor has, so nothing else in the system will ever repaint it. It goes above step 2
//!    because it is the last instant at which the window table is empty and a direct front-buffer
//!    write therefore collides with no compositor-owned pixel. See the call site for the full
//!    argument, and for why the WC-BBSYNC half of x86's pairing is deliberately not armed here.
//! 2. **The dock must be able to host a full strip** — `wcx`'s CONSOLEWIN law, unchanged and for the
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
//!    After every decline above, on `wcx`'s ownership argument: turning the bar on is a SHELL's
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
    //    aarch64 counterpart of `wcx`'s WC-X DESKTOP-CLEAR, and the gap `wcf::chrome_truth` was
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
    {
        super::cursor::undraw();
        let fb = *super::WRITER.lock();
        fb.fill_screen(wm::DESKTOP_BG);
        fb.flush_all();
    }
    serial_println!(
        "[pidesk] desktop-clear panel={}x{} bg={:08X} (pre-desktop residue off the glass; the window table is empty at this line)",
        pw,
        ph,
        wm::DESKTOP_BG
    );

    // 2-3. CONSOLEWIN — the dock must be able to host the worst-case strip, or the console gets no
    //    window (and therefore no minimise disc, and therefore nothing to strand). `wcx`'s law,
    //    unchanged in substance and NARROWED in scope, which is the one place the Pi's sequence
    //    deliberately differs from x86's.
    //
    //    On x86 this check is a TOTAL decline: `wcx::activate` returns, and the menu bar — enabled
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
    //    `[pidesk] activate DECLINE reason=dock-cannot-host-full-strip panel=640x480 rows=12` and
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
            "[pidesk] console-window ABSENT — continuing to the bar (the boot log stays on the panel and the handoff will detach exactly as before)"
        );
    } else {
        // `routed=true` states that the glyph ROUTE is installed — every console line from here lands
        // in the window's surface instead of on the panel. It is NOT a claim that lines keep coming:
        // the handoff's `fbcon::detach()` follows this seam and stops them, so the window holds the
        // desktop-bringup tail and then freezes. See the live-console ledger at the tail of this
        // function for why the detach stays, and `wcx.rs` for x86 doing the identical thing.
        serial_println!("[pidesk] activate panel={}x{} console_win={} routed={} (the window freezes at the handoff detach that follows — x86's desktop lane does the same)", pw, ph, cwin, routed);
    }

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
    serial_println!("[pidesk] menubar PAINTED (composite at the enable seam)");

    // The SHARD menu is reachable from this instant: `crystal::press_at` hit-tests
    // `menubar::crystal_box_abs`, and the aarch64 click router's furniture arm
    // (`arch::aarch64::syscall::wc_click_route`, `feature = "pidesk"`) already calls
    // `strip::press_route` ahead of every window arm. Stated on the wire because "the bar is painted"
    // and "the bar is live" are two claims and a capture should not have to infer the second.
    serial_println!(
        "[pidesk] crystal LIVE — press the crystal for the SHARD menu (About is real on aarch64; Sleep/Restart/Shut Down print their honest unimplemented lines — no PSCI wiring on this track yet)"
    );

    // ── THE LIVE-CONSOLE DECISION, and the measurement that settled it ──────────────────────────
    //
    // This returns `false` — **detach as before** — and the honest name for that is: the Pi's console
    // window is a FROZEN BOOT-LOG SNAPSHOT, exactly as x86's desktop-lane console window is. It is
    // parity, achieved, and it is not the parity this arc set out to get. What follows is why, in the
    // terms of what was actually run rather than what was argued.
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
    // effectively one service loop — while the lane an operator actually boots (`wcx::activate` →
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
    let _ = routed;
    false
}
