// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// PRINCIPIA2 (SR32): prefs_core's wire constants ARE una-abi's (prefs_core has no runtime deps, so it
// spells them; this pins the spelling).

#[test]
fn the_pref_verbs_and_the_body_ceiling_are_una_abi() {
    assert_eq!(prefs_core::wire::VERB_GET, una_abi::BUS_VERB_PREF_GET);
    assert_eq!(prefs_core::wire::VERB_SET, una_abi::BUS_VERB_PREF_SET);
    assert_eq!(prefs_core::wire::VERB_LIST, una_abi::BUS_VERB_PREF_LIST);
    assert_eq!(prefs_core::wire::VERB_CHANGED, una_abi::BUS_VERB_PREF_CHANGED);
    assert_eq!(prefs_core::wire::BODY_MAX, una_abi::BUS_BODY_MAX);
}
