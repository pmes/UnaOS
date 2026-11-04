// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! PREFS-SWAP-LOCK (SR644): the swap and lock names of a domain. The orphan rule (`<d>` absent, a parseable swap
//! adopted) reads every name `swap_of` maps to `d`: the kernel's `<d>.new` and the host's per-writer `<d>.new.<tag>`;
//! the lock file and malformed tails are neither a swap nor a domain.

use prefs_core::files::{lock_name, swap_name, swap_of, valid_domain};

#[test]
fn swap_and_lock_names_map_to_their_domain_only() {
    assert_eq!(swap_name("sound", None), "sound.new");
    assert_eq!(swap_name("sound", Some("4242-7")), "sound.new.4242-7");
    assert_eq!(lock_name("sound"), "sound.lock");
    assert_eq!(swap_of("sound.new"), Some("sound"));
    assert_eq!(swap_of("sound.new.4242-7"), Some("sound"));
    assert_eq!(swap_of(&swap_name("vug", Some("1-0"))), Some("vug"));
    for not_swap in ["sound", "sound.lock", "sound.new.", "sound.new.a.b", ".new", "x.y.new", "sound.newer", "so und.new"] {
        assert_eq!(swap_of(not_swap), None, "{not_swap}");
    }
    for name in ["sound.new", "sound.new.4242-7", "sound.lock"] {
        assert!(!valid_domain(name), "{name} is never a domain");
    }
}
