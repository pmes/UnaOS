// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! LOGIN ITEMS (PREFSUI, rmbp-ledger B389, R91) — what launches itself at login is a USER PREFERENCE:
//! `system.login.items`, a comma-joined list of program names (TOML arrays are outside the subset — the
//! `system.dock.pins` shape), EMPTY by default (R88: nothing opens itself unless the user said so).
//!
//! The rules both rings run on the list: [`parse`] (trim, drop empties / invalid names / duplicates, keep the
//! order, at most [`MAX_ITEMS`]), [`render`] (the stored value), and the edits a panel or a menu makes —
//! [`toggle`], [`remove`], [`move_by`]. Pure; no store here (the store is Principia's).

use alloc::string::String;
use alloc::vec::Vec;

/// The `system` key (namespace-relative).
pub const ITEMS_KEY: &str = "login.items";
/// At most this many items (a login launches every one).
pub const MAX_ITEMS: usize = 16;
/// A name is at most this long.
pub const NAME_MAX: usize = 64;
/// The stored value's ceiling — the schema row's `max_len`.
pub const VALUE_MAX: usize = 256;

/// A program name the list may hold: 1..=[`NAME_MAX`] bytes of `[A-Za-z0-9_./-]`. Pure.
pub fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= NAME_MAX && n.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'-'))
}

/// The stored value as a list, in order. Invalid names and duplicates (ASCII case-insensitive) are dropped. Pure.
pub fn parse(v: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in v.split([',', '\n']) {
        let n = raw.trim();
        if !valid_name(n) || out.iter().any(|x| x.eq_ignore_ascii_case(n)) { continue; }
        if out.len() == MAX_ITEMS { break; }
        out.push(String::from(n));
    }
    out
}

/// The list as the stored value (comma-joined; empty list = `""`), cut at [`VALUE_MAX`] on a whole name. Pure.
pub fn render(items: &[String]) -> String {
    let mut s = String::new();
    for n in items.iter().filter(|n| valid_name(n)).take(MAX_ITEMS) {
        if s.len() + n.len() + (!s.is_empty()) as usize > VALUE_MAX { break; }
        if !s.is_empty() { s.push(','); }
        s.push_str(n);
    }
    s
}

/// Is `name` in the list (ASCII case-insensitive)? Pure.
pub fn contains(items: &[String], name: &str) -> bool {
    items.iter().any(|x| x.eq_ignore_ascii_case(name))
}

/// Add `name` at the end when absent, remove it when present. Returns the new membership. Pure.
pub fn toggle(items: &mut Vec<String>, name: &str) -> bool {
    if remove(items, name) { return false; }
    if !valid_name(name) || items.len() >= MAX_ITEMS { return false; }
    items.push(String::from(name));
    true
}

/// Remove `name`; `true` when it was there. Pure.
pub fn remove(items: &mut Vec<String>, name: &str) -> bool {
    let n = items.len();
    items.retain(|x| !x.eq_ignore_ascii_case(name));
    items.len() != n
}

/// Move item `i` by `d` places (clamped to the list); returns its new index, `None` for no such item. Pure.
pub fn move_by(items: &mut Vec<String>, i: usize, d: isize) -> Option<usize> {
    if i >= items.len() { return None; }
    let j = (i as isize + d).clamp(0, items.len() as isize - 1) as usize;
    let it = items.remove(i);
    items.insert(j, it);
    Some(j)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| String::from(*x)).collect() }

    #[test]
    fn empty_is_the_default_and_round_trips() {
        assert!(parse("").is_empty());
        assert_eq!(render(&[]), "");
        let l = parse(" lumen, quarry ,,bad name,LUMEN,activity");
        assert_eq!(l, s(&["lumen", "quarry", "activity"]));
        assert_eq!(parse(&render(&l)), l);
    }

    #[test]
    fn toggle_remove_and_move() {
        let mut l = vec![];
        assert!(toggle(&mut l, "quarry"));
        assert!(toggle(&mut l, "lumen"));
        assert!(!toggle(&mut l, "QUARRY"));
        assert_eq!(l, s(&["lumen"]));
        toggle(&mut l, "editor");
        assert_eq!(move_by(&mut l, 1, -1), Some(0));
        assert_eq!(l, s(&["editor", "lumen"]));
        assert_eq!(move_by(&mut l, 0, -5), Some(0));
        assert_eq!(move_by(&mut l, 9, 1), None);
        assert!(remove(&mut l, "lumen") && !remove(&mut l, "lumen"));
        assert!(!toggle(&mut l, "a b"));
    }

    #[test]
    fn bounded() {
        let mut l = vec![];
        for i in 0..40 { toggle(&mut l, &alloc::format!("app{}", i)); }
        assert_eq!(l.len(), MAX_ITEMS);
        assert!(render(&l).len() <= VALUE_MAX);
    }
}
