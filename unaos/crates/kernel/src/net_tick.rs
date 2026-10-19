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
            // INPUTSTALL2 M3 (B388): off the service core where one exists — that core runs the HID pass and the
            // input task; the net drive (lease, SNTP, the SOCK ladder) spins `pump_until` for seconds. The xHCI-safe
            // worker rule (`xhci_worker_cpu`: neither render nor service), falling back to the service core.
            let cpu = crate::arch::smp::xhci_worker_cpu(0).or(crate::arch::smp::service_cpu()).unwrap_or(0);
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
    { let _s = scope(Who::Dhcp); crate::smolnet::dhcp_link_tick(); } // IDLENET (B672): its sends are the DHCP starter's. USBNET6 M3: the dongle's lease is asked for once its PHY has link (no-op once leased / with an e1000)
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    { let _s = scope(Who::Witness); crate::drivers::e1000::witness_ladder(); } // IDLENET (B672): the SOCK ladder's sends are the witness's. INPUTSTALL2 M3 (B388): the SOCK ladder off the usb-pump loop (no-op with an e1000)
    if WITNESSED.load(Ordering::Relaxed) {
        return;
    }
    let before = crate::clock::unix_now();
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    let (done, link) = {
        { let _s = scope(Who::Sntp); crate::smolnet::witness_tick_sntp(); } // IDLENET (B672): the pool.ntp.org lookup and the request are the SNTP starter's
        (
            crate::smolnet::sntp_done(),
            if crate::drivers::e1000::nic_present() { "e1000" } else { "usbnet" },
        )
    };
    #[cfg(all(feature = "sntp6", feature = "net6", target_arch = "aarch64"))]
    let (done, link) = {
        { let _s = scope(Who::Sntp); crate::net_sntp_client::service_tick(); } // IDLENET (B672): as x86
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

// ── IDLENET (rmbp-ledger B672, BP-bar-1, BP-bar-10, BP-net-11) — THE STACK'S OUTBOUND LEDGER ─────────────────────────
// "an idle boot logs zero `[net]` connects not caused by a user verb" (docs/dev/blueprint/bar.md BP-bar-1). Every
// outbound connect and send the stack counts carries WHO started it: the open [`scope`] of the task that sent it (the
// starter: `tick_body` opens DHCP, the SOCK ladder and SNTP around their drives), else the counting site's own name
// (a protocol builder is only reached by its own client: `net_sntp::build_request`, `net_dns::build_query`,
// `net_fetch::build_request` — the `fetch` verb's), else NOBODY: the unattributed bucket `tests idlenet` must read 0.
// Counted at: the three builders and the scopes above. The socket choke points (`smolnet::stack_connect` /
// `stack_sendto` / `stack_send`, the ring-3 socket door in `netring3`) count through the same [`note_connect`] /
// [`note_send`] with `None` once those files carry the one-line call (owed, ledger B672) — until then a ring-3
// program's connect is not on this ledger, and `[idlenet] window` says what is counted (`counted_at=`).

/// Who started an outbound connect or send. A user verb's sends carry the verb's name (`Fetch`, `Shell`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Who {
    Dhcp = 0,
    Dns = 1,
    Sntp = 2,
    Fetch = 3,
    Witness = 4,
    Ring3 = 5,
    Shell = 6,
}
/// The starters, by index; slot `WHO_N` of a ledger is the unattributed bucket.
const WHO_N: usize = 7;
const WHO_NAMES: [&str; WHO_N] = ["dhcp", "dns", "sntp", "fetch", "witness", "ring3", "shell"];

impl Who {
    pub fn name(self) -> &'static str {
        WHO_NAMES[self as usize]
    }
    fn from_index(i: u8) -> Option<Who> {
        Some(match i {
            0 => Who::Dhcp,
            1 => Who::Dns,
            2 => Who::Sntp,
            3 => Who::Fetch,
            4 => Who::Witness,
            5 => Who::Ring3,
            6 => Who::Shell,
            _ => return None,
        })
    }
}

/// Connects and sends per starter (`[WHO_N]` = nobody). `const`-constructible: the live one is [`LEDGER`]; the
/// fixture's control counts on its own.
pub struct Ledger {
    conn: [core::sync::atomic::AtomicU64; WHO_N + 1],
    send: [core::sync::atomic::AtomicU64; WHO_N + 1],
}

impl Ledger {
    pub const fn new() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const Z: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        Ledger { conn: [Z; WHO_N + 1], send: [Z; WHO_N + 1] }
    }
    /// One outbound event: the open scope of this task wins, then `own`, then nobody.
    pub fn note(&self, connect: bool, own: Option<Who>) {
        let i = match open_starter().or(own) {
            Some(w) => w as usize,
            None => WHO_N,
        };
        let row = if connect { &self.conn } else { &self.send };
        row[i].fetch_add(1, Ordering::Relaxed);
    }
    pub fn census(&self) -> Census {
        let mut c = Census { conn: [0; WHO_N + 1], send: [0; WHO_N + 1] };
        for i in 0..=WHO_N {
            c.conn[i] = self.conn[i].load(Ordering::Relaxed);
            c.send[i] = self.send[i].load(Ordering::Relaxed);
        }
        c
    }
}

/// The live ledger (every counting site writes here).
pub static LEDGER: Ledger = Ledger::new();

/// An outbound connect (a SYN issued / a connection the sender made). `own` = the counting site's own starter.
pub fn note_connect(own: Option<Who>) {
    LEDGER.note(true, own);
}
/// An outbound send (a datagram or a stream write handed to the stack).
pub fn note_send(own: Option<Who>) {
    LEDGER.note(false, own);
}

/// A ledger read at one instant.
#[derive(Clone, Copy)]
pub struct Census {
    pub conn: [u64; WHO_N + 1],
    pub send: [u64; WHO_N + 1],
}

impl Census {
    /// What happened between `before` and this read.
    pub fn since(&self, before: &Census) -> Census {
        let mut c = *self;
        for i in 0..=WHO_N {
            c.conn[i] = c.conn[i].saturating_sub(before.conn[i]);
            c.send[i] = c.send[i].saturating_sub(before.send[i]);
        }
        c
    }
    pub fn connects(&self) -> u64 {
        self.conn.iter().sum()
    }
    pub fn sends(&self) -> u64 {
        self.send.iter().sum()
    }
    /// Connects + sends nobody started.
    pub fn unattributed(&self) -> u64 {
        self.conn[WHO_N] + self.send[WHO_N]
    }
    /// `dhcp:<connects>/<sends>,dns:…,…,nobody:<c>/<s>` — every starter by name, zeros included.
    pub fn by(&self) -> alloc::string::String {
        let mut s = alloc::string::String::new();
        for i in 0..=WHO_N {
            if i > 0 {
                s.push(',');
            }
            s.push_str(if i < WHO_N { WHO_NAMES[i] } else { "nobody" });
            s.push_str(&alloc::format!(":{}/{}", self.conn[i], self.send[i]));
        }
        s
    }
}

/// Open starter scopes: the task's key (`current_id + 1`; `u64::MAX` outside a task; 0 = free) and its starter.
const SCOPES: usize = 8;
static SCOPE_TASK: [core::sync::atomic::AtomicU64; SCOPES] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const Z: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    [Z; SCOPES]
};
static SCOPE_WHO: [core::sync::atomic::AtomicU8; SCOPES] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const Z: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
    [Z; SCOPES]
};

fn task_key() -> u64 {
    crate::arch::sched::current_id().map(|i| i.wrapping_add(1).max(1)).unwrap_or(u64::MAX)
}

/// The starter this task opened, if any (only the opener's own key ever matches its slot).
fn open_starter() -> Option<Who> {
    let k = task_key();
    (0..SCOPES).find(|&i| SCOPE_TASK[i].load(Ordering::Acquire) == k).and_then(|i| Who::from_index(SCOPE_WHO[i].load(Ordering::Relaxed)))
}

/// An open starter scope; closing it (drop) ends the attribution.
pub struct Scope(Option<usize>);

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(i) = self.0 {
            SCOPE_TASK[i].store(0, Ordering::Release);
        }
    }
}

/// Everything this task sends until the returned guard drops is `who`'s. Nested: the OUTER starter keeps it (a DNS
/// lookup the SNTP client starts is SNTP's). A full table (8 concurrent starters) opens nothing: the counting site's
/// own name, or nobody, takes the event — loud on the ledger, never a silent credit.
pub fn scope(who: Who) -> Scope {
    if open_starter().is_some() {
        return Scope(None);
    }
    let k = task_key();
    for i in 0..SCOPES {
        if SCOPE_TASK[i].compare_exchange(0, k, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            SCOPE_WHO[i].store(who as u8, Ordering::Relaxed);
            return Scope(Some(i));
        }
    }
    Scope(None)
}

/// The idle window `tests idlenet` watches (`tests idlenet <secs>` overrides, 1..=3600).
pub const IDLE_WINDOW_S: u64 = 60;

/// IDLENET — `tests idlenet`: after login, every connect/send the stack counts over [`IDLE_WINDOW_S`] with no user verb
/// (the fixture's own shell job is the only verb running; a verb's sends would carry its name). Control first, on a
/// private [`Ledger`]: an event with no scope and no site name lands in `nobody`; under `scope(Sntp)` a DNS-site event
/// is SNTP's; after the scope drops it is DNS's again — a control that fails is a FAIL, never a green.
/// Wire: `[idlenet] boot by=<starter:c/s,…>` (since boot, by name: the boot's SNTP / DHCP / DNS), `[idlenet] window
/// by=… counted_at=…`, then `:: IDLENET: window_s=<n> connects=<n> unattributed=<n> sends=<n> -> PASS|FAIL ::`
/// (PASS = unattributed 0).
#[cfg(target_arch = "x86_64")]
pub fn idlenet_selftest() {
    let window_s = crate::tests::arg().and_then(|a| a.parse::<u64>().ok()).filter(|s| (1..=3600).contains(s)).unwrap_or(IDLE_WINDOW_S);
    let probe = Ledger::new();
    probe.note(false, None);
    let nobody_ok = probe.census().unattributed() == 1;
    {
        let _s = scope(Who::Sntp);
        probe.note(true, Some(Who::Dns));
    }
    probe.note(false, Some(Who::Dns));
    let c = probe.census();
    let scope_ok = c.conn[Who::Sntp as usize] == 1 && c.conn[Who::Dns as usize] == 0 && c.send[Who::Dns as usize] == 1 && c.unattributed() == 1;
    if !(nobody_ok && scope_ok) {
        serial_println!(":: IDLENET: control nobody={} scope={} -> FAIL ::", nobody_ok as u8, scope_ok as u8);
        return;
    }
    if !crate::boot::desktop() {
        serial_println!(":: IDLENET: window_s={} -> SKIP reason=no-session ::", window_s);
        return;
    }
    if crate::arch::sched::current_id().is_none() {
        serial_println!(":: IDLENET: window_s={} -> SKIP reason=not-a-task (the window sleeps; the shell job is a task) ::", window_s);
        return;
    }
    let boot = LEDGER.census();
    serial_println!("[idlenet] boot by={} (connects/sends since boot, by starter)", boot.by());
    let t0 = crate::arch::ms();
    while crate::arch::ms().saturating_sub(t0) < window_s * 1000 {
        crate::arch::sched::sleep_ms(1000);
    }
    let w = LEDGER.census().since(&boot);
    serial_println!(
        "[idlenet] window by={} counted_at=builders(sntp,dns,fetch)+scopes(dhcp,witness,sntp) owed=sockets(smolnet,netring3)",
        w.by()
    );
    serial_println!(
        ":: IDLENET: window_s={} connects={} unattributed={} sends={} -> {} ::",
        window_s,
        w.connects(),
        w.unattributed(),
        w.sends(),
        if w.unattributed() == 0 { "PASS" } else { "FAIL" }
    );
}
