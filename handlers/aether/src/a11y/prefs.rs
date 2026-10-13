//! The user's accessibility preferences — Principia's `system.accessibility.*` rows (prefs_core
//! `schema.rs`, docs/dev/PREFS-SCHEMA.md) — applied to the engine:
//!
//! | Principia key                            | media feature (css_core `media`)          | focus ring            |
//! |------------------------------------------|-------------------------------------------|-----------------------|
//! | `system.accessibility.reduce_motion`     | `prefers-reduced-motion: reduce`          | —                     |
//! | `system.accessibility.contrast`          | `prefers-contrast: more / less` (`custom` under forced colors) | 3px at `more` |
//! | `system.accessibility.forced_colors`     | `forced-colors: active`                   | 3px                   |
//!
//! Aether does not link Principia: the face that does (quartzite GTK) reads the store and hands each value
//! to [`A11yPrefs::from_prefs`] through a lookup closure, then [`A11yPrefs::apply`] records them for every
//! media query evaluated after (`css_core::media::set_user_preferences`) and restyles the page.

use crate::render::focusring::{self, FocusRingTheme};
use crate::AetherEngine;
use css_core::media::{Contrast, UserPreferences};

/// The namespace the keys live in.
pub const PREF_NS: &str = "system";

/// The accessibility preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct A11yPrefs {
    pub reduce_motion: bool,
    pub contrast: Contrast,
    pub forced_colors: bool,
}

fn truthy(v: Option<String>) -> bool {
    v.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on"))
}

impl A11yPrefs {
    /// Reads the three keys through `get` (a key of [`PREF_NS`] → its stored value as text; None = unset,
    /// the schema default).
    pub fn from_prefs(get: impl Fn(&str) -> Option<String>) -> Self {
        A11yPrefs {
            reduce_motion: truthy(get("accessibility.reduce_motion")),
            contrast: get("accessibility.contrast").map(|v| Contrast::parse(&v)).unwrap_or_default(),
            forced_colors: truthy(get("accessibility.forced_colors")),
        }
    }

    /// A11YFACE (SR124): the preferences from the face's `BrowserA11yPrefs` (the rows already typed; `contrast` in
    /// its stored spelling).
    pub fn from_rows(reduce_motion: bool, contrast: &str, forced_colors: bool) -> Self {
        A11yPrefs { reduce_motion, contrast: Contrast::parse(contrast), forced_colors }
    }

    /// A11YFACE (SR124): the witness the shell prints when it applies them:
    /// `:: a11y: prefs contrast=<no-preference|more|less> forced=<active|none> motion=<reduce|no-preference> -> applied ::`.
    pub fn witness(&self) -> String {
        let contrast = match self.contrast {
            Contrast::More => "more",
            Contrast::Less => "less",
            Contrast::NoPreference => "no-preference",
        };
        format!(
            ":: a11y: prefs contrast={contrast} forced={} motion={} -> applied ::",
            if self.forced_colors { "active" } else { "none" },
            if self.reduce_motion { "reduce" } else { "no-preference" }
        )
    }

    /// The media-query side of the preferences.
    pub fn media(&self) -> UserPreferences {
        UserPreferences { reduced_motion: self.reduce_motion, contrast: self.contrast, forced_colors: self.forced_colors }
    }

    /// The focus ring these preferences ask for: 3px under more contrast or forced colors (WCAG 2.2
    /// SC 2.4.13 "Focus Appearance": at least a 2px perimeter, more where the user asked for contrast).
    pub fn focus_ring(&self) -> FocusRingTheme {
        let base = FocusRingTheme::default();
        if self.forced_colors || self.contrast == Contrast::More {
            FocusRingTheme { width: 3.0, color: (0, 0, 0), ..base }
        } else {
            base
        }
    }

    /// Records the preferences (media queries and the focus ring) and, given an engine, restyles its page
    /// so `@media (prefers-…)` rules re-evaluate.
    pub fn apply(&self, engine: Option<&mut AetherEngine>) {
        css_core::media::set_user_preferences(self.media());
        focusring::set_theme(self.focus_ring());
        if let Some(e) = engine {
            e.relayout();
        }
    }

    /// The preferences currently recorded.
    pub fn current() -> Self {
        let p = css_core::media::user_preferences();
        A11yPrefs { reduce_motion: p.reduced_motion, contrast: p.contrast, forced_colors: p.forced_colors }
    }
}
