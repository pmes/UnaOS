// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Principia's preference verbs AS BYTES (PRINCIPIA2, SR32): the kernel's `PREF_GET` / `PREF_SET` /
//! `PREF_LIST` (una-abi 16/17/18) bodies, answered from THIS store through the one shared fulfiller
//! [`prefs_core::wire::fulfil`] — so a request body yields the same status and the same reply bytes from
//! Principia on the host as from PREFS in the kernel. The typed Synapse surface (`PrefGet` / `PrefSet` /
//! `PrefList` / `PrefChanged`) is the same store underneath; this is the seam a byte transport (bandy on
//! the metal, ROADMAP §3b) plugs into.

use prefs_core::schema::Applied;
use prefs_core::wire::{SetFail, Store};

use crate::prefs::{PrefStore, from_core, to_core};

impl Store for PrefStore {
    fn get(&self, ns: &str, key: &str) -> Option<prefs_core::PrefValue> {
        PrefStore::get(self, ns, key).map(|v| to_core(&v))
    }

    fn set(&mut self, ns: &str, key: &str, v: prefs_core::PrefValue) -> Result<Applied, SetFail> {
        // The same order of refusals as the reference store: name, schema, then the tree and the disk.
        prefs_core::validate_ns(ns).map_err(SetFail::Name)?;
        prefs_core::validate_key(key).map_err(SetFail::Name)?;
        prefs_core::schema::check(ns, key, v.clone()).map_err(SetFail::Refused)?;
        match PrefStore::set(self, ns, key, from_core(&v)) {
            Ok(out) => Ok(Applied { value: to_core(&out.value), clamped: out.clamped }),
            Err(e) => {
                log::warn!("[PRINCIPIA] :: wire set {ns}.{key} failed: {e:#}");
                // Validation and the schema passed above, so this is a collision or the disk.
                Err(if self.is_collision(ns, key) { SetFail::Name(prefs_core::PrefError::Collision) } else { SetFail::Io })
            }
        }
    }

    fn list(&self, ns: &str) -> Vec<(String, prefs_core::PrefValue)> {
        PrefStore::list(self, ns).into_iter().map(|(k, v)| (k, to_core(&v))).collect()
    }

    fn namespaces(&self) -> Vec<String> {
        PrefStore::namespaces(self)
    }
}

/// One PREF verb in, `(status, reply body)` out — the kernel's `prefs::bus_fulfil` contract.
pub fn fulfil(store: &mut PrefStore, verb: u8, body: &[u8], in_session: bool) -> (i64, Vec<u8>) {
    let mut out = Vec::new();
    let st = prefs_core::wire::fulfil(store, verb, body, in_session, &mut out);
    (st, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prefs_core::wire::{TreeStore, VERB_GET, VERB_LIST, VERB_SET};

    /// The SAME request bodies through the kernel-shaped reference store (prefs_core's `TreeStore`, what
    /// the kernel's `prefs::set` is minus its save) and through Principia's file-backed store: every
    /// status and every reply byte equal. Then the file Principia wrote parses, in prefs_core, to the
    /// reference tree.
    #[test]
    fn principia_answers_the_kernel_verbs_byte_for_byte() {
        let script: &[(u8, &[u8], bool)] = &[
            (VERB_GET, b"system.audio.mute", true),
            (VERB_SET, b"system.audio.mute\x00true", true),
            (VERB_GET, b"system.audio.mute", false),
            (VERB_SET, b"system.display.brightness\x009", true),
            (VERB_SET, b"system.display.brightness\x000", true),
            (VERB_SET, b"system.display.brightness\x0040", true),
            (VERB_SET, b"system.audio.volume\x00-3", true),
            (VERB_SET, b"system.display.wallpaper\x00\"/home/ann/SKY.PNG\"", true),
            (VERB_SET, b"system.dock.pins\x00\"console,shell,quarry\"", true),
            (VERB_SET, b"vein.temperature\x007.5", true),
            (VERB_SET, b"vein.provider\x00\"claude\"", true),
            (VERB_SET, b"vein.provider\x00\"hal9000\"", true),
            (VERB_SET, b"system.audio.volume\x00\"loud\"", true),
            (VERB_SET, b"system.audio.volume\x009", false),
            (VERB_SET, b"aether.window\x001", true),
            (VERB_SET, b"aether.window.width\x002", true),
            (VERB_SET, b"aether.homepage\x00'https://una.os/'", true),
            (VERB_GET, b"system.display.brightness", false),
            (VERB_GET, b"system.nothing", false),
            (VERB_GET, b"system", false),
            (VERB_SET, b"system.a\x00[1]", true),
            (VERB_LIST, b"system", false),
            (VERB_LIST, b"vein", false),
            (VERB_LIST, b"", false),
            (VERB_LIST, b"a.b", false),
            (99, b"", true),
        ];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.toml");
        let mut host = PrefStore::load(&path).unwrap();
        let mut kern = TreeStore::default();
        for (i, (verb, body, sess)) in script.iter().enumerate() {
            let h = fulfil(&mut host, *verb, body, *sess);
            let mut ko = Vec::new();
            let ks = prefs_core::wire::fulfil(&mut kern, *verb, body, *sess, &mut ko);
            assert_eq!(h, (ks, ko), "step {i}: verb {verb} body {:?}", String::from_utf8_lossy(body));
        }
        // Spot-check the bytes, so "equal" is not "equally empty".
        assert_eq!(fulfil(&mut host, VERB_GET, b"system.display.brightness", false), (0, b"16".to_vec()));
        assert_eq!(
            fulfil(&mut host, VERB_SET, b"system.display.brightness\x00-1", true),
            (0, b"1\x00clamped=true".to_vec())
        );
        prefs_core::wire::fulfil(&mut kern, VERB_SET, b"system.display.brightness\x00-1", true, &mut Vec::new());
        let file = std::fs::read_to_string(&path).unwrap();
        assert_eq!(prefs_core::PrefTree::parse(&file).unwrap(), kern.0, "the file is the reference tree");
        assert_eq!(file, kern.0.to_toml(), "and byte-identical to the kernel's emission of it");
    }
}
