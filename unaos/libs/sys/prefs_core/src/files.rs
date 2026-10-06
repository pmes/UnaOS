// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Principia — shared-core
//!
//! SETTINGSFILES (rmbp-ledger B407, R98) — settings are FILES, one per domain, in `<home>/settings/`.
//!
//! The Be shape (MACPARITY §16 B1): the store everyone reads is split by DOMAIN into human-readable TOML
//! files the user can open, read and delete in Quarry. The keys, the schema and the bus are unchanged —
//! this module is only the rule that names a key's file ([`domain_of`]), the split ([`split`]) and the
//! file's text ([`render`]): an `# auto-saved <ISO time> by <who>` line at the top and the schema's `doc`
//! as a comment above every key. [`crate::PrefTree::parse`] reads it back (comments are skipped), so a
//! rendered file IS its tree. Nothing here does I/O: the kernel (`src/prefs.rs`) and the host bring it.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{PrefTree, PrefValue};

/// The folder in the home that holds the files.
pub const DIR: &str = "settings";

/// The six system domains, one per Settings pane family (R98).
pub const SYSTEM_DOMAINS: [&str; 7] = ["display", "login", "desktop", "sound", "trackpad", "notify", "general"];

/// The namespace a program's own settings live in: `app.<name>.<key>` -> `settings/<name>`.
pub const APP_NS: &str = "app";

/// The domain (the file name under [`DIR`]) `ns`/`key` is stored in.
pub fn domain_of(ns: &str, key: &str) -> String {
    let first = key.split('.').next().unwrap_or("");
    let d = match ns {
        "system" => match first {
            "display" if key == "display.wallpaper" => "desktop",
            "display" => "display",
            "dock" => "desktop",
            "login" => "login",
            "audio" => "sound",
            "pointer" | "trackpad" => "trackpad",
            "notify" => crate::notify::DOMAIN,
            _ => "general",
        },
        APP_NS => return String::from(first),
        other => other,
    };
    String::from(d)
}

/// A domain name is one key segment that does not collide with the swap's temp (`<d>.new`).
pub fn valid_domain(d: &str) -> bool {
    crate::valid_segment(d)
}

/// The settings pane (or the program) a domain belongs to — the `by` of a write made from Settings.
pub fn pane_of(domain: &str) -> &'static str {
    match domain {
        "display" => "Settings Display pane",
        "login" => "Settings Login Items pane",
        "desktop" => "Settings General pane (dock, wallpaper)",
        "sound" => "Settings General pane (volume)",
        "trackpad" => "Settings General pane (pointer)",
        "notify" => "Settings Notifications pane",
        "general" => "Settings",
        _ => "a program",
    }
}

/// Split a tree into one tree per domain, sorted by domain.
pub fn split(t: &PrefTree) -> BTreeMap<String, PrefTree> {
    let mut out: BTreeMap<String, PrefTree> = BTreeMap::new();
    for (ns, k, v) in t.entries() {
        let _ = out.entry(domain_of(ns, k)).or_default().set(ns, k, v.clone());
    }
    out
}

/// The part of `t` stored in `domain`.
pub fn part(t: &PrefTree, domain: &str) -> PrefTree {
    let mut p = PrefTree::new();
    for (ns, k, v) in t.entries() {
        if domain_of(ns, k) == domain {
            let _ = p.set(ns, k, v.clone());
        }
    }
    p
}

/// Remove every key of `domain` from `t`; answers what was removed.
pub fn clear(t: &mut PrefTree, domain: &str) -> Vec<(String, String)> {
    let gone: Vec<(String, String)> =
        t.entries().filter(|(ns, k, _)| domain_of(ns, k) == domain).map(|(ns, k, _)| (String::from(ns), String::from(k))).collect();
    for (ns, k) in &gone {
        t.remove(ns, k);
    }
    gone
}

/// Merge `from` into `into` (a key already in `into` is replaced).
pub fn merge(into: &mut PrefTree, from: &PrefTree) {
    for (ns, k, v) in from.entries() {
        let _ = into.set(ns, k, v.clone());
    }
}

/// One comment line: no newline survives, so a doc cannot end the comment.
fn comment(out: &mut String, s: &str) {
    out.push_str("# ");
    for c in s.chars() {
        out.push(if c == '\n' || c == '\r' { ' ' } else { c });
    }
    out.push('\n');
}

/// The file text of one domain: a title, `# auto-saved <iso> by <by>`, then one `[ns]` table per namespace
/// with every key on its own dotted line under the comment `doc(ns, key)` answers (the schema's `doc`, or a
/// program's declared one). `PrefTree::parse` of the text is `t`.
pub fn render(domain: &str, t: &PrefTree, iso: &str, by: &str, doc: &dyn Fn(&str, &str) -> Option<String>) -> String {
    let mut out = String::new();
    comment(&mut out, &alloc::format!("settings/{} — UnaOS settings, one file per domain (R98). Edit with care; delete this file to reset these to their defaults.", domain));
    comment(&mut out, &alloc::format!("auto-saved {} by {}", iso, by));
    for ns in t.namespaces() {
        out.push('\n');
        out.push('[');
        out.push_str(ns);
        out.push_str("]\n");
        for (k, v) in t.list(ns) {
            if let Some(d) = doc(ns, k) {
                comment(&mut out, &d);
            }
            out.push_str(k);
            out.push_str(" = ");
            out.push_str(&v.to_literal());
            out.push('\n');
        }
    }
    out
}

/// The schema's `doc` for a declared key (the `doc` closure most callers pass to [`render`]).
pub fn schema_doc(ns: &str, key: &str) -> Option<String> {
    crate::schema::lookup(ns, key).map(|k| String::from(k.doc)).or_else(|| if ns == "system" { crate::notify::doc(key) } else { None })
}

/// `(iso, by)` from a rendered file's `# auto-saved <iso> by <by>` line.
pub fn stamp_of(text: &str) -> Option<(&str, &str)> {
    let l = text.lines().take(4).find_map(|l| l.strip_prefix("# auto-saved "))?;
    l.split_once(" by ")
}

/// `ns.key = literal` lines of a parsed file (Get Info's list).
pub fn key_lines(t: &PrefTree) -> Vec<String> {
    t.entries().map(|(ns, k, v): (&str, &str, &PrefValue)| alloc::format!("{}.{} = {}", ns, k, v.to_literal())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains() {
        assert_eq!(domain_of("system", "display.brightness"), "display");
        assert_eq!(domain_of("system", "display.wallpaper"), "desktop");
        assert_eq!(domain_of("system", "dock.pins"), "desktop");
        assert_eq!(domain_of("system", "login.items"), "login");
        assert_eq!(domain_of("system", "audio.volume"), "sound");
        assert_eq!(domain_of("system", "pointer.speed"), "trackpad");
        assert_eq!(domain_of("system", "power.lowbat_shutdown_pct"), "general");
        assert_eq!(domain_of("system", "settings.tab"), "general");
        assert_eq!(domain_of("app", "lumen.window.frame"), "lumen");
        assert_eq!(domain_of("vein", "provider"), "vein");
        // Every schema row lands in a system domain or its own namespace's.
        for r in crate::schema::SCHEMA {
            let d = domain_of(r.ns, r.key);
            assert!(valid_domain(&d));
            assert!(SYSTEM_DOMAINS.contains(&d.as_str()) || d == r.ns, "{}.{}", r.ns, r.key);
        }
    }

    #[test]
    fn rendered_file_is_its_tree() {
        let mut t = PrefTree::new();
        t.set("system", "display.brightness", PrefValue::Int(9)).unwrap();
        t.set("system", "display.font", PrefValue::Str(String::from("serif"))).unwrap();
        t.set("system", "audio.mute", PrefValue::Bool(true)).unwrap();
        t.set("app", "lumen.window.frame", PrefValue::Str(String::from("10,20,800,600"))).unwrap();
        t.set("vein", "temperature", PrefValue::Float(0.5)).unwrap();
        let parts = split(&t);
        assert_eq!(parts.keys().map(String::as_str).collect::<Vec<_>>(), ["display", "lumen", "sound", "vein"]);
        let mut back = PrefTree::new();
        for (d, p) in &parts {
            let text = render(d, p, "2026-10-06T12:00:00Z", "Settings Display pane", &schema_doc);
            assert_eq!(stamp_of(&text), Some(("2026-10-06T12:00:00Z", "Settings Display pane")));
            let parsed = PrefTree::parse(&text).unwrap();
            assert_eq!(&parsed, p, "{}", text);
            merge(&mut back, &parsed);
        }
        assert_eq!(back, t);
        let disp = render("display", &parts["display"], "t", "x", &schema_doc);
        assert!(disp.contains("# ") && disp.contains("\n[system]\n") && disp.contains("display.brightness = 9\n"), "{}", disp);
        let doc = crate::schema::lookup("system", "display.brightness").unwrap().doc;
        assert!(disp.contains(&alloc::format!("# {}\ndisplay.brightness = 9", doc.replace('\n', " "))), "{}", disp);
        let mut c = t.clone();
        assert_eq!(clear(&mut c, "display").len(), 2);
        assert_eq!(c.get("system", "display.brightness"), None);
        assert_eq!(part(&c, "sound"), parts["sound"]);
    }
}
