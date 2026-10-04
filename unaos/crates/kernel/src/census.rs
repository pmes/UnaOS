// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! CHARTER: Kernel — kernel-by-ruling (R80: nothing runs at boot but the boot)
//!
//! QUIETBOOT M2 — THE CENSUS REGISTRY. Every periodic sampler that prints on its own timer (the
//! serial census, VBJITTER, SMPLOAD, WCPAR, PTRSTUTTER, the deadman tick, the compositor rollups …)
//! asks [`on`] before it prints. The sampler's code and counters are untouched; only its DEFAULT
//! changed: OFF, so a metal boot prints the boot and nothing else (flight 19 printed ~870 census lines
//! between the installer and the desktop).
//!
//! * `census list` — every name and whether it is on.
//! * `census start <name>|all` — turn one (or every) sampler on, live.
//! * `census stop [<name>|all]` — turn it off again (no name = all).
//! * `UNAOS_CENSUS=1` (feature `census`) boots with every sampler on — the old always-on behaviour, for
//!   the QEMU lanes whose specs pin census lines (arroyo arms it for the battery verbs).
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

use crate::console::Console;

pub const SERIAL: u32 = 1 << 0;
pub const VBJITTER: u32 = 1 << 1;
pub const SMPLOAD: u32 = 1 << 2;
pub const WCPAR: u32 = 1 << 3;
pub const PTRSTUTTER: u32 = 1 << 4;
pub const DEADMAN: u32 = 1 << 5;
pub const WCW: u32 = 1 << 6;
pub const MIRROR: u32 = 1 << 7;
pub const SCHEDX86: u32 = 1 << 8;
pub const STACK: u32 = 1 << 9;
pub const RTWIT: u32 = 1 << 10;
pub const PTRINSTALL: u32 = 1 << 11;
pub const SERTX: u32 = 1 << 12;
pub const USBNET: u32 = 1 << 13;
pub const SDHCWR: u32 = 1 << 14;

/// `(name, bit, what it samples)` — the verb's table and `census list`'s rows.
pub const NAMES: &[(&str, u32, &str)] = &[
    ("serial", SERIAL, "the 1 Hz `:: SERIAL:` line-lock census"),
    ("vbjitter", VBJITTER, "the Kepler vblank period/jitter rollup"),
    ("smpload", SMPLOAD, "per-CPU busy/runq/stealable"),
    ("wcpar", WCPAR, "compositor band-worker concurrency"),
    ("ptrstutter", PTRSTUTTER, "pointer dark-window verdict"),
    ("deadman", DEADMAN, "the 1 Hz timer-ISR liveness line"),
    ("wcw", WCW, "`[wc-w]` present amplification rollup"),
    ("mirror", MIRROR, "`[mirror]` tap-loss announcements"),
    ("schedx86", SCHEDX86, "`[schedx86]` render-channel depth / load"),
    ("stack", STACK, "render task stack high-water"),
    ("rtwit", RTWIT, "`[rtwit]` input-to-present latency"),
    ("ptrinstall", PTRINSTALL, "pointer-install ledger"),
    ("sertx", SERTX, "`[sertx]` serial transmit cost"),
    ("usbnet", USBNET, "USB NIC rx/tx counters"),
    ("sdhcwr", SDHCWR, "`:: SDHCWR:` SD write-burst census"),
];

const ALL: u32 = (1 << 15) - 1;

static ON: AtomicU32 = AtomicU32::new(if cfg!(feature = "census") { ALL } else { 0 });

/// Is this sampler allowed to print? One relaxed load.
#[inline]
pub fn on(bit: u32) -> bool {
    ON.load(Relaxed) & bit != 0
}

/// The live bit set (for `tests quietboot`'s line).
pub fn bits() -> u32 {
    ON.load(Relaxed)
}

fn bit_of(name: &str) -> Option<u32> {
    if name == "all" {
        return Some(ALL);
    }
    NAMES.iter().find(|r| r.0 == name).map(|r| r.1)
}

/// The `census` shell verb.
pub fn shell_verb(args: &[&str], console: &mut Console) {
    match (args.first().copied(), args.get(1).copied()) {
        (None, _) | (Some("list"), _) => {
            for (n, b, what) in NAMES {
                console.println(&alloc::format!("{:<11} {}  {}", n, if on(*b) { "on " } else { "off" }, what));
            }
        }
        (Some("start"), Some(n)) => match bit_of(n) {
            Some(b) => {
                ON.fetch_or(b, Relaxed);
                serial_println!(":: CENSUS: start {} bits={} ::", n, bits());
                console.println(&alloc::format!("census: {} on", n));
            }
            None => console.println("census: no such sampler (try `census list`)"),
        },
        (Some("stop"), n) => match bit_of(n.unwrap_or("all")) {
            Some(b) => {
                ON.fetch_and(!b, Relaxed);
                serial_println!(":: CENSUS: stop {} bits={} ::", n.unwrap_or("all"), bits());
                console.println(&alloc::format!("census: {} off", n.unwrap_or("all")));
            }
            None => console.println("census: no such sampler (try `census list`)"),
        },
        _ => console.println("usage: census list | census start <name>|all | census stop [<name>|all]"),
    }
}

/// The armed-at-boot banner, only under the knob (so `banner-cert.sh` can see `census` in the artifact).
pub fn boot_banner() {
    #[cfg(feature = "census")]
    serial_println!(":: CENSUS: armed=all knob=UNAOS_CENSUS bits={} ::", bits());
}
