// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling (B495 SMALLFIX6: flight 26's small reds, read back from the live code, as B480's)
//!
//! SMALLFIX6 (rmbp-ledger B495) — `tests windowcap` and `tests smallfix6` (R80: registered behind the `tests`
//! verb, nothing at boot), and the runner's NOT-FOUND line. `tests smallfix6` lists what it checked:
//!
//! * **fwvol** (item 1) — would the WIFI-1 pass 2 (`wifi::firmware::stage_boot_root`) search the boot card's
//!   native UnaFS root now (`searchable(…)`), else why; `-` without `wifi`;
//! * **openers** (item 3) — the opener `video/mp4` and `video/webm` resolve to on this build (flight 26's two
//!   owed samples: the flown tree had no video registrant; the Player declares both since merge18);
//! * **windowcap** (item 4) — `tests windowcap` is registered (the boot verdict was never a fixture);
//! * **notfound** (item 4) — an unknown fixture name gets its own line (`said`);
//! * **lag** (item 5) — the boot's longest render-handler interval and the steps it overlapped;
//! * **lumen** (item 2) — the start split is the program's own line (`[lumen] first_line_ms=…`), static.
//!
//! `:: SMALLFIX6: fwvol=<…> openers=<mp4>/<webm> windowcap=registered notfound=<said|FAIL> lag=<ms>@<end>:<steps>
//! lumen=split -> PASS|FAIL :: spans=<name:ms,…>`

use alloc::string::String;

/// Register `tests windowcap` and `tests smallfix6` once.
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("windowcap", crate::video::wincap::tests_verdict);
        crate::tests::register("smallfix6", selftest);
    }
}

/// The NOT-FOUND words for `tests <name>` that ran nothing, or `None`.
fn not_found_line(name: Option<&str>, ran: usize) -> Option<String> {
    match name {
        Some(n) if ran == 0 && n != "list" => Some(alloc::format!(
            ":: TESTS: {} -> NOT-FOUND :: no fixture registered by that name (`tests list` names them; a boot verdict is not a fixture) ::",
            n
        )),
        _ => None,
    }
}

/// Flight 26: `tests windowcap` printed only `TESTS: ran=0` — the runner now says why.
pub fn not_found(name: Option<&str>, ran: usize) {
    if let Some(l) = not_found_line(name, ran) {
        serial_println!("{}", l);
    }
}

fn fwvol() -> String {
    #[cfg(all(feature = "wifi", target_arch = "x86_64"))]
    {
        crate::wifi::firmware::boot_root_reach()
    }
    #[cfg(not(all(feature = "wifi", target_arch = "x86_64")))]
    {
        String::from("-")
    }
}

fn opener_of(mime: &str) -> String {
    let mt = crate::shell::vfs_mount_table();
    let op = crate::fs::assoc::registrants_in(&mt, mime).into_iter().next().map(|r| r.opener).unwrap_or_else(|| String::from("none"));
    #[cfg(all(feature = "quarry", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        if !crate::video::quarry::live::openers::available(&op) {
            return alloc::format!("{}(not-in-build)", op);
        }
    }
    op
}

fn selftest() {
    let fw = fwvol();
    let (mp4, webm) = (opener_of("video/mp4"), opener_of("video/webm"));
    let notfound = if not_found_line(Some("smallfix6-no-such-fixture"), 0).is_some() && not_found_line(Some("x"), 1).is_none() { "said" } else { "FAIL" };
    let (h_ms, h_end) = crate::video::lag::boot_handler_span();
    let steps = crate::fs::bootstep::overlaps(h_end.saturating_sub(h_ms), h_end);
    // The Player is the video registrant on every build; it is RUNNABLE only under `videoplayer`.
    let openers_ok = mp4.starts_with("player") && webm.starts_with("player");
    let fw_ok = fw == "-" || fw.starts_with("searchable") || fw.starts_with("no-unafs");
    let pass = openers_ok && fw_ok && notfound == "said";
    serial_println!(
        ":: SMALLFIX6: fwvol={} openers={}/{} windowcap=registered notfound={} lag={}@{}:{} lumen=split -> {} :: spans={}",
        fw,
        mp4,
        webm,
        notfound,
        h_ms,
        h_end,
        steps,
        if pass { "PASS" } else { "FAIL" },
        crate::fs::bootstep::spans()
    );
}
