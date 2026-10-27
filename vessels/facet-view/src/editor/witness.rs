// CHARTER: Facet (Images) — host face over the shared cores (RASTERFACE, LEDGER SR234)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The witness: ONE token per gesture, on stdout.
//!
//! `:: FACET: gesture=<g> [fields] layers=<n> active=<i> undo=<u> redo=<r> -> ok ::`
//! `:: FACET: gesture=<g> [fields] why=<word> layers=<n> active=<i> undo=<u> redo=<r> -> refused ::`

/// The document facts every token ends with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub layers: usize,
    pub active: usize,
    pub undo: usize,
    pub redo: usize,
}

/// A field value as the token carries it: spaces would split the token's words, so they become `_`.
pub fn word(s: &str) -> String {
    s.chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect()
}

/// The one token. `fields` is the gesture's own `k=v` words (may be empty); `result` is the op's outcome.
pub fn token(gesture: &str, fields: &str, result: Result<(), &str>, f: Facts) -> String {
    let mut t = format!(":: FACET: gesture={gesture}");
    if !fields.is_empty() {
        t.push(' ');
        t.push_str(fields);
    }
    if let Err(why) = result {
        t.push_str(&format!(" why={why}"));
    }
    t.push_str(&format!(" layers={} active={} undo={} redo={}", f.layers, f.active, f.undo, f.redo));
    t.push_str(if result.is_ok() { " -> ok ::" } else { " -> refused ::" });
    t
}

/// FNV-1a 64 (tools/raster's and stria's fnv).
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}
