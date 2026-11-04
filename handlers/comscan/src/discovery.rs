// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Comscan (CODEX §2 "IO Bridge"), seam host handler — the networked controllers on the local link FOUND by
//! one mDNS / DNS-SD question, so a person picks an address instead of typing it. It finds what `crate::tcp` dials and
//! never dials: no connect, no bus message, no desk, no gate.
//!
//! VUGCOMSCANMDNS (LEDGER SR632). The road, measured against the controllers it asks:
//!
//! - FluidNC advertises `_telnet._tcp` on its telnet port (ESP131, default 23) and `_http._tcp` on its web port
//!   (ESP121, default 80), with no instance name (the instance is the hostname, `fluidnc` by default) and no TXT
//!   record; it advertises no `_websocket._tcp`. So [`query`] asks exactly [`SERVICES`], one PTR question each.
//! - ONE short-lived `UdpSocket` per [`discover_at`] call, bound to an ephemeral port: an mDNS question from a port
//!   other than 5353 is a "legacy unicast" query (RFC 6762 §6.7), answered unicast to that port, so the socket joins
//!   no group and needs no SO_REUSEADDR beside a system responder. Answers are collected until the wait ends.
//! - [`walk`] reads one response: every resource record of the answer, authority and additional sections, names with
//!   their compression pointers (RFC 1035 §4.1.4) followed, bounds-checked, with a hop cap so a pointer loop ends;
//!   PTR (a service in [`SERVICES`]) -> SRV (port, target) -> A (IPv4), all in the SAME message. Nothing is resolved
//!   beyond that (no second question): an instance whose SRV or A did not come with it is not found.
//! - [`words`] is what the Console says: `found: <name> <addr>:<port>` per answer, `found: none` for silence; the
//!   card says them through its squawk row (`crate::PumpCard::discover`). The person dials (VUG-COMSCAN-MDNS-DIAL).
//! - VUGCOMSCANMDNSV6 (LEDGER SR639): IPv6 and every interface. The question also goes to ff02::fb:5353 from a v6
//!   socket ([`MDNS_GROUP_V6`], the same legacy-unicast road), once per link the host has an fe80 address on: std has
//!   no multicast-interface setter, so the link rides the destination's scope id (`ff02::fb%<ifindex>`). The v4
//!   question goes once per local IPv4 address from a socket BOUND to that address (Linux sends a multicast datagram
//!   out of the device owning its bound source). [`walk`] reads AAAA beside A, one [`Found`] per address of the SRV
//!   target; a link-local answer keeps the interface it arrived on ([`Found::scope`], [`Found::socket_addr`]). The
//!   interface lists are Linux's `/proc/net/if_inet6` and `/proc/net/fib_trie` ([`link_scopes`], [`local_v4`]); where
//!   they cannot be read (macOS has no /proc) each family asks once from the default socket, as SR632 did.
//!
//! std only: no libc, no `unsafe`.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, UdpSocket};
use std::time::{Duration, Instant};

use make_core::gcode::Dialect;

/// The mDNS group and port every responder listens on.
pub const MDNS_GROUP: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 251), 5353);

/// The IPv6 link-local mDNS group (VUGCOMSCANMDNSV6 SR639); a send names the link by the destination's scope id.
pub const MDNS_GROUP_V6: SocketAddrV6 = SocketAddrV6::new(Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0xfb), 5353, 0, 0);

/// The service types [`query`] asks for, in question order: FluidNC's (and grblHAL's) telnet stream and web UI.
pub const SERVICES: [&str; 2] = ["_telnet._tcp.local", "_http._tcp.local"];

/// How long a person's discover listens for answers by default.
pub const DISCOVER_WAIT: Duration = Duration::from_millis(1500);

const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_SRV: u16 = 33;
const TYPE_AAAA: u16 = 28;
/// Class IN with the QU bit (RFC 6762 §5.4: a unicast answer is welcome).
const CLASS_IN_QU: u16 = 0x8001;
/// The most compression pointers one name may follow before it is refused (a loop, or a hostile message).
const MAX_HOPS: usize = 16;
/// The most labels one name may carry (a DNS name is at most 255 bytes).
const MAX_LABELS: usize = 128;

/// Which advertised service an answer came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// `_telnet._tcp`: the raw grbl stream, dialed as `tcp://<addr>:<port>`.
    Telnet,
    /// `_http._tcp`: the controller's web UI, a hint (not a pump link).
    Http,
}

/// One controller found on the link: the instance's name, its address (an A or an AAAA) and port, the interface a
/// link-local IPv6 answer arrived on, the service that advertised it, and a dialect guessed from the name alone
/// (FluidNC sends no TXT to read; `None` when the name says nothing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub addr: IpAddr,
    pub port: u16,
    /// The interface index of a link-local IPv6 `addr` (an fe80 address is undialable without it); 0 otherwise.
    pub scope: u32,
    pub service: Service,
    pub dialect_hint: Option<Dialect>,
}

/// The one question message: id 0, no flags, one PTR question (class IN, QU) per entry of [`SERVICES`].
pub fn query() -> Vec<u8> {
    let mut m = Vec::with_capacity(64);
    for v in [0u16, 0, SERVICES.len() as u16, 0, 0, 0] {
        m.extend_from_slice(&v.to_be_bytes());
    }
    for svc in SERVICES {
        for label in svc.split('.') {
            m.push(label.len() as u8);
            m.extend_from_slice(label.as_bytes());
        }
        m.push(0);
        m.extend_from_slice(&TYPE_PTR.to_be_bytes());
        m.extend_from_slice(&CLASS_IN_QU.to_be_bytes());
    }
    m
}

fn be16(msg: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*msg.get(at)?, *msg.get(at + 1)?]))
}

/// The name at `pos`: its labels and the offset just past it in the record (past the first pointer when one is
/// followed). `None` for a name that runs off the message, a reserved label type, or too many hops / labels.
fn read_name(msg: &[u8], mut pos: usize) -> Option<(Vec<String>, usize)> {
    let mut labels = Vec::new();
    let mut end = None;
    let mut hops = 0;
    loop {
        let len = *msg.get(pos)? as usize;
        if len == 0 {
            return Some((labels, end.unwrap_or(pos + 1)));
        }
        if len & 0xC0 == 0xC0 {
            let low = *msg.get(pos + 1)? as usize;
            if end.is_none() {
                end = Some(pos + 2);
            }
            hops += 1;
            if hops > MAX_HOPS {
                return None;
            }
            pos = ((len & 0x3F) << 8) | low;
            continue;
        }
        if len & 0xC0 != 0 {
            return None;
        }
        let label = msg.get(pos + 1..pos + 1 + len)?;
        labels.push(String::from_utf8_lossy(label).into_owned());
        if labels.len() > MAX_LABELS {
            return None;
        }
        pos += 1 + len;
    }
}

/// A name as one case-folded key (DNS names compare without case).
fn key(labels: &[String]) -> String {
    labels.join(".").to_ascii_lowercase()
}

fn service_of(key: &str) -> Option<Service> {
    match key {
        k if k == SERVICES[0] => Some(Service::Telnet),
        k if k == SERVICES[1] => Some(Service::Http),
        _ => None,
    }
}

/// The dialect a name suggests: `fluidnc` / `grbl` grbl, `marlin` / `esp3d` Marlin, else none.
fn hint(name: &str) -> Option<Dialect> {
    let n = name.to_ascii_lowercase();
    if n.contains("fluidnc") || n.contains("grbl") {
        Some(Dialect::Grbl { laser: false })
    } else if n.contains("marlin") || n.contains("esp3d") {
        Some(Dialect::Marlin)
    } else {
        None
    }
}

/// Walk one mDNS response: PTR (a service in [`SERVICES`]) -> SRV -> A / AAAA, all in this message, one [`Found`] per
/// address of each PTR's SRV target (scope 0: `collect` fills a link-local one's). A message that is not a response,
/// or runs short, yields what was complete before the break (nothing, when the SRV / A had not arrived yet). Never
/// panics on hostile bytes.
pub fn walk(msg: &[u8]) -> Vec<Found> {
    let mut out = Vec::new();
    if msg.len() < 12 || msg[2] & 0x80 == 0 {
        return out;
    }
    let (Some(qd), Some(an), Some(ns), Some(ar)) = (be16(msg, 4), be16(msg, 6), be16(msg, 8), be16(msg, 10)) else {
        return out;
    };
    let mut pos = 12;
    for _ in 0..qd {
        let Some((_, p)) = read_name(msg, pos) else { return out };
        pos = p + 4;
    }
    let mut ptrs: Vec<(Service, Vec<String>)> = Vec::new();
    let mut srvs: Vec<(String, u16, String)> = Vec::new();
    let mut addrs: Vec<(String, IpAddr)> = Vec::new();
    for _ in 0..(an as usize + ns as usize + ar as usize) {
        let Some((owner, p)) = read_name(msg, pos) else { break };
        let (Some(ty), Some(rdlen)) = (be16(msg, p), be16(msg, p + 8)) else { break };
        let rd = p + 10;
        let end = rd + rdlen as usize;
        if end > msg.len() {
            break;
        }
        match ty {
            TYPE_PTR => {
                if let Some(svc) = service_of(&key(&owner)) {
                    if let Some((inst, _)) = read_name(msg, rd) {
                        ptrs.push((svc, inst));
                    }
                }
            }
            TYPE_SRV if rdlen >= 7 => {
                if let (Some(port), Some((target, _))) = (be16(msg, rd + 4), read_name(msg, rd + 6)) {
                    srvs.push((key(&owner), port, key(&target)));
                }
            }
            TYPE_A if rdlen == 4 => {
                addrs.push((key(&owner), IpAddr::V4(Ipv4Addr::new(msg[rd], msg[rd + 1], msg[rd + 2], msg[rd + 3]))));
            }
            TYPE_AAAA if rdlen == 16 => {
                let mut o = [0u8; 16];
                o.copy_from_slice(&msg[rd..rd + 16]);
                addrs.push((key(&owner), IpAddr::V6(Ipv6Addr::from(o))));
            }
            _ => {}
        }
        pos = end;
    }
    for (service, inst) in ptrs {
        let k = key(&inst);
        let Some((_, port, target)) = srvs.iter().find(|s| s.0 == k) else { continue };
        let name = inst.first().cloned().unwrap_or_default();
        let dialect_hint = hint(&name);
        for (_, addr) in addrs.iter().filter(|a| &a.0 == target) {
            out.push(Found { name: name.clone(), addr: *addr, port: *port, scope: 0, service, dialect_hint });
        }
    }
    out
}

/// Send [`query`] to `dest` from one short-lived socket on an ephemeral port of `dest`'s family and collect every
/// answer that arrives within `wait` (each datagram through [`walk`]), duplicates dropped, telnet entries first then
/// by name. A socket error ends the collection with what was found. Never dials.
pub fn discover_at(dest: SocketAddr, wait: Duration) -> Vec<Found> {
    let bind = match dest {
        SocketAddr::V4(_) => SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)),
        SocketAddr::V6(_) => SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0)),
    };
    discover_from(&[(bind, dest)], wait)
}

/// [`discover_at`] both mDNS groups ([`MDNS_GROUP`], [`MDNS_GROUP_V6`]) on every interface [`targets`] lists, under one
/// deadline.
pub fn discover(wait: Duration) -> Vec<Found> {
    discover_from(&targets(), wait)
}

/// What the Console says of a discover: `found: <name> <addr>:<port>` per telnet entry, `found: <name> <addr>:<port>
/// (http)` per web entry, and `found: none` when nothing answered.
pub fn words(found: &[Found]) -> Vec<String> {
    if found.is_empty() {
        return vec!["found: none".to_string()];
    }
    found
        .iter()
        .map(|f| match f.service {
            Service::Telnet => format!("found: {} {}", f.name, SocketAddr::new(f.addr, f.port)),
            Service::Http => format!("found: {} {} (http)", f.name, SocketAddr::new(f.addr, f.port)),
        })
        .collect()
}

// --- VUGCOMSCANMDNSV6 (LEDGER SR639): ff02::fb, AAAA, a send per interface (named tail block) ---

impl Found {
    /// The address a dial takes: `addr:port`, with [`Found::scope`] on an IPv6 one (`[fe80::1%3]:23` when shown).
    pub fn socket_addr(&self) -> SocketAddr {
        match self.addr {
            IpAddr::V4(a) => SocketAddr::V4(SocketAddrV4::new(a, self.port)),
            IpAddr::V6(a) => SocketAddr::V6(SocketAddrV6::new(a, self.port, 0, self.scope)),
        }
    }
}

/// The interface indexes `/proc/net/if_inet6` lists with a link-local (fe80::/10) address, `lo` skipped, each once in
/// the order first seen. A line is `<32 hex addr> <hex ifindex> <prefix> <scope> <flags> <name>`; a malformed line is
/// skipped.
pub fn link_scopes(if_inet6: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for line in if_inet6.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 6 || f[0].len() != 32 || f[5] == "lo" {
            continue;
        }
        let (Ok(bits), Ok(idx)) = (u128::from_str_radix(f[0], 16), u32::from_str_radix(f[1], 16)) else { continue };
        if idx != 0 && Ipv6Addr::from(bits).is_unicast_link_local() && !out.contains(&idx) {
            out.push(idx);
        }
    }
    out
}

/// The host's own IPv4 addresses as `/proc/net/fib_trie` lists them: a `|-- A.B.C.D` line followed by a
/// `/32 host LOCAL` line, each once (the Main and Local tables both list them), 127/8 skipped.
pub fn local_v4(fib_trie: &str) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    let mut last: Option<Ipv4Addr> = None;
    for line in fib_trie.lines() {
        let t = line.trim();
        if let Some(a) = t.strip_prefix("|-- ") {
            last = a.trim().parse().ok();
        } else if t.starts_with("/32 host LOCAL") {
            if let Some(a) = last.take() {
                if !a.is_loopback() && !out.contains(&a) {
                    out.push(a);
                }
            }
        }
    }
    out
}

/// Every (bind, destination) pair one [`discover`] asks from: per local IPv4 address -> [`MDNS_GROUP`], per IPv6 link
/// -> `ff02::fb%<ifindex>`; a family whose list is empty or unreadable asks once from its default socket.
pub fn targets() -> Vec<(SocketAddr, SocketAddr)> {
    let read = |path: &str| std::fs::read_to_string(path).unwrap_or_default();
    let mut out: Vec<(SocketAddr, SocketAddr)> = Vec::new();
    let v4 = local_v4(&read("/proc/net/fib_trie"));
    if v4.is_empty() {
        out.push((SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)), SocketAddr::V4(MDNS_GROUP)));
    }
    for a in v4 {
        out.push((SocketAddr::V4(SocketAddrV4::new(a, 0)), SocketAddr::V4(MDNS_GROUP)));
    }
    let any6 = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0));
    let v6 = link_scopes(&read("/proc/net/if_inet6"));
    if v6.is_empty() {
        out.push((any6, SocketAddr::V6(MDNS_GROUP_V6)));
    }
    for idx in v6 {
        out.push((any6, SocketAddr::V6(SocketAddrV6::new(*MDNS_GROUP_V6.ip(), MDNS_GROUP_V6.port(), 0, idx))));
    }
    out
}

/// Bind `bind`, send [`query`] to `dest`; `None` on any error (a family the host lacks, a link that refuses the send).
fn ask(bind: SocketAddr, dest: SocketAddr) -> Option<UdpSocket> {
    let sock = UdpSocket::bind(bind).ok()?;
    if dest.is_ipv4() {
        let _ = sock.set_multicast_ttl_v4(255);
    }
    sock.send_to(&query(), dest).ok()?;
    Some(sock)
}

/// Every answer `sock` receives until `deadline`, each datagram through [`walk`]; a link-local IPv6 answer takes the
/// interface its datagram arrived on. A socket error ends the collection with what was found.
fn collect(sock: &UdpSocket, deadline: Instant) -> Vec<Found> {
    let mut found: Vec<Found> = Vec::new();
    let mut buf = [0u8; 9000];
    loop {
        let now = Instant::now();
        if now >= deadline || sock.set_read_timeout(Some(deadline - now)).is_err() {
            break;
        }
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                let mut got = walk(&buf[..n]);
                if let SocketAddr::V6(src) = from {
                    for f in got.iter_mut() {
                        if let IpAddr::V6(a) = f.addr {
                            if a.is_unicast_link_local() && f.scope == 0 {
                                f.scope = src.scope_id();
                            }
                        }
                    }
                }
                merge(&mut found, got);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    found
}

/// Append `more` to `into`, dropping an entry whose (name, addr, port, service) is already there.
fn merge(into: &mut Vec<Found>, more: Vec<Found>) {
    for f in more {
        if !into.iter().any(|g| g.name == f.name && g.addr == f.addr && g.port == f.port && g.service == f.service) {
            into.push(f);
        }
    }
}

/// Ask from every (bind, destination) pair, collect each socket on its own scoped thread until ONE deadline, merge the
/// answers (deduped), telnet entries first then by name. Never dials.
fn discover_from(pairs: &[(SocketAddr, SocketAddr)], wait: Duration) -> Vec<Found> {
    let socks: Vec<UdpSocket> = pairs.iter().filter_map(|&(bind, dest)| ask(bind, dest)).collect();
    let deadline = Instant::now() + wait;
    let mut found: Vec<Found> = Vec::new();
    std::thread::scope(|s| {
        let handles: Vec<_> = socks.iter().map(|sock| s.spawn(move || collect(sock, deadline))).collect();
        for h in handles {
            if let Ok(got) = h.join() {
                merge(&mut found, got);
            }
        }
    });
    found.sort_by(|a, b| (a.service != Service::Telnet, &a.name).cmp(&(b.service != Service::Telnet, &b.name)));
    found
}
