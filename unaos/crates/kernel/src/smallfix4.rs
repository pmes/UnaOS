// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling (B466 SMALLFIX4: the fold's unique-code checks are the kernel's own, as B416's)
//!
//! SMALLFIX4 (rmbp-ledger B466) — `tests smallfix4` extends SMALLFIX3's unique-code checks to the codes this
//! wave minted, read from the live tables (R80: registered behind the `tests` verb, nothing at boot):
//!
//! * **event codes** — `una_abi::INPUT_EV_ALL` unique and non-zero (DIALOG2's `INPUT_EV_DIALOG_ANSWER`, APPMENU2's
//!   `INPUT_EV_CLOSE_REQ`);
//! * **bus verbs** — the kernel's `BUS_VERB_ALL` (smallfix3's rule) AND the REGISTRABLE tags the kernel itself names
//!   ([`REGISTRABLE`]: the R3PREF trio — PREFSCAP's deputy gate keys on SET), each `>= BUS_VERB_FULFIL_MIN`,
//!   unique, and clear of the HOLOCRON band (a const assertion fails the build);
//! * **shell verbs** — `midden_core::HOST_VERBS` names unique (GATE-VERBS lists them; two arcs adding one verb on
//!   two branches would dispatch the first only);
//! * **Action codes** — `smallfix3::action_codes_unique` over the exhaustive [`crate::smallfix3::ACTIONS`];
//! * **a path launch's name** (item 11) — `wm::app_name_arm_launch` on a scratch owner for `/apps/LUMEN.ELF`, read
//!   back through `wm::app_name_of` (PREFSCAP's caller stamp), then forgotten.
//!
//! Witness: `:: SMALLFIX4: event_codes=unique bus_verbs=unique registrable=unique host_verbs=unique
//! action_codes=unique launch_name=<name> -> PASS :: …`.

/// The registrable bus tags the kernel names (`>= BUS_VERB_FULFIL_MIN`): the R3PREF trio.
pub const REGISTRABLE: &[u8] = &[una_abi::BUS_VERB_R3PREF_GET, una_abi::BUS_VERB_R3PREF_LIST, una_abi::BUS_VERB_R3PREF_SET];

/// Every [`REGISTRABLE`] tag is registrable, unique, and outside the HOLOCRON band.
pub const fn registrable_unique() -> bool {
    let mut i = 0;
    while i < REGISTRABLE.len() {
        let x = REGISTRABLE[i];
        if x < una_abi::BUS_VERB_FULFIL_MIN || (x >= una_abi::BUS_VERB_HOLOCRON_FIRST && x <= una_abi::BUS_VERB_HOLOCRON_LAST) {
            return false;
        }
        let mut j = i + 1;
        while j < REGISTRABLE.len() {
            if REGISTRABLE[j] == x {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}
const _: () = assert!(registrable_unique(), "SMALLFIX4: a registrable bus tag clashes (R3PREF vs HOLOCRON band)");

/// `midden_core::HOST_VERBS` carries no name twice.
pub fn host_verbs_unique() -> bool {
    let v = midden_core::HOST_VERBS;
    (0..v.len()).all(|i| (i + 1..v.len()).all(|j| v[i].0 != v[j].0))
}

/// Item 11: arm a scratch owner as a path launch of `/apps/LUMEN.ELF`, read the name PREFSCAP would stamp, forget it.
fn launch_name_probe() -> alloc::string::String {
    const SCRATCH: u64 = u64::MAX - 0x5F4; // no launcher hands out an owner this high
    crate::video::wm::app_name_arm_launch(SCRATCH, "/apps/LUMEN.ELF");
    let mut buf = [0u8; crate::video::wm::MAX_TITLE];
    let n = crate::video::wm::app_name_of(SCRATCH, &mut buf);
    crate::video::wm::app_name_forget(SCRATCH);
    alloc::string::String::from(core::str::from_utf8(&buf[..n]).unwrap_or(""))
}

/// Register `tests smallfix4` once.
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("smallfix4", selftest);
    }
}

/// `tests smallfix4` — the wave's codes, read from the live tables, and a path launch's name.
pub fn selftest() {
    let ev = una_abi::codes_unique_u64(una_abi::INPUT_EV_ALL);
    let bv = una_abi::codes_unique_u8(una_abi::BUS_VERB_ALL);
    let rg = registrable_unique();
    let hv = host_verbs_unique();
    let ac = crate::smallfix3::action_codes_unique();
    let name = launch_name_probe();
    let pass = ev && bv && rg && hv && ac && name.eq_ignore_ascii_case("lumen");
    let u = |b: bool| if b { "unique" } else { "CLASH" };
    serial_println!(
        ":: SMALLFIX4: event_codes={} bus_verbs={} registrable={} host_verbs={} action_codes={} launch_name={} -> {} :: events={} verbs={} registrable_tags={} host_verb_rows={} actions={} ::",
        u(ev), u(bv), u(rg), u(hv), u(ac), if name.is_empty() { "none" } else { name.as_str() }, if pass { "PASS" } else { "FAIL" },
        una_abi::INPUT_EV_ALL.len(), una_abi::BUS_VERB_ALL.len(), REGISTRABLE.len(), midden_core::HOST_VERBS.len(), crate::smallfix3::ACTIONS.len()
    );
}
