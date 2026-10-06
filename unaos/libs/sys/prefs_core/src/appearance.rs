// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! APPEARANCE (rmbp-ledger B408, MACPARITY rows 21/22) — Light and Dark, an accent colour and a highlight
//! colour are USER PREFERENCES: `system.appearance.mode` (`light` / `dark` / `auto`), `system.appearance.accent`
//! (one of [`ACCENTS`]) and `system.appearance.highlight` (`accent` = follow the accent, or one of [`ACCENTS`]).
//!
//! The rules both rings run: [`Mode::parse`], [`accent_index`], [`highlight_index`], and [`is_dark`] — `auto`
//! is dark from [`AUTO_DARK_FROM`]:00 to [`AUTO_DARK_UNTIL`]:00 by the local clock (the RTC's hour until
//! NETCLOCK gives a real time; no clock = light). Pure; the palette values are the kernel theme's
//! (`video/theme.rs`), the names are here.

/// The `system` keys (namespace-relative).
pub const MODE_KEY: &str = "appearance.mode";
pub const ACCENT_KEY: &str = "appearance.accent";
pub const HIGHLIGHT_KEY: &str = "appearance.highlight";

/// The mode spellings, in the Settings segment order.
pub const MODES: [&str; 3] = ["light", "dark", "auto"];
/// The eight accent names (ours). Index 0 is the kit's own accent, the default.
pub const ACCENTS: [&str; 8] = ["crispy", "teal", "moss", "amber", "clay", "rose", "violet", "slate"];
/// The highlight spellings: `accent` (follow the accent) then the eight names.
pub const HIGHLIGHTS: [&str; 9] = ["accent", "crispy", "teal", "moss", "amber", "clay", "rose", "violet", "slate"];
/// `auto`: dark from this hour …
pub const AUTO_DARK_FROM: u32 = 19;
/// … until this hour (local).
pub const AUTO_DARK_UNTIL: u32 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Light,
    Dark,
    Auto,
}

impl Mode {
    /// A stored value; unset or unknown = Light (the default).
    pub fn parse(v: Option<&str>) -> Mode {
        match v.map(str::trim) {
            Some(s) if s.eq_ignore_ascii_case("dark") => Mode::Dark,
            Some(s) if s.eq_ignore_ascii_case("auto") => Mode::Auto,
            _ => Mode::Light,
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Mode::Light => "light",
            Mode::Dark => "dark",
            Mode::Auto => "auto",
        }
    }
    pub const fn index(self) -> usize {
        self as usize
    }
    pub const fn from_index(i: usize) -> Mode {
        match i {
            1 => Mode::Dark,
            2 => Mode::Auto,
            _ => Mode::Light,
        }
    }
}

/// Is `hour` (0..=23, local) in the auto dark window?
pub const fn dark_hour(hour: u32) -> bool {
    hour >= AUTO_DARK_FROM || hour < AUTO_DARK_UNTIL
}

/// Does `mode` paint dark at local `hour` (`None` = no clock: auto is light)?
pub fn is_dark(mode: Mode, hour: Option<u32>) -> bool {
    match mode {
        Mode::Light => false,
        Mode::Dark => true,
        Mode::Auto => hour.is_some_and(|h| dark_hour(h % 24)),
    }
}

/// The [`ACCENTS`] index of a stored accent; unset or unknown = 0.
pub fn accent_index(v: Option<&str>) -> usize {
    v.and_then(|s| ACCENTS.iter().position(|a| a.eq_ignore_ascii_case(s.trim()))).unwrap_or(0)
}

/// The highlight as an [`ACCENTS`] index: `accent` / unset / unknown follow `accent`.
pub fn highlight_index(v: Option<&str>, accent: usize) -> usize {
    v.and_then(|s| ACCENTS.iter().position(|a| a.eq_ignore_ascii_case(s.trim()))).unwrap_or(accent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse_and_default_light() {
        assert_eq!(Mode::parse(None), Mode::Light);
        assert_eq!(Mode::parse(Some("Dark")), Mode::Dark);
        assert_eq!(Mode::parse(Some(" auto ")), Mode::Auto);
        assert_eq!(Mode::parse(Some("purple")), Mode::Light);
        for (i, n) in MODES.iter().enumerate() {
            assert_eq!(Mode::from_index(i).name(), *n);
            assert_eq!(Mode::parse(Some(n)).index(), i);
        }
    }

    #[test]
    fn auto_is_dark_from_19_until_7() {
        assert!(!is_dark(Mode::Auto, None));
        assert!(is_dark(Mode::Auto, Some(19)) && is_dark(Mode::Auto, Some(23)) && is_dark(Mode::Auto, Some(0)) && is_dark(Mode::Auto, Some(6)));
        assert!(!is_dark(Mode::Auto, Some(7)) && !is_dark(Mode::Auto, Some(12)) && !is_dark(Mode::Auto, Some(18)));
        assert!(is_dark(Mode::Dark, None) && !is_dark(Mode::Light, Some(22)));
    }

    #[test]
    fn accent_and_highlight() {
        assert_eq!(accent_index(None), 0);
        assert_eq!(accent_index(Some("Violet")), 6);
        assert_eq!(accent_index(Some("nope")), 0);
        assert_eq!(highlight_index(Some("accent"), 3), 3);
        assert_eq!(highlight_index(None, 5), 5);
        assert_eq!(highlight_index(Some("teal"), 5), 1);
        assert_eq!(&HIGHLIGHTS[1..], &ACCENTS[..]);
    }
}
