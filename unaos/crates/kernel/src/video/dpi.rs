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

//! CHARTER: Kernel — wm
//!
//! KERNELFONT2 (rmbp-ledger B363, R85 item 11: "the theme scales by DPI") — `video::dpi`, the panel's density
//! and the ONE scale the theme's device-px metrics are multiplied by.
//!
//! * **ppi** — the EFFECTIVE pixels per inch of the framebuffer: the EDID's native ppi (`font_core::ui::edid_ppi`,
//!   the parse KERNELFONT already linked: hactive and the physical width of the preferred timing) times the
//!   framebuffer's width over that native width, so a GOP mode below the panel's native one reads the density it
//!   really draws at (the rMBP 15-inch at its native 2880x1800: 221; the 13-inch 2560x1600: 227).
//! * **scale** — DESIGNAPPLY (rmbp-ledger B639, R120; DESIGN-LANGUAGE §2.1): `ppi / 110` rounded to the HALF pixel
//!   (the language's density target, about 110 pt per inch), carried x2 as an integer (`2` = 1.0 … `8` = 4.0): 221
//!   and 227 ppi both give **2.0** — the rMBP's 2880x1800 draws a 1440x900 logical desktop, as the Mac draws it
//!   (R120: *"we don't want to make people think we think they're blind"*). It was `ppi / 96` (2.5, a 1152x720
//!   desktop) until B639. Settings > Display keeps the other steps (1.5 and 2.5 among them, `prefs_core::modes`).
//!   No EDID (QEMU, a panel without one) or a build without the desktop engine: 1.0.
//! * The scale is LATCHED once, the first time the console's grid is armed with the panel's width
//!   ([`latch`]), so every metric derived from it agrees for the whole boot; a reader before that gets the value
//!   computed from the live panel without latching it.
//!
//! What follows the scale: the console's character cell and grid (`video::text::Face::Grid`, armed by
//! `fbcon` at the takeover seam), the `font_size` the faces are sized from, and — UIMETRICS (B372) — every
//! furniture length through `ui::Metrics` (the bar and title strip, the frame, the control discs, the gap,
//! the chrome text cell, the dock and the crystal menu) and the native kernel windows.

use core::sync::atomic::{AtomicU32, Ordering};

/// The reference density: 1 CSS px = 1 device px at 96 ppi (the unit `px` multiplies; a face's size reference).
pub const BASE_PPI: u32 = 96;
/// DESIGNAPPLY (B639, R120): the DENSITY TARGET the scale is chosen against — about 110 pt per inch (§2.1).
pub const DENSITY_PPI: u32 = 110;
/// Scale x2 bounds: 1.0 ..= 4.0.
pub const S2_MIN: u32 = 2;
pub const S2_MAX: u32 = 8;

static S2: AtomicU32 = AtomicU32::new(0);
static PPI: AtomicU32 = AtomicU32::new(0);

/// `(native hactive, native ppi)` from the panel's EDID; `None` without a trustworthy EDID or without the
/// desktop engine (no `font_core` linked).
pub fn edid_native() -> Option<(u32, u32)> {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        let b = crate::video::edid_block()?;
        let (hact, _mm, ppi) = font_core::ui::edid_ppi(&b)?;
        if hact == 0 || ppi == 0 {
            return None;
        }
        Some((hact, ppi))
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    None
}

/// `(effective ppi, scale x2)` for a framebuffer `fb_w` px wide (0 = take the native width). Pure.
pub fn compute(fb_w: usize) -> (u32, u32) {
    let Some((hact, native)) = edid_native() else {
        return (0, S2_MIN);
    };
    let eff = if fb_w == 0 { native } else { ((native as u64 * fb_w as u64 + hact as u64 / 2) / hact as u64) as u32 };
    (eff, s2_for(eff))
}

/// `ppi / 110` rounded to the half: `round(ppi * 2 / 110)`, clamped to 1.0 ..= 4.0 (x2). DESIGNAPPLY (B639, R120):
/// 221 ppi (the rMBP 15") -> 4 (2.0); 227 (13") -> 4; 110 -> 2; 192 -> 3 (1.5); 300 -> 5 (2.5).
pub const fn s2_for(ppi: u32) -> u32 {
    if ppi == 0 {
        return S2_MIN;
    }
    let s = (ppi * 2 + DENSITY_PPI / 2) / DENSITY_PPI;
    if s < S2_MIN {
        S2_MIN
    } else if s > S2_MAX {
        S2_MAX
    } else {
        s
    }
}

/// Latch the scale for a framebuffer `fb_w` px wide (first call wins); returns the latched scale x2.
pub fn latch(fb_w: usize) -> u32 {
    let s = S2.load(Ordering::Acquire);
    if s != 0 {
        return s;
    }
    let (ppi, s2) = compute(fb_w);
    PPI.store(ppi, Ordering::Relaxed);
    match S2.compare_exchange(0, s2, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => s2,
        Err(won) => won,
    }
}

/// The scale x2: latched, else computed from the live panel (not latched), else 1.0.
pub fn scale_x2() -> u32 {
    let s = S2.load(Ordering::Acquire);
    if s != 0 {
        return s;
    }
    let w = crate::video::panel_info_nonblocking().map_or(0, |i| i.width);
    compute(w).1
}

/// The effective ppi (latched, else computed; 0 = unknown).
pub fn ppi() -> u32 {
    if S2.load(Ordering::Acquire) != 0 {
        return PPI.load(Ordering::Relaxed);
    }
    let w = crate::video::panel_info_nonblocking().map_or(0, |i| i.width);
    compute(w).0
}

/// A theme length `n` (device px at 96 ppi) at the panel's scale, rounded UP to the whole pixel.
#[inline]
pub fn px(n: usize) -> usize {
    px_at(n, scale_x2())
}

/// [`px`] at an explicit scale x2.
#[inline]
pub const fn px_at(n: usize, s2: u32) -> usize {
    (n * s2 as usize + 1) / 2
}

/// The scale as the wire prints it: `2.5`.
pub fn scale_str(s2: u32) -> alloc::string::String {
    alloc::format!("{}.{}", s2 / 2, if s2 % 2 == 1 { 5 } else { 0 })
}

/// UIMETRICS (B372): `(scale x2, ppi)` for `ui::Metrics::panel` — the latched pair; before the latch, latch
/// now when the panel's width can be read without blocking, else answer the live computation unlatched
/// (a reader under the writer's lock gets the native-width answer, which is the latched one at the
/// panel's native mode).
pub fn metrics_scale() -> (u32, u32) {
    let s = S2.load(Ordering::Acquire);
    if s != 0 {
        return (s, PPI.load(Ordering::Relaxed));
    }
    match crate::video::panel_info_nonblocking() {
        Some(i) if i.width > 0 => {
            let s2 = latch(i.width);
            (s2, PPI.load(Ordering::Relaxed))
        }
        _ => {
            let (ppi, s2) = compute(0);
            (s2, ppi)
        }
    }
}

/// UIMETRICS: has the scale been latched for this boot?
pub fn latched() -> bool {
    S2.load(Ordering::Acquire) != 0
}

/// UIMETRICS: the scale x2 on the furniture's hot path — one relaxed load once latched (the compositor's
/// ignition latches it), [`metrics_scale`] before that.
#[inline]
pub fn s2() -> u32 {
    let s = S2.load(Ordering::Relaxed);
    if s != 0 {
        return s;
    }
    metrics_scale().0
}

/// KFONTPPI (B382): the EDID arrived AFTER the latch. A scale latched while no EDID existed (ppi 0 — on the rMBP
/// the console grid latches at the splash takeover, before the PCI probes reach the iGPU lane's AUX read) is
/// re-latched once from the live panel; a latch that already had a ppi is never moved, and an unlatched scale is
/// left for its first reader. Returns `(scale x2 before, after)`, `(0, 0)` when nothing was re-latched. The math
/// is [`compute`]'s, unchanged.
pub fn relatch_edid() -> (u32, u32) {
    let before = S2.load(Ordering::Acquire);
    if before == 0 || PPI.load(Ordering::Relaxed) != 0 {
        return (0, 0);
    }
    let w = crate::video::panel_info_nonblocking().map_or(0, |i| i.width);
    let (ppi, s2) = compute(w);
    if ppi == 0 {
        return (0, 0);
    }
    PPI.store(ppi, Ordering::Relaxed);
    S2.store(s2, Ordering::Release);
    (before, s2)
}

/// PREFSUI (rmbp-ledger B389, R93) — the panel's OWN density scale x2 (the EDID's; what the Resolution dropdown
/// calls the default), whatever the live scale is now.
pub fn native_s2() -> u32 {
    let w = crate::video::panel_info_nonblocking().map_or(0, |i| i.width);
    compute(w).1
}

/// PREFSUI: set the UI scale LIVE (the Resolution dropdown: the panel keeps its one native mode; a "looks like"
/// mode is this scale). The effective ppi becomes the scale's (`s2 * 48`), or the panel's own at its native scale,
/// so the faces size to it (`text::rescale`). Returns `(scale x2 before, after)`.
pub fn set_live(s2: u32) -> (u32, u32) {
    let s2 = s2.clamp(S2_MIN, S2_MAX);
    let w = crate::video::panel_info_nonblocking().map_or(0, |i| i.width);
    let (nppi, ns2) = compute(w);
    let ppi = if s2 == ns2 && nppi != 0 { nppi } else { s2 * BASE_PPI / 2 };
    PPI.store(ppi, Ordering::Relaxed);
    let before = S2.swap(s2, Ordering::AcqRel);
    (before, s2)
}

// DESIGNAPPLY (B639) — the rule's arithmetic, checked at compile time: the rMBP panels land on 2.0, the half steps
// fall where §2.1 says, and the clamp holds.
const _: () = {
    assert!(s2_for(221) == 4 && s2_for(227) == 4);
    assert!(s2_for(110) == 2 && s2_for(165) == 3 && s2_for(192) == 3 && s2_for(275) == 5 && s2_for(300) == 5);
    assert!(s2_for(1) == S2_MIN && s2_for(2000) == S2_MAX && s2_for(0) == S2_MIN);
};

/// DESIGNAPPLY (B639): the ppi a FACE should be sized at so text follows the same scale the chrome does —
/// `scale x 96` (a pt is `scale` device px, §2.1). `video::text` sizes its faces from [`ppi`] (the panel's physical
/// density, 221 on the rMBP), which now draws text 221/192 = 15 % larger than the 2.0 chrome; the type lane
/// (FONTSCRAFT) switches `text.rs` to this seam. Pure read of the live scale.
pub fn face_ppi() -> u32 {
    s2() * BASE_PPI / 2
}
