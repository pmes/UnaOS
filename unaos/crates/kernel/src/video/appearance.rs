// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Principia — shared-core
//!
//! APPEARANCE (rmbp-ledger B408, MACPARITY rows 21/22) — Light / Dark, the accent and the highlight, applied.
//!
//! The choice is Principia's (`system.appearance.mode` / `.accent` / `.highlight`, `prefs_core::schema`); the rules
//! (spellings, defaults, `auto` = dark 19:00–07:00 by the local clock) are `prefs_core::appearance`; the palette is
//! `video::theme` (the LIGHT and DARK token sets, eight accents). This module only READS the keys (at the login's
//! load and when another client's PrefChanged names one), WRITES one when the Settings pane chooses (latched off the
//! click router, drained on the settings service pass), and installs the result with ONE repaint: `theme::set`
//! bumps the theme epoch (every strip signature and Quarry's window-repaint pass read it), the kernel windows that
//! cache their pixels repaint, and the panel is damaged so the chrome and the desktop recomposite.
//!
//! Wire: `[appearance] mode=<light|dark|auto> dark=<0|1> accent=<name> highlight=<name> via=<login|bus|settings|clock|tests> repaint_ms=<n>`
//! once per switch; `tests appearance` prints
//! `:: APPEARANCE: literals_outside_theme=<n> tokens=<n> sets=2 mode=<m> accent=<name> repaint_ms=<n> -> PASS ::`.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use alloc::string::String;
use prefs_core::appearance::{self as ap, Mode};

use crate::video::{theme, wm};

/// The user's choice: mode index, accent index, highlight (0 = follow the accent, 1..=8 = `ACCENTS[n - 1]`).
static MODE_IX: AtomicU8 = AtomicU8::new(0);
static ACCENT_IX: AtomicU8 = AtomicU8::new(0);
static HL_RAW: AtomicU8 = AtomicU8::new(0);
/// A read of the store is owed: 0 none, 1 another client changed a key, 2 the login's load.
static RELOAD: AtomicU8 = AtomicU8::new(0);
/// A Settings choice owed to the service pass: 0 none, else `kind << 8 | index` + 1 (kind 0 mode, 1 accent, 2 highlight).
static CHOSEN: AtomicU32 = AtomicU32::new(0);
static NEXT_CLOCK_AT: AtomicU64 = AtomicU64::new(0);
static REPAINT_MS: AtomicU32 = AtomicU32::new(0);
static SWITCHES: AtomicU32 = AtomicU32::new(0);
static LOADED: AtomicBool = AtomicBool::new(false);
/// `auto` re-reads the clock this often.
const CLOCK_MS: u64 = 60_000;

/// The local hour (the civil anchor already carries the display offset; `None` until a clock is set).
pub fn local_hour() -> Option<u32> {
    crate::clock::try_unix_now().map(|s| ((s / 3600) % 24) as u32)
}

/// The stored mode.
pub fn mode() -> Mode {
    Mode::from_index(MODE_IX.load(Ordering::Relaxed) as usize)
}

/// The accent index (`prefs_core::appearance::ACCENTS`).
pub fn accent_ix() -> usize {
    ACCENT_IX.load(Ordering::Relaxed) as usize
}

/// The highlight as stored: 0 = follow the accent, 1..=8 a named colour.
pub fn highlight_raw() -> usize {
    HL_RAW.load(Ordering::Relaxed) as usize
}

/// The key that names the highlight (`prefs_core::appearance::HIGHLIGHTS`).
pub fn highlight_name() -> &'static str {
    ap::HIGHLIGHTS[highlight_raw().min(ap::HIGHLIGHTS.len() - 1)]
}

/// Another client's PrefChanged named an `appearance.*` key (the settings bus pass), or the login loaded the store.
pub fn mark_dirty() {
    let _ = RELOAD.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed);
}

/// The login's load (`settings::load_for_login`).
pub fn load_at_login() {
    RELOAD.store(2, Ordering::Release);
}

/// The Settings pane chose (the click router: latched, applied and stored on the service pass).
pub fn choose(kind: u32, index: usize) {
    CHOSEN.store(((kind << 8) | index as u32) + 1, Ordering::Release);
}

/// The settings service pass: a choice, a store change, the clock (auto).
pub fn service() {
    let c = CHOSEN.swap(0, Ordering::AcqRel);
    if c != 0 {
        let (kind, ix) = ((c - 1) >> 8, ((c - 1) & 0xFF) as usize);
        use crate::prefs::{key, PrefValue};
        let (k, v) = match kind {
            0 => { MODE_IX.store(ix.min(2) as u8, Ordering::Relaxed); (key::APPEARANCE_MODE, ap::MODES[ix.min(2)]) }
            1 => { ACCENT_IX.store(ix.min(7) as u8, Ordering::Relaxed); (key::APPEARANCE_ACCENT, ap::ACCENTS[ix.min(7)]) }
            _ => { HL_RAW.store(ix.min(8) as u8, Ordering::Relaxed); (key::APPEARANCE_HIGHLIGHT, ap::HIGHLIGHTS[ix.min(8)]) }
        };
        crate::prefs_client::sys_set(k, PrefValue::Str(String::from(v)));
        apply("settings");
        return;
    }
    let r = RELOAD.swap(0, Ordering::AcqRel);
    if r != 0 {
        load(if r == 2 { "login" } else { "bus" });
        return;
    }
    if mode() == Mode::Auto && LOADED.load(Ordering::Relaxed) {
        let now = crate::arch::ms();
        if now >= NEXT_CLOCK_AT.load(Ordering::Relaxed) {
            NEXT_CLOCK_AT.store(now + CLOCK_MS, Ordering::Relaxed);
            apply("clock");
        }
    }
}

/// Read the three keys (three PrefGets) and install them.
fn load(via: &str) {
    use crate::prefs::key;
    let m = Mode::parse(crate::prefs_client::sys_text(key::APPEARANCE_MODE).as_deref());
    let a = ap::accent_index(crate::prefs_client::sys_text(key::APPEARANCE_ACCENT).as_deref());
    let h = crate::prefs_client::sys_text(key::APPEARANCE_HIGHLIGHT);
    let hr = h.as_deref().and_then(|s| ap::HIGHLIGHTS.iter().position(|x| x.eq_ignore_ascii_case(s.trim()))).unwrap_or(0);
    MODE_IX.store(m.index() as u8, Ordering::Relaxed);
    ACCENT_IX.store(a as u8, Ordering::Relaxed);
    HL_RAW.store(hr as u8, Ordering::Relaxed);
    LOADED.store(true, Ordering::Relaxed);
    NEXT_CLOCK_AT.store(crate::arch::ms() + CLOCK_MS, Ordering::Relaxed);
    apply(via);
}

/// Install the stored choice; repaint once when anything moved. Returns whether it moved.
fn apply(via: &str) -> bool {
    let a = accent_ix();
    let h = ap::highlight_index(Some(highlight_name()), a);
    let dark = ap::is_dark(mode(), local_hour());
    install(dark, a, h, via)
}

/// `theme::set` + the one repaint + the wire line.
fn install(dark: bool, accent: usize, highlight: usize, via: &str) -> bool {
    let t0 = crate::arch::ms();
    if !theme::set(dark, accent, highlight) {
        return false;
    }
    repaint();
    let ms = crate::arch::ms().saturating_sub(t0) as u32;
    REPAINT_MS.store(ms, Ordering::Relaxed);
    SWITCHES.fetch_add(1, Ordering::Relaxed);
    serial_println!(
        "[appearance] mode={} dark={} accent={} highlight={} via={} repaint_ms={}",
        mode().name(), dark as u8, ap::ACCENTS[accent.min(7)], highlight_name(), via, ms
    );
    true
}

/// The wm's full-repaint path, once: every kernel window that caches its pixels repaints (Quarry's pass — the
/// same list a face restyle repaints, keyed on the theme epoch too), then the panel is damaged so the chrome,
/// the desktop and the strips (whose signatures carry the epoch) recomposite.
fn repaint() {
    #[cfg(feature = "quarry")] crate::video::quarry::live::font_repaint_pass(); // quarry::live exists only under `quarry` (a tegra+rast shape has the pane without the file manager)
    let _ = wm::damage_intersecting(0, 0, 1 << 16, 1 << 16);
}

/// `tests appearance` — the audit's certified count, the token sets, and a live switch to Dark (another accent)
/// and back, timed. The user's stored choice is NOT written; the live set is restored to it.
#[cfg(feature = "witness")]
pub fn selftest() {
    let lits = theme::AUDIT_LITERALS_OUTSIDE;
    let alt = (accent_ix() + 6) % ap::ACCENTS.len();
    let was_dark = theme::is_dark();
    let flipped = install(!was_dark, alt, alt, "tests");
    let ms = REPAINT_MS.load(Ordering::Relaxed);
    let tok_ok = theme::chrome_face() == if was_dark { theme::LIGHT[theme::Tok::WindowBg as usize] } else { theme::DARK[theme::Tok::WindowBg as usize] }
        && theme::accent() == theme::ACCENTS[alt]
        && theme::content_text() != theme::content_fill();
    let back = apply("tests");
    let restored = theme::is_dark() == was_dark && theme::accent_index() == accent_ix();
    let ok = lits == 0 && flipped && tok_ok && back && restored;
    serial_println!(
        ":: APPEARANCE: literals_outside_theme={} tokens={} sets=2 mode={} accent={} repaint_ms={} -> {} :: flipped={} tokens_ok={} restored={} switches={}",
        lits, theme::TOKENS + 2, mode().name(), ap::ACCENTS[accent_ix().min(7)], ms, if ok { "PASS" } else { "FAIL" },
        flipped as u8, tok_ok as u8, restored as u8, SWITCHES.load(Ordering::Relaxed)
    );
}
