// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Principia — shared-core
//!
//! NOTIFYPANE (rmbp-ledger B435, MACPARITY rows 24/26) — the Notifications pane's rules, one copy for both rings.
//! Each app NOTIFY has seen gets a stanza `system.notify.<app>.{allow,style,sound}` — SETTINGSFILES' declared
//! keys ([`stanza`], checked by [`crate::declare::check_kind`], their `doc` is the comment in the file) — stored in
//! the ONE domain `settings/notify` with Do Not Disturb (`notify.dnd`) and its schedule (`notify.dnd_from` /
//! `notify.dnd_until`, local hours; equal = no schedule; [`in_window`]).

use alloc::string::String;
use alloc::vec::Vec;

use crate::declare::{DeclKind, DeclKey};
use crate::schema::{Applied, Refusal};
use crate::PrefValue;

/// The domain (`settings/notify`) and the key prefix (`system.notify.*`).
pub const DOMAIN: &str = "notify";
/// The per-app fields, in the pane's column order.
pub const FIELDS: [&str; 3] = ["allow", "style", "sound"];
/// `style` values: a card slides in, or the post only collects in the Center.
pub const STYLES: [&str; 2] = ["banner", "center"];
/// An app name as NOTIFY keeps it (its `Note.app`, lowercased; one key segment).
pub const APP_MAX: usize = 16;
/// The keys that are NOT an app (`notify.dnd*`): an app may not be named one of these.
pub const RESERVED: [&str; 3] = ["dnd", "dnd_from", "dnd_until"];

/// One app's rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub allow: bool,
    /// `true` = `center` (collect only, no card).
    pub center: bool,
    pub sound: bool,
}

/// An app nobody configured: allowed, a banner, with sound (the Mac's default for a new app).
pub const DEFAULT: Rule = Rule { allow: true, center: false, sound: true };

/// The declared stanza of one app (`notify.<app>.<field>`), SETTINGSFILES' kinds.
pub fn stanza() -> Vec<DeclKey> {
    alloc::vec![
        DeclKey { key: String::from("allow"), kind: DeclKind::Bool, default: PrefValue::Bool(DEFAULT.allow),
                  doc: String::from("Allow notifications from this app (off = nothing is shown or collected) (NOTIFYPANE).") },
        DeclKey { key: String::from("style"), kind: DeclKind::Enum(STYLES.iter().map(|s| String::from(*s)).collect()),
                  default: PrefValue::Str(String::from(STYLES[0])),
                  doc: String::from("banner = a card slides in at the top right; center = collected in the Notification Center only.") },
        DeclKey { key: String::from("sound"), kind: DeclKind::Bool, default: PrefValue::Bool(DEFAULT.sound),
                  doc: String::from("Play the alert sound when this app posts (never under Do Not Disturb).") },
    ]
}

/// `notify.<app>.<field>` -> `(app, field index)`; `None` for every other key (`notify.dnd*` included).
pub fn parse_key(key: &str) -> Option<(&str, usize)> {
    let rest = key.strip_prefix("notify.")?;
    let (app, field) = rest.split_once('.')?;
    if app.is_empty() || app.len() > APP_MAX || !crate::valid_segment(app) || RESERVED.contains(&app) {
        return None;
    }
    Some((app, FIELDS.iter().position(|f| *f == field)?))
}

/// The key of `app`'s field `f` (an index into [`FIELDS`]).
pub fn key_of(app: &str, f: usize) -> String {
    alloc::format!("notify.{}.{}", app, FIELDS[f.min(FIELDS.len() - 1)])
}

/// A write of `system.<key>`: an app stanza key is checked by its declared kind; any other key passes (the
/// schema's own rows are checked by the schema).
pub fn check(key: &str, v: PrefValue) -> Result<Applied, Refusal> {
    match parse_key(key) {
        Some((_, f)) => crate::declare::check_kind(&stanza()[f].kind, v),
        None => Ok(Applied { value: v, clamped: false }),
    }
}

/// The comment above a stanza key in `settings/notify`.
pub fn doc(key: &str) -> Option<String> {
    parse_key(key).map(|(_, f)| stanza()[f].doc.clone())
}

/// Apply a stored value of field `f` to `r` (a value of the wrong type leaves it).
pub fn apply(r: &mut Rule, f: usize, v: &PrefValue) {
    match (f, v) {
        (0, PrefValue::Bool(b)) => r.allow = *b,
        (1, PrefValue::Str(s)) => r.center = s == STYLES[1],
        (2, PrefValue::Bool(b)) => r.sound = *b,
        _ => {}
    }
}

/// The value field `f` of `r` stores.
pub fn value(r: &Rule, f: usize) -> PrefValue {
    match f {
        0 => PrefValue::Bool(r.allow),
        1 => PrefValue::Str(String::from(STYLES[r.center as usize])),
        _ => PrefValue::Bool(r.sound),
    }
}

/// Is local hour `h` inside the Do Not Disturb schedule `from..until` (wrapping midnight)? `from == until` = no
/// schedule (never).
pub fn in_window(h: u32, from: u32, until: u32) -> bool {
    let (h, from, until) = (h % 24, from % 24, until % 24);
    if from == until {
        false
    } else if from < until {
        h >= from && h < until
    } else {
        h >= from || h < until
    }
}

/// What the post does: `(shown, collected, sound)` — `shown` = a card, `collected` = in the ring and the bell's
/// count, `sound` = the alert sound is asked for. `quiet` = the event has its own surface (a dialog: no card,
/// but it still sounds); `dnd` = manual or scheduled Do Not Disturb.
pub fn route(r: &Rule, quiet: bool, dnd: bool) -> (bool, bool, bool) {
    if !r.allow {
        return (false, false, false);
    }
    (!quiet && !dnd && !r.center, true, r.sound && !dnd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_parse_and_reserve_dnd() {
        assert_eq!(parse_key("notify.quarry.allow"), Some(("quarry", 0)));
        assert_eq!(parse_key("notify.quarry.sound"), Some(("quarry", 2)));
        assert_eq!(parse_key("notify.dnd"), None);
        assert_eq!(parse_key("notify.dnd_from"), None);
        assert_eq!(parse_key("notify.dnd.allow"), None);
        assert_eq!(parse_key("notify.quarry.badge"), None);
        assert_eq!(key_of("system", 1), "notify.system.style");
        assert_eq!(crate::files::domain_of("system", "notify.quarry.allow"), DOMAIN);
        assert_eq!(crate::files::domain_of("system", "notify.dnd"), DOMAIN);
    }

    #[test]
    fn stanza_checks_by_kind() {
        assert!(check("notify.lumen.style", PrefValue::Str(String::from("center"))).is_ok());
        assert_eq!(check("notify.lumen.style", PrefValue::Str(String::from("alert"))), Err(Refusal::NotInEnum));
        assert!(check("notify.lumen.allow", PrefValue::Int(1)).is_err());
        assert!(doc("notify.lumen.sound").is_some());
        let mut r = DEFAULT;
        apply(&mut r, 1, &value(&Rule { center: true, ..DEFAULT }, 1));
        assert!(r.center);
    }

    #[test]
    fn window_wraps_midnight() {
        assert!(!in_window(3, 0, 0));
        assert!(in_window(23, 22, 7) && in_window(0, 22, 7) && in_window(6, 22, 7));
        assert!(!in_window(7, 22, 7) && !in_window(12, 22, 7));
        assert!(in_window(13, 13, 14) && !in_window(14, 13, 14));
    }

    #[test]
    fn route_gates() {
        assert_eq!(route(&DEFAULT, false, false), (true, true, true));
        assert_eq!(route(&Rule { allow: false, ..DEFAULT }, false, false), (false, false, false));
        assert_eq!(route(&Rule { center: true, ..DEFAULT }, false, false), (false, true, true));
        assert_eq!(route(&DEFAULT, false, true), (false, true, false));
        assert_eq!(route(&DEFAULT, true, false), (false, true, true));
        assert_eq!(route(&Rule { sound: false, ..DEFAULT }, true, false), (false, true, false));
    }
}
