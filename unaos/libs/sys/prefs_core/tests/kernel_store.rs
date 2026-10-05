// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// PREFSKERNEL (rmbp-ledger B345) — the kernel's store as the operand of PRINCIPIA2's 26-step script.
//
// The kernel's `prefs::set` is `prefs_core::wire::persisted_set` over a `Persist` whose lock is the
// kernel's `TREE` and whose save is the read-back swap of `preferences.toml` through the VFS; its
// `prefs::bus_fulfil` is `prefs_core::wire::fulfil` over a `Store` whose `set` is that `prefs::set`. The
// kernel crate itself is not host-buildable (its store sits on the mount table, `spin` and the serial
// line), so the shim here is exactly that composition with the VFS swap replaced by a RAM save that
// records each emitted file and can be told to fail: the SAME `persisted_set` and the SAME `fulfil`.
// The script is `principia::wire::tests::principia_answers_the_kernel_verbs_byte_for_byte`'s, verbatim.

use prefs_core::schema::Applied;
use prefs_core::wire::{self, Persist, SetFail, Store, TreeStore, VERB_GET, VERB_LIST, VERB_SET};
use prefs_core::{PrefTree, PrefValue};

/// The kernel store with its swap in RAM: `files` is every `preferences.toml` the swap would have written.
#[derive(Default)]
struct KernelShim {
    tree: PrefTree,
    files: Vec<String>,
    fail_saves: bool,
}

struct KPersist<'a>(&'a mut KernelShim);

impl Persist for KPersist<'_> {
    fn with_tree<R>(&mut self, f: impl FnOnce(&mut PrefTree) -> R) -> R {
        f(&mut self.0.tree)
    }
    fn save(&mut self) -> Result<(), ()> {
        if self.0.fail_saves {
            return Err(());
        }
        // The kernel's swap: emit, read back, parse — the temp must BE the tree.
        let text = self.0.tree.to_toml();
        assert_eq!(PrefTree::parse(&text).unwrap().to_toml(), text);
        self.0.files.push(text);
        Ok(())
    }
}

impl Store for KernelShim {
    fn get(&self, ns: &str, key: &str) -> Option<PrefValue> {
        self.tree.get(ns, key).cloned()
    }
    fn set(&mut self, ns: &str, key: &str, v: PrefValue) -> Result<Applied, SetFail> {
        wire::persisted_set(&mut KPersist(self), ns, key, v).map(|s| s.applied)
    }
    fn list(&self, ns: &str) -> Vec<(String, PrefValue)> {
        self.tree.list(ns).into_iter().map(|(k, v)| (String::from(k), v.clone())).collect()
    }
    fn namespaces(&self) -> Vec<String> {
        self.tree.namespaces().into_iter().map(String::from).collect()
    }
}

const SCRIPT: &[(u8, &[u8], bool)] = &[
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

fn run(s: &mut dyn Store, verb: u8, body: &[u8], sess: bool) -> (i64, Vec<u8>) {
    let mut out = Vec::new();
    let st = wire::fulfil(s, verb, body, sess, &mut out);
    (st, out)
}

#[test]
fn the_kernel_store_answers_the_26_step_script_byte_for_byte() {
    assert_eq!(SCRIPT.len(), 26);
    let mut kern = KernelShim::default();
    let mut refr = TreeStore::default();
    for (i, (verb, body, sess)) in SCRIPT.iter().enumerate() {
        let k = run(&mut kern, *verb, body, *sess);
        let r = run(&mut refr, *verb, body, *sess);
        assert_eq!(k, r, "step {i}: verb {verb} body {:?}", String::from_utf8_lossy(body));
    }
    // Not "equally empty": the clamp is stored and answered.
    assert_eq!(run(&mut kern, VERB_GET, b"system.display.brightness", false), (0, b"16".to_vec()));
    assert_eq!(run(&mut kern, VERB_SET, b"system.display.brightness\x00-1", true), (0, b"1\x00clamped=true".to_vec()));
    run(&mut refr, VERB_SET, b"system.display.brightness\x00-1", true);
    assert_eq!(kern.tree, refr.0, "the kernel tree is the reference tree");
    // The last file the swap wrote is the tree, byte for byte.
    assert_eq!(kern.files.last().unwrap(), &refr.0.to_toml());
}

#[test]
fn an_unchanged_value_is_not_resaved_and_a_failed_save_rolls_back() {
    let mut kern = KernelShim::default();
    assert_eq!(run(&mut kern, VERB_SET, b"system.audio.volume\x009", true), (0, Vec::new()));
    assert_eq!(kern.files.len(), 1);
    assert_eq!(run(&mut kern, VERB_SET, b"system.audio.volume\x009", true), (0, Vec::new()));
    assert_eq!(kern.files.len(), 1, "the same value is not re-saved");
    // A clamp onto the stored value is not a change either (and still answers the clamp).
    run(&mut kern, VERB_SET, b"system.audio.volume\x0016", true);
    assert_eq!(run(&mut kern, VERB_SET, b"system.audio.volume\x0099", true), (0, b"16\x00clamped=true".to_vec()));
    assert_eq!(kern.files.len(), 2);
    kern.fail_saves = true;
    assert_eq!(run(&mut kern, VERB_SET, b"system.audio.volume\x003", true), (wire::EIO, Vec::new()));
    assert_eq!(kern.tree.get("system", "audio.volume"), Some(&PrefValue::Int(16)), "rolled back");
    assert_eq!(run(&mut kern, VERB_SET, b"system.audio.mute\x00true", true), (wire::EIO, Vec::new()));
    assert_eq!(kern.tree.get("system", "audio.mute"), None, "a new key rolls back to unset");
}
