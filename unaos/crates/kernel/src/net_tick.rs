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
///
/// GLASSLAG M2 (rmbp-ledger B370): on x86 the caller is the RENDER service's ~5 s clock (main.rs), and the
/// body is not cheap until it settles: `dhcp_link_tick` -> `dhcp_acquire` pumps the stack for up to
/// `DHCP_WAIT_MS` (2 s) waiting for an offer, and `witness_tick_sntp` waits on an NTP reply. Flight 22
/// printed `SOCK-5: … no offer` every ~5 s, seven times per boot — 2 s of a parked compositor each time,
/// under Peter's "delay in interactivity". The render clock now only STARTS the `net-tick` task (once) and
/// returns; that task, off the compositor, runs [`tick_body`] on the same 5 s cadence.
pub fn service_tick() {
    if !crate::boot::services_gate("net-tick") {
        return; // INSTALLBARE (R86): no lease, no SNTP before the Desktop's services
    }
    #[cfg(target_arch = "x86_64")]
    {
        if !NET_TASK.swap(true, Ordering::AcqRel) {
            let cpu = crate::arch::smp::service_cpu().unwrap_or(0);
            crate::arch::sched::spawn("net-tick", net_task, 0, cpu, crate::arch::sched::PRIO_NORMAL);
            serial_println!("[net] tick task=net-tick cpu={} cadence_ms={} (GLASSLAG: off the render service)", cpu, NET_TICK_MS);
        }
        return;
    }
    #[allow(unreachable_code)]
    tick_body()
}

/// GLASSLAG M2: the `net-tick` task has been started (x86).
#[cfg(target_arch = "x86_64")]
static NET_TASK: AtomicBool = AtomicBool::new(false);
/// GLASSLAG M2: the task's cadence — the render clock's own 5 s.
#[cfg(target_arch = "x86_64")]
const NET_TICK_MS: u64 = 5_000;

/// GLASSLAG M2: the `net-tick` task body (x86) — the old render-thread tick, on its own task.
#[cfg(target_arch = "x86_64")]
fn net_task(_: usize) {
    loop {
        tick_body();
        crate::arch::sched::sleep_ms(NET_TICK_MS);
    }
}

/// The tick itself (the body `service_tick` ran inline before GLASSLAG M2).
fn tick_body() {
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

/// NETCLOCK (B335): polls of the smolnet interface, counted at this arch-neutral seam (`smolnet::poll_now`
/// and the ICMP pump add; the usbnet census line and `tests netclock` read).
static POLLS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// One `iface.poll`.
pub fn note_poll() {
    POLLS.fetch_add(1, Ordering::Relaxed);
}
/// Polls so far this boot.
pub fn polls() -> u64 {
    POLLS.load(Ordering::Relaxed)
}
