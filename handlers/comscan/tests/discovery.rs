// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// VUGCOMSCANMDNS (LEDGER SR632): the mDNS discover road — the one question's bytes, the answer walk on a responder's
// message (compression pointers followed, hostile bytes refused without a panic), the socket road against a fake
// responder on the loopback (legacy unicast: the same road the real 224.0.0.251 question takes), silence said
// `found: none`, and the card's squawk row. Nothing here dials or posts to the bus.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use bandy::Origin;
use comscan::PumpCard;
use comscan::discovery::{Found, Service, discover_at, link_scopes, local_v4, query, walk, words};
use make_core::gcode::Dialect;

fn unhex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// The question: id 0, no flags, QDCOUNT 2, `_telnet._tcp.local` then `_http._tcp.local`, each PTR class IN with QU.
const QUERY_HEX: &str = "000000000002000000000000\
    075f74656c6e6574045f746370056c6f63616c00000c8001\
    055f68747470045f746370056c6f63616c00000c8001";

/// A response in the shape an ESP-IDF mdns responder gives FluidNC's two `mdns_service_add(NULL, ..)` services (no
/// instance name, so the hostname `fluidnc`; no TXT): two PTR answers, then SRV x2 and one A in the additional section,
/// every repeated name a compression pointer (`_tcp.local` at 20, `local` at 25, the instances at 42 / 70, the target
/// `fluidnc.local` at 98). CONSTRUCTED from RFC 1035 / 2782 / 6762 and FluidNC's source, not captured: no FluidNC
/// board answers on this seat. 144 bytes.
const FLUIDNC_HEX: &str = "\
    000084000000000200000003075f74656c6e6574045f746370056c6f63616c00\
    000c000100001194000a07666c7569646e63c00c055f68747470c014000c0001\
    00001194000a07666c7569646e63c034c02a0021800100000078001000000000\
    001707666c7569646e63c019c04600218001000000780008000000000050c062\
    c06200018001000000780004c0a80128";

fn fluidnc_found() -> Vec<Found> {
    let addr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40));
    let grbl = Some(Dialect::Grbl { laser: false });
    vec![
        Found { name: "fluidnc".into(), addr, port: 23, scope: 0, service: Service::Telnet, dialect_hint: grbl },
        Found { name: "fluidnc".into(), addr, port: 80, scope: 0, service: Service::Http, dialect_hint: grbl },
    ]
}

#[test]
fn the_question_is_one_ptr_per_service() {
    assert_eq!(query(), unhex(QUERY_HEX));
    assert_eq!(query().len(), 58);
}

#[test]
fn the_walk_follows_ptr_srv_a_through_compression_pointers() {
    let msg = unhex(FLUIDNC_HEX);
    assert_eq!(msg.len(), 144);
    assert_eq!(walk(&msg), fluidnc_found());
    assert_eq!(words(&walk(&msg)), vec!["found: fluidnc 192.168.1.40:23", "found: fluidnc 192.168.1.40:80 (http)"]);
}

#[test]
fn hostile_or_partial_messages_yield_nothing_and_never_panic() {
    let msg = unhex(FLUIDNC_HEX);
    // Our own question is not a response (QR clear).
    assert!(walk(&query()).is_empty(), "a question is not an answer");
    // A name whose pointer points at itself: the hop cap ends it.
    let looped = unhex("000084000000000100000000c00c000c00010000119400020000");
    assert!(walk(&looped).is_empty(), "a pointer loop is refused");
    // Every truncation of the response: the SRV / A had not arrived, so nothing is found (and nothing panics).
    for n in 0..msg.len() {
        assert!(walk(&msg[..n]).is_empty(), "truncated at {n}");
    }
    // The PTRs alone (ARCOUNT 0): an instance whose SRV and A did not come with it is not found; nothing is resolved further.
    let mut ptr_only = msg.clone();
    ptr_only[10] = 0;
    ptr_only[11] = 0;
    assert!(walk(&ptr_only).is_empty(), "no SRV / A in the message, no entry");
}

#[test]
fn the_socket_road_collects_a_fake_responders_unicast_answer() {
    let responder = UdpSocket::bind("127.0.0.1:0").unwrap();
    responder.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let dest: SocketAddr = responder.local_addr().unwrap();
    let answer = std::thread::spawn(move || {
        let mut buf = [0u8; 512];
        let (n, from) = responder.recv_from(&mut buf).expect("the question arrives");
        let asked = buf[..n].to_vec();
        // A legacy unicast question is answered to its source port; twice, as a responder repeating itself may.
        responder.send_to(&unhex(FLUIDNC_HEX), from).unwrap();
        responder.send_to(&unhex(FLUIDNC_HEX), from).unwrap();
        asked
    });
    let found = discover_at(dest, Duration::from_millis(800));
    assert_eq!(answer.join().unwrap(), query(), "the one question, as built");
    assert_eq!(found, fluidnc_found(), "both services, once each, telnet first");
}

#[test]
fn silence_is_found_none_and_the_card_says_it_in_the_squawk_row() {
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    let found = discover_at(silent.local_addr().unwrap(), Duration::from_millis(300));
    assert!(found.is_empty(), "nothing answered");
    assert_eq!(words(&found), vec!["found: none"]);

    let mut card = PumpCard::new(Origin::LocalUser("una".to_string()));
    assert_eq!(card.say_found(&found), vec!["found: none"]);
    assert_eq!(card.snapshot().squawk.as_deref(), Some("found: none"));
    let said = card.say_found(&fluidnc_found());
    assert_eq!(said, vec!["found: fluidnc 192.168.1.40:23", "found: fluidnc 192.168.1.40:80 (http)"]);
    assert_eq!(card.snapshot().squawk.as_deref(), Some("found: fluidnc 192.168.1.40:80 (http)"), "the last word");
}

// --- VUGCOMSCANMDNSV6 (LEDGER SR639): AAAA beside A, the v6 socket road on the loopback, the interface lists ---

fn put_name(m: &mut Vec<u8>, name: &str) {
    for label in name.split('.') {
        m.push(label.len() as u8);
        m.extend_from_slice(label.as_bytes());
    }
    m.push(0);
}

fn put_rr(m: &mut Vec<u8>, owner: &str, ty: u16, class: u16, rdata: &[u8]) {
    put_name(m, owner);
    for v in [ty, class] {
        m.extend_from_slice(&v.to_be_bytes());
    }
    m.extend_from_slice(&120u32.to_be_bytes());
    m.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    m.extend_from_slice(rdata);
}

/// A response for FluidNC's telnet service on a host with IPv4 AND IPv6 up (the ESP-IDF responder appends AAAA when
/// its netif carries a v6 address): PTR, then SRV, A and AAAA (fe80::1:2) in the additional section. CONSTRUCTED from
/// RFC 1035 / 2782 / 3596 / 6762, names uncompressed; not captured.
fn dual_stack_answer() -> Vec<u8> {
    let mut m = Vec::new();
    for v in [0u16, 0x8400, 0, 1, 0, 3] {
        m.extend_from_slice(&v.to_be_bytes());
    }
    let mut inst = Vec::new();
    put_name(&mut inst, "fluidnc._telnet._tcp.local");
    put_rr(&mut m, "_telnet._tcp.local", 12, 1, &inst);
    let mut srv = vec![0, 0, 0, 0, 0, 23];
    put_name(&mut srv, "fluidnc.local");
    put_rr(&mut m, "fluidnc._telnet._tcp.local", 33, 0x8001, &srv);
    put_rr(&mut m, "fluidnc.local", 1, 0x8001, &[192, 168, 1, 40]);
    put_rr(&mut m, "fluidnc.local", 28, 0x8001, &Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 1, 2).octets());
    m
}

fn dual_stack_found() -> Vec<Found> {
    let grbl = Some(Dialect::Grbl { laser: false });
    let v4 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40));
    let v6 = IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 1, 2));
    vec![
        Found { name: "fluidnc".into(), addr: v4, port: 23, scope: 0, service: Service::Telnet, dialect_hint: grbl },
        Found { name: "fluidnc".into(), addr: v6, port: 23, scope: 0, service: Service::Telnet, dialect_hint: grbl },
    ]
}

#[test]
fn the_walk_reads_aaaa_beside_a() {
    let found = walk(&dual_stack_answer());
    assert_eq!(found, dual_stack_found(), "one entry per address of the SRV target, A then AAAA");
    assert_eq!(words(&found), vec!["found: fluidnc 192.168.1.40:23", "found: fluidnc [fe80::1:2]:23"]);
    let mut on_link = found[1].clone();
    assert_eq!(on_link.socket_addr().to_string(), "[fe80::1:2]:23");
    on_link.scope = 3;
    assert_eq!(on_link.socket_addr().to_string(), "[fe80::1:2%3]:23", "the dial's address carries the link");
    assert_eq!(found[0].socket_addr().to_string(), "192.168.1.40:23");
}

#[test]
fn the_v6_socket_road_collects_a_loopback_responders_answer() {
    let Ok(responder) = UdpSocket::bind("[::1]:0") else {
        eprintln!("skip: no IPv6 loopback");
        return;
    };
    responder.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let dest: SocketAddr = responder.local_addr().unwrap();
    let answer = std::thread::spawn(move || {
        let mut buf = [0u8; 512];
        let (n, from) = responder.recv_from(&mut buf).expect("the question arrives over v6");
        let asked = buf[..n].to_vec();
        responder.send_to(&dual_stack_answer(), from).unwrap();
        responder.send_to(&dual_stack_answer(), from).unwrap();
        asked
    });
    let found = discover_at(dest, Duration::from_millis(800));
    assert_eq!(answer.join().unwrap(), query(), "the one question, as built, over v6");
    assert_eq!(found, dual_stack_found(), "both addresses, once each (::1's scope is 0)");
}

#[test]
fn the_interface_lists_read_from_proc_fixtures() {
    let if_inet6 = "\
00000000000000000000000000000001 01 80 10 80       lo
fe800000000000000211223344556677 02 40 20 80     eth0
20010db8000000000211223344556677 02 40 00 00     eth0
fe8000000000000002aabbccddeeff00 03 40 20 80    wlan0
not a line
";
    assert_eq!(link_scopes(if_inet6), vec![2, 3], "links with an fe80 address, lo skipped, each once");
    let fib_trie = "\
Main:
  +-- 0.0.0.0/0 3 0 5
     |-- 0.0.0.0
        /0 universe UNICAST
     +-- 127.0.0.0/8 2 0 2
           |-- 127.0.0.1
              /32 host LOCAL
     +-- 192.168.1.0/24 2 0 2
           |-- 192.168.1.0
              /24 link UNICAST
           |-- 192.168.1.5
              /32 host LOCAL
        |-- 192.168.1.255
           /32 link BROADCAST
     |-- 10.0.0.7
        /32 host LOCAL
Local:
  +-- 0.0.0.0/0 3 0 5
     |-- 127.0.0.1
        /32 host LOCAL
     |-- 192.168.1.5
        /32 host LOCAL
     |-- 10.0.0.7
        /32 host LOCAL
";
    assert_eq!(local_v4(fib_trie), vec![Ipv4Addr::new(192, 168, 1, 5), Ipv4Addr::new(10, 0, 0, 7)]);
    assert!(link_scopes("").is_empty() && local_v4("").is_empty(), "an unreadable list is empty: discover falls back");
}

// --- VUGCOMSCANMDNSDIAL (SR638): a found telnet entry dialed by index as the one tcp:// name, in a PumpLink ---

#[test]
fn a_found_telnet_entry_is_dialed_by_index_as_the_one_tcp_spec() {
    use bandy::{PumpEvent, SMessage};
    use comscan::tcp::{parse, spec};

    let mut card = PumpCard::new(Origin::LocalUser("una".to_string()));
    card.say_found(&fluidnc_found());
    let kept = card.snapshot().found;
    assert_eq!(kept.len(), 1, "the http hint is said, not kept as a row");
    assert_eq!((kept[0].port, kept[0].service), (23, Service::Telnet));
    match card.dial(0) {
        Some(SMessage::Pump(PumpEvent::PumpLink { principal, port, dialect })) => {
            assert_eq!(principal, Origin::LocalUser("una".to_string()), "the card's own principal");
            assert_eq!((port.as_str(), dialect.as_str()), ("tcp://192.168.1.40:23", "grbl"));
            assert_eq!(parse(&port).unwrap(), ("192.168.1.40".to_string(), 23), "the opener reads the name back");
        }
        other => panic!("expected PumpLink, got {other:?}"),
    }
    assert!(card.dial(1).is_none(), "no such row");

    let v6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)), 23);
    assert_eq!(spec(v6), "tcp://[fe80::1]:23");
    assert_eq!(parse(&spec(v6)).unwrap(), ("fe80::1".to_string(), 23));
    let scoped = Found {
        name: "cnc".into(),
        addr: IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
        port: 23,
        scope: 2,
        service: Service::Telnet,
        dialect_hint: Some(Dialect::Marlin),
    };
    assert_eq!(spec(scoped.socket_addr()), "tcp://[fe80::1%2]:23", "the link's scope rides the name");
    card.say_found(&[scoped]);
    match card.dial(0) {
        Some(SMessage::Pump(PumpEvent::PumpLink { port, dialect, .. })) => {
            assert_eq!((port.as_str(), dialect.as_str()), ("tcp://[fe80::1%2]:23", "marlin"), "the hinted dialect");
        }
        other => panic!("expected PumpLink, got {other:?}"),
    }
    card.say_found(&[]);
    assert!(card.snapshot().found.is_empty(), "the next discover replaces the rows");
}
