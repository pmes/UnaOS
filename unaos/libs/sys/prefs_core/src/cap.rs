// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Principia — shared-core
//!
//! PREFSCAP (rmbp-ledger B454, SECREVIEW F4) — WHO may write WHAT. A preference is Principia's (R79): the
//! owner decides who writes. The transport stamps the caller ([`Caller`]: the kernel names a ring-3 program
//! from its launcher-armed name, never from the body); [`may_set`] and [`may_declare`] are the ONE decision
//! both rings run, inside [`crate::wire::fulfil_as`], before the store is touched.
//!
//! - the kernel (Settings, the `pref` verb, the kernel's preference client) and Principia's own fulfiller
//!   ([`PRINCIPIA`]) write every key;
//! - a program `<p>` writes `app.<p>.*` (its declared stanza), the namespace named after it (`vein` from
//!   vein), and the `system` rows the schema marks [`crate::schema::Writer::Program`] — nothing else;
//! - a caller with no armed name writes only those `system` rows and declares nothing.

use crate::schema::{self, Writer};

/// Principia's ring-3 fulfiller (PREFS.ELF): it forwards the kernel client's writes under its own name.
pub const PRINCIPIA: &str = "prefs";

/// The stamped caller of a PREF verb.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller<'a> {
    /// The kernel itself (and host Principia's trusted callers).
    Kernel,
    /// A ring-3 program, by its kernel-stamped name (compared case-insensitively: `LUMEN.ELF` is `lumen`).
    Program(&'a str),
    /// A ring-3 caller the kernel cannot name.
    Anon,
}

/// Is `ns.key` a `system` row ring 3 may set (the schema's `ring 3` column)?
pub fn settable(ns: &str, key: &str) -> bool {
    schema::lookup(ns, key).is_some_and(|k| k.writers.contains(&Writer::Program))
}

/// How many schema rows ring 3 may set.
pub fn settable_count() -> usize {
    schema::SCHEMA.iter().filter(|k| k.writers.contains(&Writer::Program)).count()
}

/// May `c` SET `ns.key`?
pub fn may_set(c: Caller, ns: &str, key: &str) -> bool {
    match c {
        Caller::Kernel => true,
        Caller::Program(p) if p.eq_ignore_ascii_case(PRINCIPIA) => true,
        Caller::Program(p) => {
            settable(ns, key)
                || (ns == crate::files::APP_NS && key.split_once('.').is_some_and(|(s, _)| s.eq_ignore_ascii_case(p)))
                || (ns != crate::files::APP_NS && ns != "system" && ns.eq_ignore_ascii_case(p))
        }
        Caller::Anon => settable(ns, key),
    }
}

/// May `c` DECLARE the stanza `name` (`app.<name>.*`)?
pub fn may_declare(c: Caller, name: &str) -> bool {
    match c {
        Caller::Kernel => true,
        Caller::Program(p) => p.eq_ignore_ascii_case(PRINCIPIA) || p.eq_ignore_ascii_case(name),
        Caller::Anon => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capability_table() {
        let l = Caller::Program("LUMEN");
        assert!(may_set(l, "app", "lumen.window.frame") && may_set(l, "lumen", "x"));
        assert!(!may_set(l, "app", "vug.window.frame") && !may_set(l, "app", "lumen"));
        assert!(!may_set(l, "vein", "endpoint") && !may_set(l, "vein", "claudecode.bin") && !may_set(l, "vein", "key_file"));
        assert!(!may_set(l, "system", "login.items") && !may_set(l, "system", "display.mode") && !may_set(l, "system", "dock.pins"));
        assert!(may_set(l, "system", "audio.volume") && may_set(l, "system", "appearance.mode"));
        assert!(!may_set(l, "system", "undeclared.key"), "an undeclared system key is the kernel's");
        assert!(may_set(Caller::Program("vein"), "vein", "endpoint"));
        assert!(may_set(Caller::Program("PREFS"), "system", "login.items") && may_set(Caller::Kernel, "vein", "key_file"));
        assert!(!may_set(Caller::Anon, "app", "lumen.x") && may_set(Caller::Anon, "system", "audio.mute"));
        assert!(!may_set(Caller::Program("system"), "system", "login.items") && !may_set(Caller::Program("app"), "app", "x.y"));
        assert!(may_declare(l, "lumen") && !may_declare(l, "vug") && !may_declare(Caller::Anon, "lumen"));
        assert!(may_declare(Caller::Kernel, "sftest") && may_declare(Caller::Program("prefs"), "lumen"));
        assert_eq!(settable_count(), 18);
    }
}
