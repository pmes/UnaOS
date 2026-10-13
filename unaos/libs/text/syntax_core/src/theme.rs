// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The theme map: highlight kind → appearance-token NAME, in the Principia preference shape.
//!
//! The appearance tokens are the painter's (kernel `video/theme.rs` `TOKEN_NAMES`, whose LIGHT/DARK sets
//! carry the colours; `unaos/scripts/appearance.allow` states the painter rule: a colour is never a
//! literal outside the theme). So this core names tokens and never holds a colour: the kernel painter
//! resolves a name through its theme, a host face through its own palette. A user's choice is a
//! preference `tabula.theme.<kind-name> = "<token>"` (namespace [`THEME_NS`], key [`THEME_PREFIX`] +
//! [`HighlightKind::name`]); an unset key keeps the default below, an unknown token name is ignored
//! (the store never holds a default — Principia's rule — so the defaults live here, with the consumer).

use alloc::string::String;

use crate::HighlightKind;

/// The preference namespace the theme keys live in (Tabula's).
pub const THEME_NS: &str = "tabula";
/// The key prefix: `theme.keyword`, `theme.doc-comment`, ...
pub const THEME_PREFIX: &str = "theme.";

/// The appearance tokens a syntax kind may name: `video/theme.rs` `TOKEN_NAMES` rows that are ink on a
/// content ground, plus `Accent` (the chosen accent, `system.appearance.accent`).
pub const APPEARANCE_TOKENS: [&str; 7] =
    ["ContentText", "SyntaxCode", "SyntaxNum", "SyntaxLit", "ErrorText", "TitleTextInactive", "Accent"];

/// kind → appearance-token name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    tokens: [&'static str; HighlightKind::COUNT],
}

impl Default for Theme {
    fn default() -> Self {
        let mut tokens = ["ContentText"; HighlightKind::COUNT];
        for k in HighlightKind::ALL {
            tokens[k.index()] = Theme::default_token(k);
        }
        Theme { tokens }
    }
}

impl Theme {
    /// The shipped mapping.
    pub const fn default_token(k: HighlightKind) -> &'static str {
        use HighlightKind as K;
        match k {
            K::Keyword | K::Heading | K::Link | K::Builtin => "Accent",
            K::Type | K::Function | K::Macro | K::Code | K::Variable => "SyntaxCode",
            K::Number | K::Attribute | K::Lifetime | K::Escape => "SyntaxNum",
            K::String | K::Char => "SyntaxLit",
            K::Comment | K::DocComment => "TitleTextInactive",
            K::Error => "ErrorText",
            K::Identifier | K::Punctuation | K::Operator | K::Emphasis => "ContentText",
        }
    }

    /// The token `k` is painted with.
    pub fn token(&self, k: HighlightKind) -> &'static str {
        self.tokens[k.index()]
    }

    /// Point `k` at `token`; `false` (and no change) when `token` is not an [`APPEARANCE_TOKENS`] name.
    pub fn set(&mut self, k: HighlightKind, token: &str) -> bool {
        match APPEARANCE_TOKENS.iter().find(|t| **t == token) {
            Some(t) => {
                self.tokens[k.index()] = *t;
                true
            }
            None => false,
        }
    }

    /// The preference key (within [`THEME_NS`]) that overrides `k`: `theme.<kind-name>`.
    pub fn pref_key(k: HighlightKind) -> String {
        let mut s = String::from(THEME_PREFIX);
        s.push_str(k.name());
        s
    }

    /// The default theme with every set `tabula.theme.<kind>` applied. `get` answers a key within
    /// [`THEME_NS`] (`"theme.keyword"`), `None` when unset.
    pub fn from_prefs(get: &dyn Fn(&str) -> Option<String>) -> Theme {
        let mut t = Theme::default();
        for k in HighlightKind::ALL {
            if let Some(v) = get(&Theme::pref_key(k)) {
                t.set(k, v.trim());
            }
        }
        t
    }
}
