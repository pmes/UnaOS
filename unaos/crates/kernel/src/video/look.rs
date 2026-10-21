// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm (SOFTY B629: the desktop's LOOK switch — Principia's `system.look`, applied live)
//!
//! LOOK (rmbp-ledger B629). The desktop has two looks over ONE window system: `una` (the menu bar, the dock, the
//! UnaOS chrome — the default) and `softy` (`video::softy`: the bottom taskbar, the Start menu, the caption buttons
//! at the right). The choice is a USER PREFERENCE in Principia's store, `system.look` (`prefs_core::appearance::
//! LOOK_KEY`, the schema row beside `appearance.*`), never a dotfile and never a build knob: both looks are in
//! every desktop image and the switch is a runtime act with no reboot.
//!
//! * [`is_softy`] — the one relaxed load every hook asks (the menu bar's and the dock's paint and press, the wm's
//!   chrome paint and caption hit-test). Una is the zero state, so an image that never reads the key is the UnaOS
//!   look exactly as it was.
//! * [`set`] — the switch: flyouts closed, the leaving look's furniture vacated, every window's head damaged and
//!   one composite run, then `[look] switch to=<l> from=<l> ms=<n> windows_rechromed=<n> via=<who>`.
//! * [`service`] — the settings service pass (ONE line in `settings::service`): the first pass with a session reads
//!   the key, a login or logout arrives as the reload's PrefChanged (SESSIONGENPOLL B782), PrefChanged (verb 19: a change naming `system.look`, `prefs_client::changed_take`, so the key is re-read —
//!   another client's `PrefSet system.look softy` switches the glass with no reboot), a Settings choice owed by
//!   [`choose`] (stored here, off the click router), and the Start menu's hover pass.

use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use alloc::string::String;
use prefs_core::appearance as ap;

use crate::video::wm;

/// The look on the glass.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Look {
    Una = 0,
    Softy = 1,
}

impl Look {
    pub fn name(self) -> &'static str {
        ap::LOOKS[self as usize]
    }
    fn from_index(i: usize) -> Look {
        if i == 1 { Look::Softy } else { Look::Una }
    }
}

static LOOK: AtomicU8 = AtomicU8::new(0);
/// The PrefChanged mark the key was last read at (`u32::MAX` = never read: the first pass that sees a session reads;
/// PREFSCLIENTKEYS B779: `prefs_client::changed_mark`'s sequence, claimed by [`service`] through `prefs_client::changed_take`).
/// SESSIONGENPOLL (B782): a login or logout reaches the look as the reload's PrefChanged (PREFSRELOAD B763), never as a
/// session-generation poll.
static SEEN_CHANGES: AtomicU32 = AtomicU32::new(u32::MAX);
/// A Settings choice owed to the service pass: 0 none, else `look index + 1`.
static CHOSEN: AtomicU8 = AtomicU8::new(0);
static SWITCHES: AtomicU32 = AtomicU32::new(0);

/// The look on the glass.
#[inline]
pub fn current() -> Look {
    Look::from_index(LOOK.load(Ordering::Relaxed) as usize)
}

/// Is Softy the look? (Every hook's one question.)
#[inline]
pub fn is_softy() -> bool {
    LOOK.load(Ordering::Relaxed) == Look::Softy as u8
}

/// **Switch the look** (`via` names who asked, for the wire). `false` when it was already `to`.
pub fn set(to: Look, via: &str) -> bool {
    let from = current();
    if from == to {
        return false;
    }
    let t0 = crate::arch::ms();
    // The leaving look's transient surfaces go first: a dropdown hung from the menu bar, a Start menu.
    super::crystal::dismiss_for_switch();
    super::softy::startmenu::close("switch");
    super::softy::chrome::hover_clear();
    LOOK.store(to as u8, Ordering::Release);
    // The leaving furniture's pixels are paid BEFORE the windows recomposite over them (a strip vacated after the
    // window pass would erase whatever window sat under the dock).
    match to {
        Look::Softy => {
            let _ = super::dock::vacate_off();
        }
        Look::Una => {
            let _ = super::softy::taskbar::vacate();
        }
    }
    let n = wm::damage_intersecting(0, 0, 1 << 16, 1 << 16);
    super::screen::request_full_present();
    wm::composite();
    let ms = crate::arch::ms().saturating_sub(t0);
    SWITCHES.fetch_add(1, Ordering::Relaxed);
    serial_println!("[look] switch to={} from={} ms={} windows_rechromed={} via={}", to.name(), from.name(), ms, n, via);
    true
}

/// The Settings control chose look `i` (latched off the click router; stored and applied by [`service`]).
pub fn choose(i: usize) {
    CHOSEN.store(i.min(ap::LOOKS.len() - 1) as u8 + 1, Ordering::Release);
}

/// The settings service pass (see the module doc). Cheap when nothing moved: three relaxed loads.
pub fn service() {
    let c = CHOSEN.swap(0, Ordering::AcqRel);
    if c != 0 {
        let to = Look::from_index(c as usize - 1);
        crate::prefs_client::sys_set(ap::LOOK_KEY, crate::prefs::PrefValue::Str(String::from(to.name())));
        SEEN_CHANGES.store(crate::prefs_client::changed_mark(), Ordering::Relaxed);
        set(to, "settings");
    }
    let login = SEEN_CHANGES.load(Ordering::Relaxed) == u32::MAX; // SESSIONGENPOLL (B782): never read; a switch arrives as the reload's PrefChanged
    let mut wb = [0u8; crate::prefs::WHO_BUF];
    if crate::prefs::user_name_in(&mut wb).is_some() {
        let bus = crate::prefs_client::changed_take(&SEEN_CHANGES, is_look_key); // PREFSCLIENTKEYS (B779): only a change naming `system.look`; claims the edge
        if login || bus {
            let v = crate::prefs_client::sys_text(ap::LOOK_KEY);
            set(Look::from_index(ap::look_index(v.as_deref())), if login { "login" } else { "bus" });
        }
    } // SESSIONGENPOLL (B782): no session = the change is left unconsumed (the mark untouched), so the next login's first pass reads it
    }
    super::softy::startmenu::service();
}

/// PREFSCLIENTKEYS (rmbp-ledger B779): `<ns>.<key>` is the one key the look reads (`system.look`).
fn is_look_key(ns: &str, k: &str) -> bool {
    ns == crate::prefs::NS && k == ap::LOOK_KEY
}
