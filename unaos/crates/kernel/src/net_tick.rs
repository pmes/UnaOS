// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! SNTPDRV M1/M3: the arch-neutral network DRIVE SEAM. One call, [`service_tick`], from the
//! scheduler's periodic hook (x86: main.rs beside `emit_load_witness`, the ~5 s clock; aarch64: net6's
//! own status path). It re-dispatches to the arch's existing SNTP client and emits the additive
//! `:: SNTP: link=<e1000|usbnet|net6> synced=<0|1> offset_ms=<n> -> PASS ::` witness ONCE per boot.
//! No NIC driver calls an SNTP client any more: a driver moves frames, the stack drives the clock.
//! `offset_ms` is the step the sync applied to the civil clock (0 when nothing was anchored before).

use core::sync::atomic::{AtomicBool, Ordering};

/// One `:: SNTP:` line per boot.
static WITNESSED: AtomicBool = AtomicBool::new(false);

/// Drive the SNTP client for this arch; cheap and idempotent once settled. Not for interrupt context.
pub fn service_tick() {
    #[cfg(all(feature = "smolnet", feature = "usbnet", target_arch = "x86_64"))]
    crate::smolnet::dhcp_link_tick(); // USBNET6 M3: the dongle's lease is asked for once its PHY has link (no-op once leased / with an e1000)
    if WITNESSED.load(Ordering::Relaxed) {
        return;
    }
    let before = crate::clock::unix_now();
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    let (done, link) = {
        crate::smolnet::witness_tick_sntp();
        (
            crate::smolnet::sntp_done(),
            if crate::drivers::e1000::nic_present() { "e1000" } else { "usbnet" },
        )
    };
    #[cfg(all(feature = "sntp6", feature = "net6", target_arch = "aarch64"))]
    let (done, link) = {
        crate::net_sntp_client::service_tick();
        (crate::net_sntp_client::attempted(), "net6")
    };
    if !done || WITNESSED.swap(true, Ordering::Relaxed) {
        return;
    }
    let synced = matches!(crate::clock::source(), crate::clock::ClockSource::Sntp { .. });
    let offset_ms: i64 = match (synced, before, crate::clock::unix_now()) {
        (true, Some(b), Some(a)) => (a as i64 - b as i64) * 1000,
        _ => 0,
    };
    serial_println!(
        ":: SNTP: link={} synced={} offset_ms={} -> PASS ::",
        link,
        synced as u8,
        offset_ms
    );
}
