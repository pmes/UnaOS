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
//! - IPv4 only, the default route's interface only (VUG-COMSCAN-MDNS-V6: ff02::fb, AAAA, a send per interface).
//!
//! std only: no libc, no `unsafe`.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use make_core::gcode::Dialect;

/// The mDNS group and port every responder listens on.
pub const MDNS_GROUP: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 251), 5353);

/// The service types [`query`] asks for, in question order: FluidNC's (and grblHAL's) telnet stream and web UI.
pub const SERVICES: [&str; 2] = ["_telnet._tcp.local", "_http._tcp.local"];

/// How long a person's discover listens for answers by default.
pub const DISCOVER_WAIT: Duration = Duration::from_millis(1500);

const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_SRV: u16 = 33;
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

/// One controller found on the link: the instance's name, its IPv4 address and port, the service that advertised it,
/// and a dialect guessed from the name alone (FluidNC sends no TXT to read; `None` when the name says nothing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub addr: Ipv4Addr,
    pub port: u16,
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

/// Walk one mDNS response: PTR (a service in [`SERVICES`]) -> SRV -> A, all in this message, one [`Found`] per PTR
/// whose SRV and A are both present. A message that is not a response, or runs short, yields what was complete before
/// the break (nothing, when the SRV / A had not arrived yet). Never panics on hostile bytes.
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
    let mut addrs: Vec<(String, Ipv4Addr)> = Vec::new();
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
                addrs.push((key(&owner), Ipv4Addr::new(msg[rd], msg[rd + 1], msg[rd + 2], msg[rd + 3])));
            }
            _ => {}
        }
        pos = end;
    }
    for (service, inst) in ptrs {
        let k = key(&inst);
        let Some((_, port, target)) = srvs.iter().find(|s| s.0 == k) else { continue };
        let Some((_, addr)) = addrs.iter().find(|a| &a.0 == target) else { continue };
        let name = inst.first().cloned().unwrap_or_default();
        let dialect_hint = hint(&name);
        out.push(Found { name, addr: *addr, port: *port, service, dialect_hint });
    }
    out
}

/// Send [`query`] to `dest` from one short-lived socket on an ephemeral port and collect every answer that arrives
/// within `wait` (each datagram through [`walk`]), duplicates dropped, telnet entries first then by name. A socket
/// error ends the collection with what was found. Never dials.
pub fn discover_at(dest: SocketAddr, wait: Duration) -> Vec<Found> {
    let mut found: Vec<Found> = Vec::new();
    let Ok(sock) = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)) else { return found };
    let _ = sock.set_multicast_ttl_v4(255);
    if sock.send_to(&query(), dest).is_err() {
        return found;
    }
    let deadline = Instant::now() + wait;
    let mut buf = [0u8; 9000];
    loop {
        let now = Instant::now();
        if now >= deadline || sock.set_read_timeout(Some(deadline - now)).is_err() {
            break;
        }
        match sock.recv_from(&mut buf) {
            Ok((n, _)) => {
                for f in walk(&buf[..n]) {
                    if !found.contains(&f) {
                        found.push(f);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    found.sort_by(|a, b| (a.service != Service::Telnet, &a.name).cmp(&(b.service != Service::Telnet, &b.name)));
    found
}

/// [`discover_at`] the mDNS group ([`MDNS_GROUP`]).
pub fn discover(wait: Duration) -> Vec<Found> {
    discover_at(SocketAddr::V4(MDNS_GROUP), wait)
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
            Service::Telnet => format!("found: {} {}:{}", f.name, f.addr, f.port),
            Service::Http => format!("found: {} {}:{} (http)", f.name, f.addr, f.port),
        })
        .collect()
}
