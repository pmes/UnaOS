// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! SNTP-NET6 — the internet time client that sits on the SHARED NET6 socket surface.
//!
//! # Why this file exists
//!
//! Every piece of internet time sync was already in this tree and none of them were joined on this
//! board. [`crate::net_sntp`] is the shared, arch-neutral RFC 4330 parser + request builder (the one
//! security surface — a 48-byte datagram straight off the wire, every field bounds-checked, no path
//! that can panic). [`crate::clock`] is the shared wall-clock service, and CLOCK-3 already derives
//! every FAT mtime from it, so a synced board stamps real last-write times with no operator action.
//! [`crate::video::menubar`] has drawn a clock in its upper right since it was written, and declines
//! to draw one until the civil clock is anchored.
//!
//! What was missing was a CLIENT on aarch64. x86 has one (`smolnet::sntp_sync_once`, driven from
//! `drivers/e1000.rs`) and the Pi has one (`arch/aarch64/genet.rs`, PI-NET-16). Both hang off a
//! SPECIFIC NIC driver, and that coupling is the whole defect: change the NIC and you lose the
//! clock. This module is written so it cannot repeat it — it names no NIC, no board and no arch
//! register. It talks ONLY to the public [`crate::net_phy::net6`] surface
//! (`open`/`bind`/`sendto`/`recvfrom`/`close`/`gateway`/`resolver`/`dns`), which routes through a
//! `NicOps` adapter that `virtio_net.rs` registers on QEMU `virt` and `rtl8168_tegra.rs` registers
//! on Orin metal. The same bytes therefore run on both, and on whatever NIC registers next.
//!
//! # The shape of one boot
//!
//! [`service_tick`] is the drive seam: NIC-AGNOSTIC, bounded, latched, one attempt sequence per
//! boot. It declines silently while the bring-up has not settled (`net6::ipv4()` / `net6::gateway()`
//! both `None`), so it tolerates being called before the network is up; it stands down entirely if
//! the operator already seeded the clock with `date -s`, because an operator's time beats the
//! network's. When it does run it calls [`sync_now`] exactly once and never again this boot.
//!
//! **A retry loop that prints per packet is forbidden** (SO30 is exactly that defect: a per-pass
//! flood that ate a third of a boot's wire). The bound here is structural, not conventional:
//! [`MAX_ATTEMPTS`] candidate ADDRESSES, deduplicated, one witness line each, one summary line.
//! Four lines is the hard ceiling for a boot, and `no reply` is a COMPLETE and honest outcome — the
//! clock stays unsynced, the bar draws no clock, and nothing anywhere fabricates a time.
//!
//! # Server selection, and why the gateway fallback is the live path
//!
//! In order, each step witnessed, first address that ANSWERS wins:
//!
//!   1. the DHCP-leased resolver ([`net6::resolver`]) — a home router that answers DNS almost always
//!      answers NTP, and it is one hop away;
//!   2. `pool.ntp.org` resolved through [`net6::dns`];
//!   3. the default gateway ([`net6::gateway`]).
//!
//! Step 2 is currently expected to fail on metal (SO47: `net6::dns` returns a terminal error on the
//! Orin), which is why step 3 is not a nicety — on the next Orin boot it is the path this actually
//! takes. The witness line names WHICH source produced the address that answered, so a capture
//! never has to guess. A source that yields no address costs no line and no packet; a source that
//! yields an address already tried costs neither either (the dedupe below).

use core::sync::atomic::{AtomicBool, Ordering};

use crate::clock;
use crate::net_phy::net6;
use crate::net_sntp::{self, Sntp, NTP_PORT, SNTP_LEN};

/// The host step 2 resolves. The RFC-standard NTP pool name, the same string the x86 client uses.
pub const SNTP_POOL_HOST: &str = "pool.ntp.org";

/// The client's ephemeral source port. Distinct from every other bound port on this surface —
/// `net6::dns` uses `next_ephemeral()` (49152+) and the NET6 fixture's UDP leg pins 49252 — so a
/// co-resident witness can never collide with this one.
const SNTP_SPORT: u16 = 49_254;

/// The hard ceiling on sync attempts per [`sync_now`] call: one per candidate SOURCE, deduplicated
/// by address. There is no retry, no backoff loop and no timer — the bound is the length of the
/// ladder, which is a `const`, which is what makes "this cannot flood the wire" a structural claim
/// rather than a promise.
pub const MAX_ATTEMPTS: usize = 3;

/// The three candidate sources, named for the wire. A witness line carries one of these verbatim, so
/// a capture says which step of the ladder produced the address that answered (or did not).
const SRC_RESOLVER: &str = "lease-resolver";
const SRC_DNS: &str = "dns-pool";
const SRC_GATEWAY: &str = "gateway";

/// Why one attempt did not produce a time. Every arm is a DISTINCT honest outcome, and the witness
/// prints the arm — this is the first departure from `smolnet::sntp_sync_once`, which collapses all
/// of these into `None` and then reports every one of them on the wire as "no reply". A Kiss-o'-Death
/// is not a silent server, and a spoofed datagram is not a silent server either.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Why {
    /// Every socket slot on the persistent set was in use, or no NIC has registered.
    NoSocket,
    /// `bind`/`sendto` refused — the socket surface is up but could not put the request on the wire.
    SendFailed,
    /// Nothing came back inside `net6::recvfrom`'s bounded pump. The honest, expected outcome on a
    /// LAN with no NTP responder, and NOT a failure of this code.
    NoReply,
    /// A datagram arrived from somewhere other than `server:123`. Stale or spoofed; dropped unparsed.
    WrongPeer,
    /// Stratum 0 — RFC 4330 §8 Kiss-o'-Death (rate-limit / deny). Back off, never use the time.
    KissOfDeath,
    /// The parser rejected it: short, wrong mode/version, LI=alarm, zero or out-of-band timestamp.
    Malformed,
}

impl Why {
    /// The wire word. Deliberately free of the `FAULT_PATTERNS` tokens (`FAIL`, `PANIC`): a silent
    /// LAN is a complete outcome, and reddening a healthy gate for it would be a lie in the other
    /// direction.
    pub fn word(self) -> &'static str {
        match self {
            Why::NoSocket => "no socket slot free",
            Why::SendFailed => "request could not be sent",
            Why::NoReply => "no reply within budget",
            Why::WrongPeer => "reply from the wrong peer (dropped unparsed)",
            Why::KissOfDeath => "Kiss-o'-Death (stratum 0) — server says back off",
            Why::Malformed => "malformed reply (rejected by the shared parser)",
        }
    }
}

/// ONE bounded, non-blocking sync attempt against `server`.
///
/// Modelled on `smolnet::sntp_sync_once` and departing from it in three measured ways, each noted
/// where it happens below: the typed [`Why`], the peer check, and the socket owner convention.
///
/// NO PANIC PATH on a 48-byte datagram off the wire. This function does exactly one slice index of
/// its own, `&buf[..n]`, and `net6::recvfrom` clamps `n` to `out.len()` at net_phy.rs:1558
/// (`let n = data.len().min(out.len())`), so `n <= buf.len()` holds by the callee's construction and
/// the slice cannot panic. Everything past that point is [`crate::net_sntp::parse`], which is the
/// single hostile-input surface and is bounds-checked before any field read.
pub fn sync_once(server: [u8; 4]) -> Result<(u64, u8), Why> {
    // DEPARTURE 2 of 3 from the x86 client: the socket owner. `net6` hands a kernel-side caller a
    // ring-3 slot and takes it straight back under `u64::MAX` (the shell's address space, which no
    // EL0 teardown sweeps) — the convention `net6::dns` already follows at net_phy.rs:1340. The x86
    // client uses smolnet's own `usize::MAX` persistent-set owner, which does not exist here.
    let Some(sid) = net6::open(u64::MAX, false) else {
        return Err(Why::NoSocket);
    };
    let mut req = [0u8; SNTP_LEN];
    net_sntp::build_request(&mut req);
    let mut out = Err(Why::SendFailed);
    if net6::bind(sid, SNTP_SPORT).is_ok() && net6::sendto(sid, server, NTP_PORT, &req).is_ok() {
        let mut buf = [0u8; 64];
        out = match net6::recvfrom(sid, &mut buf) {
            // DEPARTURE 3 of 3: the PEER CHECK. `smolnet::sntp_sync_once` discards `recvfrom`'s
            // source tuple (`let (_src, _port, n) = got?`) and parses whatever landed on the bound
            // port. The parser is hardened enough that a garbage datagram is rejected, but a
            // WELL-FORMED reply from a machine we never asked would be accepted and would set this
            // machine's clock. One comparison closes it, and the outcome is its own honest arm.
            Some((src, sport, _)) if src != server || sport != NTP_PORT => Err(Why::WrongPeer),
            Some((_, _, n)) => match net_sntp::parse(&buf[..n]) {
                Sntp::Ok { unix_secs, stratum } => {
                    // The CLOCK-1 seam: capture the monotonic tick in the same breath as the anchor,
                    // so the extrapolation that follows is measured from the instant of the reading.
                    let mono = clock::mono_ticks().unwrap_or(0);
                    clock::set_anchor(unix_secs, mono, clock::ClockSource::Sntp { stratum });
                    Ok((unix_secs, stratum))
                }
                Sntp::KissOfDeath => Err(Why::KissOfDeath),
                Sntp::Malformed => Err(Why::Malformed),
            },
            None => Err(Why::NoReply),
        };
    }
    net6::close(sid); // always, on every path — a leaked slot is one of four.
    out
}

/// The wire word for a clock anchor's source. Deliberately local: `clock.rs` needs no edit for this
/// client, and a rendering helper for ONE witness family does not belong in the shared clock service.
fn src_word(s: clock::ClockSource) -> &'static str {
    match s {
        clock::ClockSource::Unset => "unset",
        clock::ClockSource::Manual => "manual (date -s)",
        clock::ClockSource::Sntp { .. } => "sntp",
    }
}

/// Render `unix` as ISO-8601 into `out` and borrow it as `&str`. The renderer is
/// [`crate::clock::render_iso8601`] — NOT a second copy of it — so `time`, the FAT stamp, the x86
/// client, the pi client and this one all print one calendar.
fn iso<'a>(unix: u64, out: &'a mut [u8; 24]) -> &'a str {
    let n = clock::render_iso8601(unix, out);
    core::str::from_utf8(&out[..n]).unwrap_or("????")
}

/// Run the selection ladder and anchor the shared clock from the first candidate that answers.
///
/// Returns `true` iff the clock is anchored as a result. Emits at most `MAX_ATTEMPTS` attempt lines
/// plus exactly one summary line — the ceiling is the ladder's length, so this can never flood.
/// Callable from an operator path (`tste`) as well as from [`service_tick`]; it takes no latch of its
/// own, so an operator can ask for a fresh sync after the boot attempt reported `no reply`.
pub fn sync_now() -> bool {
    let mut tried: [Option<[u8; 4]>; MAX_ATTEMPTS] = [None; MAX_ATTEMPTS];
    let mut n = 0usize;
    let mut won: Option<(&'static str, [u8; 4], u64, u8)> = None;

    for step in 0..MAX_ATTEMPTS {
        let cand = match step {
            // Step 1 — the leased resolver. `net6::resolver()` answers the lease's DNS option, or the
            // gateway when the lease carried none; either way it is a real box one hop away.
            0 => net6::resolver().map(|a| (a, SRC_RESOLVER)),
            // Step 2 — `pool.ntp.org`. ⚠ SO47: on Orin metal this currently returns `None`, which is
            // why step 3 exists and why this client is written to be correct without it. `net6::dns`
            // prints its own typed line, so this step is witnessed even when it yields nothing.
            1 => net6::dns(SNTP_POOL_HOST).map(|a| (a, SRC_DNS)),
            // Step 3 — the default gateway. The path the next Orin boot is expected to take.
            _ => net6::gateway().map(|a| (a, SRC_GATEWAY)),
        };
        let Some((server, source)) = cand else {
            continue; // this source named no address: no packet, no line, no cost
        };
        if tried[..n].iter().any(|t| *t == Some(server)) {
            continue; // the ladder converged on one box (a router that is resolver AND gateway) —
                      // asking it twice would be a retry loop wearing a different source's name
        }
        tried[n] = Some(server);
        n += 1;
        match sync_once(server) {
            Ok((unix, stratum)) => {
                let mut b = [0u8; 24];
                serial_println!(
                    "{} [sntp6] attempt {}/{} server={}.{}.{}.{}:{} source={} -> {} stratum={} (civil clock anchored) ::",
                    net6::P6, n, MAX_ATTEMPTS,
                    server[0], server[1], server[2], server[3], NTP_PORT,
                    source, iso(unix, &mut b), stratum
                );
                won = Some((source, server, unix, stratum));
                break;
            }
            Err(why) => serial_println!(
                "{} [sntp6] attempt {}/{} server={}.{}.{}.{}:{} source={} -> {} ::",
                net6::P6, n, MAX_ATTEMPTS,
                server[0], server[1], server[2], server[3], NTP_PORT,
                source, why.word()
            ),
        }
    }

    match won {
        Some((source, server, unix, stratum)) => {
            let mut b = [0u8; 24];
            serial_println!(
                "{} [sntp6] sync COMPLETE anchored=yes source={} server={}.{}.{}.{} iso={} stratum={} attempts={}/{} ::",
                net6::P6, source,
                server[0], server[1], server[2], server[3],
                iso(unix, &mut b), stratum, n, MAX_ATTEMPTS
            );
            true
        }
        None => {
            serial_println!(
                "{} [sntp6] sync COMPLETE anchored=no attempts={}/{} — clock stays unsynced, the bar draws no clock (honest: nothing on this LAN answered :123) ::",
                net6::P6, n, MAX_ATTEMPTS
            );
            false
        }
    }
}

/// One-shot latch for [`service_tick`]: the boot's sync sequence runs at most once.
static SYNC_ATTEMPTED: AtomicBool = AtomicBool::new(false);

/// **THE DRIVE SEAM.** Call this from any periodic, NIC-AGNOSTIC service path; it is cheap enough to
/// call every pass (one relaxed atomic on the settled path) and safe to call before the network
/// exists.
///
/// Deliberately NOT hung off a NIC driver's service tick. On x86 the only caller of
/// `smolnet::witness_tick_sntp` is a statement inside `drivers/e1000.rs`, guarded
/// `target_arch = "x86_64"` — and THAT coupling is why this board had no clock: the Jetson's NIC is
/// an rtl8168, so the time client went with the Intel part. A seam that belongs to one device is a
/// clock that belongs to one device.
///
/// Three guards, in this order, each of them a documented requirement:
///   * **latched** — one attempt sequence per boot, so a caller on a compositor or scheduler cadence
///     cannot turn it into a retry loop (SO30);
///   * **operator wins** — an existing anchor (a `date -s`, or a previous successful sync) stands
///     down the client rather than overwriting a human's correction;
///   * **network-up** — both `net6::ipv4()` and `net6::gateway()` must answer, i.e. the persistent
///     stack is built AND a DHCP/static config settled. Before that this returns with no line, no
///     packet and no latch, so being called early is free and correct.
pub fn service_tick() {
    if SYNC_ATTEMPTED.load(Ordering::Relaxed) {
        return;
    }
    if clock::raw_anchor().is_some() {
        SYNC_ATTEMPTED.store(true, Ordering::Relaxed);
        serial_println!(
            "{} [sntp6] stand down — the civil clock is already anchored this boot ({}); an operator's time beats the network's ::",
            net6::P6,
            src_word(clock::source())
        );
        return;
    }
    if net6::ipv4().is_none() || net6::gateway().is_none() {
        return; // bring-up has not settled: not an error, not yet our turn
    }
    SYNC_ATTEMPTED.store(true, Ordering::Relaxed);
    let _ = sync_now();
}

// =================================================================================================
// The deterministic fixture — canned datagrams, no NIC, no network, any environment
// =================================================================================================

/// The injected wall-clock instant this fixture scripts: 2026-07-22T15:30:45Z. The SAME instant
/// `smolnet::sntp_x86_gate` and pi/genet's NET16 fixture use, so all three arches assert one
/// round-trip anchor and a divergence is visible by inspection rather than by arithmetic.
const INJ_UNIX: u64 = 1_784_734_245;
/// The string [`clock::render_iso8601`] must reproduce from [`INJ_UNIX`].
const INJ_ISO: &str = "2026-07-22T15:30:45Z";

/// Drive canned SNTP datagrams through the SHARED parser and the SHARED clock anchor path, asserting
/// each outcome, with no NIC and no network — so the client's correctness is provable in ANY
/// environment, exactly as `sntp_x86_gate` proves x86's.
///
/// Bitmask `w`, and the one line that reports it, follow `sntp_x86_gate`'s shape on purpose:
///   * `0x01` a well-formed reply parses to the exact Unix second and renders to [`INJ_ISO`];
///   * `0x02` a short (<48 B) datagram is rejected;
///   * `0x04` stratum 0 surfaces as Kiss-o'-Death;
///   * `0x08` an LI=3 alarm reply is rejected;
///   * `0x10` a live parse ANCHORS `crate::clock` as `Sntp{stratum}` and the deterministic anchor
///     renders exactly [`INJ_ISO`];
///   * `0x20` **the menu bar's honesty rule** — with the anchor cleared, `clock::try_unix_now()` is
///     `None`, which is the exact predicate `video::menubar::clock_hhmm` reads to decide whether to
///     draw a clock at all. Deliverable 1 makes the other branch reachable on this arch for the first
///     time, so the unsynced branch is asserted here rather than assumed.
///
/// **It cleans up after itself** (`b3408a24` is the commit that had to teach the x86 fixture this):
/// the pre-existing civil reading is snapshotted FIRST and restored at the end — restored from
/// `unix_now()` rather than `raw_anchor()`, so an operator's wall time comes back at its CURRENT
/// value instead of jumping backwards to the instant they seeded it. With no prior anchor the clock
/// is left honestly unset.
pub fn fixture() -> Result<(), &'static str> {
    let prior = clock::unix_now().map(|u| (u, clock::source()));
    let mut w: u32 = 0;

    // 0x01 — well-formed reply: exact Unix second + ISO round-trip.
    let good = net_sntp::build_reply(INJ_UNIX, 0, 4, 4, 2);
    let parse_ok = match net_sntp::parse(&good) {
        Sntp::Ok { unix_secs, stratum } => {
            let mut b = [0u8; 24];
            unix_secs == INJ_UNIX && stratum == 2 && iso(unix_secs, &mut b) == INJ_ISO
        }
        _ => false,
    };
    if parse_ok {
        w |= 0x01;
    }
    serial_println!(
        "{} [sntp6] parse well-formed => {} ::",
        net6::P6,
        if parse_ok { "resolved+ISO PASS" } else { "FAIL" }
    );

    // 0x02 — reject a short packet.
    let rej_short = matches!(net_sntp::parse(&good[..40]), Sntp::Malformed);
    if rej_short {
        w |= 0x02;
    }
    serial_println!(
        "{} [sntp6] reject short packet => {} ::",
        net6::P6,
        if rej_short { "malformed PASS" } else { "FAIL" }
    );

    // 0x04 — surface stratum-0 Kiss-o'-Death.
    let is_kod = matches!(net_sntp::parse(&net_sntp::build_reply(INJ_UNIX, 0, 4, 4, 0)), Sntp::KissOfDeath);
    if is_kod {
        w |= 0x04;
    }
    serial_println!(
        "{} [sntp6] surface KoD (stratum 0) => {} ::",
        net6::P6,
        if is_kod { "KoD PASS" } else { "FAIL" }
    );

    // 0x08 — reject an LI=3 alarm reply.
    let rej_alarm = matches!(net_sntp::parse(&net_sntp::build_reply(INJ_UNIX, 3, 4, 4, 2)), Sntp::Malformed);
    if rej_alarm {
        w |= 0x08;
    }
    serial_println!(
        "{} [sntp6] reject LI=3 alarm => {} ::",
        net6::P6,
        if rej_alarm { "malformed PASS" } else { "FAIL" }
    );

    // 0x10 — the live anchor path. `raw_anchor` (non-extrapolated) so the free-running counter never
    // races the assertion.
    let set_ok = match net_sntp::parse(&good) {
        Sntp::Ok { unix_secs, stratum } => {
            let mono = clock::mono_ticks().unwrap_or(0);
            clock::set_anchor(unix_secs, mono, clock::ClockSource::Sntp { stratum });
            match clock::raw_anchor() {
                Some((a, clock::ClockSource::Sntp { stratum: s })) => {
                    let mut b = [0u8; 24];
                    s == 2 && iso(a, &mut b) == INJ_ISO
                }
                _ => false,
            }
        }
        _ => false,
    };
    if set_ok {
        w |= 0x10;
    }
    serial_println!(
        "{} [sntp6] canned reply sets clock => {} ::",
        net6::P6,
        if set_ok { "2026-07-22T15:30:45Z PASS" } else { "FAIL" }
    );

    // 0x20 — the bar's honesty rule, asserted in BOTH directions in three statements: anchored, the
    // composite-safe read yields a time; cleared, it yields `None` and the bar draws nothing.
    let honest_set = clock::try_unix_now().is_some();
    clock::clear_anchor();
    let honest_unset = clock::try_unix_now().is_none();
    if honest_set && honest_unset {
        w |= 0x20;
    }
    serial_println!(
        "{} [sntp6] bar honesty: anchored=>Some unsynced=>None => {} ::",
        net6::P6,
        if honest_set && honest_unset { "both directions PASS" } else { "FAIL" }
    );

    // Cleanup. The fixture planted a canned anchor; it does not get to keep it.
    match prior {
        Some((u, src)) => {
            let t = clock::mono_ticks().unwrap_or(0);
            clock::set_anchor(u, t, src);
            serial_println!(
                "{} [sntp6] canned anchor cleared — the pre-existing {} clock is restored at its current reading ::",
                net6::P6,
                src_word(src)
            );
        }
        None => serial_println!(
            "{} [sntp6] canned anchor cleared — clock unanchored again, exactly as this fixture found it ::",
            net6::P6
        ),
    }

    let pass = w == 0x3f;
    serial_println!(
        "{} [sntp6] SNTP-NET6-GATE: aarch64 sntp client battery {} [w=0x{:x}] (parse-ok+iso|reject-short|kod|reject-alarm|set-clock|bar-honesty) ::",
        net6::P6,
        if pass { "PASS" } else { "FAIL" },
        w
    );
    if pass {
        Ok(())
    } else {
        Err("SNTP-NET6 fixture: one or more legs failed (see the [sntp6] lines on serial)")
    }
}
