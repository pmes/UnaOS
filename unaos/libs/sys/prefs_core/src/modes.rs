// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! DISPLAY MODES (PREFSUI, rmbp-ledger B389, R93: "i guess we need to put adding a dropdown of the supported
//! resolutions?") — the entries of the Display pane's Resolution dropdown and the `system.display.mode` value.
//!
//! The display path sets ONE panel mode (the panel's native one: the rMBP's 2880x1800 after the takeover). What
//! a user can choose is how big things look: the UI scale (UIMETRICS, carried x2: 2 = 1x … 8 = 4x). A mode is the
//! native mode at a scale; its "looks like" size is native / scale. [`modes`] lists the native mode at every
//! half-step scale from 1x whose looks-like width is at least [`MIN_LOOKS_W`], marking the panel's own density
//! scale as the default. The stored value is the looks-like size `"<w>x<h>"`; unset = the default. Pure.

use alloc::string::String;
use alloc::vec::Vec;

/// The `system` key (namespace-relative).
pub const MODE_KEY: &str = "display.mode";
/// A scaled mode narrower than this is not offered.
pub const MIN_LOOKS_W: u32 = 1024;
/// Scale x2 bounds (UIMETRICS: 1.0 ..= 4.0).
pub const S2_MIN: u32 = 2;
pub const S2_MAX: u32 = 8;

/// One dropdown entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    /// The panel mode the hardware runs (always the native one).
    pub w: u32,
    pub h: u32,
    /// UI scale x2.
    pub s2: u32,
    /// What it looks like: `w * 2 / s2` x `h * 2 / s2`.
    pub looks_w: u32,
    pub looks_h: u32,
    /// The panel's own density scale.
    pub default: bool,
}

impl Mode {
    /// The stored value (`"<looks_w>x<looks_h>"`).
    pub fn value(&self) -> String { alloc::format!("{}x{}", self.looks_w, self.looks_h) }
    /// The dropdown caption: the native mode for 1x, `Looks like WxH` for a scale; `(default)` appended.
    pub fn label(&self) -> String {
        let mut s = if self.s2 == S2_MIN { alloc::format!("{}x{} (native)", self.w, self.h) } else { alloc::format!("Looks like {}x{}", self.looks_w, self.looks_h) };
        if self.default { s.push_str(" - default"); }
        s
    }
}

/// The entries for a `w`x`h` native panel whose density scale is `default_s2`. Pure.
pub fn modes(w: u32, h: u32, default_s2: u32) -> Vec<Mode> {
    let mut out = Vec::new();
    if w == 0 || h == 0 { return out; }
    let d = default_s2.clamp(S2_MIN, S2_MAX);
    for s2 in S2_MIN..=S2_MAX {
        let (lw, lh) = (w * 2 / s2, h * 2 / s2);
        if s2 != S2_MIN && lw < MIN_LOOKS_W && s2 != d { continue; }
        out.push(Mode { w, h, s2, looks_w: lw, looks_h: lh, default: s2 == d });
    }
    out
}

/// The index in `ms` of a stored value (`None` = unset or no such entry: the caller takes the default). Pure.
pub fn index_of(ms: &[Mode], v: &str) -> Option<usize> {
    let (a, b) = v.trim().split_once('x')?;
    let (lw, lh) = (a.parse::<u32>().ok()?, b.parse::<u32>().ok()?);
    ms.iter().position(|m| m.looks_w == lw && m.looks_h == lh)
}

/// The default entry's index. Pure.
pub fn default_index(ms: &[Mode]) -> usize {
    ms.iter().position(|m| m.default).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rmbp_panel() {
        let ms = modes(2880, 1800, 5);
        let looks: Vec<(u32, u32)> = ms.iter().map(|m| (m.looks_w, m.looks_h)).collect();
        assert_eq!(looks, [(2880, 1800), (1920, 1200), (1440, 900), (1152, 720)]);
        assert!(ms.iter().all(|m| m.w == 2880 && m.h == 1800));
        assert_eq!(default_index(&ms), 3);
        assert_eq!(ms[2].label(), "Looks like 1440x900");
        assert_eq!(ms[0].label(), "2880x1800 (native)");
        assert_eq!(index_of(&ms, "1440x900"), Some(2));
        assert_eq!(index_of(&ms, "800x600"), None);
        assert_eq!(index_of(&ms, ""), None);
    }

    #[test]
    fn a_96ppi_panel_has_one_mode() {
        let ms = modes(640, 480, 2);
        assert_eq!(ms.len(), 1);
        assert!(ms[0].default);
        assert!(modes(0, 0, 2).is_empty());
    }
}
