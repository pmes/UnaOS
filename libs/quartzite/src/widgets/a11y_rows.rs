//! CHARTER: Aether (CODEX §2 Web) — Principia's accessibility rows as the face sends them, and the F6 focus order.
//!
//! AETHERMACDEVTOOLS (SR312): A11YFACE (SR124)'s rows moved here from `platforms/gtk/a11y_prefs.rs` (which re-exports
//! them and keeps its gio folder watch), platform-neutral (std + bandy + principia), read by both faces: the three
//! `system.accessibility.*` rows from Principia's store (BP-handlers-5), sent as `SMessage::BrowserA11yPrefs` at start
//! and on every change; the shell's coordinator hands them to every tab (`A11yPrefs::apply`). F6 / Shift+F6 move focus
//! between the chrome's address field and the page, in [`F6_ORDER`] ([`f6_step`]); AppKit's Develop menu reads it.

use bandy::{PrefValue, PrincipiaCommand, SMessage};

/// Principia's namespace and the three keys (prefs_core `schema.rs`, docs/dev/PREFS-SCHEMA.md).
pub const NS: &str = "system";
pub const KEYS: [&str; 3] = ["accessibility.reduce_motion", "accessibility.contrast", "accessibility.forced_colors"];

/// The three rows as the face sends them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A11yRows {
    pub reduce_motion: bool,
    pub contrast: String,
    pub forced_colors: bool,
}

impl A11yRows {
    /// From a lookup of a `system` key's value (None: unset — the schema defaults false / `no-preference`).
    pub fn from_values(get: impl Fn(&str) -> Option<PrefValue>) -> Self {
        let truthy = |k: &str| match get(k) {
            Some(PrefValue::Bool(b)) => b,
            Some(PrefValue::Str(s)) => matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on"),
            Some(PrefValue::Int(i)) => i != 0,
            _ => false,
        };
        let contrast = match get(KEYS[1]) {
            Some(PrefValue::Str(s)) if !s.trim().is_empty() => s.trim().to_ascii_lowercase(),
            _ => "no-preference".to_string(),
        };
        A11yRows { reduce_motion: truthy(KEYS[0]), contrast, forced_colors: truthy(KEYS[2]) }
    }

    pub fn message(&self) -> SMessage {
        SMessage::BrowserA11yPrefs { reduce_motion: self.reduce_motion, contrast: self.contrast.clone(), forced_colors: self.forced_colors }
    }
}

/// The rows in force now: Principia's store (a store that does not load reads as every row unset).
pub fn read_rows() -> A11yRows {
    let store = principia::prefs::PrefStore::load(principia::default_prefs_path()).ok();
    A11yRows::from_values(|k| {
        let s = store.as_ref()?;
        s.get(NS, k).or_else(|| s.effective(NS, k, |e| std::env::var(e).ok()).map(|(v, _)| v))
    })
}

/// A Principia change to one of the three rows.
pub fn is_a11y_change(msg: &SMessage) -> bool {
    matches!(msg, SMessage::Principia(PrincipiaCommand::PrefChanged { ns, key, .. }) if ns == NS && KEYS.contains(&key.as_str()))
}

/// Where F6 puts the keys: the chrome's address field or the page surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusStop {
    Address,
    Page,
}

/// F6's cycle, in order (GTK `platforms/gtk/mod.rs`'s F6 shortcut walks the same two stops).
pub const F6_ORDER: [FocusStop; 2] = [FocusStop::Address, FocusStop::Page];

/// The stop F6 (`back` = Shift+F6) moves to from `current` (None: focus is on neither stop — the page takes it, as GTK
/// does from anywhere but the page).
pub fn f6_step(current: Option<FocusStop>, back: bool) -> FocusStop {
    let Some(cur) = current else { return FocusStop::Page };
    let n = F6_ORDER.len();
    let i = F6_ORDER.iter().position(|s| *s == cur).unwrap_or(0);
    let j = if back { (i + n - 1) % n } else { (i + 1) % n };
    F6_ORDER[j]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows from a lookup: unset rows are the schema defaults; Principia's spellings parse; the message carries
    /// them; only a change to one of the three keys counts.
    #[test]
    fn a11y_rows_rows_kat() {
        let unset = A11yRows::from_values(|_| None);
        assert_eq!(unset, A11yRows { reduce_motion: false, contrast: "no-preference".into(), forced_colors: false });
        let set = A11yRows::from_values(|k| match k {
            "accessibility.reduce_motion" => Some(PrefValue::Str(" Yes ".into())),
            "accessibility.contrast" => Some(PrefValue::Str("MORE".into())),
            "accessibility.forced_colors" => Some(PrefValue::Int(1)),
            _ => None,
        });
        assert_eq!(
            format!("{:?}", set.message()),
            "BrowserA11yPrefs { reduce_motion: true, contrast: \"more\", forced_colors: true }"
        );
        let change = |ns: &str, key: &str| {
            SMessage::Principia(PrincipiaCommand::PrefChanged { ns: ns.into(), key: key.into(), value: PrefValue::Bool(true), clamped: false })
        };
        assert!(is_a11y_change(&change("system", "accessibility.contrast")));
        assert!(!is_a11y_change(&change("aether", "accessibility.contrast")));
        assert!(!is_a11y_change(&change("system", "accessibility.other")));
    }

    /// F6 and Shift+F6 alternate the two stops; from neither, the page takes the keys.
    #[test]
    fn a11y_rows_f6_kat() {
        assert_eq!(F6_ORDER, [FocusStop::Address, FocusStop::Page]);
        assert_eq!(f6_step(Some(FocusStop::Page), false), FocusStop::Address);
        assert_eq!(f6_step(Some(FocusStop::Address), false), FocusStop::Page);
        assert_eq!(f6_step(Some(FocusStop::Page), true), FocusStop::Address);
        assert_eq!(f6_step(Some(FocusStop::Address), true), FocusStop::Page);
        assert_eq!(f6_step(None, false), FocusStop::Page);
        assert_eq!(f6_step(None, true), FocusStop::Page);
    }
}
