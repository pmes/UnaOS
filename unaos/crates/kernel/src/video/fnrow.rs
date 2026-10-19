// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//! CHARTER: Kernel — kernel-by-ruling (B438 XHCIMEDIA: the one fn-row router)
//!
//! XHCIMEDIA (rmbp-ledger B438) — THE fn-row router. Both HID pumps (`drivers/ehci`'s boot-keyboard
//! decode and `drivers/xhci`'s) call [`fn_row_usage`] with the raw keyboard-page usage on each PRESS edge;
//! this is the only place an F-row key acts, so no key is bound in two places and neither path can miss one:
//!
//! * F1 / F2 (0x3A / 0x3B) — whatever the theme's keymap row says (R60: the theme is the binding; CRISPY
//!   binds them to brightness down / up) — `brightkeys::key` stages BRIGHTSLIDER's step, the bezel arms.
//! * F7 / F8 / F9 (0x40 / 0x41 / 0x42) — previous / play-pause / next, latched for the open player
//!   (`player::media_key`, PLAYER B419).
//! * F10 / F11 / F12 (0x43 / 0x44 / 0x45) — mute / down / up through the volume model
//!   (`status::volkey_usage`: the store, the codec amp, the bezel).
//!
//! The pumps run the boot protocol, so the rMBP's fn-row arrives as plain F-key usages; a report-protocol
//! keyboard's consumer page is not parsed by either pump (owed, see the evidence note). Atomics and a
//! staged step only — no port I/O, no lock the pump could be holding; runs in the polled HID service.
//! Wire per press: `[hid] fnrow path=<ehci|xhci> usage=0x<nn> -> <action>`. `tests xhcimedia` (R80:
//! registered on the desktop pass, never run at boot):
//! `:: XHCIMEDIA: router=one paths=ehci,xhci keys=8 bright=ok media=ok volume=ok -> PASS ::`.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::keymap::{self, Action};

/// The pump a usage came from.
pub const PATH_EHCI: u8 = 0;
pub const PATH_XHCI: u8 = 1;

/// The usages the router owns (the eight F-row keys this keyboard sends), in F-key order.
pub const KEYS: [u8; 8] = [0x3A, 0x3B, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45];

/// Presses routed per path (`[ehci, xhci]`), for the witness and a `[kbdpoll]`-style read.
static ROUTED: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// What a routed key did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FnAct {
    Bright(bool),
    Media(u8),
    Volume(u8), #[cfg(all(target_arch = "x86_64", feature = "wc"))] Help, // HELPF1 (B746): F1 / Shift+F1 opened the focused app's Help page (`help_chord`, file tail). LINE-NEUTRAL fold, code first
}

impl FnAct {
    pub fn name(self) -> &'static str {
        match self {
            FnAct::Bright(false) => "brightness-down",
            FnAct::Bright(true) => "brightness-up",
            FnAct::Media(0) => "media-previous",
            FnAct::Media(1) => "media-play-pause",
            FnAct::Media(_) => "media-next",
            FnAct::Volume(0) => "volume-up",
            FnAct::Volume(1) => "volume-down",
            FnAct::Volume(_) => "volume-mute", #[cfg(all(target_arch = "x86_64", feature = "wc"))] FnAct::Help => "help", // HELPF1 (B746). LINE-NEUTRAL fold, code first
        }
    }
}

fn path_name(path: u8) -> &'static str {
    if path == PATH_XHCI { "xhci" } else { "ehci" }
}

/// The decision alone (no effect): what `usage` under `modifiers` means on the F-row, or `None`.
pub fn decide(usage: u8, modifiers: u8) -> Option<FnAct> {
    if let Some(a) = keymap::resolve(keymap::active(), modifiers, usage) {
        if a.is_brightness() {
            return Some(FnAct::Bright(matches!(a, Action::BrightnessUp)));
        }
        return None; // the theme binds this usage to something else: not the F-row's
    }
    match usage {
        0x40..=0x42 => Some(FnAct::Media(usage - 0x40)),
        0x45 => Some(FnAct::Volume(0)),
        0x44 => Some(FnAct::Volume(1)),
        0x43 => Some(FnAct::Volume(2)),
        _ => None,
    }
}

/// THE router: a usage that just went DOWN on `path`. Returns what it did (`None`: not an F-row key).
pub fn fn_row_usage(path: u8, usage: u8, modifiers: u8) -> Option<FnAct> {
    if usage <= 1 {
        return None;
    }
    if super::perfhud::chord(usage, modifiers) { return None; } #[cfg(all(target_arch = "x86_64", feature = "wc"))] if help_chord(path, usage, modifiers) { return Some(FnAct::Help); } let act = decide(usage, modifiers)?; // PERFHUD (B710): Win+F12 toggles the performance overlay and is consumed here (the one F-row router), before the F12 volume step. LINE-NEUTRAL fold, code first
    match act {
        FnAct::Bright(up) => super::brightkeys::key(if up { Action::BrightnessUp } else { Action::BrightnessDown }),
        FnAct::Media(k) => super::player::media_key(k),
        FnAct::Volume(_) => {
            let _ = super::status::volkey_usage(usage);
        } #[cfg(all(target_arch = "x86_64", feature = "wc"))] FnAct::Help => {} // HELPF1 (B746): never reached — `help_chord` returns before `decide`. LINE-NEUTRAL fold, code first
    }
    ROUTED[(path == PATH_XHCI) as usize].fetch_add(1, Ordering::Relaxed);
    serial_println!("[hid] fnrow path={} usage={:#04x} -> {}", path_name(path), usage, act.name());
    Some(act)
}

/// Presses routed so far, `(ehci, xhci)`.
pub fn routed() -> (u32, u32) {
    (ROUTED[0].load(Ordering::Relaxed), ROUTED[1].load(Ordering::Relaxed))
}

// ── `tests xhcimedia` (R80: registered from the desktop pass, never run at boot) ─────────────────────────

/// Register `tests xhcimedia` once (called from `brightkeys::service`, the desktop pass).
pub fn ensure_registered() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("xhcimedia", fixture);
    }
}

/// Feed the eight usages through the router — alternating the two path tags, as the two pumps call it —
/// and read the models back: the backlight's bezel armed for F1/F2, the player's media latch for F7/F8/F9,
/// the volume level and mute for F10/F11/F12. Restores the backlight register, the volume, the mute and
/// the latch; drops the bezel.
pub fn fixture() {
    let prev_raw = super::backlight::cur_raw();
    let (l0, m0) = super::status::volume();
    let _ = super::player::media_take();
    let mut bright_ok = true;
    let mut media_ok = true;
    let mut vol_ok = true;
    let mut keys = 0u32;
    let mut seen = [false; 2];
    super::status::set_volume(8, false);
    for (i, &u) in KEYS.iter().enumerate() {
        let path = if i % 2 == 0 { PATH_EHCI } else { PATH_XHCI };
        super::status::bezel_disarm();
        let before = super::status::volume();
        let r = fn_row_usage(path, u, 0);
        if r.is_some() {
            keys += 1;
            seen[path as usize] = true;
        }
        match u {
            0x3A | 0x3B => {
                bright_ok &= r == Some(FnAct::Bright(u == 0x3B))
                    && super::bezel::indicator() == super::bezel::K_BRIGHT;
            }
            0x40..=0x42 => {
                media_ok &= r == Some(FnAct::Media(u - 0x40)) && super::player::media_take() == u - 0x40 + 1;
            }
            _ => {
                let (lv, mu) = super::status::volume();
                vol_ok &= match u {
                    0x45 => r == Some(FnAct::Volume(0)) && lv == before.0 + 1 && !mu,
                    0x44 => r == Some(FnAct::Volume(1)) && lv + 1 == before.0 && !mu,
                    _ => r == Some(FnAct::Volume(2)) && mu != before.1,
                };
            }
        }
    }
    // a non-F-row usage is not the router's (the letter `a`)
    let other_ok = fn_row_usage(PATH_XHCI, 0x04, 0).is_none();
    // restore: the staged step lands through the one writer, then the register goes back
    super::brightkeys::service();
    let _ = super::backlight::set_raw_via(prev_raw, "xhcimedia-restore");
    let _ = super::status::set_volume(l0, m0);
    let _ = super::player::media_take();
    super::status::bezel_disarm();
    let ok = bright_ok && media_ok && vol_ok && other_ok && keys == KEYS.len() as u32 && seen[0] && seen[1];
    serial_println!(
        ":: XHCIMEDIA: router=one paths=ehci,xhci keys={} bright={} media={} volume={} other={} -> {} ::",
        keys,
        if bright_ok { "ok" } else { "FAIL" },
        if media_ok { "ok" } else { "FAIL" },
        if vol_ok { "ok" } else { "FAIL" },
        if other_ok { "ok" } else { "FAIL" },
        if ok { "PASS" } else { "FAIL" }
    );
}

// ══════════════ HELPF1 (rmbp-ledger B746, BP-video-35) — TAIL-APPENDED: F1 opens Help for the focused app ══════════════
// x86 `wc` (the Help window, `help::window`, is x86 `wc`). The ONE fold is on `fn_row_usage`'s PERFHUD line, before
// `decide`: a press `help_key` accepts latches `help::window::request_for_focus()` (an atomic store; the desktop pass
// opens the page) and is consumed — the brightness step does not also run. R60, the theme is the binding: a table that
// binds F1 to brightness (CRISPY_ROWS: both Una tables; the rMBP's brightness key IS usage 0x3A) keeps bare F1 for
// brightness and gives Help to Shift+F1; a table that binds no F1 (PC_ROWS: both Softy tables) gives Help to F1 itself;
// a table that binds F1 to anything else keeps it. The keymap names no Help action, so no other chord exists.
// Wire per press: `[hid] fnrow path=<ehci|xhci> usage=0x3a -> help`, then the pass's `[help] f1 owner= app= page=`.

/// F1's HID keyboard-page usage.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub const USAGE_F1: u8 = 0x3A;

/// The decision alone (pure): does `usage` under `modifiers` mean Help on `table`?
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn help_key(table: &keymap::Table, usage: u8, modifiers: u8) -> bool {
    if usage != USAGE_F1 {
        return false;
    }
    match keymap::resolve(table, modifiers, usage) {
        None => true,
        Some(a) if a.is_brightness() => modifiers & crate::drivers::xhci::HID_MOD_SHIFT != 0,
        Some(_) => false,
    }
}

/// The router's Help arm: `true` = the press was Help on the live table, the focused app's page is latched, consumed.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn help_chord(path: u8, usage: u8, modifiers: u8) -> bool {
    if !help_key(keymap::active(), usage, modifiers) {
        return false;
    }
    crate::help::window::request_for_focus();
    ROUTED[(path == PATH_XHCI) as usize].fetch_add(1, Ordering::Relaxed);
    serial_println!("[hid] fnrow path={} usage={:#04x} -> {}", path_name(path), usage, FnAct::Help.name());
    true
}
