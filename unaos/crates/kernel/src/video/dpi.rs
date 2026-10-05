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
//! * **scale** — `ppi / 96` rounded to the HALF pixel, carried x2 as an integer (`2` = 1.0 … `8` = 4.0): 221 and
//!   227 ppi both give 2.5. No EDID (QEMU, a panel without one) or a build without the desktop engine: 1.0.
//! * The scale is LATCHED once, the first time the console's grid is armed with the panel's width
//!   ([`latch`]), so every metric derived from it agrees for the whole boot; a reader before that gets the value
//!   computed from the live panel without latching it.
//!
//! What follows the scale: the console's character cell and grid (`video::text::Face::Grid`, armed by
//! `fbcon` at the takeover seam), the `font_size` the faces are sized from, and — UIMETRICS (B372) — every
//! furniture length through `ui::Metrics` (the bar and title strip, the frame, the control discs, the gap,
//! the chrome text cell, the dock and the crystal menu) and the native kernel windows.

use core::sync::atomic::{AtomicU32, Ordering};

/// The reference density: 1 CSS px = 1 device px at 96 ppi.
pub const BASE_PPI: u32 = 96;
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

/// `ppi / 96` rounded to the half: `round(ppi * 2 / 96)`, clamped to 1.0 ..= 4.0 (x2).
pub const fn s2_for(ppi: u32) -> u32 {
    if ppi == 0 {
        return S2_MIN;
    }
    let s = (ppi * 2 + BASE_PPI / 2) / BASE_PPI;
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
