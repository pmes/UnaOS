//! CHARTER: Kernel — wm (FBDIRECT, rmbp-ledger B536: the registry of direct-framebuffer paints the compositor must learn about)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//!
//! FBDIRECT (rmbp-ledger B536, flight 31 REDBAND + SPLASHWORD). A few painters write straight onto the panel because
//! the thing they report may be holding the compositor (SESSIONWEDGE's PUMPWEDGE band, TESTWATCH's band, the
//! RESOLVEHANG2 OVER line) or because they own the glass before any screen exists (the held splash's stage word).
//! Before this file each of them "cleared" by FILLING its rectangle again, and the compositor's scene never learned
//! those pixels were stale: after flight 31's 2.2 s dongle wedge the red band stayed until the cursor and the app bar
//! overpainted it piecemeal, and boot 2's "Resolving the session" stayed under the login window.
//!
//! One rule, here: a direct paint registers its rect ([`note`]); its END hands the rect back to the compositor
//! ([`clear`]) — `wm::damage_intersecting` (the window half) and `screen::request_present_rect` (the backdrop half),
//! the pair `strip::restore_vacated` / `cursor::repair` use — never a fill; and a screen open hands back every rect
//! still registered ([`expose_all`], `login::open_as` and the desktop build). The panic screen stays direct.
//!
//! The table is one `AtomicU64` per painter (x, y, w, h packed 16 bits each; 0 = nothing on the glass): no lock, no
//! heap, so the watchdog painters keep their "takes nothing" contract on the PAINT. Only the hand-back takes the
//! compositor's two short locks (`TABLE`, `PRESENT_RECTS`), and it runs after the thing that wedged has moved again.
//!
//! SPLASHWORD2 (rmbp-ledger B544, flight 32 boot 2: "no resolving session is still there"). The hand-back landed
//! (`clear painter=splash-word rect=0,1680,2880x80 -> damage handed=1`) and nothing repainted the rect, because the
//! desktop present (`Screen::present_background`) subtracts every furniture strip `strip::rects` names, and before the
//! desktop is built the dock's `strip_rect` still answers its pinned layout — a centred bottom box over the word —
//! while `strip::compose_all` paints nothing there. The rows belonged to nobody. [`strips_paint`] is that gate, read
//! by the present (screen.rs's same-line fold) so an unpainted strip withholds nothing; `painted=` on the clear says
//! whether the handed rect reaches a painter; and a screen open re-hands every rect a clear handed since the last
//! open ([`SINCE`]), so the login window's first present repaints the word's band with the screen up
//! (`expose why=screen-open rects=1 px=230400`; flight 32 read `rects=0 px=0` because the clear had already
//! emptied [`RECT`] one line earlier).

use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use core::sync::atomic::{AtomicU32, AtomicU64};

/// SESSIONWEDGE's PUMPWEDGE band (`sessionwedge.rs` `paint`).
pub const PUMP_WEDGE: usize = 0;
/// TESTWATCH's band (`tests_watch.rs` `paint`).
pub const TEST_WATCH: usize = 1;
/// The held splash's stage word and the OVER band under it (`splash::step_label`).
pub const SPLASH_WORD: usize = 2;
/// The boot watchdog's OVER line straight on the panel (`splash::over_panel`, fed by `fs::bootstep::poll`).
pub const BOOT_OVER: usize = 3;
const SLOTS: usize = 4;
const NAMES: [&str; SLOTS] = ["pumpwedge", "testwatch", "splash-word", "boot-over"];

static RECT: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
/// SPLASHWORD2 (B544): the rect each slot's last [`clear`] handed back since the last screen open — [`expose_all`]
/// hands it again, so a clear that ran a moment before the screen's first present is not the only chance.
static SINCE: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
/// Rects handed back to the compositor this boot (every [`clear`] and every rect an [`expose_all`] carried).
static HANDED: AtomicU32 = AtomicU32::new(0);

fn pack(x: usize, y: usize, w: usize, h: usize) -> u64 {
    let c = |v: usize| (v.min(0xffff)) as u64;
    (c(x) << 48) | (c(y) << 32) | (c(w) << 16) | c(h)
}

fn unpack(r: u64) -> (usize, usize, usize, usize) {
    (((r >> 48) & 0xffff) as usize, ((r >> 32) & 0xffff) as usize, ((r >> 16) & 0xffff) as usize, (r & 0xffff) as usize)
}

/// A direct paint put `(x, y, w, h)` on the panel for `slot`. Atomics only — safe from a watchdog that may not wait.
#[inline]
pub fn note(slot: usize, x: usize, y: usize, w: usize, h: usize) {
    if slot >= SLOTS || w == 0 || h == 0 {
        return;
    }
    RECT[slot].store(pack(x, y, w, h), Release);
}

/// SPLASHWORD2 (B544): do the furniture strips OWN their rows right now? Only while something paints them —
/// `strip::compose_all`'s gate, verbatim: a GLASSEYES bare shot holds them, a built desktop composes them; before the
/// desktop is built (setter, create-user, the login screen, after Log Out) nothing does, so the desktop present must
/// not withhold their rects. Read by `Screen::present_background` (screen.rs, same-line fold) and by [`painted`].
pub fn strips_paint() -> bool {
    #[cfg(feature = "login")]
    {
        crate::boot::shot_bare_held() || crate::video::desktopbuild::built()
    }
    #[cfg(not(feature = "login"))]
    {
        true
    }
}

/// SPLASHWORD2 (B544): does a hand-back of `(x, y, w, h)` reach a painter? The windows it meets repaint
/// (`damage_intersecting`); the desktop present publishes it unless a strip that owns its rows (one that paints)
/// stands on it. `false` is the flight-32 shape: a rect the present subtracts and nothing else paints. No panel
/// geometry to ask with (the writer busy) answers `false` — the claim needs its evidence.
fn painted(x: usize, y: usize, w: usize, h: usize) -> bool {
    if !strips_paint() {
        return true;
    }
    let Some((pw, ph)) = crate::video::panel_info_nonblocking().map(|i| (i.width, i.height)) else { return false };
    let mut furn = [None; crate::video::strip::STRIP_MAX];
    let _ = crate::video::strip::rects(pw, ph, &mut furn);
    !furn.iter().flatten().any(|&(sx, sy, sw, sh)| sw != 0 && sh != 0 && sx < x + w && x < sx + sw && sy < y + h && y < sy + sh)
}

/// The compositor repaints its scene over `r`: the windows it meets, then the backdrop. Returns the rect's area.
fn hand(r: u64) -> usize {
    let (x, y, w, h) = unpack(r);
    let _ = crate::video::wm::damage_intersecting(x, y, w, h);
    crate::video::screen::request_present_rect(x, y, w, h);
    HANDED.fetch_add(1, Relaxed);
    w * h
}

/// `slot`'s paint ENDED: its rect goes back to the compositor (a damage, never a fill). Returns whether one was
/// registered. `[fbdirect] clear painter=<name> rect=<x>,<y>,<w>x<h> -> damage`.
pub fn clear(slot: usize) -> bool {
    if slot >= SLOTS {
        return false;
    }
    let r = RECT[slot].swap(0, AcqRel);
    if r == 0 {
        return false;
    }
    let (x, y, w, h) = unpack(r);
    hand(r);
    SINCE[slot].store(r, Release); // SPLASHWORD2 (B544): the next screen open hands it again
    serial_println!("[fbdirect] clear painter={} rect={},{},{}x{} -> damage handed={} painted={}", NAMES[slot], x, y, w, h, HANDED.load(Relaxed), painted(x, y, w, h) as u8);
    true
}

/// A screen opened (`login::open_as`, the desktop build): every rect still registered — and, SPLASHWORD2 (B544), every
/// rect a [`clear`] handed since the last screen open — is handed back once, so the compositor's next pass (the new
/// screen's first present) repaints what any direct painter left on the glass. Returns how many.
/// `[fbdirect] expose why=<why> rects=<n> px=<n> handed=<n> painted=<n>`.
pub fn expose_all(why: &str) -> usize {
    let (mut n, mut px, mut ok) = (0usize, 0usize, 0usize);
    for (slot, since) in RECT.iter().zip(SINCE.iter()) {
        let r = match slot.swap(0, AcqRel) {
            0 => since.swap(0, AcqRel),
            r => {
                since.store(0, Release);
                r
            }
        };
        if r != 0 {
            let (x, y, w, h) = unpack(r);
            px += hand(r);
            n += 1;
            ok += painted(x, y, w, h) as usize;
        }
    }
    serial_println!("[fbdirect] expose why={} rects={} px={} handed={} painted={}", why, n, px, HANDED.load(Relaxed), ok);
    n
}

/// Rects handed back to the compositor this boot — the fixtures' `rects=`.
pub fn handed() -> u32 {
    HANDED.load(Relaxed)
}

/// Whether `slot` has a rect on the glass the compositor has not been handed.
pub fn owed(slot: usize) -> bool {
    slot < SLOTS && RECT[slot].load(Acquire) != 0
}

/// How many of the splash's slots (the stage word, the boot OVER line) are still owed — `tests splash`'s `owed=`.
pub fn owed_splash() -> usize {
    owed(SPLASH_WORD) as usize + owed(BOOT_OVER) as usize
}
