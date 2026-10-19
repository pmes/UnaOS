// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm (SOFTY B629: the second LOOK of the UnaOS desktop — comfort for someone arriving from Windows)
//!
//! SOFTY (rmbp-ledger B629). Peter, 2026-10-14: "as a test of the flexibility of our interface and practice for you to
//! make an aesthetically pleasing interface, we need to come up with our windows clone i want to call softy" — and,
//! on what it is for: "knowing most people prefer mac or win and coming up with clones skirting the line of
//! legality not to steal but for people's comfort. they deserve a ONE OS -- UnaOS".
//!
//! So Softy follows the FUNCTIONAL layout and idioms a Windows user's hands know — a bottom taskbar with a Start
//! button and centred app tiles, the tray and clock at the right, a Start menu with search, pinned apps and power,
//! the caption buttons at a window's top RIGHT with close turning red — and copies none of the vendor's marks: our
//! own Start mark (a crystal), our own glyphs, the glass's own faces, our own icons (`appres`). SOFTY.md lists what
//! is followed and what is not copied.
//!
//! The pieces: [`palette`] (every colour and dimension, named once), [`paint`] (the drawing facade GLASSKIT's
//! `video::draw` replaces at the fold), [`taskbar`], [`startmenu`] (Start and the calendar), [`chrome`] (the window
//! head). The switch is `video::look` (`system.look = una | softy`, Principia's key).
//!
//! `tests softy` — [`selftest`]: switch to Softy, assert the band's geometry, toggle Start, press a tile, hit-test
//! the caption buttons at the right edge, switch back and assert the menu bar and the dock return.

pub mod chrome;
pub mod paint;
pub mod palette;
pub mod startmenu;
pub mod taskbar;

/// `tests softy` — the look's whole loop, on the running desktop; restores the look it found. One line:
/// `:: SOFTY: switch=ok bar=<x>,<y>,<w>x<h> tiles=<n> start=open,close tile=<word> buttons=right(min,max,close) back=menubar,dock -> PASS ::`.
#[cfg(feature = "witness")]
pub fn selftest() {
    use crate::video::{dock, look, menubar, wm};
    let was = look::current();
    let (pw, ph) = {
        let fb = *crate::video::WRITER.lock();
        if !fb.is_ready() {
            serial_println!(":: SOFTY: -> SKIP :: reason=no-panel");
            return;
        }
        (fb.width(), fb.height())
    };
    let Some(want) = taskbar::rect(pw, ph) else {
        serial_println!(":: SOFTY: -> SKIP :: reason=panel-too-small {}x{}", pw, ph);
        return;
    };
    #[cfg(feature = "login")]
    if !crate::video::desktopbuild::built() {
        serial_println!(":: SOFTY: -> SKIP :: reason=no-desktop (the furniture is built at login)");
        return;
    }
    let bar_was = menubar::strip_rect(pw, ph).is_some();
    // 1. The switch: the dock tenant answers the taskbar's rect, the menu bar is absent, a pass paints the band.
    let switched = look::set(look::Look::Softy, "tests");
    wm::composite();
    let tenant = dock::strip_rect(pw, ph);
    let painted = taskbar::painted_rect();
    let geo_ok = tenant == Some(want) && want.1 + want.3 == ph && want.2 == pw && want.3 == palette::px(palette::BAR_H) && menubar::strip_rect(pw, ph).is_none();
    // 2. Start: the button opens it, a second press closes it.
    let start_btn = taskbar::probe(pw, ph, 0);
    let (tiles, sx, sy) = start_btn.unwrap_or((0, 0, 0));
    let open_ok = start_btn.is_some() && taskbar::press_at(sx as i32, sy as i32) && startmenu::open_kind() == 1 && startmenu::rect().map(|r| r.1 + r.3 < want.1).unwrap_or(false);
    let close_ok = taskbar::press_at(sx as i32, sy as i32) && startmenu::open_kind() == 0;
    // 3. A tile: the first pinned table app's tile, pressed (it launches directly or raises its window).
    let p0 = taskbar::PRESSES.load(core::sync::atomic::Ordering::Relaxed);
    let tile_word = match (1..tiles).find(|&i| taskbar::probe_pin(i).is_some()) {
        Some(i) => match taskbar::probe(pw, ph, i) {
            Some((_, x, y)) if taskbar::press_at(x as i32, y as i32) => dock::DP_PINS[taskbar::probe_pin(i).unwrap_or(0)].name,
            _ => "unpressed",
        },
        None => "none",
    };
    let tile_ok = tile_word == "none" || taskbar::PRESSES.load(core::sync::atomic::Ordering::Relaxed) == p0 + 1;
    // 4. The caption buttons: on the topmost window with controls, minimise / maximise / close sit at the RIGHT edge.
    let btn = buttons_probe();
    let btn_ok = btn != "fail";
    // SOFTY2 (B641) — the finish, each leg on the live desktop with Softy on the glass:
    // 6. The Boot Camp keys (R126): the mirrored tables and the Windows-key tap, driven as a decoder drives them.
    let (k_mac, k_pc, k_tap, k_spoil) = crate::video::keymap::softy2_selftest();
    let keys_ok = k_mac && k_pc && k_tap && k_spoil;
    // 7. Quick settings (the tray) and the notification calendar (the clock): a press opens each above the bar with
    //    its shadow band, a second press closes it.
    let fly = |which: u8| -> (bool, bool, usize) {
        let Some((x, y)) = taskbar::probe_hot(pw, ph, which) else { return (false, false, 0) };
        let opened = taskbar::press_at(x as i32, y as i32) && startmenu::open_kind() == which && startmenu::rect().map(|r| r.1 + r.3 < want.1).unwrap_or(false);
        let band = startmenu::shadow_band();
        let closed = taskbar::press_at(x as i32, y as i32) && startmenu::open_kind() == 0;
        (opened, closed, band)
    };
    let (q_open, q_close, _) = if taskbar::probe_hot(pw, ph, 3).is_some() { fly(3) } else { (true, true, 0) };
    let q_word = if taskbar::probe_hot(pw, ph, 3).is_none() { "notray" } else if q_open && q_close { "open,close" } else { "no" };
    let (c_open, c_close, band) = fly(2);
    // 8. The dark palette: every §2.3 token Softy names resolves, the dark elevations rise (window < raised <
    //    overlay), ink reads over the window, and the accent is the ACCENT TABLE's fill in both modes.
    let lum = |c: u32| ((c >> 16) & 0xFF) * 3 + ((c >> 8) & 0xFF) * 6 + (c & 0xFF);
    let d = palette::dark;
    use palette::Sem as S;
    let named = palette::SEMS.iter().filter(|&&t| d(t) >> 24 == 0 && !t.name().is_empty()).count();
    let dark_ok = named == palette::SEMS.len()
        && lum(d(S::SurfaceWindow)) < lum(d(S::SurfaceRaised)) && lum(d(S::SurfaceRaised)) < lum(d(S::SurfaceOverlay))
        && lum(d(S::TextPrimary)) > 4 * lum(d(S::SurfaceWindow))
        && d(S::AccentFill) == crate::video::theme::accent();
    let accent_ok = palette::ACCENT() == crate::video::theme::accent();
    let (own, own_n) = palette::dark_own();
    // 9. The caption buttons' pressed state: a latched press on the focused window's close button reads back pressed.
    let pressed = match wm::wl_focused() {
        Some((id, _)) => {
            chrome::latch_for_test(Some((id, 2, true)));
            let r = chrome::latched() == Some((id, 2, true));
            chrome::latch_for_test(None);
            if r { "ok" } else { "no" }
        }
        None => "nowindow",
    };
    let s2_ok = keys_ok && q_open && q_close && c_open && c_close && band > 0 && dark_ok && accent_ok && pressed != "no" && own == own_n;
    // 10. SOFTY3 (B651): the clock's flyout opens notify's OWN Center beside it and closes it with itself — the request
    //     is notify's seam (`request_center`), acted on by its service pass, which the leg drives once per press.
    //     The Center the operator had open is asked back at the end.
    let center_was = crate::video::notify::center_open();
    let clock_center = |want_open: bool| -> bool {
        let Some((x, y)) = taskbar::probe_hot(pw, ph, 2) else { return false };
        let _ = taskbar::press_at(x as i32, y as i32);
        crate::video::notify::service();
        (startmenu::open_kind() == 2) == want_open && crate::video::notify::center_open() == want_open
    };
    let cc_open = clock_center(true);
    let cc_close = clock_center(false);
    if center_was {
        crate::video::notify::request_center(true);
    }
    // 5. Back to the UnaOS look: the taskbar leaves, the dock and (when it was on) the menu bar return.
    let back = look::set(was, "tests");
    wm::composite();
    let gone = taskbar::painted_rect().is_none() || was == look::Look::Softy;
    let dock_back = dock::strip_rect(pw, ph) != Some(want) || was == look::Look::Softy;
    let bar_back = menubar::strip_rect(pw, ph).is_some() == bar_was;
    let ok = (switched || was == look::Look::Softy) && geo_ok && painted == Some(want) && open_ok && close_ok && tile_ok && btn_ok && s2_ok && (back || was == look::Look::Softy) && gone && dock_back && bar_back;
    let yn = |b: bool| if b { "ok" } else { "no" };
    serial_println!(
        ":: SOFTY: switch={} bar={},{},{}x{} tiles={} start={},{} tile={} buttons={} keys=softy-mac:{},una-pc:{},tap:{},spoil:{} quick={} calendar={},{} shadow={} dark={}/{} own={}/{} accent={} pressed={} back={},{} -> {} ::",
        if switched { "ok" } else { "no" }, want.0, want.1, want.2, want.3, tiles,
        if open_ok { "open" } else { "no" }, if close_ok { "close" } else { "no" }, tile_word, btn,
        yn(k_mac), yn(k_pc), yn(k_tap), yn(k_spoil), q_word,
        if c_open { "open" } else { "no" }, if c_close { "close" } else { "no" }, band,
        if dark_ok { named } else { 0 }, palette::SEMS.len(), own, own_n, if accent_ok { "table" } else { "no" }, pressed,
        if bar_back { "menubar" } else { "no-menubar" }, if dock_back { "dock" } else { "no-dock" },
        if ok { "PASS" } else { "FAIL" }
    );
    serial_println!(":: SOFTY: clock->center open={} close={} -> {} ::", yn(cc_open), yn(cc_close), if cc_open && cc_close { "PASS" } else { "FAIL" });
}

/// The caption buttons' hit-test on the topmost window that draws them: `right(min,max,close)` when the three sit
/// left to right against the right border, `nowindow` when no window is up to ask, `fail` otherwise.
#[cfg(feature = "witness")]
fn buttons_probe() -> &'static str {
    use crate::video::wm::{self, Ctrl};
    let Some((id, _)) = wm::wl_focused() else { return "nowindow" };
    let Some(i) = wm::info(id) else { return "nowindow" };
    let (b, t) = (wm::BORDER(), wm::TITLE_H());
    let right = i.x + i.w * i.scale + b; // the outer box's right edge (exclusive)
    let y = i.y.saturating_sub(t / 2) as i32;
    let bw = palette::px(palette::BTN_W);
    if right < 4 * bw {
        return "nowindow";
    }
    let at = |dx: usize| wm::control_hit(id, (right - 1 - dx) as i32, y);
    let close = at(bw / 2);
    let max = at(bw + bw / 2);
    let min = at(2 * bw + bw / 2);
    let left = wm::control_hit(id, (i.x + b) as i32, y);
    match (min, max, close, left) {
        (Some(Ctrl::Minimise), Some(Ctrl::Zoom), Some(Ctrl::Close), None) => "right(min,max,close)",
        (None, None, None, _) => "nowindow",
        _ => "fail",
    }
}
